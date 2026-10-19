//! SQLite + FTS5 search index over a library. See design scratchpad #444
//! ("SQLite index").
//!
//! The index is a disposable cache of the library's `guide.json` files: it
//! can be deleted at any time and rebuilt with [`Index::rebuild`]. It holds
//! one index per library; [`Index::sync`] against another library root
//! rebuilds it.
//!
//! Writer and readers follow the library's rules (see [`crate::library`]):
//!
//! - The app is the only writer. It opens the index with [`Index::open`]
//!   (WAL journal, busy timeout), and keeps it current with [`Index::sync`]
//!   at startup and [`Index::update`] / [`Index::remove`] from its file
//!   watcher. The index is built from [`Library::read`], which never writes,
//!   so indexing never migrates or rewrites a guide, nor changes which
//!   duplicate copy the library treats as canonical.
//! - MCP and the CLI open it with [`Index::open_read_only`]. When the file is
//!   missing, from another format version or [`Index::is_stale`], they fall
//!   back to [`search_library`] / [`list_library`], which scan the folders
//!   without SQLite. [`search_with_fallback`] and [`list_with_fallback`] do
//!   all of that, reopening the index on every call. A long-lived reader
//!   [`Index`] should do the same: reopen when a query fails with
//!   [`IndexError::Incompatible`] (the app upgraded the format) or when
//!   [`Index::is_stale`] stays true (the app may have replaced the file).
//!
//! # Safety of the file
//!
//! The index is disposable, but the path may be misconfigured, so the writer
//! only ever recreates a file it can prove is a GuideRecorder index: one stamped
//! with [`APPLICATION_ID`] (or an empty file). Any other file fails with
//! [`IndexError::NotAnIndex`] and is left untouched. An index of ours from
//! another format version is rebuilt in place; a corrupt one (found by
//! `PRAGMA quick_check` on open, or `SQLITE_CORRUPT` during
//! [`Index::sync`] or [`Index::rebuild`]) is renamed aside to
//! `<name>.corrupt-<unix ms>` (with its `-wal` and `-shm`) and rebuilt.
//!
//! Staleness is decided per bundle by the `guide.json` modification time and
//! size, plus the set of bundles in the root. Indexing reads the stamp before
//! the file, so a guide that changes while it is read is picked up again by
//! the next sync.
//!
//! Bundles that hold the same guide id (e.g. a Finder copy) are all indexed.
//! Each copy but the canonical one (as [`Library::find`] picks it) is marked
//! with [`IndexedGuide::duplicate_of`], and search leaves the copies out
//! unless [`SearchOptions::include_duplicates`] is set.
//!
//! # Search
//!
//! `steps_fts` has one row per guide (title, summary, tags, intro and
//! prerequisites) and one per item: step (title, and description plus the
//! titles and descriptions of its hotspots), section (title) and note
//! (body). Item rows also hold the guide title, so a query
//! can mix both. Everything is plain text ([`markdown::to_plain_text`]).
//! Hits ([`StepHit`]) carry the item's id and [`HitKind`], and are ranked by
//! bm25 with column weights ([`WEIGHT_ITEM_TITLE`] and so on).
//!
//! An item hit needs at least one query word in the item's own title or
//! body; the guide title in its row only lets queries mix guide and item
//! words. Otherwise a query for a guide's title would return every step of
//! that guide.
//!
//! User input never reaches FTS5 syntax: every word (split on whitespace
//! and control characters) becomes a quoted phrase, words are ANDed, and
//! the last word is a prefix (search as you type). See [`fts_query`]. The tokenizer is `unicode61`
//! with diacritics removed, so matching ignores case and accents.
//! [`search_library`] matches the same words but folds case only.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use chrono::{DateTime, Utc};
use rusqlite::types::Type;
use rusqlite::{params, Connection, ErrorCode, OpenFlags, OptionalExtension, Transaction};
use ulid::Ulid;

use crate::library::{
    bundle_paths, is_bundle_extension, GuideHandle, Library, LibraryError, GUIDE_FILE,
};
use crate::markdown;
use crate::model::{Guide, Item, ReviewInfo, Shape};

/// Version of the index layout. Bump it whenever the tables or what goes
/// into them change; an index at another version is rebuilt by the writer
/// and reported stale to readers.
pub const FORMAT_VERSION: i64 = 6;

/// `PRAGMA application_id` of a GuideRecorder index ("GYix"). The writer only
/// recreates files that carry it.
pub const APPLICATION_ID: i32 = 0x4759_6978;

/// bm25 weight of the guide title (every row).
pub const WEIGHT_GUIDE_TITLE: f64 = 3.0;
/// bm25 weight of the guide summary and tags (guide rows).
pub const WEIGHT_GUIDE_META: f64 = 1.0;
/// bm25 weight of a step or section title.
pub const WEIGHT_ITEM_TITLE: f64 = 10.0;
/// bm25 weight of a step description, a note body, and the guide intro and
/// prerequisites.
pub const WEIGHT_ITEM_BODY: f64 = 4.0;

/// Marks around matched words in [`StepHit::snippet`] (Markdown bold).
pub const HIGHLIGHT: &str = "**";
/// Marks text cut from either end of a snippet.
pub const ELLIPSIS: &str = "…";
/// Most words in a snippet.
pub const SNIPPET_TOKENS: usize = 12;
/// Most characters of a note's first line used as its [`StepHit::title`].
pub const NOTE_TITLE_CHARS: usize = 80;

/// How long a connection waits on a lock before failing with `SQLITE_BUSY`.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

const META_FORMAT: &str = "format_version";
const META_SCHEMA: &str = "guide_schema_version";
const META_ROOT: &str = "library_root";

const WEIGHTS: [f64; 4] = [
    WEIGHT_GUIDE_TITLE,
    WEIGHT_GUIDE_META,
    WEIGHT_ITEM_TITLE,
    WEIGHT_ITEM_BODY,
];

const SCHEMA_SQL: &str = "
CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
-- One row per bundle folder in the library, readable or not.
CREATE TABLE guides (
    bundle         TEXT PRIMARY KEY, -- path relative to the root, '/' separated,
                                     -- e.g. 'Work/My Guide.guiderecorder'
    mtime_ns       INTEGER,          -- guide.json; NULL if missing
    size           INTEGER,
    error          TEXT,             -- set if unreadable; the fields below are NULL
    id             TEXT,
    schema_version INTEGER,
    read_only      INTEGER,
    title          TEXT,
    summary        TEXT,
    tags           TEXT,             -- JSON array as written
    tags_folded    TEXT,             -- JSON array, lowercase
    step_count     INTEGER,
    updated_us     INTEGER,          -- microseconds since the Unix epoch
    created_us     INTEGER,
    review         TEXT,             -- JSON ReviewInfo; NULL if no field is set
    duplicate_of   TEXT              -- canonical bundle holding the same id
);
CREATE INDEX guides_updated ON guides (updated_us);
CREATE INDEX guides_id ON guides (id);
-- steps_fts rows by bundle: FTS5 cannot index its UNINDEXED columns, so
-- deleting a bundle's rows by `bundle` would scan the whole table.
CREATE TABLE fts_rows (
    fts_rowid INTEGER PRIMARY KEY,
    bundle    TEXT NOT NULL
);
CREATE INDEX fts_rows_bundle ON fts_rows (bundle);
CREATE VIRTUAL TABLE steps_fts USING fts5 (
    bundle UNINDEXED,
    item_id UNINDEXED,    -- NULL for the guide's own row
    kind UNINDEXED,       -- HitKind::as_str
    position UNINDEXED,   -- index in guide.items; NULL for the guide row
    step_index UNINDEXED, -- index among the steps; NULL unless a step
    guide_title,
    meta,
    title,
    body,
    tokenize = 'unicode61 remove_diacritics 2',
    prefix = '2 3'
);
";

