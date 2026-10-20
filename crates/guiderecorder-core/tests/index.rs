//! The SQLite index (#340) over a temp copy of `fixtures/guides`. See
//! design scratchpad #444 ("SQLite index").

mod support;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use guiderecorder_core::index::{
    fts_query, list_library, list_with_fallback, search_library, search_library_with,
    search_with_fallback, HitKind, Index, IndexError, ListFilter, SearchOptions, Source, StepHit,
    FORMAT_VERSION,
};
use guiderecorder_core::library::{Library, GUIDE_FILE};
use guiderecorder_core::model::*;
use guiderecorder_core::Ulid;

const FIXTURE_COUNT: usize = 4;

/// A temp folder holding a copy of the fixture library and an index path.
struct Fixture {
    library: Library,
    index_path: PathBuf,
    // Removed on drop, after `library`.
    _dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("library");
        support::copy_dir(&support::fixture_guides_dir(), &root);
        Self {
            library: Library::open(&root).unwrap(),
            index_path: dir.path().join("support/index.sqlite"),
            _dir: dir,
        }
    }

    fn built_index(&self) -> Index {
        let mut index = Index::open(&self.index_path).unwrap();
        index.rebuild(&self.library).unwrap();
        index
    }

    fn bundle(&self, name: &str) -> PathBuf {
        self.library.root().join(format!("{name}.guiderecorder"))
    }

    /// Loads, edits and saves a guide.
    fn edit(&self, name: &str, change: impl FnOnce(&mut Guide)) {
        let mut loaded = self.library.load(self.bundle(name)).unwrap();
        change(&mut loaded.guide);
        self.library.save_to(&loaded.handle, &loaded.guide).unwrap();
    }
}

fn step(title: &str, description: &str) -> Item {
    let id = Ulid::new();
    Item::Step(Step {
        id,
        title: title.to_owned(),
        description: description.to_owned(),
        kind: StepKind::Click,
        point: None,
        end_point: None,
        element: None,
        keystrokes: None,
        image: StepImage {
            full: format!("steps/{id}.webp"),
            crop: None,
            after: None,
            show: ImageShow::Full,
            crop_rect: None,
        },
        annotations: Vec::new(),
        ai_state: AiState::default(),
    })
}

/// (guide title, step index) of each hit, in order.
fn keys(hits: &[StepHit]) -> Vec<(String, Option<usize>)> {
    hits.iter()
        .map(|h| (h.guide_title.clone(), h.step_index))
        .collect()
}

/// (guide path, kind, item id) of each hit: unique per hit.
fn ids(hits: &[StepHit]) -> BTreeSet<(PathBuf, HitKind, Option<Ulid>)> {
    hits.iter()
        .map(|h| (h.guide_path.clone(), h.kind, h.item_id))
        .collect()
}

fn titles(guides: &[guiderecorder_core::index::IndexedGuide]) -> Vec<&str> {
    guides.iter().map(|g| g.title.as_str()).collect()
}

// ---------------------------------------------------------------------------

#[test]
fn rebuild_indexes_every_fixture() {
    let fx = Fixture::new();
    let mut index = Index::open(&fx.index_path).unwrap();
    assert!(index.is_stale(&fx.library).unwrap(), "empty index");
    let report = index.rebuild(&fx.library).unwrap();
    assert!(report.rebuilt);
    assert_eq!(report.added.len(), FIXTURE_COUNT);

    let guides = index.list(&ListFilter::default()).unwrap();
    let summaries = fx.library.list().unwrap();
    assert_eq!(guides.len(), FIXTURE_COUNT);
    for (guide, summary) in guides.iter().zip(&summaries) {
        assert_eq!(guide.id, summary.id);
        assert_eq!(guide.path, summary.path);
        assert_eq!(guide.title, summary.title);
        assert_eq!(guide.summary, summary.summary);
        assert_eq!(guide.tags, summary.tags);
        assert_eq!(guide.step_count, summary.step_count);
        assert_eq!(guide.kind, summary.kind);
        assert_eq!(guide.hotspot_count, summary.hotspot_count);
        assert_eq!(guide.updated_at, summary.updated_at);
        assert!(!guide.read_only);
    }
    let floor_plan = guides
        .iter()
        .find(|g| g.title == "Office floor plan")
        .unwrap();
    assert_eq!(
        (
            floor_plan.kind,
            floor_plan.step_count,
            floor_plan.hotspot_count
        ),
        (GuideKind::Annotated, 2, 6)
    );
    let rotate = guides
        .iter()
        .find(|g| g.title == "Rotate an API key")
        .unwrap();
    assert_eq!(
        (rotate.kind, rotate.hotspot_count),
        (GuideKind::Walkthrough, 0)
    );
    assert!(!index.is_stale(&fx.library).unwrap());

    // Rebuilding again is idempotent.
    index.rebuild(&fx.library).unwrap();
    assert_eq!(index.list(&ListFilter::default()).unwrap(), guides);
}

