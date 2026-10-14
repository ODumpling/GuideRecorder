//! The editor's library session: opening the library, listing, opening and
//! saving guides, and reading step images. Blocking file IO only; the Tauri
//! commands in `commands.rs` run it on the blocking pool.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

use guidesya_core::index::{self, Index, IndexedGuide, ListFilter};
use guidesya_core::library::{GuideHandle, Library, LibraryError};
use guidesya_core::model::{Guide, Item};
use guidesya_core::Ulid;
use serde::{Deserialize, Serialize};

/// Environment variable that overrides the library folder.
pub const LIBRARY_ENV: &str = "GUIDESYA_LIBRARY";

/// An error the UI can show: `kind` picks the presentation, `message` is
/// for people.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppError {
    pub kind: ErrorKind,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorKind {
    /// The guide, step or image does not exist.
    NotFound,
    /// The guide is read-only (newer schema or failed migration).
    ReadOnly,
    /// The guide was not opened with `open_guide` first.
    NotOpen,
    /// The bundle on disk changed under us (moved, replaced or corrupt).
    Conflict,
    /// Bad input from the UI (e.g. an invalid id).
    Invalid,
    /// Anything else: IO, index, startup.
    Io,
}

impl AppError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl From<LibraryError> for AppError {
    fn from(e: LibraryError) -> Self {
        let kind = match &e {
            LibraryError::NotFound(_) => ErrorKind::NotFound,
            LibraryError::NewerSchema { .. } | LibraryError::NotMigrated { .. } => {
                ErrorKind::ReadOnly
            }
            LibraryError::IdMismatch { .. }
            | LibraryError::Unverifiable { .. }
            | LibraryError::ChangedOnDisk(_)
            | LibraryError::Json { .. }
            | LibraryError::NotABundle(_) => ErrorKind::Conflict,
            LibraryError::InvalidRelativePath(_) | LibraryError::SymlinkInBundle(_) => {
                ErrorKind::Invalid
            }
            _ => ErrorKind::Io,
        };
        Self::new(kind, e.to_string())
    }
}

pub type Result<T, E = AppError> = std::result::Result<T, E>;

/// A library listing row.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GuideListItem {
    pub id: String,
    pub path: PathBuf,
    pub title: String,
    pub summary: String,
    pub tags: Vec<String>,
    pub step_count: usize,
    pub updated_at: String,
    pub read_only: bool,
    pub duplicate_of: Option<PathBuf>,
}

impl From<IndexedGuide> for GuideListItem {
    fn from(g: IndexedGuide) -> Self {
        Self {
            id: g.id.to_string(),
            path: g.path,
            title: g.title,
            summary: g.summary,
            tags: g.tags,
            step_count: g.step_count,
            updated_at: g.updated_at.to_rfc3339(),
            read_only: g.read_only,
            duplicate_of: g.duplicate_of,
        }
    }
}

/// `open_guide`'s result.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenedGuide {
    pub guide: Guide,
    pub path: PathBuf,
    pub read_only: bool,
    pub migrated_from: Option<u32>,
    /// Why a migrated guide could not be written back (it is then
    /// read-only).
    pub migration_error: Option<String>,
}

/// Which step image `step_image` returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImageVariant {
    Full,
    Crop,
    After,
}

/// A guide opened for editing: the bundle it came from and the latest copy
/// (as opened or last saved), for image lookups.
#[derive(Debug)]
struct OpenGuide {
    handle: GuideHandle,
    read_only: bool,
    guide: Guide,
}

/// The library plus the index and the guides the UI has open.
pub struct Session {
    library: Library,
    /// `None` if there is no index (not macOS) or it failed to open; listing
    /// then scans the library.
    index: Mutex<Option<Index>>,
    open: Mutex<HashMap<Ulid, OpenGuide>>,
    /// One lock per guide, held across a whole open or save (write, index
    /// update, cache update), so two saves of one guide never overlap.
    /// Kept apart from `open` so a re-open cannot swap the lock mid-save.
    guide_locks: Mutex<HashMap<Ulid, Arc<Mutex<()>>>>,
}