#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error("index: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error(transparent)]
    Library(#[from] LibraryError),
    /// The index was written by another format version (or is not an index
    /// at all). Readers fall back; the writer rebuilds it on open.
    #[error("index format {found:?} does not match {expected}")]
    Incompatible {
        found: Option<String>,
        expected: i64,
    },
    /// The file is not a GuideRecorder index (no [`APPLICATION_ID`]). It was not
    /// touched.
    #[error("{0}: not a GuideRecorder index; refusing to overwrite it")]
    NotAnIndex(PathBuf),
    /// A write on an index opened with [`Index::open_read_only`].
    #[error("index is open read-only")]
    ReadOnly,
}

pub type Result<T, E = IndexError> = std::result::Result<T, E>;

/// A guide as listed by the index.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexedGuide {
    pub id: Ulid,
    /// Absolute path of the `.guiderecorder` folder.
    pub path: PathBuf,
    pub title: String,
    pub summary: String,
    pub tags: Vec<String>,
    pub step_count: usize,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// The library folder holding the bundle (see
    /// [`crate::library::GuideSummary::folder`]).
    pub folder: String,
    /// Owner, status, version and review schedule.
    pub review: ReviewInfo,
    /// The guide is from a newer app or not yet migrated (see
    /// [`crate::library::LoadedGuide::read_only`]).
    pub read_only: bool,
    /// Set on a copy of another bundle with the same id: the canonical
    /// bundle's path (see [`crate::library::GuideSummary::duplicate_of`]).
    pub duplicate_of: Option<PathBuf>,
}

impl IndexedGuide {
    pub fn handle(&self) -> GuideHandle {
        GuideHandle {
            id: self.id,
            path: self.path.clone(),
        }
    }

    /// When the next review is due (see [`ReviewInfo::review_due`]).
    pub fn review_due(&self) -> Option<chrono::NaiveDate> {
        self.review.review_due(self.created_at.date_naive())
    }

    /// Whether the review is overdue on `today` (see
    /// [`ReviewInfo::is_review_overdue`]).
    pub fn is_review_overdue(&self, today: chrono::NaiveDate) -> bool {
        self.review
            .is_review_overdue(self.created_at.date_naive(), today)
    }
}

/// Filter for [`Index::list`] and [`list_library`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListFilter {
    /// Only guides with this tag (case-insensitive).
    pub tag: Option<String>,
    /// Only guides whose title, summary, tags or steps match (see
    /// [`fts_query`]); results are then ordered by best match.
    pub query: Option<String>,
    pub limit: Option<usize>,
}

/// What a [`StepHit`] matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum HitKind {
    /// The guide's title, summary, tags, intro or prerequisites.
    Guide,
    Step,
    Section,
    Note,
}

impl HitKind {
    pub fn as_str(self) -> &'static str {
        match self {
            HitKind::Guide => "guide",
            HitKind::Step => "step",
            HitKind::Section => "section",
            HitKind::Note => "note",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        [Self::Guide, Self::Step, Self::Section, Self::Note]
            .into_iter()
            .find(|kind| kind.as_str() == s)
    }
}

/// One search result: the guide itself, or one of its items.
#[derive(Debug, Clone, PartialEq)]
pub struct StepHit {
    pub guide_id: Ulid,
    /// Absolute path of the `.guiderecorder` folder.
    pub guide_path: PathBuf,
    pub guide_title: String,
    pub kind: HitKind,
    /// The step, section or note; `None` for [`HitKind::Guide`].
    pub item_id: Option<Ulid>,
    /// Position among the guide's steps, from 0; only for [`HitKind::Step`].
    pub step_index: Option<usize>,
    /// Plain text: the guide, step or section title, or a note's first line
    /// (at most [`NOTE_TITLE_CHARS`] characters).
    pub title: String,
    /// Plain text around the match, matched words in [`HIGHLIGHT`] marks,
    /// cut ends marked with [`ELLIPSIS`].
    pub snippet: String,
    /// Lower is better (bm25 is negative). Comparable only within one search.
    pub rank: f64,
}

/// Options for [`Index::search_with`] and [`search_library_with`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchOptions {
    pub limit: usize,
    /// Also return hits in duplicate copies (see
    /// [`IndexedGuide::duplicate_of`]); by default only the canonical bundle.
    pub include_duplicates: bool,
}

impl SearchOptions {
    pub fn limit(limit: usize) -> Self {
        Self {
            limit,
            include_duplicates: false,
        }
    }
}

/// What [`Index::sync`] changed, as bundle paths relative to the root.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub added: Vec<String>,
    pub updated: Vec<String>,
    pub removed: Vec<String>,
    /// Whether the whole index was rebuilt (new index or another library).
    pub rebuilt: bool,
}

/// Where a `*_with_fallback` result came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Index,
    /// The index was missing, incompatible or stale; the folders were scanned.
    Scan,
}

/// `guide.json`'s modification time and size, the staleness key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    mtime_ns: i64,
    size: i64,
}

/// An open index.
#[derive(Debug)]
pub struct Index {
    conn: Connection,
    path: PathBuf,
    read_only: bool,
    /// The format version matches ([`Index::open`] guarantees it).
    compatible: bool,
    /// The file opened, to notice when the writer replaced it.
    file_id: Option<FileId>,
}

/// What [`Index::open`] found at the path.
enum Existing {
    /// Missing, empty, or a database nothing was written to.
    New,
    Ours,
    OursCorrupt,
    Foreign,
}

impl Index {
    /// Where the app keeps its index and readers look for it:
    /// `~/Library/Application Support/dev.dufflebear.guiderecorder/index.sqlite` on
    /// macOS (from `$HOME`); `None` elsewhere or without `$HOME`.
    pub fn default_path() -> Option<PathBuf> {
        if !cfg!(target_os = "macos") {
            return None;
        }
        let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
        Some(
            PathBuf::from(home)
                .join("Library/Application Support/dev.dufflebear.guiderecorder/index.sqlite"),
        )
    }