#[test]
fn list_filters_by_tag_query_and_limit() {
    let fx = Fixture::new();
    let index = fx.built_index();
    let list = |tag: Option<&str>, query: Option<&str>, limit| {
        let filter = ListFilter {
            tag: tag.map(Into::into),
            query: query.map(Into::into),
            limit,
        };
        let indexed = index.list(&filter).unwrap();
        // The fallback agrees.
        assert_eq!(
            titles(&indexed),
            titles(&list_library(&fx.library, &filter).unwrap()),
            "{filter:?}"
        );
        indexed
    };
    assert_eq!(
        titles(&list(Some("SECURITY"), None, None)),
        ["Rotate an API key"]
    );
    assert!(list(Some("nope"), None, None).is_empty());
    assert_eq!(
        titles(&list(None, Some("invite"), None)),
        ["Share a calendar event"]
    );
    assert_eq!(
        titles(&list(None, Some("papier"), None)),
        ["Kurzanleitung Drucker"]
    );
    assert!(list(Some("macos"), Some("papier"), None).is_empty());
    // Hotspot titles and descriptions.
    for query in ["fjord", "visitor badges"] {
        assert_eq!(
            titles(&list(None, Some(query), None)),
            ["Office floor plan"]
        );
    }
    assert_eq!(list(None, None, Some(2)).len(), 2);
    assert!(list(None, Some("--"), None).is_empty());
}

#[test]
fn search_finds_steps_and_guides_with_snippets() {
    let fx = Fixture::new();
    let index = fx.built_index();

    let hits = index.search("password", 10).unwrap();
    assert_eq!(
        keys(&hits),
        [
            ("Rotate an API key".to_owned(), Some(1)),
            ("Rotate an API key".to_owned(), Some(2)),
        ],
        "title match outranks description match: {hits:#?}"
    );
    let first = &hits[0];
    assert_eq!(first.title, "Enter your password to confirm");
    assert_eq!(first.snippet, "Enter your **password** to confirm");
    assert_eq!(first.kind, HitKind::Step);
    assert!(first.item_id.is_some());
    assert_eq!(
        first.guide_path,
        fx.bundle("Rotate an API key").canonicalize().unwrap()
    );
    assert!(hits[0].rank <= hits[1].rank);
    assert!(
        hits[1].snippet.contains("your **password** manager"),
        "{:?}",
        hits[1].snippet
    );

    // Markdown is indexed as plain text.
    let hits = index.search("create key", 10).unwrap();
    assert_eq!(hits[0].title, "Click Create key and copy the new key");
    assert!(!hits[0].snippet.contains("****"), "{:?}", hits[0].snippet);

    // Guide-level hits: summary and tags.
    let hits = index.search("colleague", 10).unwrap();
    assert_eq!(keys(&hits), [("Share a calendar event".to_owned(), None)]);
    assert_eq!((hits[0].kind, hits[0].item_id), (HitKind::Guide, None));
    assert_eq!(hits[0].title, "Share a calendar event");
    assert!(hits[0].snippet.contains("**colleague**"));
    let hits = index.search("security", 10).unwrap();
    assert_eq!(keys(&hits), [("Rotate an API key".to_owned(), None)]);

    // Words can mix the guide title and the step.
    let hits = index.search("rotate drag", 10).unwrap();
    assert_eq!(keys(&hits), [("Rotate an API key".to_owned(), Some(3))]);

    // The last word is a prefix; accents and case are ignored.
    let kinds: Vec<_> = index
        .search("invit", 10)
        .unwrap()
        .iter()
        .map(|h| h.kind)
        .collect();
    assert_eq!(
        BTreeSet::from_iter(kinds),
        BTreeSet::from([HitKind::Guide, HitKind::Step, HitKind::Section]),
        "summary and intro, step, section"
    );
    let hits = index.search("UBERFULLEN", 10).unwrap();
    assert_eq!(keys(&hits), [("Kurzanleitung Drucker".to_owned(), Some(0))]);
    assert!(
        hits[0].snippet.contains("**überfüllen**"),
        "{:?}",
        hits[0].snippet
    );

    // Limit.
    assert_eq!(index.search("the", 2).unwrap().len(), 2);
    assert!(index.search("the", 0).unwrap().is_empty());
}

#[test]
fn search_ranks_title_before_description() {
    let fx = Fixture::new();
    let mut guide = Guide::new("Zoo tour", "en");
    guide.items = vec![
        step("Feed the penguins", "Mind the zebra crossing on the way."),
        step("Photograph the **zebra**", ""),
        step("Leave", "No zebra here, just zebu."),
    ];
    fx.library.create(&guide).unwrap();
    let index = fx.built_index();
    let hits = index.search("zebra", 10).unwrap();
    assert_eq!(hits[0].step_index, Some(1), "{hits:#?}");
    assert_eq!(hits[0].snippet, "Photograph the **zebra**");
    assert_eq!(hits.len(), 3);
    let scanned = search_library(&fx.library, "zebra", 10).unwrap();
    assert_eq!(scanned[0].step_index, Some(1), "{scanned:#?}");
    assert_eq!(scanned[0].snippet, hits[0].snippet);
}

