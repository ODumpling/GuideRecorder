//! Collections: manuals made of several guides.
//!
//! A [`Collection`] is an ordered list of [`Entry`]s, guides and text
//! between them, with an optional title and intro. It references guides by
//! id rather than copying them, so a change to a guide shows up in every
//! collection that has it. [`render`] resolves it into one [`RenderGuide`]
//! (each guide a [`RenderChapter`](crate::export::RenderChapter)), which the
//! PDF, Word, HTML and Markdown writers export with a table of contents.
//!
//! # Files
//!
//! Collections live in the library, like brand presets, so a synced or
//! shared library folder carries them along:
//!
//! ```text
//! <root>/
//!   .collections/
//!     01J….collection.json     one collection, named by its id
//! ```
//!
//! The folder is hidden, so the library never lists it as a guide folder.
//! Files are written atomically; a file that can't be read is listed with
//! its error rather than hiding the others.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ulid::Ulid;

use crate::export::ast::parse_blocks;
use crate::export::{RenderGuide, RenderItem, RenderTarget, RenderText};
use crate::library::{write_atomic, Durability, GuideHandle};
use crate::model::Guide;

/// The hidden folder in a library root that holds its collections.
pub const COLLECTIONS_DIR: &str = ".collections";
/// Extension of a collection file: `<id>.collection.json`.
pub const COLLECTION_EXTENSION: &str = ".collection.json";
/// The collection format this version writes.
pub const COLLECTION_VERSION: u32 = 1;
/// Largest collection file read.
pub const MAX_COLLECTION_BYTES: u64 = 4 * 1024 * 1024;
/// Most entries (guides and texts) in a collection.
pub const MAX_ENTRIES: usize = 500;
/// Longest title, in characters.
pub const MAX_TITLE_CHARS: usize = 200;
/// Longest intro or text entry, in characters.
pub const MAX_TEXT_CHARS: usize = 100_000;

/// How steps are numbered across the guides of a collection.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Numbering {
    /// 1, 2, 3, ... through the whole document.
    #[default]
    Continuous,
    /// Every guide starts again at 1.
    PerChapter,
}

/// One entry of a collection. `id` identifies the entry itself (a guide
/// may be in a collection more than once).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Entry {
    /// A guide of the library, by its id.
    Guide { id: Ulid, guide_id: Ulid },
    /// Text between guides, in the guide Markdown subset.
    Text { id: Ulid, body: String },
}

impl Entry {
    pub fn id(&self) -> Ulid {
        match self {
            Entry::Guide { id, .. } | Entry::Text { id, .. } => *id,
        }
    }
}

/// A manual: guides in order, with a title, an intro and text between
/// them (see the module docs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Collection {
    #[serde(default)]
    pub version: u32,
    pub id: Ulid,
    /// Plain text.
    #[serde(default)]
    pub title: String,
    /// Guide Markdown, before the first guide.
    #[serde(default)]
    pub intro: String,
    #[serde(default)]
    pub numbering: Numbering,
    #[serde(default)]
    pub entries: Vec<Entry>,
    pub updated_at: DateTime<Utc>,
}

impl Collection {
    /// An empty collection titled `title`.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            version: COLLECTION_VERSION,
            id: Ulid::new(),
            title: title.into(),
            intro: String::new(),
            numbering: Numbering::default(),
            entries: Vec::new(),
            updated_at: Utc::now(),
        }
    }

    /// The ids of its guides, in order (repeats included).
    pub fn guide_ids(&self) -> impl Iterator<Item = Ulid> + '_ {
        self.entries.iter().filter_map(|entry| match entry {
            Entry::Guide { guide_id, .. } => Some(*guide_id),
            Entry::Text { .. } => None,
        })
    }

    /// Checks the lengths and that entry ids are unique.
    pub fn validate(&self) -> Result<()> {
        if self.title.chars().count() > MAX_TITLE_CHARS {
            return Err(invalid(format!(
                "the title is over {MAX_TITLE_CHARS} characters"
            )));
        }
        if self.intro.chars().count() > MAX_TEXT_CHARS {
            return Err(invalid(format!(
                "the intro is over {MAX_TEXT_CHARS} characters"
            )));
        }
        if self.entries.len() > MAX_ENTRIES {
            return Err(invalid(format!(
                "a collection holds at most {MAX_ENTRIES} guides and texts"
            )));
        }
        let mut ids = std::collections::HashSet::new();
        for entry in &self.entries {
            if !ids.insert(entry.id()) {
                return Err(invalid(format!("entry {} is in twice", entry.id())));
            }
            if let Entry::Text { body, .. } = entry {
                if body.chars().count() > MAX_TEXT_CHARS {
                    return Err(invalid(format!(
                        "a text is over {MAX_TEXT_CHARS} characters"
                    )));
                }
            }
        }
        Ok(())
    }
}