    /// Opens the index at `path` for writing, creating it (and its folder)
    /// if missing. An index from another format version is recreated empty
    /// and a corrupt one is renamed aside; [`Index::sync`] then rebuilds it.
    /// A file that is not a GuideRecorder index fails with
    /// [`IndexError::NotAnIndex`] and is not touched.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent).map_err(|source| IndexError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        match inspect(path)? {
            Existing::Foreign => Err(IndexError::NotAnIndex(path.to_path_buf())),
            Existing::OursCorrupt => {
                set_aside(path)?;
                Self::connect_writer(path)
            }
            Existing::New | Existing::Ours => Self::connect_writer(path),
        }
    }

    /// Opens `path` (new or known to be ours) and creates the schema unless
    /// it is at the current format.
    fn connect_writer(path: &Path) -> Result<Self> {
        let mut conn = Connection::open(path)?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        if read_format(&conn)?.as_deref() != Some(&expected_format()) {
            let tx = conn.transaction()?;
            tx.execute_batch(
                "DROP TABLE IF EXISTS steps_fts;
                 DROP TABLE IF EXISTS fts_rows;
                 DROP TABLE IF EXISTS guides;
                 DROP TABLE IF EXISTS meta;",
            )?;
            tx.execute_batch(SCHEMA_SQL)?;
            tx.pragma_update(None, "application_id", APPLICATION_ID)?;
            tx.execute(
                "INSERT INTO meta (key, value) VALUES (?1, ?2), (?3, ?4)",
                params![
                    META_FORMAT,
                    FORMAT_VERSION.to_string(),
                    META_SCHEMA,
                    crate::SCHEMA_VERSION.to_string()
                ],
            )?;
            tx.commit()?;
        }
        Ok(Self {
            conn,
            path: path.to_path_buf(),
            read_only: false,
            compatible: true,
            file_id: file_id(path),
        })
    }

    /// Opens an existing index for reading (MCP, CLI). Fails if the file is
    /// missing or is not a GuideRecorder index. An index from another format
    /// version opens, but [`Index::is_stale`] says so and queries fail with
    /// [`IndexError::Incompatible`]. A long-lived reader should reopen when
    /// that happens (see the module docs).
    pub fn open_read_only(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        // SQLite reports a missing file as a vague CANTOPEN.
        fs::metadata(path).map_err(|source| IndexError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        let ours = conn
            .query_row("PRAGMA application_id", [], |row| row.get::<_, i32>(0))
            .is_ok_and(|id| id == APPLICATION_ID);
        if !ours {
            return Err(IndexError::NotAnIndex(path.to_path_buf()));
        }
        let compatible = match read_format(&conn) {
            Ok(found) => found.as_deref() == Some(&expected_format()),
            Err(IndexError::Sqlite(e)) if is_corrupt(&e) => false,
            Err(e) => return Err(e),
        };
        Ok(Self {
            conn,
            path: path.to_path_buf(),
            read_only: true,
            compatible,
            file_id: file_id(path),
        })
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// Whether the index is out of date for `library`: another format
    /// version, another library, or a bundle added, removed or changed
    /// (`guide.json` modification time or size) since it was indexed.
    /// Also true when the index file was replaced since it was opened (the
    /// writer set a corrupt one aside) or its format changed (the app was
    /// upgraded); reopen to see the new one.
    /// Costs a directory listing and a `stat` per bundle.
    pub fn is_stale(&self, library: &Library) -> Result<bool> {
        if !self.compatible
            || file_id(&self.path) != self.file_id
            || read_format(&self.conn)?.as_deref() != Some(&expected_format())
            || self.root()?.as_deref() != Some(library.root())
        {
            return Ok(true);
        }
        Ok(bundles_on_disk(library.root())? != self.stamps()?)
    }

    /// Re-reads every bundle and replaces the whole index in one
    /// transaction. Readers see the old index until it commits. A corrupt
    /// index is set aside and built anew (see the module docs).
    pub fn rebuild(&mut self, library: &Library) -> Result<SyncReport> {
        self.check_writable()?;
        let result = self.rebuild_inner(library);
        self.recover_if_corrupt(result, library)
    }

    fn rebuild_inner(&mut self, library: &Library) -> Result<SyncReport> {
        let disk = bundles_on_disk(library.root())?;
        let entries = read_entries(library, disk.keys());
        let tx = self.conn.transaction()?;
        tx.execute_batch("DELETE FROM steps_fts; DELETE FROM fts_rows; DELETE FROM guides;")?;
        for (bundle, (stamp, entry)) in &entries {
            insert(&tx, bundle, *stamp, entry)?;
        }
        set_root(&tx, library.root())?;
        mark_duplicates(&tx, Some(library))?;
        tx.commit()?;
        Ok(SyncReport {
            added: entries.into_keys().collect(),
            rebuilt: true,
            ..SyncReport::default()
        })
    }

    /// Brings the index up to date with `library`: re-reads bundles whose
    /// `guide.json` changed (modification time or size), adds new ones and
    /// drops ones that are gone. Rebuilds if the index belongs to another
    /// library, or is corrupt (see the module docs).
    pub fn sync(&mut self, library: &Library) -> Result<SyncReport> {
        self.check_writable()?;
        let result = self.sync_inner(library);
        self.recover_if_corrupt(result, library)
    }

    fn sync_inner(&mut self, library: &Library) -> Result<SyncReport> {
        if self.root()?.as_deref() != Some(library.root()) {
            return self.rebuild_inner(library);
        }
        let disk = bundles_on_disk(library.root())?;
        let indexed = self.stamps()?;
        let changed: Vec<&String> = disk
            .iter()
            .filter(|(bundle, stamp)| indexed.get(*bundle) != Some(*stamp))
            .map(|(bundle, _)| bundle)
            .collect();
        let entries = read_entries(library, changed);
        let mut report = SyncReport::default();
        let tx = self.conn.transaction()?;
        for (bundle, (stamp, entry)) in entries {
            delete(&tx, &bundle)?;
            insert(&tx, &bundle, stamp, &entry)?;
            if indexed.contains_key(&bundle) {
                report.updated.push(bundle);
            } else {
                report.added.push(bundle);
            }
        }
        for bundle in indexed.keys().filter(|b| !disk.contains_key(*b)) {
            delete(&tx, bundle)?;
            report.removed.push(bundle.clone());
        }
        mark_duplicates(&tx, Some(library))?;
        tx.commit()?;
        Ok(report)
    }

    /// Re-indexes the bundle at `path` (a `.guiderecorder` folder in the
    /// library root or one of its folders), or drops it if it is gone. For
    /// the app's file watcher.
    pub fn update(&mut self, library: &Library, path: impl AsRef<Path>) -> Result<()> {
        self.check_writable()?;
        if self.root()?.as_deref() != Some(library.root()) {
            self.rebuild(library)?;
            return Ok(());
        }
        let Some(bundle) = bundle_name(library.root(), path.as_ref()) else {
            return Err(LibraryError::NotABundle(path.as_ref().to_path_buf()).into());
        };
        let bundle_path = library.root().join(&bundle);
        let read = bundle_path
            .is_dir()
            .then(|| read_entry(library, &bundle_path));
        let tx = self.conn.transaction()?;
        delete(&tx, &bundle)?;
        if let Some((stamp, entry)) = read {
            insert(&tx, &bundle, stamp, &entry)?;
        }
        mark_duplicates(&tx, Some(library))?;
        tx.commit()?;
        Ok(())
    }

    /// Drops the bundle at `path` from the index.
    pub fn remove(&mut self, path: impl AsRef<Path>) -> Result<()> {
        self.check_writable()?;
        let Some(root) = self.root()? else {
            return Ok(());
        };
        if let Some(bundle) = bundle_name(&root, path.as_ref()) {
            let tx = self.conn.transaction()?;
            delete(&tx, &bundle)?;
            mark_duplicates(&tx, None)?;
            tx.commit()?;
        }
        Ok(())
    }

    /// Indexed guides, most recently updated first, or best match first when
    /// `filter.query` is set. Unreadable bundles are left out; duplicate
    /// copies are listed with [`IndexedGuide::duplicate_of`] set.
    pub fn list(&self, filter: &ListFilter) -> Result<Vec<IndexedGuide>> {
        self.check_compatible()?;
        let Some(root) = self.root()? else {
            return Ok(Vec::new());
        };
        let tag = filter.tag.as_deref().map(str::to_lowercase);
        let limit = sql_limit(filter.limit);
        let columns = "g.bundle, g.id, g.title, g.summary, g.tags, g.step_count, g.updated_us, \
                       g.read_only, g.duplicate_of, g.created_us, g.review";
        let tag_clause = "(?1 IS NULL OR EXISTS \
                          (SELECT 1 FROM json_each(g.tags_folded) WHERE value = ?1))";
        let match_query = filter.query.as_deref().map(fts_queries);
        let mut guides = Vec::new();
        let mut push = |row: &rusqlite::Row<'_>| -> rusqlite::Result<()> {
            let bundle: String = row.get(0)?;
            let updated_at = parse_micros(row, 6)?;
            guides.push(IndexedGuide {
                path: root.join(&bundle),
                folder: bundle_folder(&bundle).to_owned(),
                id: parse_ulid(row, 1)?,
                title: row.get(2)?,
                summary: row.get(3)?,
                tags: parse_tags(row, 4)?,
                step_count: row.get::<_, i64>(5)? as usize,
                created_at: match row.get::<_, Option<i64>>(9)? {
                    Some(_) => parse_micros(row, 9)?,
                    None => updated_at,
                },
                updated_at,
                review: parse_review(row, 10)?,
                read_only: row.get(7)?,
                duplicate_of: row.get::<_, Option<String>>(8)?.map(|b| root.join(b)),
            });
            Ok(())
        };
        match match_query {
            None => {
                let sql = format!(
                    "SELECT {columns} FROM guides g WHERE g.error IS NULL AND {tag_clause} \
                     ORDER BY g.updated_us DESC, g.id, g.bundle LIMIT ?2"
                );
                let mut run = || -> rusqlite::Result<()> {
                    let mut stmt = self.conn.prepare(&sql)?;
                    let mut rows = stmt.query(params![tag, limit])?;
                    while let Some(row) = rows.next()? {
                        push(row)?;
                    }
                    Ok(())
                };
                run().map_err(|e| self.query_error(e))?;
            }
            Some(None) => {}
            Some(Some((query, own))) => {
                let sql = format!(
                    // Materialized: bm25() cannot run inside the aggregate.
                    "WITH hits AS MATERIALIZED (
                         SELECT bundle, {bm25} AS score FROM steps_fts
                         WHERE steps_fts MATCH ?3 AND {own_clause}
                     ), m AS (
                         SELECT bundle, min(score) AS best FROM hits GROUP BY bundle
                     )
                     SELECT {columns} FROM m JOIN guides g ON g.bundle = m.bundle
                     WHERE g.error IS NULL AND {tag_clause}
                     ORDER BY m.best, g.updated_us DESC, g.bundle LIMIT ?2",
                    bm25 = bm25_sql(),
                    own_clause = own_words_clause("?4"),
                );
                let mut run = || -> rusqlite::Result<()> {
                    let mut stmt = self.conn.prepare(&sql)?;
                    let mut rows = stmt.query(params![tag, limit, query, own])?;
                    while let Some(row) = rows.next()? {
                        push(row)?;
                    }
                    Ok(())
                };
                run().map_err(|e| self.query_error(e))?;
            }
        }
        Ok(guides)
    }

    /// Full-text search over guides and their items, best first, leaving
    /// out duplicate copies. A query without any word characters returns
    /// nothing. See [`fts_query`].
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<StepHit>> {
        self.search_with(query, &SearchOptions::limit(limit))
    }

    /// [`Index::search`] with options.
    pub fn search_with(&self, query: &str, options: &SearchOptions) -> Result<Vec<StepHit>> {
        self.check_compatible()?;
        let (Some(root), Some((query, own))) = (self.root()?, fts_queries(query)) else {
            return Ok(Vec::new());
        };
        let sql = format!(
            "SELECT steps_fts.bundle, g.id, g.title, steps_fts.kind, steps_fts.item_id,
                    steps_fts.step_index, steps_fts.title, steps_fts.body,
                    snippet(steps_fts, -1, ?3, ?3, ?4, ?5), {bm25} AS score
             FROM steps_fts JOIN guides g ON g.bundle = steps_fts.bundle
             WHERE steps_fts MATCH ?1 AND {own_clause} AND (?6 OR g.duplicate_of IS NULL)
             ORDER BY score, g.title, g.bundle, steps_fts.position IS NOT NULL,
                      steps_fts.position
             LIMIT ?2",
            bm25 = bm25_sql(),
            own_clause = own_words_clause("?7"),
        );
        let run = || -> rusqlite::Result<Vec<StepHit>> {
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(
                params![
                    query,
                    sql_limit(Some(options.limit)),
                    HIGHLIGHT,
                    ELLIPSIS,
                    SNIPPET_TOKENS as i64,
                    options.include_duplicates,
                    own,
                ],
                |row| {
                    let kind_text: String = row.get(3)?;
                    let kind = HitKind::parse(&kind_text).ok_or_else(|| {
                        conversion_error(3, io::Error::other(format!("unknown kind {kind_text:?}")))
                    })?;
                    let item_id = row
                        .get::<_, Option<String>>(4)?
                        .map(|_| parse_ulid(row, 4))
                        .transpose()?;
                    let guide_title: String = row.get(2)?;
                    let title = match kind {
                        HitKind::Guide => guide_title.clone(),
                        HitKind::Step | HitKind::Section => row.get(6)?,
                        HitKind::Note => note_title(&row.get::<_, String>(7)?),
                    };
                    Ok(StepHit {
                        guide_path: root.join(row.get::<_, String>(0)?),
                        guide_id: parse_ulid(row, 1)?,
                        guide_title,
                        kind,
                        item_id,
                        step_index: row.get::<_, Option<i64>>(5)?.map(|i| i as usize),
                        title,
                        snippet: row.get(8)?,
                        rank: row.get(9)?,
                    })
                },
            )?;
            rows.collect()
        };
        run().map_err(|e| self.query_error(e))
    }

    /// After `SQLITE_CORRUPT` from a write: sets the file aside, starts a
    /// new index and rebuilds it. The file was opened by [`Index::open`], so
    /// it is ours.
    fn recover_if_corrupt(
        &mut self,
        result: Result<SyncReport>,
        library: &Library,
    ) -> Result<SyncReport> {
        match result {
            Err(IndexError::Sqlite(e)) if is_corrupt(&e) => {
                let closed = std::mem::replace(&mut self.conn, Connection::open_in_memory()?);
                // Errors closing a corrupt database do not matter here.
                let _ = closed.close();
                set_aside(&self.path)?;
                *self = Self::connect_writer(&self.path)?;
                self.rebuild_inner(library)
            }
            other => other,
        }
    }

    /// Turns a failed query into [`IndexError::Incompatible`] when the
    /// writer changed the format under a long-lived reader.
    fn query_error(&self, e: rusqlite::Error) -> IndexError {
        match read_format(&self.conn) {
            Ok(found) if found.as_deref() != Some(&expected_format()) => IndexError::Incompatible {
                found,
                expected: FORMAT_VERSION,
            },
            _ => e.into(),
        }
    }

    fn check_writable(&self) -> Result<()> {
        if self.read_only {
            Err(IndexError::ReadOnly)
        } else {
            Ok(())
        }
    }

    fn check_compatible(&self) -> Result<()> {
        if self.compatible {
            return Ok(());
        }
        Err(IndexError::Incompatible {
            found: read_format(&self.conn).ok().flatten(),
            expected: FORMAT_VERSION,
        })
    }

    /// The library root the index was built from, if any.
    fn root(&self) -> Result<Option<PathBuf>> {
        if !self.compatible {
            return Ok(None);
        }
        Ok(self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                [META_ROOT],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(PathBuf::from))
    }

    fn stamps(&self) -> Result<BTreeMap<String, Option<Stamp>>> {
        let mut stmt = self
            .conn
            .prepare("SELECT bundle, mtime_ns, size FROM guides")?;
        let rows = stmt.query_map([], |row| {
            let stamp = match (row.get(1)?, row.get(2)?) {
                (Some(mtime_ns), Some(size)) => Some(Stamp { mtime_ns, size }),
                _ => None,
            };
            Ok((row.get(0)?, stamp))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

/// Searches the index at `index_path` if it is present, compatible and
/// current for `library`, and scans the library otherwise. For readers.
pub fn search_with_fallback(
    index_path: impl AsRef<Path>,
    library: &Library,
    query: &str,
    limit: usize,
) -> Result<(Vec<StepHit>, Source)> {
    if let Some(index) = usable_index(index_path.as_ref(), library) {
        if let Ok(hits) = index.search(query, limit) {
            return Ok((hits, Source::Index));
        }
    }
    Ok((search_library(library, query, limit)?, Source::Scan))
}

/// [`Index::list`] with the same fallback as [`search_with_fallback`].
pub fn list_with_fallback(
    index_path: impl AsRef<Path>,
    library: &Library,
    filter: &ListFilter,
) -> Result<(Vec<IndexedGuide>, Source)> {
    if let Some(index) = usable_index(index_path.as_ref(), library) {
        if let Ok(guides) = index.list(filter) {
            return Ok((guides, Source::Index));
        }
    }
    Ok((list_library(library, filter)?, Source::Scan))
}

fn usable_index(path: &Path, library: &Library) -> Option<Index> {
    let index = Index::open_read_only(path).ok()?;
    matches!(index.is_stale(library), Ok(false)).then_some(index)
}

/// Turns user input into a safe FTS5 query, or `None` if it has no word
/// characters. Each word (split on whitespace and control characters, so a
/// NUL cannot end the query string early) becomes a quoted phrase (so
/// `AND`, `NEAR`, `-`, `:`, `*`, `^` and quotes are literal text, and
/// `user@example.com` matches those three words in a row); words are ANDed;
/// the last word matches as a prefix.
pub fn fts_query(query: &str) -> Option<String> {
    Some(fts_phrases(query)?.join(" "))
}

/// [`fts_query`] and the query an item row's own text must match: any of
/// the words in its `title` or `body`.
fn fts_queries(query: &str) -> Option<(String, String)> {
    let phrases = fts_phrases(query)?;
    let own = format!("{{title body}} : ({})", phrases.join(" OR "));
    Some((phrases.join(" "), own))
}

fn fts_phrases(query: &str) -> Option<Vec<String>> {
    let words: Vec<&str> = split_words(query)
        .filter(|word| word.chars().any(char::is_alphanumeric))
        .collect();
    let last = words.len().checked_sub(1)?;
    Some(
        words
            .iter()
            .enumerate()
            .map(|(i, word)| {
                let star = if i == last { "*" } else { "" };
                format!("\"{}\"{star}", word.replace('"', "\"\""))
            })
            .collect(),
    )
}

/// Query words: split on whitespace and control characters.
fn split_words(query: &str) -> impl Iterator<Item = &str> {
    query
        .split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|word| !word.is_empty())
}

/// SQL condition: the `steps_fts` row is a guide row, or its own title or
/// body matches the query bound at `param` (see [`fts_queries`]).
fn own_words_clause(param: &str) -> String {
    format!(
        "(steps_fts.kind = 'guide' OR steps_fts.rowid IN \
         (SELECT rowid FROM steps_fts WHERE steps_fts MATCH {param}))"
    )
}

// ---------------------------------------------------------------------------
// Fallback: scanning the library without SQLite.

/// Searches the library's guides directly, for readers without a usable
/// index. Matches the same words as [`Index::search`] (see [`fts_query`]),
/// ignoring case but not accents, and ranks by a weighted count of matches
/// with the same column weights instead of bm25. Unreadable bundles and
/// duplicate copies are skipped.
pub fn search_library(library: &Library, query: &str, limit: usize) -> Result<Vec<StepHit>> {
    search_library_with(library, query, &SearchOptions::limit(limit))
}

/// [`search_library`] with options.
pub fn search_library_with(
    library: &Library,
    query: &str,
    options: &SearchOptions,
) -> Result<Vec<StepHit>> {
    let Some(words) = query_words(query) else {
        return Ok(Vec::new());
    };
    // Hits with their item position, for ordering like the index.
    struct Ranked {
        hit: StepHit,
        position: Option<usize>,
    }
    impl std::ops::Deref for Ranked {
        type Target = StepHit;
        fn deref(&self) -> &StepHit {
            &self.hit
        }
    }
    let mut hits = Vec::new();
    for summary in library.list()? {
        if summary.duplicate_of.is_some() && !options.include_duplicates {
            continue;
        }
        let Ok(loaded) = library.read(&summary.path) else {
            continue;
        };
        let guide = &loaded.guide;
        for row in rows(guide) {
            if let Some((score, snippet)) = score_row(&row, &words) {
                let position = row.position;
                let hit = StepHit {
                    guide_id: guide.id,
                    guide_path: summary.path.clone(),
                    guide_title: guide.title.clone(),
                    kind: row.kind,
                    item_id: row.item_id,
                    step_index: row.step_index,
                    title: row.title,
                    snippet,
                    rank: -score,
                };
                hits.push(Ranked { hit, position });
            }
        }
    }
    hits.sort_by(|a, b| {
        a.rank
            .total_cmp(&b.rank)
            .then_with(|| a.guide_title.cmp(&b.guide_title))
            .then_with(|| a.guide_path.cmp(&b.guide_path))
            .then_with(|| a.position.cmp(&b.position))
    });
    hits.truncate(options.limit);
    Ok(hits.into_iter().map(|ranked| ranked.hit).collect())
}

/// Lists the library's guides directly, like [`Index::list`]. With a
/// query, every guide is read.
pub fn list_library(library: &Library, filter: &ListFilter) -> Result<Vec<IndexedGuide>> {
    let tag = filter.tag.as_deref().map(str::to_lowercase);
    let mut guides: Vec<(f64, IndexedGuide)> = Vec::new();
    for summary in library.list()? {
        if tag
            .as_ref()
            .is_some_and(|tag| !summary.tags.iter().any(|t| t.to_lowercase() == *tag))
        {
            continue;
        }
        let mut score = 0.0;
        let mut read_only = summary.schema_version > crate::SCHEMA_VERSION;
        if let Some(query) = filter.query.as_deref() {
            let Some(words) = query_words(query) else {
                return Ok(Vec::new());
            };
            let Ok(loaded) = library.read(&summary.path) else {
                continue;
            };
            read_only = loaded.read_only;
            match rows(&loaded.guide)
                .iter()
                .filter_map(|row| score_row(row, &words).map(|(s, _)| s))
                .max_by(f64::total_cmp)
            {
                Some(best) => score = best,
                None => continue,
            }
        } else if summary.schema_version < crate::SCHEMA_VERSION {
            read_only = true;
        }
        guides.push((
            score,
            IndexedGuide {
                id: summary.id,
                path: summary.path,
                title: summary.title,
                summary: summary.summary,
                tags: summary.tags,
                step_count: summary.step_count,
                created_at: summary.created_at,
                updated_at: summary.updated_at,
                folder: summary.folder,
                review: summary.review,
                read_only,
                duplicate_of: summary.duplicate_of,
            },
        ));
    }
    // `Library::list` is already newest first; a query reorders by score.
    guides.sort_by(|(a, _), (b, _)| b.total_cmp(a));
    Ok(guides
        .into_iter()
        .map(|(_, guide)| guide)
        .take(filter.limit.unwrap_or(usize::MAX))
        .collect())
}

/// A query word: its tokens (a phrase) and whether the last one is a prefix.
struct Word {
    tokens: Vec<String>,
    prefix: bool,
}

fn query_words(query: &str) -> Option<Vec<Word>> {
    let mut words: Vec<Word> = split_words(query)
        .map(|word| Word {
            tokens: tokenize(word).into_iter().map(|t| t.folded).collect(),
            prefix: false,
        })
        .filter(|word| !word.tokens.is_empty())
        .collect();
    words.last_mut()?.prefix = true;
    Some(words)
}

struct Token {
    start: usize,
    end: usize,
    folded: String,
}

/// Runs of alphanumeric characters, lowercased; close to `unicode61`.
fn tokenize(text: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut start = None;
    for (i, c) in text.char_indices().chain([(text.len(), ' ')]) {
        match (c.is_alphanumeric(), start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                tokens.push(Token {
                    start: s,
                    end: i,
                    folded: text[s..i].to_lowercase(),
                });
                start = None;
            }
            _ => {}
        }
    }
    tokens
}

/// Start positions of `word` in `tokens`.
fn phrase_matches(tokens: &[Token], word: &Word) -> Vec<usize> {
    let n = word.tokens.len();
    if tokens.len() < n {
        return Vec::new();
    }
    (0..=tokens.len() - n)
        .filter(|&start| {
            word.tokens.iter().enumerate().all(|(k, want)| {
                let have = &tokens[start + k].folded;
                if word.prefix && k == n - 1 {
                    have.starts_with(want.as_str())
                } else {
                    have == want
                }
            })
        })
        .collect()
}

/// Columns of an item row's own text (title, body), as opposed to the guide
/// title and meta.
const OWN_COLUMNS: [usize; 2] = [2, 3];

/// A row's score (higher is better) and snippet, if every word matches and,
/// for an item row, at least one matches its own text.
fn score_row(row: &Row, words: &[Word]) -> Option<(f64, String)> {
    let columns: Vec<Vec<Token>> = row.columns.iter().map(|c| tokenize(c)).collect();
    let mut score = 0.0;
    // Per column: weighted hits and the highlighted token positions.
    let mut per_column: Vec<(f64, Vec<usize>)> = vec![(0.0, Vec::new()); columns.len()];
    for word in words {
        let mut found = false;
        for (c, tokens) in columns.iter().enumerate() {
            let starts = phrase_matches(tokens, word);
            if starts.is_empty() {
                continue;
            }
            found = true;
            let weighted = WEIGHTS[c] * starts.len() as f64;
            score += weighted;
            per_column[c].0 += weighted;
            per_column[c]
                .1
                .extend(starts.iter().flat_map(|&s| s..s + word.tokens.len()));
        }
        if !found {
            return None;
        }
    }
    if row.kind != HitKind::Guide && OWN_COLUMNS.iter().all(|&c| per_column[c].1.is_empty()) {
        return None;
    }
    let (best, (_, highlighted)) = per_column
        .into_iter()
        .enumerate()
        .max_by(|(_, (a, _)), (_, (b, _))| a.total_cmp(b))?;
    Some((
        score,
        snippet(&row.columns[best], &columns[best], highlighted),
    ))
}

/// Up to [`SNIPPET_TOKENS`] words of `text` starting just before the first
/// match, like FTS5's `snippet()`.
fn snippet(text: &str, tokens: &[Token], mut highlighted: Vec<usize>) -> String {
    highlighted.sort_unstable();
    highlighted.dedup();
    let first = highlighted.first().copied().unwrap_or(0);
    let start = first
        .saturating_sub(2)
        .min(tokens.len().saturating_sub(SNIPPET_TOKENS));
    let end = (start + SNIPPET_TOKENS).min(tokens.len());
    let mut out = String::new();
    let mut pos = if start == 0 {
        0
    } else {
        out.push_str(ELLIPSIS);
        tokens[start].start
    };
    for (i, token) in tokens.iter().enumerate().take(end).skip(start) {
        out.push_str(&text[pos..token.start]);
        if highlighted.binary_search(&i).is_ok() {
            out.push_str(HIGHLIGHT);
            out.push_str(&text[token.start..token.end]);
            out.push_str(HIGHLIGHT);
        } else {
            out.push_str(&text[token.start..token.end]);
        }
        pos = token.end;
    }
    if end == tokens.len() {
        out.push_str(&text[pos..]);
    } else {
        out.push_str(ELLIPSIS);
    }
    out
}

// ---------------------------------------------------------------------------
// Rows and bundles.

/// One `steps_fts` row.
struct Row {
    kind: HitKind,
    item_id: Option<Ulid>,
    position: Option<usize>,
    step_index: Option<usize>,
    /// Shown as [`StepHit::title`].
    title: String,
    /// guide_title, meta, title, body; plain text.
    columns: [String; 4],
}

fn rows(guide: &Guide) -> Vec<Row> {
    let guide_title = markdown::to_plain_text(&guide.title);
    let mut meta = markdown::to_plain_text(&guide.summary);
    for tag in &guide.tags {
        meta.push('\n');
        meta.push_str(tag);
    }
    let about = [&guide.intro, &guide.prerequisites]
        .into_iter()
        .map(|text| markdown::to_plain_text(text))
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let mut rows = vec![Row {
        kind: HitKind::Guide,
        item_id: None,
        position: None,
        step_index: None,
        title: guide.title.clone(),
        columns: [guide_title.clone(), meta, String::new(), about],
    }];
    let mut steps = 0;
    for (position, item) in guide.items.iter().enumerate() {
        let (kind, title, body, step_index) = match item {
            Item::Step(step) => {
                steps += 1;
                let title = markdown::to_plain_text(&step.title);
                let mut body = markdown::to_plain_text(&step.description);
                // Hotspot text is plain text already.
                for annotation in &step.annotations {
                    if let Shape::Hotspot(hotspot) = &annotation.shape {
                        for text in [&hotspot.title, &hotspot.description] {
                            if !text.trim().is_empty() {
                                if !body.is_empty() {
                                    body.push('\n');
                                }
                                body.push_str(text.trim());
                            }
                        }
                    }
                }
                (HitKind::Step, title, body, Some(steps - 1))
            }
            Item::Section(section) => (
                HitKind::Section,
                markdown::to_plain_text(&section.title),
                String::new(),
                None,
            ),
            Item::Note(note) => (
                HitKind::Note,
                String::new(),
                markdown::to_plain_text(&note.body),
                None,
            ),
        };
        rows.push(Row {
            kind,
            item_id: Some(item.id()),
            position: Some(position),
            step_index,
            title: if kind == HitKind::Note {
                note_title(&body)
            } else {
                title.clone()
            },
            columns: [guide_title.clone(), String::new(), title, body],
        });
    }
    rows
}

/// A note's title: its first non-empty line, at most [`NOTE_TITLE_CHARS`]
/// characters.
fn note_title(body: &str) -> String {
    let line = body
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    if line.chars().count() <= NOTE_TITLE_CHARS {
        return line.to_owned();
    }
    let cut: String = line.chars().take(NOTE_TITLE_CHARS - 1).collect();
    format!("{}{ELLIPSIS}", cut.trim_end())
}

/// What indexing a bundle found.
enum Entry {
    Guide { guide: Box<Guide>, read_only: bool },
    Error(String),
}

fn read_entry(library: &Library, bundle_path: &Path) -> (Option<Stamp>, Entry) {
    // The stamp first: if the file changes during the read, the next sync
    // sees a newer stamp and reads it again.
    let stamp = stamp(bundle_path);
    let entry = match library.read(bundle_path) {
        Ok(loaded) => Entry::Guide {
            read_only: loaded.read_only,
            guide: Box::new(loaded.guide),
        },
        Err(e) => Entry::Error(e.to_string()),
    };
    (stamp, entry)
}

fn read_entries<'a>(
    library: &Library,
    bundles: impl IntoIterator<Item = &'a String>,
) -> BTreeMap<String, (Option<Stamp>, Entry)> {
    bundles
        .into_iter()
        .map(|bundle| {
            let entry = read_entry(library, &library.root().join(bundle));
            (bundle.clone(), entry)
        })
        .collect()
}