#[test]
fn hotspot_text_is_searchable() {
    let fx = Fixture::new();
    let index = fx.built_index();
    // A hotspot title and a word from the second line of a description.
    for query in ["fjord", "visitor badges"] {
        let hits = index.search(query, 10).unwrap();
        assert_eq!(hits.len(), 1, "{query}: {hits:#?}");
        assert_eq!(hits[0].guide_title, "Office floor plan");
        assert_eq!((hits[0].kind, hits[0].step_index), (HitKind::Step, Some(0)));
        assert_eq!(hits[0].title, "Third floor");
        let scanned = search_library(&fx.library, query, 10).unwrap();
        assert_eq!(scanned.len(), 1, "{query}: {scanned:#?}");
        assert_eq!(scanned[0].item_id, hits[0].item_id);
    }
}

#[test]
fn special_characters_in_queries_are_literal() {
    let fx = Fixture::new();
    let index = fx.built_index();
    for query in [
        "\"",
        "\"\"",
        "'",
        "*",
        "**",
        "(",
        ")",
        "-key",
        "key -",
        "NOT key",
        "key OR",
        "AND",
        "NEAR(key password, 2)",
        "title:key",
        "{title body}: key",
        "^key",
        "key*",
        "k\"ey",
        "\u{0}",
        "a\u{0}b",
        "key\u{0}",
        "pass\u{1}word",
        "\u{7f}key\u{1b}",
        "%_\\",
        "🔑",
        "   ",
        "",
    ] {
        let hits = index
            .search(query, 10)
            .unwrap_or_else(|e| panic!("{query:?} -> {:?}: {e}", fts_query(query)));
        let scanned = search_library(&fx.library, query, 10).unwrap();
        assert_eq!(ids(&hits), ids(&scanned), "{query:?}");
        index
            .list(&ListFilter {
                query: Some(query.into()),
                ..ListFilter::default()
            })
            .unwrap();
    }

    // Operators are words, not syntax: "NOT key" needs the word "not".
    assert!(index.search("NOT key", 10).unwrap().is_empty());
    assert!(!index.search("-key", 10).unwrap().is_empty());
    // Punctuated words match as a phrase.
    let hits = index.search("api.example.com/v1", 10).unwrap();
    assert_eq!(keys(&hits), [("Rotate an API key".to_owned(), Some(4))]);
    assert!(index.search("example.api", 10).unwrap().is_empty());
}

#[test]
fn fallback_search_matches_the_index() {
    let fx = Fixture::new();
    let index = fx.built_index();
    for query in [
        "password",
        "key",
        "the",
        "invit",
        "guest email",
        "calendar",
        "Drucker papier",
        "rotate drag",
        "revokes",
        "open the event",
        "icloud",
        "90 days",
        "handbuch",
        "a",
    ] {
        assert_eq!(
            ids(&index.search(query, 100).unwrap()),
            ids(&search_library(&fx.library, query, 100).unwrap()),
            "{query:?}"
        );
    }
}

#[test]
fn sync_picks_up_edits_additions_and_deletions() {
    let fx = Fixture::new();
    let mut index = fx.built_index();
    assert_eq!(index.sync(&fx.library).unwrap(), Default::default());

    // Edit.
    fx.edit("Rotate an API key", |guide| {
        if let Item::Step(step) = &mut guide.items[1] {
            step.title = "Open **Preferences → Tokens**".into();
        }
        guide.tags.push("Rotation".into());
    });
    assert!(index.is_stale(&fx.library).unwrap());
    let report = index.sync(&fx.library).unwrap();
    assert_eq!(report.updated, ["Rotate an API key.guiderecorder"]);
    assert!(report.added.is_empty() && report.removed.is_empty() && !report.rebuilt);
    assert!(!index.is_stale(&fx.library).unwrap());
    assert_eq!(index.search("tokens", 10).unwrap().len(), 1);
    assert!(index.search("settings", 10).unwrap().is_empty());
    let tagged = index
        .list(&ListFilter {
            tag: Some("rotation".into()),
            ..ListFilter::default()
        })
        .unwrap();
    assert_eq!(titles(&tagged), ["Rotate an API key"]);

    // Add.
    let mut guide = Guide::new("Water the plants", "en");
    guide.items = vec![step("Fill the watering can", "")];
    let handle = fx.library.create(&guide).unwrap();
    assert!(index.is_stale(&fx.library).unwrap());
    let report = index.sync(&fx.library).unwrap();
    assert_eq!(report.added, ["Water the plants.guiderecorder"]);
    assert_eq!(index.search("watering", 10).unwrap()[0].guide_id, guide.id);
    assert_eq!(
        index.list(&ListFilter::default()).unwrap()[0].id,
        guide.id,
        "newest first"
    );

    // Delete.
    fx.library.delete(&handle).unwrap();
    assert!(index.is_stale(&fx.library).unwrap());
    let report = index.sync(&fx.library).unwrap();
    assert_eq!(report.removed, ["Water the plants.guiderecorder"]);
    assert!(index.search("watering", 10).unwrap().is_empty());
    assert_eq!(
        index.list(&ListFilter::default()).unwrap().len(),
        FIXTURE_COUNT
    );
    assert!(!index.is_stale(&fx.library).unwrap());
}