/// Why a collection was refused or could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum CollectionError {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("{path}: not a valid collection: {source}")]
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("{0}: the collection file is over {MAX_COLLECTION_BYTES} bytes")]
    TooLarge(PathBuf),
    #[error("{0}: the collection was made by a newer version of GuideRecorder")]
    Newer(PathBuf),
    #[error("no collection {0} in this library")]
    NotFound(String),
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T, E = CollectionError> = std::result::Result<T, E>;

fn invalid(message: impl Into<String>) -> CollectionError {
    CollectionError::Invalid(message.into())
}

/// A collection as listed: its id, and the collection or why it can't be
/// read.
#[derive(Debug)]
pub struct CollectionEntry {
    pub id: Ulid,
    pub collection: Result<Collection>,
}

/// The collections of the library at `root` (see the module docs).
#[derive(Debug, Clone)]
pub struct CollectionStore {
    dir: PathBuf,
}

impl CollectionStore {
    pub fn new(library_root: impl AsRef<Path>) -> Self {
        Self {
            dir: library_root.as_ref().join(COLLECTIONS_DIR),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, id: Ulid) -> PathBuf {
        self.dir.join(format!("{id}{COLLECTION_EXTENSION}"))
    }

    fn io(path: &Path) -> impl FnOnce(io::Error) -> CollectionError + '_ {
        move |source| CollectionError::Io {
            path: path.to_path_buf(),
            source,
        }
    }

    /// Every collection, by title (ids break ties). No folder: none.
    pub fn list(&self) -> Result<Vec<CollectionEntry>> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(Self::io(&self.dir)(e)),
        };
        let mut collections = Vec::new();
        for entry in entries {
            let entry = entry.map_err(Self::io(&self.dir))?;
            let name = entry.file_name();
            let Some(id) = name
                .to_str()
                .and_then(|n| n.strip_suffix(COLLECTION_EXTENSION))
                .and_then(|id| Ulid::from_str(id).ok())
            else {
                continue;
            };
            collections.push(CollectionEntry {
                id,
                collection: self.read(id),
            });
        }
        let key = |e: &CollectionEntry| {
            let title = e.collection.as_ref().map(|c| c.title.to_lowercase()).ok();
            (title, e.id)
        };
        collections.sort_by_key(key);
        Ok(collections)
    }

    /// The collection with id `id`.
    pub fn read(&self, id: Ulid) -> Result<Collection> {
        let path = self.path(id);
        let file = match fs::File::open(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(CollectionError::NotFound(id.to_string()))
            }
            Err(e) => return Err(Self::io(&path)(e)),
        };
        let mut bytes = Vec::new();
        file.take(MAX_COLLECTION_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(Self::io(&path))?;
        if bytes.len() as u64 > MAX_COLLECTION_BYTES {
            return Err(CollectionError::TooLarge(path));
        }
        let collection: Collection =
            serde_json::from_slice(&bytes).map_err(|source| CollectionError::Json {
                path: path.clone(),
                source,
            })?;
        if collection.version > COLLECTION_VERSION {
            return Err(CollectionError::Newer(path));
        }
        if collection.id != id {
            return Err(invalid(format!(
                "{}: holds collection {}",
                path.display(),
                collection.id
            )));
        }
        Ok(collection)
    }

    /// Saves `collection` (validated first) under its id, stamped with the
    /// current version and time; returns what was written.
    pub fn save(&self, collection: &Collection) -> Result<Collection> {
        collection.validate()?;
        fs::create_dir_all(&self.dir).map_err(Self::io(&self.dir))?;
        let collection = Collection {
            version: COLLECTION_VERSION,
            title: collection.title.trim().to_owned(),
            updated_at: Utc::now(),
            ..collection.clone()
        };
        let path = self.path(collection.id);
        let mut json =
            serde_json::to_vec_pretty(&collection).map_err(|source| CollectionError::Json {
                path: path.clone(),
                source,
            })?;
        json.push(b'\n');
        write_atomic(&path, &json, Durability::Full).map_err(|e| invalid(e.to_string()))?;
        Ok(collection)
    }

    /// Deletes the collection `id`. Its guides stay as they are.
    pub fn delete(&self, id: Ulid) -> Result<()> {
        let path = self.path(id);
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                Err(CollectionError::NotFound(id.to_string()))
            }
            Err(e) => Err(Self::io(&path)(e)),
        }
    }
}