/// Adds a bundle that has no rows yet (see [`delete`]).
fn insert(tx: &Transaction<'_>, bundle: &str, stamp: Option<Stamp>, entry: &Entry) -> Result<()> {
    let (mtime_ns, size) = match stamp {
        Some(Stamp { mtime_ns, size }) => (Some(mtime_ns), Some(size)),
        None => (None, None),
    };
    let (guide, read_only) = match entry {
        Entry::Error(error) => {
            tx.execute(
                "INSERT INTO guides (bundle, mtime_ns, size, error) VALUES (?1, ?2, ?3, ?4)",
                params![bundle, mtime_ns, size, error],
            )?;
            return Ok(());
        }
        Entry::Guide { guide, read_only } => (guide, *read_only),
    };
    let folded: Vec<String> = guide.tags.iter().map(|t| t.to_lowercase()).collect();
    tx.execute(
        "INSERT INTO guides (bundle, mtime_ns, size, id, schema_version, read_only, title,
                             summary, tags, tags_folded, step_count, updated_us, created_us,
                             review)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            bundle,
            mtime_ns,
            size,
            guide.id.to_string(),
            guide.schema_version,
            read_only,
            guide.title,
            guide.summary,
            serde_json::to_string(&guide.tags).expect("strings serialize"),
            serde_json::to_string(&folded).expect("strings serialize"),
            guide.steps().count() as i64,
            guide.updated_at.timestamp_micros(),
            guide.created_at.timestamp_micros(),
            (guide.review != ReviewInfo::default())
                .then(|| serde_json::to_string(&guide.review).expect("review serializes")),
        ],
    )?;
    let mut insert = tx.prepare_cached(
        "INSERT INTO steps_fts (bundle, item_id, kind, position, step_index, guide_title, meta,
                                title, body)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )?;
    let mut track =
        tx.prepare_cached("INSERT INTO fts_rows (fts_rowid, bundle) VALUES (?1, ?2)")?;
    for row in rows(guide) {
        let [guide_title, meta, title, body] = &row.columns;
        insert.execute(params![
            bundle,
            row.item_id.map(|id| id.to_string()),
            row.kind.as_str(),
            row.position.map(|i| i as i64),
            row.step_index.map(|i| i as i64),
            guide_title,
            meta,
            title,
            body,
        ])?;
        track.execute(params![tx.last_insert_rowid(), bundle])?;
    }
    Ok(())
}