#[test]
fn update_and_remove_single_bundles() {
    let fx = Fixture::new();
    let mut index = fx.built_index();
    let bundle = fx.bundle("Share a calendar event");

    fx.edit("Share a calendar event", |guide| {
        guide.summary = "Add a teammate to a meeting.".into();
    });
    index.update(&fx.library, &bundle).unwrap();
    assert!(!index.is_stale(&fx.library).unwrap());
    assert_eq!(index.search("teammate", 10).unwrap().len(), 1);

    index.remove(&bundle).unwrap();
    assert!(index.search("teammate", 10).unwrap().is_empty());
    assert!(
        index.is_stale(&fx.library).unwrap(),
        "bundle is still on disk"
    );
    index.update(&fx.library, &bundle).unwrap();
    assert!(!index.is_stale(&fx.library).unwrap());

    // Updating a bundle that is gone drops it.
    fs::remove_dir_all(&bundle).unwrap();
    index.update(&fx.library, &bundle).unwrap();
    assert_eq!(
        index.list(&ListFilter::default()).unwrap().len(),
        FIXTURE_COUNT - 1
    );
    assert!(!index.is_stale(&fx.library).unwrap());

    assert!(matches!(
        index.update(&fx.library, fx.library.root().join("notes.txt")),
        Err(IndexError::Library(_))
    ));
}

#[test]
fn unreadable_bundles_are_tracked_but_not_listed() {
    let fx = Fixture::new();
    let broken = fx.library.root().join("Broken.guiderecorder");
    fs::create_dir(&broken).unwrap();
    fs::write(broken.join(GUIDE_FILE), b"{ not json").unwrap();
    let empty = fx.library.root().join("Empty.guiderecorder");
    fs::create_dir(&empty).unwrap();

    let mut index = fx.built_index();
    assert_eq!(
        index.list(&ListFilter::default()).unwrap().len(),
        FIXTURE_COUNT
    );
    assert!(
        !index.is_stale(&fx.library).unwrap(),
        "errors count as indexed"
    );

    // Fixing the file is picked up.
    fs::copy(
        fx.bundle("Kurzanleitung Drucker").join(GUIDE_FILE),
        broken.join(GUIDE_FILE),
    )
    .unwrap();
    fs::remove_dir_all(fx.bundle("Kurzanleitung Drucker")).unwrap();
    assert!(index.is_stale(&fx.library).unwrap());
    let report = index.sync(&fx.library).unwrap();
    assert_eq!(report.updated, ["Broken.guiderecorder"]);
    assert_eq!(report.removed, ["Kurzanleitung Drucker.guiderecorder"]);
    let hits = index.search("drucker", 10).unwrap();
    assert!(
        hits.iter()
            .all(|h| h.guide_path.ends_with("Broken.guiderecorder")),
        "{hits:#?}"
    );
}

#[test]
fn read_only_open() {
    let fx = Fixture::new();
    assert!(matches!(
        Index::open_read_only(&fx.index_path),
        Err(IndexError::Io { .. })
    ));
    let writer = fx.built_index();

    let mut reader = Index::open_read_only(&fx.index_path).unwrap();
    assert!(reader.is_read_only() && !writer.is_read_only());
    assert!(!reader.is_stale(&fx.library).unwrap());
    assert_eq!(
        reader.search("password", 10).unwrap(),
        writer.search("password", 10).unwrap()
    );
    assert_eq!(
        reader.list(&ListFilter::default()).unwrap(),
        writer.list(&ListFilter::default()).unwrap()
    );
    assert!(matches!(
        reader.rebuild(&fx.library),
        Err(IndexError::ReadOnly)
    ));
    assert!(matches!(
        reader.sync(&fx.library),
        Err(IndexError::ReadOnly)
    ));
    assert!(matches!(
        reader.remove(fx.bundle("Rotate an API key")),
        Err(IndexError::ReadOnly)
    ));
    drop(writer);
    // Still readable after the writer closes (WAL checkpointed away).
    assert_eq!(reader.search("password", 10).unwrap().len(), 2);
}

#[test]
fn staleness_and_fallback_for_readers() {
    let fx = Fixture::new();

    // Missing index: scan.
    let (hits, source) = search_with_fallback(&fx.index_path, &fx.library, "password", 10).unwrap();
    assert_eq!((hits.len(), source), (2, Source::Scan));

    let mut writer = fx.built_index();
    let (hits, source) = search_with_fallback(&fx.index_path, &fx.library, "password", 10).unwrap();
    assert_eq!((hits.len(), source), (2, Source::Index));
    let (guides, source) =
        list_with_fallback(&fx.index_path, &fx.library, &ListFilter::default()).unwrap();
    assert_eq!((guides.len(), source), (FIXTURE_COUNT, Source::Index));

    // An edit the app has not indexed yet: readers see it through the scan.
    fx.edit("Rotate an API key", |guide| {
        guide.items.push(step("Revoke the old password", ""));
    });
    let reader = Index::open_read_only(&fx.index_path).unwrap();
    assert!(reader.is_stale(&fx.library).unwrap());
    let (hits, source) = search_with_fallback(&fx.index_path, &fx.library, "password", 10).unwrap();
    assert_eq!((hits.len(), source), (3, Source::Scan));
    writer.sync(&fx.library).unwrap();
    assert!(!reader.is_stale(&fx.library).unwrap());
    let (hits, source) = search_with_fallback(&fx.index_path, &fx.library, "password", 10).unwrap();
    assert_eq!((hits.len(), source), (3, Source::Index));

    // Same size, new mtime still counts.
    let json = fx.bundle("Share a calendar event").join(GUIDE_FILE);
    let file = fs::File::options().write(true).open(&json).unwrap();
    file.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5))
        .unwrap();
    assert!(reader.is_stale(&fx.library).unwrap());
    writer.sync(&fx.library).unwrap();

    // Another library.
    let other_dir = tempfile::tempdir().unwrap();
    let other = Library::open(other_dir.path()).unwrap();
    assert!(reader.is_stale(&other).unwrap());
    let report = writer.sync(&other).unwrap();
    assert!(report.rebuilt);
    assert!(writer.list(&ListFilter::default()).unwrap().is_empty());
    assert!(reader.is_stale(&fx.library).unwrap());
}