impl Session {
    /// Opens the library at `root` (creating it), runs the startup
    /// [`Library::recover`] and opens and syncs the index at `index_path`.
    pub fn start(root: &Path, index_path: Option<PathBuf>) -> Result<Self> {
        let library = Library::open(root)?;
        let recovery = library.recover();
        if !recovery.removed.is_empty() || !recovery.restored.is_empty() {
            eprintln!(
                "guidesya: recovered library: removed {:?}, restored {:?}",
                recovery.removed, recovery.restored
            );
        }
        let index = index_path.and_then(|path| match Index::open(&path) {
            Ok(mut index) => {
                if let Err(e) = index.sync(&library) {
                    eprintln!("guidesya: index sync failed: {e}");
                }
                Some(index)
            }
            Err(e) => {
                eprintln!("guidesya: index unavailable ({}): {e}", path.display());
                None
            }
        });
        Ok(Self {
            library,
            index: Mutex::new(index),
            open: Mutex::new(HashMap::new()),
            guide_locks: Mutex::new(HashMap::new()),
        })
    }

    fn guide_lock(&self, id: Ulid) -> Arc<Mutex<()>> {
        Arc::clone(self.guide_locks.lock().unwrap().entry(id).or_default())
    }

    pub fn root(&self) -> &Path {
        self.library.root()
    }

    /// Lists guides, best match first when `query` is set. Uses the index
    /// after syncing it, and scans the library if that fails.
    pub fn list_guides(&self, query: Option<String>) -> Result<Vec<GuideListItem>> {
        let filter = ListFilter {
            query: query.filter(|q| !q.trim().is_empty()),
            ..ListFilter::default()
        };
        let mut index = self.index.lock().unwrap();
        if let Some(index) = index.as_mut() {
            let listed = index.sync(&self.library).and_then(|_| index.list(&filter));
            match listed {
                Ok(guides) => return Ok(guides.into_iter().map(Into::into).collect()),
                Err(e) => eprintln!("guidesya: index list failed, scanning: {e}"),
            }
        }
        let guides = index::list_library(&self.library, &filter)
            .map_err(|e| AppError::new(ErrorKind::Io, e.to_string()))?;
        Ok(guides.into_iter().map(Into::into).collect())
    }

    /// Loads a guide for editing (migrating it if older) and remembers its
    /// bundle for `save_guide` and `step_image`.
    pub fn open_guide(&self, id: &str) -> Result<OpenedGuide> {
        let id = parse_id(id)?;
        let lock = self.guide_lock(id);
        let _guard = lock.lock().unwrap();
        let handle = self.library.find(id)?.ok_or(LibraryError::NotFound(id))?;
        let loaded = self.library.load(&handle.path)?;
        let opened = OpenedGuide {
            guide: loaded.guide.clone(),
            path: loaded.handle.path.clone(),
            read_only: loaded.read_only,
            migrated_from: loaded.migrated_from,
            migration_error: loaded.migration_error.map(|e| e.to_string()),
        };
        self.open.lock().unwrap().insert(
            id,
            OpenGuide {
                handle: loaded.handle,
                read_only: loaded.read_only,
                guide: loaded.guide,
            },
        );
        Ok(opened)
    }

    /// Saves an opened guide into the bundle it was opened from. Refuses
    /// read-only guides. The caller sets `updated_at`.
    pub fn save_guide(&self, guide: Guide) -> Result<()> {
        let lock = self.guide_lock(guide.id);
        let _guard = lock.lock().unwrap();
        let handle = {
            let open = self.open.lock().unwrap();
            let entry = open.get(&guide.id).ok_or_else(|| {
                AppError::new(
                    ErrorKind::NotOpen,
                    format!("guide {} is not open", guide.id),
                )
            })?;
            if entry.read_only {
                return Err(AppError::new(
                    ErrorKind::ReadOnly,
                    format!("guide {} is read-only", guide.id),
                ));
            }
            entry.handle.clone()
        };
        self.library.save_to(&handle, &guide)?;
        if let Some(index) = self.index.lock().unwrap().as_mut() {
            if let Err(e) = index.update(&self.library, &handle.path) {
                eprintln!("guidesya: index update failed: {e}");
            }
        }
        if let Some(entry) = self.open.lock().unwrap().get_mut(&guide.id) {
            entry.guide = guide;
        }
        Ok(())
    }