/// Removes a bundle's rows, finding its `steps_fts` rows through the
/// indexed `fts_rows` table.
fn delete(tx: &Transaction<'_>, bundle: &str) -> Result<()> {
    let rowids: Vec<i64> = tx
        .prepare_cached("SELECT fts_rowid FROM fts_rows WHERE bundle = ?1")?
        .query_map([bundle], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut delete_row = tx.prepare_cached("DELETE FROM steps_fts WHERE rowid = ?1")?;
    for rowid in rowids {
        delete_row.execute([rowid])?;
    }
    tx.execute("DELETE FROM fts_rows WHERE bundle = ?1", [bundle])?;
    tx.execute("DELETE FROM guides WHERE bundle = ?1", [bundle])?;
    Ok(())
}

/// Sets `guides.duplicate_of` for every id held by more than one readable
/// bundle. The canonical bundle is the one `library` finds for the id;
/// without a library (or if it finds none of them) the current canonical
/// one is kept, else the first by name.
fn mark_duplicates(tx: &Transaction<'_>, library: Option<&Library>) -> Result<()> {
    tx.execute(
        "UPDATE guides SET duplicate_of = NULL WHERE duplicate_of IS NOT NULL AND id NOT IN
             (SELECT id FROM guides WHERE id IS NOT NULL GROUP BY id HAVING count(*) > 1)",
        [],
    )?;
    let ids: Vec<String> = tx
        .prepare("SELECT id FROM guides WHERE id IS NOT NULL GROUP BY id HAVING count(*) > 1")?
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for id in ids {
        let copies: Vec<(String, bool)> = tx
            .prepare(
                "SELECT bundle, duplicate_of IS NULL FROM guides WHERE id = ?1 ORDER BY bundle",
            )?
            .query_map([&id], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let found = library
            .zip(id.parse::<Ulid>().ok())
            .and_then(|(library, id)| {
                let handle = library.find(id).ok().flatten()?;
                bundle_name(library.root(), &handle.path)
            })
            .filter(|found| copies.iter().any(|(bundle, _)| bundle == found));
        let current = || {
            let mut canonical = copies.iter().filter(|(_, is_canonical)| *is_canonical);
            match (canonical.next(), canonical.next()) {
                (Some((bundle, _)), None) => Some(bundle.clone()),
                _ => None,
            }
        };
        let canonical = found
            .or_else(current)
            .unwrap_or_else(|| copies[0].0.clone());
        tx.execute(
            "UPDATE guides SET duplicate_of = CASE WHEN bundle = ?1 THEN NULL ELSE ?1 END
             WHERE id = ?2",
            params![canonical, id],
        )?;
    }
    Ok(())
}

fn set_root(tx: &Transaction<'_>, root: &Path) -> Result<()> {
    tx.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
        params![META_ROOT, root.to_string_lossy()],
    )?;
    Ok(())
}

/// Bundle folders in `root` and its folders (as [`Library::scan`] finds
/// them), by relative path, with their `guide.json` stamps. Paths that are
/// not UTF-8 are skipped.
fn bundles_on_disk(root: &Path) -> Result<BTreeMap<String, Option<Stamp>>> {
    let paths = bundle_paths(root).map_err(|source| IndexError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    Ok(paths
        .into_iter()
        .filter_map(|path| Some((bundle_name(root, &path)?, stamp(&path))))
        .collect())
}

/// The path of a bundle relative to `root`, `/` separated (the `bundle`
/// key), if it is a bundle below `root`. `path` need not be canonical.
fn bundle_name(root: &Path, path: &Path) -> Option<String> {
    is_bundle_extension(path.extension()?).then_some(())?;
    let relative = match path.strip_prefix(root) {
        Ok(relative) => relative.to_path_buf(),
        Err(_) => {
            let parent = fs::canonicalize(path.parent()?).ok()?;
            parent
                .join(path.file_name()?)
                .strip_prefix(root)
                .ok()?
                .to_path_buf()
        }
    };
    let parts: Option<Vec<&str>> = relative
        .components()
        .map(|c| match c {
            std::path::Component::Normal(name) => name.to_str(),
            _ => None,
        })
        .collect();
    let parts = parts?;
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// The folder part of a `bundle` key: `""` in the root.
fn bundle_folder(bundle: &str) -> &str {
    bundle.rsplit_once('/').map_or("", |(folder, _)| folder)
}

fn stamp(bundle_path: &Path) -> Option<Stamp> {
    let meta = fs::metadata(bundle_path.join(GUIDE_FILE)).ok()?;
    let mtime_ns = match meta.modified().ok()?.duration_since(UNIX_EPOCH) {
        Ok(after) => after.as_nanos() as i64,
        Err(before) => -(before.duration().as_nanos() as i64),
    };
    Some(Stamp {
        mtime_ns,
        size: meta.len() as i64,
    })
}

// ---------------------------------------------------------------------------
// SQLite helpers.

/// Decides what is at `path` without writing to it.
fn inspect(path: &Path) -> Result<Existing> {
    let meta = match fs::metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Existing::New),
        Err(source) => {
            return Err(IndexError::Io {
                path: path.to_path_buf(),
                source,
            })
        }
    };
    if !meta.is_file() {
        return Ok(Existing::Foreign);
    }
    if meta.len() == 0 {
        return Ok(Existing::New);
    }
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    let id = match conn.query_row("PRAGMA application_id", [], |row| row.get::<_, i32>(0)) {
        Ok(id) => id,
        // Not even a readable header: nothing proves it is ours.
        Err(e) if is_corrupt(&e) => return Ok(Existing::Foreign),
        Err(e) => return Err(e.into()),
    };
    if id != APPLICATION_ID {
        let empty = conn
            .query_row("SELECT count(*) FROM sqlite_master", [], |row| {
                row.get::<_, i64>(0)
            })
            .is_ok_and(|n| n == 0);
        return Ok(if id == 0 && empty {
            Existing::New
        } else {
            Existing::Foreign
        });
    }
    let check = conn.query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0));
    match check {
        Ok(result) if result == "ok" => Ok(Existing::Ours),
        Ok(_) => Ok(Existing::OursCorrupt),
        Err(e) if is_corrupt(&e) => Ok(Existing::OursCorrupt),
        Err(e) => Err(e.into()),
    }
}