#[test]
fn other_format_versions_and_corrupt_files() {
    let fx = Fixture::new();
    drop(fx.built_index());
    set_format(&fx.index_path, "999");

    let reader = Index::open_read_only(&fx.index_path).unwrap();
    assert!(reader.is_stale(&fx.library).unwrap());
    assert!(matches!(
        reader.search("key", 10),
        Err(IndexError::Incompatible {
            found: Some(_),
            expected: FORMAT_VERSION
        })
    ));
    let (_, source) = search_with_fallback(&fx.index_path, &fx.library, "key", 10).unwrap();
    assert_eq!(source, Source::Scan);
    drop(reader);

    // The writer starts over.
    let mut writer = Index::open(&fx.index_path).unwrap();
    assert!(writer.list(&ListFilter::default()).unwrap().is_empty());
    assert!(writer.sync(&fx.library).unwrap().rebuilt);
    assert_eq!(writer.search("password", 10).unwrap().len(), 2);
    drop(writer);

    // A long-lived reader notices an upgrade.
    let reader = Index::open_read_only(&fx.index_path).unwrap();
    assert!(!reader.is_stale(&fx.library).unwrap());
    set_format(&fx.index_path, "999");
    assert!(reader.is_stale(&fx.library).unwrap());
}

/// Every file at `path` and its `-wal` / `-shm`, by name, with contents.
fn sqlite_set(path: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    ["", "-wal", "-shm"]
        .iter()
        .filter_map(|suffix| {
            let mut name = path.as_os_str().to_owned();
            name.push(suffix);
            let name = PathBuf::from(name);
            fs::read(&name).ok().map(|bytes| (name, bytes))
        })
        .collect()
}

#[test]
fn foreign_files_are_never_overwritten() {
    let fx = Fixture::new();
    let dir = fx.index_path.parent().unwrap().to_path_buf();
    fs::create_dir_all(&dir).unwrap();

    // A text file.
    let text = dir.join("notes.txt");
    fs::write(&text, b"my notes, not an index\n".repeat(300)).unwrap();
    let before = fs::read(&text).unwrap();
    assert!(matches!(Index::open(&text), Err(IndexError::NotAnIndex(_))));
    assert!(matches!(
        Index::open_read_only(&text),
        Err(IndexError::NotAnIndex(_))
    ));
    assert_eq!(fs::read(&text).unwrap(), before);

    // Another app's SQLite database, even with tables named like ours.
    let other = dir.join("other.sqlite");
    {
        let conn = rusqlite::Connection::open(&other).unwrap();
        conn.execute_batch(
            "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO meta VALUES ('format_version', '1'), ('owner', 'someone else');
             CREATE TABLE guides (bundle TEXT);",
        )
        .unwrap();
    }
    let before = sqlite_set(&other);
    assert!(matches!(
        Index::open(&other),
        Err(IndexError::NotAnIndex(_))
    ));
    assert!(matches!(
        Index::open_read_only(&other),
        Err(IndexError::NotAnIndex(_))
    ));
    assert_eq!(sqlite_set(&other), before);
    let conn = rusqlite::Connection::open(&other).unwrap();
    let rows: i64 = conn
        .query_row("SELECT count(*) FROM meta", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 2);

    // A folder.
    assert!(matches!(Index::open(&dir), Err(IndexError::NotAnIndex(_))));

    // Readers fall back.
    let (_, source) = search_with_fallback(&text, &fx.library, "key", 10).unwrap();
    assert_eq!(source, Source::Scan);

    // An empty file is fine to take over.
    let empty = dir.join("empty.sqlite");
    fs::write(&empty, b"").unwrap();
    let mut index = Index::open(&empty).unwrap();
    index.sync(&fx.library).unwrap();
    assert_eq!(index.search("password", 10).unwrap().len(), 2);
}