    /// The raw WebP bytes of a step image of an opened guide.
    pub fn step_image(
        &self,
        guide_id: &str,
        step_id: &str,
        variant: ImageVariant,
    ) -> Result<Vec<u8>> {
        let guide_id = parse_id(guide_id)?;
        let step_id = parse_id(step_id)?;
        let (handle, relative) = {
            let open = self.open.lock().unwrap();
            let entry = open.get(&guide_id).ok_or_else(|| {
                AppError::new(ErrorKind::NotOpen, format!("guide {guide_id} is not open"))
            })?;
            let step = entry
                .guide
                .items
                .iter()
                .find_map(|item| match item {
                    Item::Step(step) if step.id == step_id => Some(step),
                    _ => None,
                })
                .ok_or_else(|| {
                    AppError::new(ErrorKind::NotFound, format!("step {step_id} not found"))
                })?;
            let relative = match variant {
                ImageVariant::Full => Some(step.image.full.clone()),
                ImageVariant::Crop => step.image.crop.clone(),
                ImageVariant::After => step.image.after.clone(),
            }
            .ok_or_else(|| {
                AppError::new(
                    ErrorKind::NotFound,
                    format!("step {step_id} has no {variant:?} image"),
                )
            })?;
            (entry.handle.clone(), relative)
        };
        let path = handle.resolve(&relative)?;
        std::fs::read(&path).map_err(|e| {
            let kind = if e.kind() == std::io::ErrorKind::NotFound {
                ErrorKind::NotFound
            } else {
                ErrorKind::Io
            };
            AppError::new(kind, format!("{}: {e}", path.display()))
        })
    }
}

fn parse_id(id: &str) -> Result<Ulid> {
    Ulid::from_string(id)
        .map_err(|_| AppError::new(ErrorKind::Invalid, format!("invalid id {id:?}")))
}

/// The library folder: `$GUIDESYA_LIBRARY` if set, else
/// `<documents>/GuidesYa`.
pub fn library_root(
    env: Option<std::ffi::OsString>,
    documents: Option<PathBuf>,
) -> Option<PathBuf> {
    env.filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| documents.map(|d| d.join("GuidesYa")))
}

/// Starts the [`Session`] once in the background; commands wait for it.
#[derive(Default)]
pub struct SessionCell {
    state: Mutex<Option<Result<Arc<Session>>>>,
    ready: Condvar,
}

impl SessionCell {
    pub fn set(&self, session: Result<Session>) {
        *self.state.lock().unwrap() = Some(session.map(Arc::new));
        self.ready.notify_all();
    }

    /// Blocks until [`SessionCell::set`] has run. Call from the blocking
    /// pool only.
    pub fn get(&self) -> Result<Arc<Session>> {
        let state = self.state.lock().unwrap();
        let state = self.ready.wait_while(state, |s| s.is_none()).unwrap();
        state.as_ref().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const FIXTURE: &str = "Rotate an API key.guidesya";

    /// A fresh library under `target/` holding a copy of the fixtures.
    fn library() -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/guidesya-app-tests")
            .join(format!(
                "{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/guides");
        for bundle in fs::read_dir(&fixtures).unwrap() {
            let bundle = bundle.unwrap().path();
            copy_dir(&bundle, &root.join(bundle.file_name().unwrap()));
        }
        root
    }

    fn copy_dir(from: &Path, to: &Path) {
        fs::create_dir_all(to).unwrap();
        for entry in fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_dir(&entry.path(), &target);
            } else {
                fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    fn fixture_id(root: &Path) -> String {
        let json: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join(FIXTURE).join("guide.json")).unwrap())
                .unwrap();
        json["id"].as_str().unwrap().to_owned()
    }

    #[test]
    fn library_root_prefers_env() {
        let docs = Some(PathBuf::from("/Users/a/Documents"));
        assert_eq!(
            library_root(Some("/x/lib".into()), docs.clone()),
            Some(PathBuf::from("/x/lib"))
        );
        assert_eq!(
            library_root(Some("".into()), docs.clone()),
            Some(PathBuf::from("/Users/a/Documents/GuidesYa"))
        );
        assert_eq!(library_root(None, None), None);
    }