/// What [`render`] made of a collection.
#[derive(Debug, Clone)]
pub struct RenderedCollection {
    pub guide: RenderGuide,
    /// Guides the collection names that `resolve` did not find, in order.
    pub missing: Vec<Ulid>,
}

/// Resolves `collection` into one [`RenderGuide`]: the collection's title
/// and intro, then per entry a chapter with the guide's steps, or the text.
/// `resolve` loads a guide by id (`None`: gone, and it is left out); a step
/// keeps the bundle it came from, so its image is read from there.
///
/// Steps are numbered by [`Collection::numbering`]; anchors run on through
/// the whole document either way, so in-page links stay unique. A decision
/// option that leads to another guide of the collection leads to that
/// guide's chapter instead.
pub fn render(
    collection: &Collection,
    library_root: &Path,
    mut resolve: impl FnMut(Ulid) -> Option<(Guide, GuideHandle)>,
) -> RenderedCollection {
    let mut items = Vec::new();
    let mut missing = Vec::new();
    let mut language = None;
    let mut updated_at = collection.updated_at;
    let mut anchors = 0;
    let mut chapters = 0;
    // The first step of each chapter, by guide: (number, anchor, title).
    let mut firsts: Vec<(Ulid, u32, u32, String)> = Vec::new();
    for entry in &collection.entries {
        match entry {
            Entry::Text { body, .. } => {
                let body = parse_blocks(body);
                if !body.is_empty() {
                    items.push(RenderItem::Text(RenderText { body }));
                }
            }
            Entry::Guide { guide_id, .. } => {
                let Some((guide, handle)) = resolve(*guide_id) else {
                    missing.push(*guide_id);
                    continue;
                };
                language.get_or_insert_with(|| guide.language.clone());
                updated_at = updated_at.max(guide.updated_at);
                chapters += 1;
                let numbers = match collection.numbering {
                    Numbering::Continuous => anchors,
                    Numbering::PerChapter => 0,
                };
                let chapter =
                    RenderGuide::new(&guide, &handle).into_chapter(chapters, numbers, anchors);
                anchors += chapter.steps().count() as u32;
                if let Some(first) = chapter.steps().next() {
                    firsts.push((guide.id, first.number, first.anchor, guide.title.clone()));
                }
                items.extend(chapter.items);
            }
        }
    }
    for item in &mut items {
        let RenderItem::Decision(decision) = item else {
            continue;
        };
        for option in &mut decision.options {
            let RenderTarget::Guide { id, .. } = option.target else {
                continue;
            };
            if let Some((_, number, anchor, title)) = firsts.iter().find(|f| f.0 == id) {
                option.target = RenderTarget::Step {
                    number: Some(*number),
                    anchor: Some(*anchor),
                    section: Some(title.split_whitespace().collect::<Vec<_>>().join(" ")),
                };
            }
        }
    }
    let handle = GuideHandle {
        id: collection.id,
        path: library_root.join(COLLECTIONS_DIR),
    };
    let guide = RenderGuide {
        handle,
        id: collection.id,
        title: collection.title.clone(),
        summary: String::new(),
        language: language.unwrap_or_else(|| "en".to_owned()),
        kind: Default::default(),
        intro: parse_blocks(&collection.intro),
        prerequisites: Vec::new(),
        items,
        review: Default::default(),
        cover: None,
        updated_at,
    };
    RenderedCollection { guide, missing }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::{RenderChapter, RenderDecision, RenderOption};
    use crate::model::{
        BranchTarget, Decision, DecisionOption, ImageShow, Item, Step, StepImage, StepKind,
    };

    fn step(title: &str) -> Item {
        Item::Step(Step {
            id: Ulid::new(),
            title: title.into(),
            description: String::new(),
            kind: StepKind::Click,
            point: None,
            end_point: None,
            element: None,
            keystrokes: None,
            url: None,
            image: StepImage {
                full: "steps/x.webp".into(),
                crop: None,
                crop_rect: None,
                after: None,
                show: ImageShow::Full,
            },
            annotations: Vec::new(),
            narration: String::new(),
            ai_state: Default::default(),
        })
    }

    fn guide(title: &str, steps: usize) -> Guide {
        let mut guide = Guide::new(title, "de");
        guide.intro = format!("About {title}");
        guide.items = (0..steps).map(|i| step(&format!("{title} {i}"))).collect();
        guide
    }

    fn handle(guide: &Guide) -> GuideHandle {
        GuideHandle {
            id: guide.id,
            path: PathBuf::from(format!("/lib/{}.guiderecorder", guide.title)),
        }
    }

    fn entry(guide: &Guide) -> Entry {
        Entry::Guide {
            id: Ulid::new(),
            guide_id: guide.id,
        }
    }

    fn text(body: &str) -> Entry {
        Entry::Text {
            id: Ulid::new(),
            body: body.into(),
        }
    }

    fn rendered(collection: &Collection, guides: &[Guide]) -> RenderedCollection {
        render(collection, Path::new("/lib"), |id| {
            let guide = guides.iter().find(|g| g.id == id)?;
            Some((guide.clone(), handle(guide)))
        })
    }

    fn chapters(guide: &RenderGuide) -> Vec<&RenderChapter> {
        guide.chapters().collect()
    }

    #[test]
    fn guides_become_chapters_with_continuous_numbers() {
        let (a, b) = (guide("A", 2), guide("B", 3));
        let gone = Ulid::new();
        let mut collection = Collection::new("Manual");
        collection.intro = "Welcome".into();
        collection.entries = vec![
            entry(&a),
            text("Between"),
            Entry::Guide {
                id: Ulid::new(),
                guide_id: gone,
            },
            entry(&b),
            text("  "),
        ];
        let out = rendered(&collection, &[a.clone(), b.clone()]);
        assert_eq!(out.missing, [gone]);
        let render = out.guide;
        assert_eq!(render.title, "Manual");
        assert_eq!(render.language, "de");
        assert_eq!(render.intro.len(), 1);
        let chapters = chapters(&render);
        assert_eq!(chapters.len(), 2);
        assert_eq!((chapters[0].number, chapters[0].title.as_str()), (1, "A"));
        assert_eq!((chapters[1].number, chapters[1].title.as_str()), (2, "B"));
        assert_eq!(chapters[1].intro.len(), 1);
        let numbers: Vec<(u32, u32)> = render.steps().map(|s| (s.number, s.anchor)).collect();
        assert_eq!(numbers, [(1, 1), (2, 2), (3, 3), (4, 4), (5, 5)]);
        // Each step reads its image from its own guide's bundle.
        let paths: Vec<_> = render.steps().map(|s| s.handle.path.clone()).collect();
        assert_eq!(paths[0], handle(&a).path);
        assert_eq!(paths[4], handle(&b).path);
        // The blank text is left out; the other sits between the chapters.
        let texts = render
            .items
            .iter()
            .filter(|i| matches!(i, RenderItem::Text(_)))
            .count();
        assert_eq!(texts, 1);
        assert!(matches!(render.items[3], RenderItem::Text(_)));
    }

    #[test]
    fn per_chapter_numbers_restart_but_anchors_do_not() {
        let (a, b) = (guide("A", 2), guide("B", 2));
        let mut collection = Collection::new("Manual");
        collection.numbering = Numbering::PerChapter;
        collection.entries = vec![entry(&a), entry(&b)];
        let render = rendered(&collection, &[a, b]).guide;
        let numbers: Vec<(u32, u32)> = render.steps().map(|s| (s.number, s.anchor)).collect();
        assert_eq!(numbers, [(1, 1), (2, 2), (1, 3), (2, 4)]);
    }

    #[test]
    fn decisions_follow_the_numbering_and_lead_to_chapters() {
        let a = guide("A", 1);
        let mut b = guide("B", 2);
        let target = b.items[1].id();
        let ask = |target| {
            Item::Decision(Decision {
                id: Ulid::new(),
                question: "Which?".into(),
                options: vec![DecisionOption {
                    id: Ulid::new(),
                    label: "x".into(),
                    target,
                }],
            })
        };
        b.items.insert(0, ask(BranchTarget::Step { id: target }));
        b.items.push(ask(BranchTarget::Guide {
            id: a.id,
            title: "A".into(),
        }));
        let mut collection = Collection::new("Manual");
        collection.entries = vec![entry(&a), entry(&b)];
        let continuous = rendered(&collection, &[a.clone(), b.clone()]).guide;
        let targets = |render: &RenderGuide| -> Vec<RenderTarget> {
            render
                .decisions()
                .flat_map(|d: &RenderDecision| {
                    d.options.iter().map(|o: &RenderOption| o.target.clone())
                })
                .collect()
        };
        assert_eq!(
            targets(&continuous),
            [
                RenderTarget::Step {
                    number: Some(3),
                    anchor: Some(3),
                    section: None
                },
                RenderTarget::Step {
                    number: Some(1),
                    anchor: Some(1),
                    section: Some("A".into())
                },
            ]
        );
        collection.numbering = Numbering::PerChapter;
        let per_chapter = rendered(&collection, &[a, b]).guide;
        assert_eq!(
            targets(&per_chapter)[0],
            RenderTarget::Step {
                number: Some(2),
                anchor: Some(3),
                section: None
            }
        );
    }

    #[test]
    fn store_round_trips_and_lists_broken_files() {
        let dir = tempfile::tempdir().unwrap();
        let store = CollectionStore::new(dir.path());
        assert!(store.list().unwrap().is_empty());
        let mut collection = Collection::new("  Onboarding ");
        collection.entries = vec![text("Hi")];
        let saved = store.save(&collection).unwrap();
        assert_eq!(saved.title, "Onboarding");
        assert_eq!(store.read(collection.id).unwrap(), saved);

        let broken = Ulid::new();
        fs::write(store.path(broken), b"{").unwrap();
        fs::write(store.dir().join("notes.txt"), b"x").unwrap();
        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 2);
        assert!(listed
            .iter()
            .any(|e| e.id == broken && e.collection.is_err()));

        store.delete(collection.id).unwrap();
        assert!(matches!(
            store.read(collection.id),
            Err(CollectionError::NotFound(_))
        ));
        assert!(matches!(
            store.delete(collection.id),
            Err(CollectionError::NotFound(_))
        ));
    }

    #[test]
    fn refuses_what_it_cannot_store() {
        let mut collection = Collection::new("x".repeat(MAX_TITLE_CHARS + 1));
        assert!(collection.validate().is_err());
        collection.title = "ok".into();
        let twice = text("a");
        collection.entries = vec![twice.clone(), twice];
        assert!(collection.validate().is_err());
        collection.entries = vec![text(&"a".repeat(MAX_TEXT_CHARS + 1))];
        assert!(collection.validate().is_err());
    }

    #[test]
    fn newer_and_misnamed_files_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let store = CollectionStore::new(dir.path());
        let collection = store.save(&Collection::new("A")).unwrap();
        let mut newer = collection.clone();
        newer.version = COLLECTION_VERSION + 1;
        fs::write(
            store.path(collection.id),
            serde_json::to_vec(&newer).unwrap(),
        )
        .unwrap();
        assert!(matches!(
            store.read(collection.id),
            Err(CollectionError::Newer(_))
        ));
        let other = Ulid::new();
        fs::write(store.path(other), serde_json::to_vec(&collection).unwrap()).unwrap();
        assert!(matches!(
            store.read(other),
            Err(CollectionError::Invalid(_))
        ));
    }

    #[test]
    fn file_format() {
        let json = r#"{"id":"01JA0000000000000000000000","title":"M","numbering":"per-chapter",
            "entries":[{"type":"guide","id":"01JA0000000000000000000001","guideId":"01JA0000000000000000000002"},
                       {"type":"text","id":"01JA0000000000000000000003","body":"Hi"}],
            "updatedAt":"2026-10-23T00:00:00Z"}"#;
        let collection: Collection = serde_json::from_str(json).unwrap();
        assert_eq!(collection.numbering, Numbering::PerChapter);
        assert_eq!(collection.entries.len(), 2);
        assert_eq!(collection.guide_ids().count(), 1);
        assert_eq!(collection.version, 0);
    }
}