#[test]
fn corrupt_index_is_set_aside_on_open() {
    let fx = Fixture::new();
    for i in 0..40 {
        let mut guide = Guide::new(format!("Bulk guide {i}"), "en");
        guide.items = (0..10)
            .map(|s| step(&format!("Step {s} of bulk {i}"), "Some description text."))
            .collect();
        fx.library.create(&guide).unwrap();
    }
    drop(fx.built_index());
    corrupt_tail(&fx.index_path);

    let mut writer = Index::open(&fx.index_path).unwrap();
    let dir = fx.index_path.parent().unwrap();
    let aside: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.starts_with("index.sqlite.corrupt-"))
        .collect();
    // The database, plus any -wal / -shm beside it.
    let main: Vec<_> = aside
        .iter()
        .filter(|name| !name.ends_with("-wal") && !name.ends_with("-shm"))
        .collect();
    assert_eq!(main.len(), 1, "{aside:?}");
    let saved = dir.join(main[0]);
    assert!(fs::metadata(&saved).unwrap().len() > 0);
    assert!(writer.sync(&fx.library).unwrap().rebuilt);
    assert_eq!(writer.search("password", 10).unwrap().len(), 2);
    assert_eq!(writer.search("bulk", 1000).unwrap().len(), 40 * 11);
}

/// Overwrites the second half of a closed database's pages with garbage,
/// keeping the header and schema pages readable.
fn corrupt_tail(path: &Path) {
    let mut bytes = fs::read(path).unwrap();
    assert!(bytes.len() >= 8 * 4096, "{} bytes", bytes.len());
    let start = (bytes.len() / 2) / 4096 * 4096;
    for (i, byte) in bytes[start..].iter_mut().enumerate() {
        *byte = (i % 251) as u8 ^ 0xA5;
    }
    fs::write(path, bytes).unwrap();
}

#[test]
fn item_hits_need_their_own_words() {
    let fx = Fixture::new();
    let index = fx.built_index();
    for query in ["rotate", "Kurzanleitung", "Share"] {
        let hits = index.search(query, 50).unwrap();
        assert_eq!(
            hits.iter().map(|h| h.kind).collect::<Vec<_>>(),
            [HitKind::Guide],
            "{query:?}: {hits:#?}"
        );
        let scanned = search_library(&fx.library, query, 50).unwrap();
        assert_eq!(ids(&scanned), ids(&hits), "{query:?}");
    }
    // A guide word plus an item word still finds the item.
    let hits = index.search("rotate drag", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].kind, HitKind::Step);
    // "calendar" is in the Share guide's title and in one step's title.
    let hits = index.search("calendar", 50).unwrap();
    let kinds: BTreeSet<_> = hits.iter().map(|h| (h.kind, h.step_index)).collect();
    assert!(kinds.contains(&(HitKind::Guide, None)), "{hits:#?}");
    assert!(kinds.contains(&(HitKind::Step, Some(0))), "{hits:#?}");
    assert!(!kinds.contains(&(HitKind::Step, Some(1))), "{hits:#?}");
    assert_eq!(
        ids(&search_library(&fx.library, "calendar", 50).unwrap()),
        ids(&hits)
    );
}