    #[test]
    fn lists_by_scan_and_with_index() {
        let root = library();
        let session = Session::start(&root, None).unwrap();
        let all = session.list_guides(None).unwrap();
        assert_eq!(all.len(), 3);

        let index_path = root.join("index.sqlite");
        let indexed = Session::start(&root, Some(index_path.clone())).unwrap();
        assert!(index_path.exists());
        let mut listed = indexed.list_guides(Some("   ".into())).unwrap();
        listed.sort_by(|a, b| a.id.cmp(&b.id));
        let mut scanned = all;
        scanned.sort_by(|a, b| a.id.cmp(&b.id));
        assert_eq!(
            listed.iter().map(|g| &g.id).collect::<Vec<_>>(),
            scanned.iter().map(|g| &g.id).collect::<Vec<_>>()
        );
        let hits = indexed.list_guides(Some("API key".into())).unwrap();
        assert_eq!(
            hits.first().map(|g| g.title.as_str()),
            Some("Rotate an API key")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn open_save_round_trip_and_updates_index() {
        let root = library();
        let session = Session::start(&root, Some(root.join("index.sqlite"))).unwrap();
        let id = fixture_id(&root);
        let opened = session.open_guide(&id).unwrap();
        assert!(!opened.read_only);
        assert_eq!(opened.migrated_from, None);

        let mut guide = opened.guide;
        guide.title = "Rotate a key, renamed".into();
        session.save_guide(guide.clone()).unwrap();

        let reopened = session.open_guide(&id).unwrap();
        assert_eq!(reopened.guide, guide);
        let hits = session.list_guides(Some("renamed".into())).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, id);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn save_requires_open_and_refuses_read_only() {
        let root = library();
        let session = Session::start(&root, None).unwrap();
        let id = fixture_id(&root);
        let path = root.join(FIXTURE).join("guide.json");
        let mut json: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let guide: Guide = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(
            session.save_guide(guide).unwrap_err().kind,
            ErrorKind::NotOpen
        );

        json["schemaVersion"] = (guidesya_core::SCHEMA_VERSION + 1).into();
        fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
        let opened = session.open_guide(&id).unwrap();
        assert!(opened.read_only);
        let before = fs::read(&path).unwrap();
        let err = session.save_guide(opened.guide).unwrap_err();
        assert_eq!(err.kind, ErrorKind::ReadOnly);
        assert_eq!(fs::read(&path).unwrap(), before);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_saves_of_one_guide_do_not_overlap() {
        let root = library();
        let session = Arc::new(Session::start(&root, None).unwrap());
        let id = fixture_id(&root);
        let guide = session.open_guide(&id).unwrap().guide;
        let threads: Vec<_> = (0..8)
            .map(|n| {
                let session = Arc::clone(&session);
                let mut guide = guide.clone();
                guide.title = format!("Save {n}");
                std::thread::spawn(move || session.save_guide(guide))
            })
            .collect();
        for thread in threads {
            thread.join().unwrap().unwrap();
        }
        // The cache and the file agree on whichever save finished last.
        let cached = session.open.lock().unwrap()[&guide.id].guide.title.clone();
        let on_disk = session.open_guide(&id).unwrap().guide.title;
        assert_eq!(cached, on_disk);
        assert!(on_disk.starts_with("Save "));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn step_images() {
        let root = library();
        let session = Session::start(&root, None).unwrap();
        let id = fixture_id(&root);
        let guide = session.open_guide(&id).unwrap().guide;
        let step = guide.steps().next().unwrap();
        let bytes = session
            .step_image(&id, &step.id.to_string(), ImageVariant::Full)
            .unwrap();
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WEBP");
        assert_eq!(
            bytes,
            fs::read(root.join(FIXTURE).join(&step.image.full)).unwrap()
        );

        let missing = session.step_image(&id, &Ulid::new().to_string(), ImageVariant::Full);
        assert_eq!(missing.unwrap_err().kind, ErrorKind::NotFound);
        let bad = session.step_image(&id, "nope", ImageVariant::Crop);
        assert_eq!(bad.unwrap_err().kind, ErrorKind::Invalid);
        let unopened = session.step_image(
            &Ulid::new().to_string(),
            &step.id.to_string(),
            ImageVariant::Full,
        );
        assert_eq!(unopened.unwrap_err().kind, ErrorKind::NotOpen);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn open_unknown_guide_is_not_found() {
        let root = library();
        let session = Session::start(&root, None).unwrap();
        let err = session.open_guide(&Ulid::new().to_string()).unwrap_err();
        assert_eq!(err.kind, ErrorKind::NotFound);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn session_cell_waits_for_set() {
        let cell = Arc::new(SessionCell::default());
        let waiter = {
            let cell = cell.clone();
            std::thread::spawn(move || cell.get().map(|_| ()))
        };
        cell.set(Err(AppError::new(ErrorKind::Io, "boom")));
        assert_eq!(waiter.join().unwrap().unwrap_err().message, "boom");
    }
}