/// Renames a corrupt index of ours (and its `-wal` and `-shm`) to
/// `<name>.corrupt-<unix ms>[-wal|-shm]`, keeping it for inspection.
fn set_aside(path: &Path) -> Result<()> {
    let millis = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let mut base = path.as_os_str().to_owned();
    base.push(format!(".corrupt-{millis}"));
    for (file, suffix) in sqlite_files(path).into_iter().zip(["", "-wal", "-shm"]) {
        let mut target = base.clone();
        target.push(suffix);
        match fs::rename(&file, PathBuf::from(target)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => {
                return Err(IndexError::Io {
                    path: file,
                    source: e,
                })
            }
            _ => {}
        }
    }
    Ok(())
}

/// Identifies the file at a path, to notice when it is replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileId {
    dev: u64,
    ino: u64,
}

#[cfg(unix)]
fn file_id(path: &Path) -> Option<FileId> {
    use std::os::unix::fs::MetadataExt;
    let meta = fs::metadata(path).ok()?;
    Some(FileId {
        dev: meta.dev(),
        ino: meta.ino(),
    })
}

#[cfg(not(unix))]
fn file_id(_path: &Path) -> Option<FileId> {
    None
}

fn expected_format() -> String {
    format!("{FORMAT_VERSION}/{}", crate::SCHEMA_VERSION)
}