/// Writes `count` guides of `steps` steps straight to `guide.json` (no
/// images, no fsync), for timing.
fn write_synthetic(root: &Path, count: usize, steps: usize, tag: &str) {
    for i in 0..count {
        let mut guide = Guide::new(format!("Synthetic guide {i} rotate widget"), "en");
        guide.summary = format!("Summary {i} {tag}");
        guide.items = (0..steps)
            .map(|s| {
                step(
                    &format!("Step {s} {tag} of {i}"),
                    "Click the button and wait.",
                )
            })
            .collect();
        let bundle = root.join(format!("Synthetic {i}.guiderecorder"));
        fs::create_dir_all(&bundle).unwrap();
        fs::write(
            bundle.join(GUIDE_FILE),
            serde_json::to_vec_pretty(&guide).unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn rebuild_and_mass_sync_scale_linearly() {
    use std::time::{Duration, Instant};
    // Generous for debug builds on slow CI; the quadratic version took
    // about 2 s at 1000 guides in a debug build.
    const BOUND: Duration = Duration::from_secs(30);
    let dir = tempfile::tempdir().unwrap();
    let library = Library::open(dir.path().join("library")).unwrap();
    let mut index = Index::open(dir.path().join("index.sqlite")).unwrap();
    let mut timings = Vec::new();
    for count in [250, 500] {
        write_synthetic(library.root(), count, 20, "alpha");
        let start = Instant::now();
        index.rebuild(&library).unwrap();
        let rebuild = start.elapsed();

        // Every guide.json changes, e.g. after a migration.
        std::thread::sleep(Duration::from_millis(10));
        write_synthetic(library.root(), count, 20, "betaword");
        let start = Instant::now();
        let report = index.sync(&library).unwrap();
        let sync = start.elapsed();
        assert_eq!(report.updated.len(), count);
        assert_eq!(index.search("betaword", 1000).unwrap().len(), 1000);
        assert!(index.search("alpha", 10).unwrap().is_empty());
        let start = Instant::now();
        index.search("rotate widget", 10).unwrap();
        let search = start.elapsed();
        println!("{count} guides x 20 steps: rebuild {rebuild:?}, sync of all {sync:?}, search {search:?}");
        assert!(rebuild < BOUND && sync < BOUND, "{rebuild:?} {sync:?}");
        timings.push((rebuild, sync));
    }
    println!(
        "rebuild ratio 500/250: {:.2}, sync ratio: {:.2}",
        timings[1].0.as_secs_f64() / timings[0].0.as_secs_f64(),
        timings[1].1.as_secs_f64() / timings[0].1.as_secs_f64()
    );
}

fn set_format(path: &Path, version: &str) {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute(
        "UPDATE meta SET value = ?1 WHERE key = 'format_version'",
        [version],
    )
    .unwrap();
}

#[test]
fn concurrent_reader_while_writer_updates() {
    let fx = Fixture::new();
    let mut writer = fx.built_index();
    let done = Arc::new(AtomicBool::new(false));
    let reader = {
        let path = fx.index_path.clone();
        let done = done.clone();
        thread::spawn(move || {
            let reader = Index::open_read_only(&path).unwrap();
            let mut searches = 0;
            while !done.load(Ordering::Relaxed) || searches < 50 {
                // Each query sees a whole snapshot: 2 or 3 hits, never torn.
                let hits = reader.search("password", 10).unwrap();
                assert!(matches!(hits.len(), 2 | 3), "{hits:#?}");
                let guides = reader.list(&ListFilter::default()).unwrap();
                assert_eq!(guides.len(), FIXTURE_COUNT);
                searches += 1;
            }
            searches
        })
    };
    for i in 0..20 {
        fx.edit("Rotate an API key", |guide| {
            if i % 2 == 0 {
                guide.items.push(step("Forget the old password", ""));
            } else {
                guide.items.pop();
            }
        });
        if i % 3 == 0 {
            writer.rebuild(&fx.library).unwrap();
        } else {
            writer.sync(&fx.library).unwrap();
        }
    }
    done.store(true, Ordering::Relaxed);
    assert!(reader.join().unwrap() >= 50);
}

#[test]
fn notes_sections_intro_and_prerequisites_are_searchable() {
    let fx = Fixture::new();
    let index = fx.built_index();
    let one = |query: &str| {
        let hits = index.search(query, 10).unwrap();
        assert_eq!(hits.len(), 1, "{query:?}: {hits:#?}");
        hits.into_iter().next().unwrap()
    };

    let note = one("revokes");
    assert_eq!(note.kind, HitKind::Note);
    assert_eq!(
        note.item_id,
        Some("01M4WZCE2000000000000002R1".parse().unwrap())
    );
    assert_eq!(note.step_index, None);
    assert_eq!(
        note.title,
        "Creating a new key revokes the old one immediately. Update these first:"
    );
    assert!(note.snippet.contains("**revokes**"), "{:?}", note.snippet);

    // Words are ANDed, so the ⌘O tip ("select the event ... to open it")
    // matches too, below the section title.
    let hits = index.search("open the event", 10).unwrap();
    assert_eq!(hits.len(), 2, "{hits:#?}");
    assert_eq!(hits[1].kind, HitKind::Note);
    let section = hits[0].clone();
    assert_eq!(section.kind, HitKind::Section);
    assert_eq!(
        section.item_id,
        Some("01M4WT7MM000000000000002G1".parse().unwrap())
    );
    assert_eq!(section.title, "Open the event");

    for query in ["icloud", "90 days", "admin access"] {
        let guide = one(query);
        assert_eq!(
            (guide.kind, guide.item_id),
            (HitKind::Guide, None),
            "{query:?}"
        );
    }
    assert_eq!(one("2. Stock").guide_title, "Kurzanleitung Drucker");
}

#[test]
fn duplicate_copies_are_marked_and_left_out_of_search() {
    let fx = Fixture::new();
    let original = fx.bundle("Rotate an API key").canonicalize().unwrap();
    let copy = fx
        .library
        .root()
        .join("Rotate an API key copy.guiderecorder");
    support::copy_dir(&original, &copy);
    // The app has the copy open, so the library treats it as canonical.
    let id = fx.library.load(&copy).unwrap().handle.id;
    assert_eq!(fx.library.find(id).unwrap().unwrap().path, copy);

    let mut index = Index::open(&fx.index_path).unwrap();
    index.sync(&fx.library).unwrap();
    // Indexing read both copies without changing the library's choice.
    assert_eq!(fx.library.find(id).unwrap().unwrap().path, copy);

    let guides = index.list(&ListFilter::default()).unwrap();
    assert_eq!(guides.len(), FIXTURE_COUNT + 1);
    let marked: Vec<_> = guides
        .iter()
        .filter(|g| g.id == id)
        .map(|g| (g.path.clone(), g.duplicate_of.clone()))
        .collect();
    assert_eq!(
        BTreeSet::from_iter(marked),
        BTreeSet::from([(copy.clone(), None), (original.clone(), Some(copy.clone()))])
    );

    let hits = index.search("password", 10).unwrap();
    assert_eq!(hits.len(), 2);
    assert!(hits.iter().all(|h| h.guide_path == copy), "{hits:#?}");
    let all = SearchOptions {
        limit: 10,
        include_duplicates: true,
    };
    assert_eq!(index.search_with("password", &all).unwrap().len(), 4);
    assert_eq!(
        ids(&search_library(&fx.library, "password", 10).unwrap()),
        ids(&hits)
    );
    assert_eq!(
        ids(&search_library_with(&fx.library, "password", &all).unwrap()),
        ids(&index.search_with("password", &all).unwrap())
    );
    let scanned = list_library(&fx.library, &ListFilter::default()).unwrap();
    assert_eq!(
        scanned
            .iter()
            .find(|g| g.path == original)
            .unwrap()
            .duplicate_of,
        Some(copy.clone())
    );
    // The fallback read every copy, too, without changing the choice.
    assert_eq!(fx.library.find(id).unwrap().unwrap().path, copy);

    // Removing the canonical copy makes the other one canonical again.
    fs::remove_dir_all(&copy).unwrap();
    index.remove(&copy).unwrap();
    let guides = index.list(&ListFilter::default()).unwrap();
    assert_eq!(guides.len(), FIXTURE_COUNT);
    assert!(guides.iter().all(|g| g.duplicate_of.is_none()));
    assert_eq!(index.search("password", 10).unwrap().len(), 2);
}

#[test]
fn default_path_is_in_application_support() {
    let path = Index::default_path();
    if cfg!(target_os = "macos") && std::env::var_os("HOME").is_some_and(|h| !h.is_empty()) {
        let path = path.unwrap();
        assert!(path.starts_with(std::env::var_os("HOME").unwrap()));
        assert!(
            path.ends_with("Library/Application Support/dev.dufflebear.guiderecorder/index.sqlite")
        );
    } else {
        assert_eq!(path, None);
    }
}

#[test]
fn guides_in_folders_are_indexed_listed_and_searched() {
    let fx = Fixture::new();
    let mut index = fx.built_index();
    fx.library.create_folder("", "Work").unwrap();
    fx.library.create_folder("Work", "Team").unwrap();
    let mut guide = Guide::new("Water the plants", "en");
    guide.tags = vec!["Garden".into()];
    guide
        .items
        .push(step("Fill the can", "Use the watering can"));
    let handle = fx.library.create(&guide).unwrap();
    let handle = fx.library.move_to(&handle, "Work/Team").unwrap();

    assert!(index.is_stale(&fx.library).unwrap());
    let report = index.sync(&fx.library).unwrap();
    assert_eq!(report.added, ["Work/Team/Water the plants.guiderecorder"]);
    assert!(!index.is_stale(&fx.library).unwrap());
    let hits = index.search("watering", 10).unwrap();
    assert_eq!(hits[0].guide_path, handle.path);

    let tagged = ListFilter {
        tag: Some("garden".into()),
        ..ListFilter::default()
    };
    for listed in [
        index.list(&tagged).unwrap(),
        list_library(&fx.library, &tagged).unwrap(),
    ] {
        assert_eq!(titles(&listed), ["Water the plants"]);
        assert_eq!(listed[0].folder, "Work/Team");
        assert_eq!(listed[0].path, handle.path);
        // The index keeps microseconds; Windows clocks tick in 100 ns.
        assert_eq!(
            listed[0].created_at.timestamp_micros(),
            guide.created_at.timestamp_micros()
        );
    }
    assert_eq!(
        search_library(&fx.library, "watering", 10).unwrap().len(),
        1
    );

    // Moving it back to the root is a remove plus an add; `update` takes
    // either path.
    let moved = fx.library.move_to(&handle, "").unwrap();
    index.update(&fx.library, &handle.path).unwrap();
    index.update(&fx.library, &moved.path).unwrap();
    assert!(!index.is_stale(&fx.library).unwrap());
    let listed = index.list(&tagged).unwrap();
    assert_eq!(listed[0].folder, "");
    assert_eq!(listed[0].path, moved.path);
    index.remove(&moved.path).unwrap();
    assert!(index.list(&tagged).unwrap().is_empty());
}

#[test]
fn review_fields_are_indexed_and_listed() {
    let fx = Fixture::new();
    let mut index = fx.built_index();
    let mut guide = Guide::new("Audit the firewall", "en");
    guide.tags = vec!["Compliance".into()];
    guide.review = ReviewInfo {
        owner: Some("Security".into()),
        status: Some(GuideStatus::Published),
        version: Some("3".into()),
        last_reviewed: chrono::NaiveDate::from_ymd_opt(2026, 1, 31),
        review_interval_months: Some(1),
    };
    let handle = fx.library.create(&guide).unwrap();
    index.update(&fx.library, &handle.path).unwrap();

    let tagged = ListFilter {
        tag: Some("compliance".into()),
        ..ListFilter::default()
    };
    let due = chrono::NaiveDate::from_ymd_opt(2026, 2, 28).unwrap();
    for listed in [
        index.list(&tagged).unwrap(),
        list_library(&fx.library, &tagged).unwrap(),
    ] {
        assert_eq!(listed[0].review, guide.review);
        assert_eq!(listed[0].review_due(), Some(due));
        assert!(!listed[0].is_review_overdue(due));
        assert!(listed[0].is_review_overdue(due.succ_opt().unwrap()));
    }
    // Guides without the fields list them unset.
    let all = index.list(&ListFilter::default()).unwrap();
    assert!(all
        .iter()
        .filter(|g| g.id != guide.id)
        .all(|g| g.review == ReviewInfo::default() && g.review_due().is_none()));
}