/// `"<format>/<guide schema>"`, or `None` for a database without a meta
/// table (e.g. a new file).
fn read_format(conn: &Connection) -> Result<Option<String>> {
    let has_meta: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'meta')",
        [],
        |row| row.get(0),
    )?;
    if !has_meta {
        return Ok(None);
    }
    let get = |key: &str| -> Result<Option<String>> {
        Ok(conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()?)
    };
    Ok(match (get(META_FORMAT)?, get(META_SCHEMA)?) {
        (Some(format), Some(schema)) => Some(format!("{format}/{schema}")),
        _ => None,
    })
}

fn bm25_sql() -> String {
    format!(
        "bm25(steps_fts, 0.0, 0.0, 0.0, 0.0, 0.0, {WEIGHT_GUIDE_TITLE:?}, {WEIGHT_GUIDE_META:?}, \
         {WEIGHT_ITEM_TITLE:?}, {WEIGHT_ITEM_BODY:?})"
    )
}

/// SQLite's "no limit" is -1.
fn sql_limit(limit: Option<usize>) -> i64 {
    limit.map_or(-1, |n| i64::try_from(n).unwrap_or(i64::MAX))
}

fn is_corrupt(e: &rusqlite::Error) -> bool {
    matches!(
        e.sqlite_error_code(),
        Some(ErrorCode::NotADatabase | ErrorCode::DatabaseCorrupt)
    )
}

fn sqlite_files(path: &Path) -> Vec<PathBuf> {
    let mut files = vec![path.to_path_buf()];
    for suffix in ["-wal", "-shm"] {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        files.push(PathBuf::from(name));
    }
    files
}

fn conversion_error(
    idx: usize,
    e: impl std::error::Error + Send + Sync + 'static,
) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(idx, Type::Text, Box::new(e))
}

fn parse_ulid(row: &rusqlite::Row<'_>, idx: usize) -> rusqlite::Result<Ulid> {
    row.get::<_, String>(idx)?
        .parse()
        .map_err(|e| conversion_error(idx, e))
}

fn parse_tags(row: &rusqlite::Row<'_>, idx: usize) -> rusqlite::Result<Vec<String>> {
    serde_json::from_str(&row.get::<_, String>(idx)?).map_err(|e| conversion_error(idx, e))
}

fn parse_review(row: &rusqlite::Row<'_>, idx: usize) -> rusqlite::Result<ReviewInfo> {
    match row.get::<_, Option<String>>(idx)? {
        Some(json) => serde_json::from_str(&json).map_err(|e| conversion_error(idx, e)),
        None => Ok(ReviewInfo::default()),
    }
}

fn parse_micros(row: &rusqlite::Row<'_>, idx: usize) -> rusqlite::Result<DateTime<Utc>> {
    let micros: i64 = row.get(idx)?;
    DateTime::from_timestamp_micros(micros)
        .ok_or_else(|| rusqlite::Error::IntegralValueOutOfRange(idx, micros))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_name_accepts_legacy_extension() {
        let root = Path::new("/l");
        assert_eq!(
            bundle_name(root, Path::new("/l/A.guiderecorder")).as_deref(),
            Some("A.guiderecorder")
        );
        assert_eq!(
            bundle_name(root, Path::new("/l/A.guidesya")).as_deref(),
            Some("A.guidesya")
        );
        assert_eq!(bundle_name(root, Path::new("/l/A.txt")), None);
    }

    #[test]
    fn bundle_name_is_relative_to_the_root() {
        let root = Path::new("/l");
        assert_eq!(
            bundle_name(root, Path::new("/l/Work/Team/A.guiderecorder")).as_deref(),
            Some("Work/Team/A.guiderecorder")
        );
        assert_eq!(bundle_folder("Work/Team/A.guiderecorder"), "Work/Team");
        assert_eq!(bundle_folder("A.guiderecorder"), "");
    }

    #[test]
    fn fts_query_quotes_words_and_prefixes_the_last() {
        assert_eq!(fts_query(""), None);
        assert_eq!(fts_query("  -- ** "), None);
        assert_eq!(fts_query("api"), Some("\"api\"*".into()));
        assert_eq!(
            fts_query("rotate AND key"),
            Some("\"rotate\" \"AND\" \"key\"*".into())
        );
        assert_eq!(
            fts_query("say \"hi\" NEAR(x)"),
            Some("\"say\" \"\"\"hi\"\"\" \"NEAR(x)\"*".into())
        );
    }

    #[test]
    fn corruption_during_rebuild_sets_the_file_aside() {
        use crate::model::{AiState, ImageShow, Step, StepImage, StepKind};
        let dir = tempfile::tempdir().unwrap();
        let library = Library::open(dir.path().join("library")).unwrap();
        for i in 0..40 {
            let mut guide = Guide::new(format!("Guide {i}"), "en");
            guide.items = (0..10)
                .map(|s| {
                    let id = Ulid::new();
                    Item::Step(Step {
                        id,
                        title: format!("Step {s} of bulk {i}"),
                        description: "Some description text.".into(),
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
                })
                .collect();
            library.create(&guide).unwrap();
        }
        let path = dir.path().join("index.sqlite");
        let mut index = Index::open(&path).unwrap();
        index.rebuild(&library).unwrap();
        drop(index);

        // Damage the second half of the pages, then connect without the
        // quick_check that `open` runs, as if it happened while running.
        let mut bytes = fs::read(&path).unwrap();
        let start = (bytes.len() / 2) / 4096 * 4096;
        for (i, byte) in bytes[start..].iter_mut().enumerate() {
            *byte = (i % 251) as u8 ^ 0xA5;
        }
        fs::write(&path, bytes).unwrap();
        let mut index = Index::connect_writer(&path).unwrap();
        let report = index.rebuild(&library).unwrap();
        assert!(report.rebuilt);
        assert_eq!(index.search("bulk", 1000).unwrap().len(), 400);
        let aside = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.unwrap().file_name().into_string().ok())
            .filter(|name| name.starts_with("index.sqlite.corrupt-"))
            .count();
        assert!(aside >= 1);
        assert!(!index.is_stale(&library).unwrap());
    }

    #[test]
    fn control_characters_split_words() {
        assert_eq!(fts_query("a\0b"), Some("\"a\" \"b\"*".into()));
        assert_eq!(fts_query("\0"), None);
        assert_eq!(
            fts_queries("key\tpass"),
            Some((
                "\"key\" \"pass\"*".into(),
                "{title body} : (\"key\" OR \"pass\"*)".into()
            ))
        );
        let words = query_words("x\u{1}y").unwrap();
        assert_eq!(words.len(), 2);
    }

    #[test]
    fn note_title_is_the_first_line_shortened() {
        assert_eq!(note_title("\n  Mind the gap.\nMore"), "Mind the gap.");
        assert_eq!(note_title(""), "");
        let long = "word ".repeat(30);
        let title = note_title(&long);
        assert_eq!(title.chars().count(), NOTE_TITLE_CHARS);
        assert!(title.ends_with("word…"), "{title}");
    }

    #[test]
    fn hit_kinds_round_trip() {
        for kind in [
            HitKind::Guide,
            HitKind::Step,
            HitKind::Section,
            HitKind::Note,
        ] {
            assert_eq!(HitKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(HitKind::parse("item"), None);
    }

    #[test]
    fn tokenize_splits_on_non_alphanumerics() {
        let tokens: Vec<_> = tokenize("Größe: user@Example.com 42")
            .into_iter()
            .map(|t| t.folded)
            .collect();
        assert_eq!(tokens, ["größe", "user", "example", "com", "42"]);
    }

    #[test]
    fn snippet_highlights_and_trims() {
        let text = "one two three four five six seven eight nine ten eleven twelve thirteen \
                    fourteen fifteen";
        let tokens = tokenize(text);
        assert_eq!(
            snippet(text, &tokens, vec![5]),
            "…four five **six** seven eight nine ten eleven twelve thirteen fourteen fifteen"
        );
        let short = "Open the menu.";
        assert_eq!(
            snippet(short, &tokenize(short), vec![2]),
            "Open the **menu**."
        );
    }

    #[test]
    fn phrase_matching_with_prefix() {
        let tokens = tokenize("send user@example.com an invite");
        let word = |s: &str, prefix| Word {
            tokens: tokenize(s).into_iter().map(|t| t.folded).collect(),
            prefix,
        };
        assert_eq!(
            phrase_matches(&tokens, &word("user@example.com", false)),
            [1]
        );
        assert_eq!(phrase_matches(&tokens, &word("INV", true)), [5]);
        assert!(phrase_matches(&tokens, &word("inv", false)).is_empty());
    }
}
