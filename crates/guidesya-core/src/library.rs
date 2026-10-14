//! Guide library: `.guidesya` bundles, image IO, atomic writes. See design
//! scratchpad #444 ("Library", "Guide bundle", "Writes").
//!
//! A library is a user-chosen folder of bundles:
//!
//! ```text
//! <root>/
//!   My Guide.guidesya/
//!     guide.json
//!     steps/      <ulid>.webp, <ulid>.crop.webp, <ulid>.after.webp
//!     assets/     manually inserted images
//!     .backup/    pre-migration copies of guide.json
//! ```
//!
//! Bundles are identified by the guide's `id`, not by folder name. Every
//! operation on a [`GuideHandle`] re-checks that the bundle still holds that
//! id, so a stale handle never touches another guide.
//!
//! The folder name is derived from the title once, at [`Library::create`],
//! and is **kept when the title changes**: renaming a folder under
//! git/Dropbox/iCloud churns sync, breaks open file handles and file-watcher
//! paths, and is not atomic with the `guide.json` write. Callers that want the
//! folder to follow the title call [`Library::rename_bundle`] explicitly.
//!
//! Bundles are built and torn down under hidden temp names in the root
//! (`.create-<ulid>.tmp`, `.rename-<ulid>.tmp`, `.delete-<ulid>.tmp`), so a
//! visible `.guidesya` folder is always complete. [`Library::open`] sweeps
//! leftovers older than an hour: `.create-*` folders and `.guide.json.*.tmp`
//! files are removed, and a `.rename-*` folder holding a guide is put back
//! as a bundle. `.delete-*` folders are left alone: a failed removal leaves
//! the whole guide there, and only the user should throw it away.
//!
//! `list` scans the folders and never writes; the SQLite index (#340) is a
//! separate layer.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::RwLock;
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Utc};
use image::codecs::webp::WebPEncoder;
use image::{ExtendedColorType, ImageFormat, RgbaImage};
use serde::Deserialize;
use serde_json::Value;
use ulid::Ulid;

use crate::model::Guide;
use crate::SCHEMA_VERSION;

/// Extension of a guide bundle folder.
pub const BUNDLE_EXTENSION: &str = "guidesya";
/// The guide document inside a bundle.
pub const GUIDE_FILE: &str = "guide.json";
/// Step screenshots and crops.
pub const STEPS_DIR: &str = "steps";
/// Manually inserted images.
pub const ASSETS_DIR: &str = "assets";
/// Pre-migration copies of `guide.json`.
pub const BACKUP_DIR: &str = ".backup";
/// Largest WebP width or height.
pub const MAX_WEBP_DIMENSION: u32 = 16383;

/// Longest bundle stem, in bytes, before collision suffixes.
const MAX_STEM_BYTES: usize = 100;
const UNTITLED: &str = "Untitled";
/// Device names Windows reserves regardless of extension.
const WINDOWS_RESERVED: [&str; 30] = [
    "CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$", "COM1", "COM2", "COM3", "COM4", "COM5",
    "COM6", "COM7", "COM8", "COM9", "COM¹", "COM²", "COM³", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5",
    "LPT6", "LPT7", "LPT8", "LPT9", "LPT¹", "LPT²", "LPT³",
];
/// Age after which [`Library::open`] sweeps leftover temp files and folders.
const STALE_TEMP_AGE: Duration = Duration::from_secs(60 * 60);

#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("{path}: invalid guide.json: {source}")]
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("{path}: image error: {source}")]
    Image {
        path: PathBuf,
        source: image::ImageError,
    },
    #[error("{0}: not a .guidesya bundle")]
    NotABundle(PathBuf),
    #[error("{0}: bundle is not inside the library")]
    OutsideLibrary(PathBuf),
    #[error("guide {0} not found in library")]
    NotFound(Ulid),
    /// The bundle (or the guide being saved) has a different id than the
    /// handle, e.g. a stale handle whose folder now holds another guide.
    #[error("{path}: expected guide {expected}, found {found}")]
    IdMismatch {
        path: PathBuf,
        expected: Ulid,
        found: Ulid,
    },
    /// [`Library::create`] was given an id another bundle already holds.
    #[error("guide {id} already exists at {existing}")]
    DuplicateId { id: Ulid, existing: PathBuf },
    #[error("{0:?}: not a safe bundle-relative path")]
    InvalidRelativePath(String),
    #[error("{0}: path goes through a symlink inside the bundle")]
    SymlinkInBundle(PathBuf),
    /// `guide.json` is missing or unreadable, so a write cannot check that
    /// the bundle still holds the guide. [`Library::save_to_force`] skips
    /// the check (e.g. to repair the file from the copy in memory).
    #[error("{path}: cannot verify guide.json: {source}")]
    Unverifiable {
        path: PathBuf,
        source: Box<LibraryError>,
    },
    #[error("{path}: missing or invalid schemaVersion")]
    InvalidSchemaVersion { path: PathBuf },
    #[error("{path}: guide.json schema version {found} is newer than supported {supported}")]
    NewerSchema {
        path: PathBuf,
        found: u32,
        supported: u32,
    },
    /// `needed` is `usize::MAX` if the size computation overflowed.
    #[error("pixel buffer too small: {width}x{height} at {bytes_per_row} bytes/row needs {needed} bytes, got {got}")]
    BufferSize {
        width: u32,
        height: u32,
        bytes_per_row: usize,
        needed: usize,
        got: usize,
    },
    #[error("image size {width}x{height} is outside WebP's 1..={MAX_WEBP_DIMENSION} per side")]
    ImageSize { width: u32, height: u32 },
}

pub type Result<T, E = LibraryError> = std::result::Result<T, E>;

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> LibraryError + '_ {
    move |source| LibraryError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// A guide bundle on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuideHandle {
    pub id: Ulid,
    /// Absolute path of the `.guidesya` folder.
    pub path: PathBuf,
}

impl GuideHandle {
    pub fn guide_json(&self) -> PathBuf {
        self.path.join(GUIDE_FILE)
    }

    pub fn steps_dir(&self) -> PathBuf {
        self.path.join(STEPS_DIR)
    }

    pub fn assets_dir(&self) -> PathBuf {
        self.path.join(ASSETS_DIR)
    }

    pub fn backup_dir(&self) -> PathBuf {
        self.path.join(BACKUP_DIR)
    }

    /// Resolves a bundle-relative path from `guide.json` (e.g.
    /// `steps/<id>.webp`) so it cannot leave the bundle: rejects empty, `.`,
    /// absolute and `..` paths, and any existing component that is a symlink
    /// (bundles live in synced folders and may come from other people).
    pub fn resolve(&self, relative: &str) -> Result<PathBuf> {
        let rel = Path::new(relative);
        let textual_ok = rel.components().next().is_some()
            && rel.components().all(|c| matches!(c, Component::Normal(_)));
        if !textual_ok {
            return Err(LibraryError::InvalidRelativePath(relative.to_owned()));
        }
        let mut path = self.path.clone();
        for component in rel.components() {
            path.push(component);
            match fs::symlink_metadata(&path) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    return Err(LibraryError::SymlinkInBundle(path));
                }
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(io_err(&path)(e)),
            }
        }
        Ok(path)
    }

    /// Writes a step image as lossless WebP, creating `steps/` if needed
    /// (but never the bundle itself: a stale handle fails instead of
    /// recreating a moved bundle). Returns the bundle-relative path to store
    /// in [`crate::model::StepImage`].
    pub fn write_step_image(
        &self,
        step: Ulid,
        variant: StepImageVariant,
        pixels: &PixelBuffer<'_>,
    ) -> Result<String> {
        verify_header(self)?;
        let relative = step_image_path(step, variant);
        let path = self.resolve(&relative)?;
        // `resolve` has rejected a symlinked `steps`; a missing one is created
        // as a real folder.
        let steps = self.steps_dir();
        match fs::create_dir(&steps) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(io_err(&steps)(e)),
        }
        write_webp(&path, pixels)?;
        Ok(relative)
    }

    /// Reads a bundle-relative WebP image as RGBA. WebP only: assets in
    /// other formats (PNG, JPEG) are not supported yet.
    pub fn read_image(&self, relative: &str) -> Result<RgbaImage> {
        read_webp(&self.resolve(relative)?)
    }
}

/// Listing entry for a guide, read from its `guide.json`.
#[derive(Debug, Clone, PartialEq)]
pub struct GuideSummary {
    pub id: Ulid,
    pub path: PathBuf,
    /// Version as stored on disk. Greater than [`SCHEMA_VERSION`] means the
    /// guide is from a newer app; it is listed but `load` refuses it (the
    /// read-only behaviour is decided in #346).
    pub schema_version: u32,
    pub title: String,
    pub summary: String,
    pub tags: Vec<String>,
    pub step_count: usize,
    pub updated_at: DateTime<Utc>,
    /// Set when another bundle holds the same id (e.g. a Finder copy): the
    /// path of the canonical bundle, which `find` and `save` use. The app
    /// should offer to give this copy a new id.
    pub duplicate_of: Option<PathBuf>,
}

impl GuideSummary {
    pub fn handle(&self) -> GuideHandle {
        GuideHandle {
            id: self.id,
            path: self.path.clone(),
        }
    }
}

/// A library folder. Cheap to share by reference; methods take `&self`.
#[derive(Debug)]
pub struct Library {
    root: PathBuf,
    /// Guide id → bundle path, filled by `scan`, `create` and `load`.
    /// Only a hint: entries are re-checked before use.
    known: RwLock<HashMap<Ulid, PathBuf>>,
}

impl Library {
    /// Opens the library at `root`, creating the folder if it is missing.
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        fs::create_dir_all(root).map_err(io_err(root))?;
        let root = fs::canonicalize(root).map_err(io_err(root))?;
        let library = Self {
            root,
            known: RwLock::new(HashMap::new()),
        };
        library.sweep(STALE_TEMP_AGE);
        Ok(library)
    }

    /// Best-effort cleanup of temp files and folders older than `min_age`
    /// left by crashes (see the module docs). Errors are ignored.
    fn sweep(&self, min_age: Duration) {
        let entries = || {
            fs::read_dir(&self.root)
                .into_iter()
                .flatten()
                .filter_map(|e| e.ok())
                .map(|e| e.path())
        };
        for path in entries() {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if !name.ends_with(".tmp") || !path.is_dir() || !is_older_than(&path, min_age) {
                continue;
            }
            if name.starts_with(".create-") {
                let _ = fs::remove_dir_all(&path);
            } else if name.starts_with(".rename-") {
                self.recover_renamed(&path);
            }
        }
        // After recovery, so recovered bundles are swept too.
        for path in entries().filter(|p| is_bundle_path(p) && p.is_dir()) {
            sweep_guide_temps(&path, min_age);
        }
    }

    /// Puts a bundle stranded under a `.rename-*` name back in the library,
    /// named after its title; removes the folder if it holds no guide.
    fn recover_renamed(&self, path: &Path) {
        if !path.join(GUIDE_FILE).is_file() {
            let _ = fs::remove_dir_all(path);
            return;
        }
        let title = read_summary(path)
            .map(|summary| summary.title)
            .unwrap_or_else(|_| "Recovered".to_owned());
        if let Ok(Claim::Fresh(target)) = self.claim_bundle_dir(&sanitize_title(&title), None) {
            if fs::rename(path, &target).is_err() {
                let _ = fs::remove_dir(&target);
            }
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// All readable guides, sorted by most recently updated, including ones
    /// with a newer schema (see [`GuideSummary::schema_version`]) and
    /// duplicate copies (see [`GuideSummary::duplicate_of`]). Unreadable
    /// bundles are left out; [`Library::scan`] reports them.
    pub fn list(&self) -> Result<Vec<GuideSummary>> {
        let mut guides: Vec<_> = self
            .scan()?
            .into_iter()
            .filter_map(|(_, entry)| entry.ok())
            .collect();
        guides.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then(a.id.cmp(&b.id)));
        Ok(guides)
    }

    /// Every `.guidesya` folder in the root, sorted by path, with its summary
    /// or error. Read-only.
    ///
    /// When several bundles share an id, one is canonical and the others get
    /// [`GuideSummary::duplicate_of`]. The canonical one is, in order: the
    /// bundle this `Library` last created, opened or chose; the one whose
    /// folder is named after the title (`<stem>` or `<stem> N`); the oldest
    /// by creation time; the first by path.
    pub fn scan(&self) -> Result<Vec<(PathBuf, Result<GuideSummary>)>> {
        let mut paths: Vec<PathBuf> = fs::read_dir(&self.root)
            .map_err(io_err(&self.root))?
            // One unreadable entry should not hide the rest of the library.
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| is_bundle_path(path) && path.is_dir())
            .collect();
        paths.sort();

        let mut out: Vec<_> = paths
            .into_iter()
            .map(|path| {
                let entry = read_summary(&path);
                (path, entry)
            })
            .collect();

        let mut groups: HashMap<Ulid, Vec<usize>> = HashMap::new();
        for (i, (_, entry)) in out.iter().enumerate() {
            if let Ok(summary) = entry {
                groups.entry(summary.id).or_default().push(i);
            }
        }
        let known = self.known.read().unwrap().clone();
        let mut canonical: HashMap<Ulid, PathBuf> = HashMap::new();
        for (id, indices) in &groups {
            let best = *indices
                .iter()
                .min_by_key(|&&i| {
                    let (path, entry) = &out[i];
                    let title = entry.as_ref().map(|s| s.title.as_str()).unwrap_or("");
                    let created = fs::metadata(path).and_then(|m| m.created()).ok();
                    (
                        known.get(id) != Some(path),
                        !bundle_stem(path)
                            .is_some_and(|stem| has_stem(stem, &sanitize_title(title))),
                        created.is_none(),
                        created,
                        path.clone(),
                    )
                })
                .expect("groups are non-empty");
            canonical.insert(*id, out[best].0.clone());
        }
        for (path, entry) in &mut out {
            if let Ok(summary) = entry {
                let main = &canonical[&summary.id];
                summary.duplicate_of = (main != path).then(|| main.clone());
            }
        }
        self.known.write().unwrap().extend(canonical);
        Ok(out)
    }

    /// The bundle holding guide `id`, if any.
    pub fn find(&self, id: Ulid) -> Result<Option<GuideHandle>> {
        let cached = self.known.read().unwrap().get(&id).cloned();
        if let Some(path) = cached {
            if read_header(&path).is_ok_and(|header| header.id == id) {
                return Ok(Some(GuideHandle { id, path }));
            }
        }
        Ok(self.scan()?.into_iter().find_map(|(path, entry)| {
            entry
                .ok()
                .filter(|summary| summary.id == id && summary.duplicate_of.is_none())
                .map(|_| GuideHandle { id, path })
        }))
    }

    /// Creates a new bundle for `guide` (folder named after its title, with
    /// `steps/` and `assets/`) and writes `guide.json`. Fails with
    /// [`LibraryError::DuplicateId`] if the id is already in the library.
    ///
    /// The bundle is built under a hidden temp name and renamed into place,
    /// so a failure never leaves a partial `.guidesya` folder.
    pub fn create(&self, guide: &Guide) -> Result<GuideHandle> {
        if let Some(existing) = self.find(guide.id)? {
            return Err(LibraryError::DuplicateId {
                id: guide.id,
                existing: existing.path,
            });
        }
        let staging = self.temp_path("create");
        let built = (|| {
            for dir in [
                staging.clone(),
                staging.join(STEPS_DIR),
                staging.join(ASSETS_DIR),
            ] {
                fs::create_dir(&dir).map_err(io_err(&dir))?;
            }
            write_guide(&staging, guide)
        })();
        if let Err(e) = built {
            let _ = fs::remove_dir_all(&staging);
            return Err(e);
        }
        let target = match self.claim_bundle_dir(&sanitize_title(&guide.title), None) {
            Ok(Claim::Fresh(target)) => target,
            Ok(Claim::Same(_)) => unreachable!("no current bundle to match"),
            Err(e) => {
                let _ = fs::remove_dir_all(&staging);
                return Err(e);
            }
        };
        // Replaces the empty folder that reserved the name.
        if let Err(e) = fs::rename(&staging, &target) {
            let _ = fs::remove_dir(&target);
            let _ = fs::remove_dir_all(&staging);
            return Err(io_err(&target)(e));
        }
        // The bundle is in place; a failed fsync must not make it look failed.
        let _ = sync_dir(&self.root);
        let handle = GuideHandle {
            id: guide.id,
            path: target,
        };
        self.remember(&handle);
        Ok(handle)
    }

    /// Loads the guide in bundle `path` (the `.guidesya` folder).
    ///
    /// Fails with [`LibraryError::NewerSchema`] for guides from a newer app.
    /// Explicit opens are where #346 adds the `.backup/` copy and the
    /// rewrite of migrated files; `list`/`scan`/`find` must stay read-only.
    pub fn load(&self, path: impl AsRef<Path>) -> Result<(GuideHandle, Guide)> {
        let path = path.as_ref();
        if !is_bundle_path(path) {
            return Err(LibraryError::NotABundle(path.to_path_buf()));
        }
        let path = fs::canonicalize(path).map_err(io_err(path))?;
        let guide = read_guide(&path)?;
        let handle = GuideHandle { id: guide.id, path };
        self.remember(&handle);
        Ok((handle, guide))
    }

    /// Atomically rewrites `guide.json` in the canonical bundle that holds
    /// `guide.id`. Does not touch `updated_at`; the caller sets it. Fails
    /// with [`LibraryError::NewerSchema`] rather than overwrite a file from a
    /// newer app.
    pub fn save(&self, guide: &Guide) -> Result<GuideHandle> {
        let handle = self
            .find(guide.id)?
            .ok_or(LibraryError::NotFound(guide.id))?;
        self.save_to(&handle, guide)?;
        Ok(handle)
    }

    /// Like [`Library::save`] for a known bundle, without the scan (the
    /// autosave path). Fails with [`LibraryError::IdMismatch`] if `guide` or
    /// the bundle on disk is not `handle.id`, [`LibraryError::NewerSchema`]
    /// if the file on disk is from a newer app, and
    /// [`LibraryError::Unverifiable`] if it is missing or unreadable.
    pub fn save_to(&self, handle: &GuideHandle, guide: &Guide) -> Result<()> {
        check_guide_id(handle, guide)?;
        let header = verify_header(handle)?;
        if let Some(found) = header.schema_version.filter(|v| *v > SCHEMA_VERSION) {
            return Err(LibraryError::NewerSchema {
                path: handle.guide_json(),
                found,
                supported: SCHEMA_VERSION,
            });
        }
        write_guide(&handle.path, guide)
    }

    /// Writes `guide.json` without checking what is on disk: for "Save
    /// anyway" after [`LibraryError::Unverifiable`], e.g. to repair a
    /// truncated file. Overwrites whatever is there, including newer-schema
    /// files. Still requires `guide.id == handle.id` and an existing bundle
    /// folder; it never creates one.
    pub fn save_to_force(&self, handle: &GuideHandle, guide: &Guide) -> Result<()> {
        check_guide_id(handle, guide)?;
        let is_dir = fs::symlink_metadata(&handle.path).map_err(io_err(&handle.path))?;
        if !is_dir.is_dir() {
            return Err(LibraryError::NotABundle(handle.path.clone()));
        }
        write_guide(&handle.path, guide)
    }

    /// Renames the bundle folder to match `title`.
    ///
    /// A no-op if the folder is already `<stem>` or `<stem> N` for the
    /// sanitized title. A change only in case or Unicode form (which APFS
    /// treats as the same name) goes through a temp name and gets no
    /// suffix; otherwise a free name is claimed with collision suffixes.
    pub fn rename_bundle(&self, handle: &GuideHandle, title: &str) -> Result<GuideHandle> {
        self.check_inside(&handle.path)?;
        verify_header(handle)?;
        let stem = sanitize_title(title);
        if bundle_stem(&handle.path).is_some_and(|current| has_stem(current, &stem)) {
            return Ok(handle.clone());
        }
        let target = match self.claim_bundle_dir(&stem, Some(&handle.path))? {
            Claim::Fresh(target) => {
                // Replaces the empty folder that reserved the name.
                if let Err(e) = fs::rename(&handle.path, &target) {
                    let _ = fs::remove_dir(&target);
                    return Err(io_err(&handle.path)(e));
                }
                target
            }
            Claim::Same(target) => {
                let temp = self.temp_path("rename");
                fs::rename(&handle.path, &temp).map_err(io_err(&handle.path))?;
                if let Err(e) = fs::rename(&temp, &target) {
                    let _ = fs::rename(&temp, &handle.path);
                    return Err(io_err(&target)(e));
                }
                target
            }
        };
        // The rename happened; a failed fsync must not make it look failed.
        let _ = sync_dir(&self.root);
        let renamed = GuideHandle {
            id: handle.id,
            path: target,
        };
        self.remember(&renamed);
        Ok(renamed)
    }

    /// Permanently removes a bundle. Refuses paths that are not a
    /// `.guidesya` folder directly inside the library root, and bundles that
    /// no longer hold `handle.id`. The folder is first renamed to a hidden
    /// temp name, so a failed removal never leaves a half-deleted bundle
    /// visible.
    pub fn delete(&self, handle: &GuideHandle) -> Result<()> {
        self.check_inside(&handle.path)?;
        verify_header(handle)?;
        let doomed = self.temp_path("delete");
        fs::rename(&handle.path, &doomed).map_err(io_err(&handle.path))?;
        self.known.write().unwrap().remove(&handle.id);
        let removed = fs::remove_dir_all(&doomed).map_err(io_err(&doomed));
        // Best effort: the bundle is already gone from the library.
        let _ = sync_dir(&self.root);
        removed
    }

    fn remember(&self, handle: &GuideHandle) {
        self.known
            .write()
            .unwrap()
            .insert(handle.id, handle.path.clone());
    }

    /// A fresh hidden path in the root; not a bundle path, so scans skip it.
    fn temp_path(&self, purpose: &str) -> PathBuf {
        self.root.join(format!(".{purpose}-{}.tmp", Ulid::new()))
    }

    fn check_inside(&self, path: &Path) -> Result<()> {
        if !is_bundle_path(path) {
            return Err(LibraryError::NotABundle(path.to_path_buf()));
        }
        let parent = path
            .parent()
            .and_then(|p| fs::canonicalize(p).ok())
            .ok_or_else(|| LibraryError::OutsideLibrary(path.to_path_buf()))?;
        if parent != self.root {
            return Err(LibraryError::OutsideLibrary(path.to_path_buf()));
        }
        Ok(())
    }

    /// Picks a bundle folder for `stem`: `Name.guidesya`, then
    /// `Name 2.guidesya`, `Name 3.guidesya`, ...
    ///
    /// A free name is reserved by creating an empty folder (`create_dir` is
    /// atomic, so concurrent creators never share one) and returned as
    /// [`Claim::Fresh`]. If a candidate is the `current` bundle itself under
    /// a name differing in case or Unicode form, it is returned as
    /// [`Claim::Same`] without creating anything.
    fn claim_bundle_dir(&self, stem: &str, current: Option<&Path>) -> Result<Claim> {
        for n in 1u32.. {
            let name = if n == 1 {
                format!("{stem}.{BUNDLE_EXTENSION}")
            } else {
                format!("{stem} {n}.{BUNDLE_EXTENSION}")
            };
            let path = self.root.join(name);
            if current.is_some_and(|c| same_file(c, &path)) {
                return Ok(Claim::Same(path));
            }
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Claim::Fresh(path)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(io_err(&path)(e)),
            }
        }
        unreachable!("u32 range exhausted")
    }
}

enum Claim {
    /// A newly created empty folder.
    Fresh(PathBuf),
    /// The current bundle's own folder, under the spelling to rename to.
    Same(PathBuf),
}

/// Whether two paths name the same existing file (same device and inode, so
/// spellings that differ only in case or Unicode form match on APFS).
fn same_file(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (fs::symlink_metadata(a), fs::symlink_metadata(b)) {
            (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn is_bundle_path(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == BUNDLE_EXTENSION)
}

fn bundle_stem(path: &Path) -> Option<&str> {
    path.file_stem().and_then(|s| s.to_str())
}

/// Whether a folder stem is `stem` or `stem N` (a collision suffix, N ≥ 2).
fn has_stem(current: &str, stem: &str) -> bool {
    let Some(rest) = current.strip_prefix(stem) else {
        return false;
    };
    rest.is_empty()
        || rest.strip_prefix(' ').is_some_and(|n| {
            !n.starts_with('0')
                && n.bytes().all(|b| b.is_ascii_digit())
                && n.parse::<u32>().is_ok_and(|n| n >= 2)
        })
}

fn is_older_than(path: &Path, min_age: Duration) -> bool {
    fs::symlink_metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age >= min_age)
}

/// Removes stale `.guide.json.*.tmp` files left by an interrupted
/// [`write_atomic`] in one bundle.
fn sweep_guide_temps(bundle: &Path, min_age: Duration) {
    let Ok(entries) = fs::read_dir(bundle) else {
        return;
    };
    let prefix = format!(".{GUIDE_FILE}.");
    for entry in entries.filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        if name.starts_with(&prefix)
            && name.ends_with(".tmp")
            && path.is_file()
            && is_older_than(&path, min_age)
        {
            let _ = fs::remove_file(&path);
        }
    }
}

/// Turns a guide title into a bundle folder stem (without extension).
///
/// - Replaces path separators and characters invalid on Windows or common
///   sync services (`/ \ : * ? " < > |`) with `-`.
/// - Turns control characters and whitespace runs into one space, and drops
///   invisible format characters (Unicode Cf: zero-width, bidi overrides).
/// - Drops leading dots (hidden files) and trailing dots and spaces.
/// - Truncates to 100 bytes on a character boundary.
/// - Appends `_` to Windows device names (`CON`, `nul.txt` → `nul_.txt`,
///   `CONIN$`, `COM¹`).
///
/// Returns `Untitled` if nothing is left.
pub fn sanitize_title(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut pending_space = false;
    for c in title.chars() {
        if is_format_char(c) {
            continue;
        }
        let c = match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            c if c.is_control() || c.is_whitespace() => ' ',
            c => c,
        };
        if c == ' ' {
            pending_space = !out.is_empty();
            continue;
        }
        if c == '.' && out.is_empty() {
            continue;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        out.push(c);
    }
    if out.len() > MAX_STEM_BYTES {
        let mut end = MAX_STEM_BYTES;
        while !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
    }
    let mut out = out.trim_end_matches(['.', ' ']).to_owned();
    if out.is_empty() {
        return UNTITLED.to_owned();
    }
    // Windows reserves the device name before the first dot, ignoring case
    // and trailing spaces.
    let base = out[..out.find('.').unwrap_or(out.len())].trim_end();
    if WINDOWS_RESERVED
        .iter()
        .any(|name| name.eq_ignore_ascii_case(base))
    {
        out.insert(base.len(), '_');
    }
    out
}

/// Unicode general category Cf (format characters), as of Unicode 15.
fn is_format_char(c: char) -> bool {
    matches!(
        c as u32,
        0xAD | 0x600..=0x605
            | 0x61C
            | 0x6DD
            | 0x70F
            | 0x890..=0x891
            | 0x8E2
            | 0x180E
            | 0x200B..=0x200F
            | 0x202A..=0x202E
            | 0x2060..=0x2064
            | 0x2066..=0x206F
            | 0xFEFF
            | 0xFFF9..=0xFFFB
            | 0x110BD
            | 0x110CD
            | 0x13430..=0x1343F
            | 0x1BCA0..=0x1BCA3
            | 0x1D173..=0x1D17A
            | 0xE0001
            | 0xE0020..=0xE007F
    )
}

// ---------------------------------------------------------------------------
// guide.json IO

/// Reads `guide.json` and passes it through [`upgrade`]. Never writes.
fn read_value(bundle: &Path) -> Result<(PathBuf, Value, u32)> {
    let path = bundle.join(GUIDE_FILE);
    let bytes = fs::read(&path).map_err(io_err(&path))?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|source| LibraryError::Json {
        path: path.clone(),
        source,
    })?;
    let (value, version) = upgrade(&path, value)?;
    Ok((path, value, version))
}

/// Migration hook: the single place every `guide.json` read passes through
/// before it is decoded. Pure and in-memory; `path` is only for errors.
///
/// Returns the value at [`SCHEMA_VERSION`] plus the version found on disk.
/// Migrations (#346) go here as ordered `vN → vN+1` steps; callers that
/// see an original version below the current one decide whether to back up
/// and rewrite (only [`Library::load`] should). Values from a newer app are
/// returned unchanged; [`read_guide`] refuses them, summaries list them.
fn upgrade(path: &Path, value: Value) -> Result<(Value, u32)> {
    let version = value
        .get("schemaVersion")
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .filter(|v| *v >= 1)
        .ok_or_else(|| LibraryError::InvalidSchemaVersion {
            path: path.to_path_buf(),
        })?;
    // Only version 1 exists today, so there is nothing to migrate yet.
    Ok((value, version))
}

fn read_guide(bundle: &Path) -> Result<Guide> {
    let (path, value, version) = read_value(bundle)?;
    if version > SCHEMA_VERSION {
        return Err(LibraryError::NewerSchema {
            path,
            found: version,
            supported: SCHEMA_VERSION,
        });
    }
    serde_json::from_value(value).map_err(|source| LibraryError::Json { path, source })
}

/// The fields `list` needs, decoded leniently so guides from newer
/// schemas can still be listed.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawSummary {
    id: Ulid,
    title: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    tags: Vec<String>,
    updated_at: DateTime<Utc>,
    #[serde(default)]
    items: Vec<RawItem>,
}

#[derive(Deserialize)]
struct RawItem {
    #[serde(rename = "type")]
    kind: Option<String>,
}

fn read_summary(bundle: &Path) -> Result<GuideSummary> {
    let (path, value, schema_version) = read_value(bundle)?;
    let raw: RawSummary =
        serde_json::from_value(value).map_err(|source| LibraryError::Json { path, source })?;
    Ok(GuideSummary {
        id: raw.id,
        path: bundle.to_path_buf(),
        schema_version,
        title: raw.title,
        summary: raw.summary,
        tags: raw.tags,
        step_count: raw
            .items
            .iter()
            .filter(|item| item.kind.as_deref() == Some("step"))
            .count(),
        updated_at: raw.updated_at,
        duplicate_of: None,
    })
}

/// The fields a write checks before replacing `guide.json`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Header {
    id: Ulid,
    #[serde(default)]
    schema_version: Option<u32>,
}

/// The id and schema version in a bundle's `guide.json`, without decoding
/// the rest. A missing or undecodable file is [`LibraryError::Unverifiable`].
fn read_header(bundle: &Path) -> Result<Header> {
    let path = bundle.join(GUIDE_FILE);
    let unverifiable = |source| LibraryError::Unverifiable {
        path: path.clone(),
        source: Box::new(source),
    };
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(unverifiable(io_err(&path)(e)));
        }
        Err(e) => return Err(io_err(&path)(e)),
    };
    serde_json::from_slice(&bytes).map_err(|source| {
        unverifiable(LibraryError::Json {
            path: path.clone(),
            source,
        })
    })
}

/// Checks that the bundle on disk still holds `handle.id`.
fn verify_header(handle: &GuideHandle) -> Result<Header> {
    let header = read_header(&handle.path)?;
    if header.id != handle.id {
        return Err(LibraryError::IdMismatch {
            path: handle.path.clone(),
            expected: handle.id,
            found: header.id,
        });
    }
    Ok(header)
}

fn check_guide_id(handle: &GuideHandle, guide: &Guide) -> Result<()> {
    if guide.id != handle.id {
        return Err(LibraryError::IdMismatch {
            path: handle.path.clone(),
            expected: handle.id,
            found: guide.id,
        });
    }
    Ok(())
}

fn write_guide(bundle: &Path, guide: &Guide) -> Result<()> {
    let path = bundle.join(GUIDE_FILE);
    let mut json = serde_json::to_vec_pretty(guide).map_err(|source| LibraryError::Json {
        path: path.clone(),
        source,
    })?;
    json.push(b'\n');
    write_atomic(&path, &json, Durability::Full)
}

/// How hard [`write_atomic`] tries to survive power loss.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Durability {
    /// fsync the file, rename, fsync the folder. On macOS `sync_all` is
    /// `F_FULLFSYNC` (10–30 ms). For `guide.json`.
    Full,
    /// Atomic rename only. Readers still never see a partial file, but after
    /// power loss the file may be missing, stale or empty. For images, which
    /// are written often during capture.
    RenameOnly,
}

/// Replaces `path` with `bytes` atomically: writes a temp file in the same
/// folder and renames it over `path` (with fsyncs per `durability`). On
/// error the temp file is removed.
pub fn write_atomic(path: &Path, bytes: &[u8], durability: Durability) -> Result<()> {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let prefix = format!(".{name}.");
    let mut builder = tempfile::Builder::new();
    builder.prefix(&prefix).suffix(".tmp");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Normal document permissions (umask applies), not tempfile's 0600.
        builder.permissions(fs::Permissions::from_mode(0o666));
    }
    let mut tmp = builder.tempfile_in(dir).map_err(io_err(dir))?;
    tmp.write_all(bytes).map_err(io_err(tmp.path()))?;
    if durability == Durability::Full {
        tmp.as_file().sync_all().map_err(io_err(tmp.path()))?;
    }
    tmp.persist(path).map_err(|e| io_err(path)(e.error))?;
    if durability == Durability::Full {
        sync_dir(dir)?;
    }
    Ok(())
}

fn sync_dir(dir: &Path) -> Result<()> {
    File::open(dir)
        .and_then(|f| f.sync_all())
        .map_err(io_err(dir))
}

// ---------------------------------------------------------------------------
// Images

/// Which image of a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepImageVariant {
    /// Full screenshot: `steps/<id>.webp`.
    Full,
    /// Click crop: `steps/<id>.crop.webp`.
    Crop,
    /// Frame after the action: `steps/<id>.after.webp`.
    After,
}

/// Bundle-relative path of a step image, as stored in `guide.json`.
pub fn step_image_path(step: Ulid, variant: StepImageVariant) -> String {
    let suffix = match variant {
        StepImageVariant::Full => "",
        StepImageVariant::Crop => ".crop",
        StepImageVariant::After => ".after",
    };
    format!("{STEPS_DIR}/{step}{suffix}.webp")
}

/// Byte order of 8-bit, 4-channel pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Rgba,
    /// ScreenCaptureKit / CoreGraphics `kCVPixelFormatType_32BGRA`.
    Bgra,
}

/// A borrowed 8-bit, 4-channel image, possibly with row padding.
///
/// Alpha must be straight (not premultiplied); it is stored as given.
/// Whole-display frames are opaque so this does not matter for them, but
/// premultiplied window captures (transparent corners and shadows) must be
/// un-premultiplied by the caller first.
#[derive(Debug, Clone, Copy)]
pub struct PixelBuffer<'a> {
    pub width: u32,
    pub height: u32,
    /// Row stride; at least `width * 4`.
    pub bytes_per_row: usize,
    pub format: PixelFormat,
    pub data: &'a [u8],
}

impl<'a> PixelBuffer<'a> {
    /// A tightly packed RGBA buffer.
    pub fn rgba(width: u32, height: u32, data: &'a [u8]) -> Self {
        Self {
            width,
            height,
            bytes_per_row: width as usize * 4,
            format: PixelFormat::Rgba,
            data,
        }
    }

    /// A tightly packed BGRA buffer.
    pub fn bgra(width: u32, height: u32, data: &'a [u8]) -> Self {
        Self {
            format: PixelFormat::Bgra,
            ..Self::rgba(width, height, data)
        }
    }

    /// Packed RGBA copy, validating the size.
    pub fn to_rgba(&self) -> Result<Vec<u8>> {
        let height = self.height as usize;
        let row = (self.width as usize).checked_mul(4);
        let needed = row.and_then(|row| match height {
            0 => Some(0),
            h => self.bytes_per_row.checked_mul(h - 1)?.checked_add(row),
        });
        let (row, needed) = match (row, needed) {
            (Some(row), Some(needed)) if self.bytes_per_row >= row && self.data.len() >= needed => {
                (row, needed)
            }
            _ => {
                return Err(LibraryError::BufferSize {
                    width: self.width,
                    height: self.height,
                    bytes_per_row: self.bytes_per_row,
                    needed: needed.unwrap_or(usize::MAX),
                    got: self.data.len(),
                })
            }
        };
        debug_assert!(row * height <= needed);
        let mut out = Vec::with_capacity(row * height);
        for y in 0..height {
            let start = y * self.bytes_per_row;
            out.extend_from_slice(&self.data[start..start + row]);
        }
        if self.format == PixelFormat::Bgra {
            for px in out.chunks_exact_mut(4) {
                px.swap(0, 2);
            }
        }
        Ok(out)
    }
}

/// Encodes `pixels` as lossless WebP and writes it atomically to `path`
/// ([`Durability::RenameOnly`]).
pub fn write_webp(path: &Path, pixels: &PixelBuffer<'_>) -> Result<()> {
    write_atomic(path, &encode_webp(path, pixels)?, Durability::RenameOnly)
}

/// Encodes `pixels` as lossless WebP. `path` is only used in errors.
fn encode_webp(path: &Path, pixels: &PixelBuffer<'_>) -> Result<Vec<u8>> {
    let valid = 1..=MAX_WEBP_DIMENSION;
    if !valid.contains(&pixels.width) || !valid.contains(&pixels.height) {
        // The encoder accepts larger sizes but writes unreadable files.
        return Err(LibraryError::ImageSize {
            width: pixels.width,
            height: pixels.height,
        });
    }
    let rgba = pixels.to_rgba()?;
    let mut out = Vec::new();
    WebPEncoder::new_lossless(&mut out)
        .encode(&rgba, pixels.width, pixels.height, ExtendedColorType::Rgba8)
        .map_err(|source| LibraryError::Image {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(out)
}

/// Reads a WebP file as RGBA.
pub fn read_webp(path: &Path) -> Result<RgbaImage> {
    let bytes = fs::read(path).map_err(io_err(path))?;
    image::load_from_memory_with_format(&bytes, ImageFormat::WebP)
        .map(|img| img.into_rgba8())
        .map_err(|source| LibraryError::Image {
            path: path.to_path_buf(),
            source,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Item, Section};

    fn library() -> (tempfile::TempDir, Library) {
        let dir = tempfile::tempdir().unwrap();
        let lib = Library::open(dir.path().join("Library")).unwrap();
        (dir, lib)
    }

    fn files_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn sample_guide() -> Guide {
        serde_json::from_str(include_str!("../testdata/guide.json")).unwrap()
    }

    #[test]
    fn create_lays_out_bundle() {
        let (_tmp, lib) = library();
        let guide = Guide::new("Export a PDF", "en");
        let handle = lib.create(&guide).unwrap();
        assert_eq!(handle.path, lib.root().join("Export a PDF.guidesya"));
        assert_eq!(files_in(&handle.path), ["assets", "guide.json", "steps"]);
        // No staging folder left behind.
        assert_eq!(files_in(lib.root()), ["Export a PDF.guidesya"]);
    }

    #[test]
    fn create_rejects_existing_id() {
        let (_tmp, lib) = library();
        let guide = Guide::new("Once", "en");
        let handle = lib.create(&guide).unwrap();
        let fresh = Library::open(lib.root()).unwrap();
        assert!(matches!(
            fresh.create(&guide),
            Err(LibraryError::DuplicateId { id, existing }) if id == guide.id && existing == handle.path
        ));
        assert_eq!(files_in(lib.root()), ["Once.guidesya"]);
    }

    #[test]
    fn save_load_round_trip() {
        let (_tmp, lib) = library();
        let mut guide = sample_guide();
        let handle = lib.create(&guide).unwrap();
        let (loaded_handle, loaded) = lib.load(&handle.path).unwrap();
        assert_eq!(loaded_handle, handle);
        assert_eq!(loaded, guide);

        guide.title = "Renamed".into();
        guide.items.push(Item::Section(Section {
            id: Ulid::new(),
            title: "More".into(),
        }));
        assert_eq!(lib.save(&guide).unwrap(), handle);
        assert_eq!(lib.load(&handle.path).unwrap().1, guide);
        // The folder name is kept when the title changes.
        assert!(handle.guide_json().is_file());

        // A fresh Library (empty cache) still finds the guide by scanning.
        let fresh = Library::open(lib.root()).unwrap();
        guide.summary = "changed".into();
        assert_eq!(fresh.save(&guide).unwrap(), handle);
        assert_eq!(fresh.load(&handle.path).unwrap().1.summary, "changed");
    }

    #[test]
    fn save_unknown_guide_fails() {
        let (_tmp, lib) = library();
        let guide = Guide::new("Nowhere", "en");
        assert!(matches!(lib.save(&guide), Err(LibraryError::NotFound(id)) if id == guide.id));
    }

    #[test]
    fn stale_handle_cannot_touch_other_guide() {
        let (_tmp, lib) = library();
        let a = Guide::new("Name", "en");
        let stale = lib.create(&a).unwrap();
        lib.rename_bundle(&stale, "Moved").unwrap();
        let b = Guide::new("Name", "en");
        let hb = lib.create(&b).unwrap();
        assert_eq!(hb.path, stale.path);
        let before = fs::read(hb.guide_json()).unwrap();

        assert!(matches!(
            lib.delete(&stale),
            Err(LibraryError::IdMismatch { .. })
        ));
        assert!(matches!(
            lib.save_to(&stale, &a),
            Err(LibraryError::IdMismatch { .. })
        ));
        // Right handle, wrong guide.
        assert!(matches!(
            lib.save_to(&hb, &a),
            Err(LibraryError::IdMismatch { .. })
        ));
        assert_eq!(fs::read(hb.guide_json()).unwrap(), before);
        assert_eq!(files_in(lib.root()), ["Moved.guidesya", "Name.guidesya"]);
    }

    #[test]
    fn save_refuses_newer_schema_on_disk() {
        let (_tmp, lib) = library();
        let guide = Guide::new("Synced", "en");
        let handle = lib.create(&guide).unwrap();
        let mut value = serde_json::to_value(&guide).unwrap();
        value["schemaVersion"] = (SCHEMA_VERSION + 1).into();
        value["fromTheFuture"] = true.into();
        let text = value.to_string();
        fs::write(handle.guide_json(), &text).unwrap();

        assert!(matches!(
            lib.save_to(&handle, &guide),
            Err(LibraryError::NewerSchema { .. })
        ));
        assert!(matches!(
            lib.save(&guide),
            Err(LibraryError::NewerSchema { .. })
        ));
        assert_eq!(fs::read_to_string(handle.guide_json()).unwrap(), text);
    }

    #[test]
    fn corrupt_guide_json_needs_forced_save() {
        let (_tmp, lib) = library();
        let mut guide = Guide::new("Corrupt", "en");
        let handle = lib.create(&guide).unwrap();
        guide.summary = "from memory".into();

        fs::write(handle.guide_json(), "{\"id\": \"01J").unwrap();
        assert!(matches!(
            lib.save_to(&handle, &guide),
            Err(LibraryError::Unverifiable { .. })
        ));
        lib.save_to_force(&handle, &guide).unwrap();
        assert_eq!(lib.load(&handle.path).unwrap().1, guide);

        fs::remove_file(handle.guide_json()).unwrap();
        assert!(matches!(
            lib.save_to(&handle, &guide),
            Err(LibraryError::Unverifiable { .. })
        ));
        lib.save_to_force(&handle, &guide).unwrap();
        assert_eq!(lib.load(&handle.path).unwrap().1, guide);

        // Never recreates a missing bundle or writes another guide's id.
        let other = Guide::new("Other", "en");
        assert!(matches!(
            lib.save_to_force(&handle, &other),
            Err(LibraryError::IdMismatch { .. })
        ));
        fs::remove_dir_all(&handle.path).unwrap();
        assert!(lib.save_to_force(&handle, &guide).is_err());
        assert!(!handle.path.exists());
    }

    #[test]
    fn stale_handle_image_write_does_not_recreate_bundle() {
        let (_tmp, lib) = library();
        let old = lib.create(&Guide::new("Old", "en")).unwrap();
        let new = lib.rename_bundle(&old, "New").unwrap();
        let rgba = pattern(2, 2);
        assert!(old
            .write_step_image(
                Ulid::new(),
                StepImageVariant::Full,
                &PixelBuffer::rgba(2, 2, &rgba)
            )
            .is_err());
        assert!(!old.path.exists());
        assert_eq!(files_in(lib.root()), ["New.guidesya"]);
        assert!(files_in(&new.steps_dir()).is_empty());
    }

    #[test]
    fn open_sweeps_stale_temps() {
        let (_tmp, lib) = library();
        let guide = Guide::new("Stranded", "en");
        let handle = lib.create(&guide).unwrap();
        let root = lib.root().to_path_buf();
        let two_hours_ago = SystemTime::now() - Duration::from_secs(2 * 60 * 60);
        let age = |path: &Path| {
            File::open(path)
                .unwrap()
                .set_modified(two_hours_ago)
                .unwrap()
        };

        // Old leftovers.
        let create = root.join(format!(".create-{}.tmp", Ulid::new()));
        fs::create_dir_all(create.join(STEPS_DIR)).unwrap();
        age(&create);
        let temp_json = handle.path.join(".guide.json.abc123.tmp");
        fs::write(&temp_json, "partial").unwrap();
        age(&temp_json);
        let empty_rename = root.join(format!(".rename-{}.tmp", Ulid::new()));
        fs::create_dir(&empty_rename).unwrap();
        age(&empty_rename);
        // A bundle stranded mid-rename is recovered, not deleted.
        let stranded = root.join(format!(".rename-{}.tmp", Ulid::new()));
        fs::rename(&handle.path, &stranded).unwrap();
        age(&stranded);
        // Kept: deletes, and anything recent.
        let doomed = root.join(format!(".delete-{}.tmp", Ulid::new()));
        fs::create_dir(&doomed).unwrap();
        age(&doomed);
        let recent = root.join(format!(".create-{}.tmp", Ulid::new()));
        fs::create_dir(&recent).unwrap();

        let reopened = Library::open(&root).unwrap();
        let mut expected = vec![
            doomed.file_name().unwrap().to_string_lossy().into_owned(),
            recent.file_name().unwrap().to_string_lossy().into_owned(),
            "Stranded.guidesya".to_owned(),
        ];
        expected.sort();
        assert_eq!(files_in(&root), expected);
        let (recovered, loaded) = reopened.load(root.join("Stranded.guidesya")).unwrap();
        assert_eq!(loaded, guide);
        assert_eq!(files_in(&recovered.path), ["assets", "guide.json", "steps"]);
    }

    #[test]
    fn atomic_write_leaves_no_temp_files() {
        let (_tmp, lib) = library();
        let mut guide = Guide::new("Atomic", "en");
        let handle = lib.create(&guide).unwrap();
        for i in 0..5 {
            guide.summary = format!("rev {i}");
            lib.save_to(&handle, &guide).unwrap();
        }
        assert_eq!(files_in(&handle.path), ["assets", "guide.json", "steps"]);
        assert_eq!(lib.load(&handle.path).unwrap().1.summary, "rev 4");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(handle.guide_json())
                .unwrap()
                .permissions()
                .mode();
            assert_ne!(mode & 0o044, 0, "guide.json should not be owner-only");
        }
    }

    #[test]
    fn atomic_write_failure_cleans_up() {
        let (tmp, _lib) = library();
        // Target is an existing non-empty directory, so the rename fails.
        let target = tmp.path().join("occupied");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("x"), b"x").unwrap();
        for durability in [Durability::Full, Durability::RenameOnly] {
            assert!(write_atomic(&target, b"data", durability).is_err());
        }
        assert_eq!(files_in(tmp.path()), ["Library", "occupied"]);
    }

    #[test]
    fn list_and_find() {
        let (_tmp, lib) = library();
        let a = Guide::new("Alpha", "en");
        let mut b = Guide::new("Beta", "en");
        b.updated_at = a.updated_at + chrono::Duration::seconds(10);
        b.items = sample_guide().items;
        let ha = lib.create(&a).unwrap();
        let hb = lib.create(&b).unwrap();
        // Ignored: non-bundle folder, stray file. Reported: broken bundle.
        fs::create_dir(lib.root().join("Other")).unwrap();
        fs::write(lib.root().join("notes.txt"), "x").unwrap();
        fs::create_dir(lib.root().join("Broken.guidesya")).unwrap();

        let listed = lib.list().unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].id, b.id);
        assert_eq!(listed[0].step_count, 3);
        assert_eq!(listed[0].schema_version, SCHEMA_VERSION);
        assert_eq!(listed[1].handle(), ha);

        let scanned = lib.scan().unwrap();
        assert_eq!(scanned.len(), 3);
        assert!(scanned
            .iter()
            .any(|(p, r)| p.ends_with("Broken.guidesya") && r.is_err()));

        let fresh = Library::open(lib.root()).unwrap();
        assert_eq!(fresh.find(b.id).unwrap(), Some(hb));
        assert_eq!(fresh.find(Ulid::new()).unwrap(), None);
    }

    /// Copies a bundle's guide.json into a new bundle folder, as Finder's
    /// Duplicate would.
    fn duplicate(handle: &GuideHandle, root: &Path, name: &str) -> PathBuf {
        let copy = root.join(name);
        fs::create_dir(&copy).unwrap();
        fs::copy(handle.guide_json(), copy.join(GUIDE_FILE)).unwrap();
        copy
    }

    #[test]
    fn duplicates_are_listed_and_original_wins() {
        let (_tmp, lib) = library();
        let guide = Guide::new("Original", "en");
        let handle = lib.create(&guide).unwrap();
        // " copy" sorts before ".guidesya", so path order alone would pick it.
        let copy = duplicate(&handle, lib.root(), "Original copy.guidesya");

        let fresh = Library::open(lib.root()).unwrap();
        let listed = fresh.list().unwrap();
        assert_eq!(listed.len(), 2);
        let by_path = |p: &Path| listed.iter().find(|s| s.path == p).unwrap().clone();
        assert_eq!(by_path(&handle.path).duplicate_of, None);
        assert_eq!(by_path(&copy).duplicate_of, Some(handle.path.clone()));
        assert_eq!(fresh.find(guide.id).unwrap(), Some(handle.clone()));

        let mut edited = guide.clone();
        edited.summary = "edited".into();
        assert_eq!(fresh.save(&edited).unwrap(), handle);
        assert_eq!(fresh.load(&handle.path).unwrap().1.summary, "edited");
        assert_eq!(read_summary(&copy).unwrap().summary, "");
    }

    #[test]
    fn opened_duplicate_becomes_canonical() {
        let (_tmp, lib) = library();
        let guide = Guide::new("Original", "en");
        let handle = lib.create(&guide).unwrap();
        let copy = duplicate(&handle, lib.root(), "Original copy.guidesya");

        let fresh = Library::open(lib.root()).unwrap();
        let (opened, _) = fresh.load(&copy).unwrap();
        assert_eq!(opened.path, copy);
        let scanned = fresh.scan().unwrap();
        let original = scanned.iter().find(|(p, _)| *p == handle.path).unwrap();
        assert_eq!(
            original.1.as_ref().unwrap().duplicate_of,
            Some(copy.clone())
        );
        assert_eq!(fresh.find(guide.id).unwrap().unwrap().path, copy);
    }

    #[test]
    fn oldest_duplicate_wins_without_name_match() {
        let (_tmp, lib) = library();
        let guide = Guide::new("Title", "en");
        let handle = lib.create(&guide).unwrap();
        let older = duplicate(&handle, lib.root(), "B elsewhere.guidesya");
        let newer = duplicate(&handle, lib.root(), "A elsewhere.guidesya");
        let delete = Library::open(lib.root()).unwrap();
        delete.delete(&handle).unwrap();

        let fresh = Library::open(lib.root()).unwrap();
        assert_eq!(fresh.find(guide.id).unwrap().unwrap().path, older);
        let scanned = fresh.scan().unwrap();
        let entry = scanned.iter().find(|(p, _)| *p == newer).unwrap();
        assert_eq!(entry.1.as_ref().unwrap().duplicate_of, Some(older));
    }

    #[test]
    fn newer_schema_is_listed_but_not_loaded() {
        let (_tmp, lib) = library();
        let guide = Guide::new("Future", "en");
        let handle = lib.create(&guide).unwrap();
        let mut value = serde_json::to_value(&guide).unwrap();
        value["schemaVersion"] = (SCHEMA_VERSION + 1).into();
        value["someFutureField"] = "x".into();
        let text = value.to_string();
        fs::write(handle.guide_json(), &text).unwrap();

        let listed = lib.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].schema_version, SCHEMA_VERSION + 1);
        assert_eq!(listed[0].title, "Future");
        assert!(matches!(
            lib.load(&handle.path),
            Err(LibraryError::NewerSchema { found, .. }) if found == SCHEMA_VERSION + 1
        ));
        // Listing and loading never rewrite the file.
        assert_eq!(fs::read_to_string(handle.guide_json()).unwrap(), text);
        assert_eq!(files_in(&handle.path), ["assets", "guide.json", "steps"]);

        assert!(matches!(
            lib.load(lib.root()),
            Err(LibraryError::NotABundle(_))
        ));
    }

    #[test]
    fn missing_schema_version_is_an_error() {
        let (_tmp, lib) = library();
        let handle = lib.create(&Guide::new("Bad", "en")).unwrap();
        let mut value = serde_json::to_value(lib.load(&handle.path).unwrap().1).unwrap();
        value.as_object_mut().unwrap().remove("schemaVersion");
        fs::write(handle.guide_json(), value.to_string()).unwrap();
        assert!(matches!(
            lib.load(&handle.path),
            Err(LibraryError::InvalidSchemaVersion { .. })
        ));
    }

    #[test]
    fn name_collisions_get_suffixes() {
        let (_tmp, lib) = library();
        let names: Vec<_> = (0..3)
            .map(|_| lib.create(&Guide::new("Same: name", "en")).unwrap())
            .map(|h| h.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            [
                "Same- name.guidesya",
                "Same- name 2.guidesya",
                "Same- name 3.guidesya"
            ]
        );
    }

    #[test]
    fn rename_bundle_follows_title() {
        let (_tmp, lib) = library();
        let mut guide = Guide::new("Old", "en");
        let handle = lib.create(&guide).unwrap();
        lib.create(&Guide::new("Taken", "en")).unwrap();

        assert_eq!(lib.rename_bundle(&handle, "Old").unwrap(), handle);
        let renamed = lib.rename_bundle(&handle, "Taken").unwrap();
        assert_eq!(renamed.path, lib.root().join("Taken 2.guidesya"));
        assert!(!handle.path.exists());
        guide.title = "Taken".into();
        assert_eq!(lib.save(&guide).unwrap(), renamed);

        // Same title again is a no-op, not a Taken 2 → Taken 3 → Taken 2 cycle.
        for _ in 0..3 {
            assert_eq!(lib.rename_bundle(&renamed, "Taken").unwrap(), renamed);
        }
        assert_eq!(files_in(lib.root()), ["Taken 2.guidesya", "Taken.guidesya"]);
    }

    #[test]
    fn rename_case_or_unicode_form_only() {
        let (_tmp, lib) = library();
        let handle = lib.create(&Guide::new("export pdf", "en")).unwrap();
        let renamed = lib.rename_bundle(&handle, "Export PDF").unwrap();
        assert_eq!(renamed.path, lib.root().join("Export PDF.guidesya"));
        assert_eq!(files_in(lib.root()), ["Export PDF.guidesya"]);

        let nfd = lib.create(&Guide::new("Cafe\u{301}", "en")).unwrap();
        let nfc = lib.rename_bundle(&nfd, "Caf\u{e9}").unwrap();
        assert_eq!(nfc.path, lib.root().join("Caf\u{e9}.guidesya"));
        assert_eq!(fs::read_dir(lib.root()).unwrap().count(), 2);
        assert!(lib.load(&nfc.path).is_ok());
    }

    #[test]
    fn failed_rename_leaves_no_folder() {
        let (_tmp, lib) = library();
        let handle = lib.create(&Guide::new("A", "en")).unwrap();
        fs::remove_dir_all(&handle.path).unwrap();
        assert!(lib.rename_bundle(&handle, "B").is_err());
        assert!(files_in(lib.root()).is_empty());
    }

    #[test]
    fn has_stem_matches_suffixes() {
        assert!(has_stem("Taken", "Taken"));
        assert!(has_stem("Taken 2", "Taken"));
        assert!(has_stem("Taken 17", "Taken"));
        assert!(!has_stem("Taken 1", "Taken"));
        assert!(!has_stem("Taken 02", "Taken"));
        assert!(!has_stem("Taken 2x", "Taken"));
        assert!(!has_stem("Takenly", "Taken"));
        assert!(!has_stem("taken", "Taken"));
    }

    #[test]
    fn delete_removes_bundle() {
        let (tmp, lib) = library();
        let guide = Guide::new("Doomed", "en");
        let handle = lib.create(&guide).unwrap();
        lib.delete(&handle).unwrap();
        assert!(!handle.path.exists());
        assert!(files_in(lib.root()).is_empty());
        assert_eq!(lib.find(guide.id).unwrap(), None);

        // Refuses folders outside the root or that are not bundles.
        let outside = tmp.path().join("Elsewhere.guidesya");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join(GUIDE_FILE), "{}").unwrap();
        let foreign = GuideHandle {
            id: guide.id,
            path: outside.clone(),
        };
        assert!(matches!(
            lib.delete(&foreign),
            Err(LibraryError::OutsideLibrary(_))
        ));
        assert!(outside.exists());
        let plain = GuideHandle {
            id: guide.id,
            path: lib.root().to_path_buf(),
        };
        assert!(matches!(
            lib.delete(&plain),
            Err(LibraryError::NotABundle(_))
        ));
    }

    #[test]
    fn sanitizes_titles() {
        assert_eq!(sanitize_title("Export a PDF"), "Export a PDF");
        assert_eq!(
            sanitize_title("a/b\\c:d*e?f\"g<h>i|j"),
            "a-b-c-d-e-f-g-h-i-j"
        );
        assert_eq!(sanitize_title("  lots \t of\n\nspace  "), "lots of space");
        assert_eq!(sanitize_title("..hidden"), "hidden");
        assert_eq!(sanitize_title("trailing. . "), "trailing");
        assert_eq!(sanitize_title("ctrl\u{0}\u{7f}chars"), "ctrl chars");
        assert_eq!(sanitize_title(""), "Untitled");
        assert_eq!(sanitize_title(" ... "), "Untitled");
        assert_eq!(sanitize_title("Café ✓ 日本語"), "Café ✓ 日本語");

        let long = "é".repeat(80); // 160 bytes
        let cut = sanitize_title(&long);
        assert!(cut.len() <= MAX_STEM_BYTES);
        assert_eq!(cut, "é".repeat(50));
    }

    #[test]
    fn sanitize_strips_format_chars() {
        assert_eq!(sanitize_title("a\u{200B}b\u{202E}c\u{FEFF}d"), "abcd");
        assert_eq!(sanitize_title("evil\u{202E}fdp.exe"), "evilfdp.exe");
        assert_eq!(sanitize_title("\u{200B}"), "Untitled");
        assert_eq!(sanitize_title("soft\u{AD}hyphen"), "softhyphen");
    }

    #[test]
    fn sanitize_escapes_windows_device_names() {
        assert_eq!(sanitize_title("CON"), "CON_");
        assert_eq!(sanitize_title("nul.txt"), "nul_.txt");
        assert_eq!(sanitize_title("com1"), "com1_");
        assert_eq!(sanitize_title("LPT9 .x"), "LPT9_ .x");
        assert_eq!(sanitize_title("Console"), "Console");
        assert_eq!(sanitize_title("COM10"), "COM10");
        assert_eq!(sanitize_title("my CON"), "my CON");
        assert_eq!(sanitize_title("conin$"), "conin$_");
        assert_eq!(sanitize_title("CONOUT$.log"), "CONOUT$_.log");
        assert_eq!(sanitize_title("COM\u{b9}"), "COM\u{b9}_");
        assert_eq!(sanitize_title("lpt\u{b3}"), "lpt\u{b3}_");
    }

    #[test]
    fn step_image_paths() {
        let id: Ulid = "01J9ZQ3K8V4N6T2W5X7Y8Z9A0D".parse().unwrap();
        assert_eq!(
            step_image_path(id, StepImageVariant::Full),
            "steps/01J9ZQ3K8V4N6T2W5X7Y8Z9A0D.webp"
        );
        assert_eq!(
            step_image_path(id, StepImageVariant::Crop),
            "steps/01J9ZQ3K8V4N6T2W5X7Y8Z9A0D.crop.webp"
        );
        assert_eq!(
            step_image_path(id, StepImageVariant::After),
            "steps/01J9ZQ3K8V4N6T2W5X7Y8Z9A0D.after.webp"
        );
    }

    #[test]
    fn resolve_rejects_escapes() {
        let handle = GuideHandle {
            id: Ulid::new(),
            path: PathBuf::from("/nonexistent-lib/G.guidesya"),
        };
        assert_eq!(
            handle.resolve("steps/a.webp").unwrap(),
            Path::new("/nonexistent-lib/G.guidesya/steps/a.webp")
        );
        for bad in ["", ".", "./", "/etc/passwd", "../x.webp", "steps/../../x"] {
            assert!(handle.resolve(bad).is_err(), "{bad:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_cannot_escape_bundle() {
        use std::os::unix::fs::symlink;
        let (tmp, lib) = library();
        let handle = lib.create(&Guide::new("Linked", "en")).unwrap();
        let outside = tmp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        let rgba = pattern(4, 4);
        let pixels = PixelBuffer::rgba(4, 4, &rgba);

        // Symlinked folder.
        fs::remove_dir(handle.steps_dir()).unwrap();
        symlink(&outside, handle.steps_dir()).unwrap();
        let step = Ulid::new();
        assert!(matches!(
            handle.write_step_image(step, StepImageVariant::Full, &pixels),
            Err(LibraryError::SymlinkInBundle(_))
        ));
        assert!(files_in(&outside).is_empty());

        // Symlinked file.
        fs::remove_file(handle.steps_dir()).unwrap();
        fs::create_dir(handle.steps_dir()).unwrap();
        write_webp(&outside.join("secret.webp"), &pixels).unwrap();
        symlink(
            outside.join("secret.webp"),
            handle.steps_dir().join("x.webp"),
        )
        .unwrap();
        assert!(matches!(
            handle.read_image("steps/x.webp"),
            Err(LibraryError::SymlinkInBundle(_))
        ));
    }

    /// Deterministic RGBA test pattern with varied alpha.
    fn pattern(width: u32, height: u32) -> Vec<u8> {
        (0..width * height)
            .flat_map(|i| {
                let (x, y) = (i % width, i / width);
                [
                    (x * 7) as u8,
                    (y * 13) as u8,
                    (x ^ y) as u8,
                    if (x + y) % 5 == 0 {
                        255
                    } else {
                        (64 + x * 3) as u8
                    },
                ]
            })
            .collect()
    }

    #[test]
    fn webp_round_trip_is_lossless() {
        let (_tmp, lib) = library();
        let handle = lib.create(&Guide::new("Images", "en")).unwrap();
        let (w, h) = (37, 23);
        let rgba = pattern(w, h);
        let step = Ulid::new();

        let rel = handle
            .write_step_image(
                step,
                StepImageVariant::Full,
                &PixelBuffer::rgba(w, h, &rgba),
            )
            .unwrap();
        assert_eq!(rel, step_image_path(step, StepImageVariant::Full));
        let back = handle.read_image(&rel).unwrap();
        assert_eq!(back.dimensions(), (w, h));
        assert_eq!(back.as_raw(), &rgba);

        // BGRA with row padding decodes to the same RGBA.
        let stride = w as usize * 4 + 12;
        let mut bgra = vec![0xAB; stride * h as usize];
        for y in 0..h as usize {
            for x in 0..w as usize {
                let s = (y * w as usize + x) * 4;
                let d = y * stride + x * 4;
                bgra[d..d + 4].copy_from_slice(&[rgba[s + 2], rgba[s + 1], rgba[s], rgba[s + 3]]);
            }
        }
        let buffer = PixelBuffer {
            bytes_per_row: stride,
            ..PixelBuffer::bgra(w, h, &bgra)
        };
        let rel = handle
            .write_step_image(step, StepImageVariant::Crop, &buffer)
            .unwrap();
        assert_eq!(handle.read_image(&rel).unwrap().as_raw(), &rgba);
        assert_eq!(files_in(&handle.steps_dir()).len(), 2);
    }

    #[test]
    fn write_step_image_creates_steps_dir() {
        let (_tmp, lib) = library();
        let handle = lib.create(&Guide::new("No steps", "en")).unwrap();
        fs::remove_dir(handle.steps_dir()).unwrap();
        let rgba = pattern(2, 2);
        let rel = handle
            .write_step_image(
                Ulid::new(),
                StepImageVariant::After,
                &PixelBuffer::rgba(2, 2, &rgba),
            )
            .unwrap();
        assert_eq!(handle.read_image(&rel).unwrap().as_raw(), &rgba);
    }

    #[test]
    fn webp_rejects_out_of_range_sizes() {
        let (tmp, _lib) = library();
        let wide = vec![0u8; (MAX_WEBP_DIMENSION as usize + 1) * 4];
        for (w, h, data) in [
            (MAX_WEBP_DIMENSION + 1, 1, &wide[..]),
            (1, MAX_WEBP_DIMENSION + 1, &wide[..]),
            (0, 0, &[][..]),
            (0, 5, &[][..]),
        ] {
            let path = tmp.path().join("x.webp");
            assert!(
                matches!(
                    write_webp(&path, &PixelBuffer::rgba(w, h, data)),
                    Err(LibraryError::ImageSize { .. })
                ),
                "{w}x{h}"
            );
            assert!(!path.exists());
        }
        let edge = &wide[..MAX_WEBP_DIMENSION as usize * 4];
        let path = tmp.path().join("edge.webp");
        write_webp(&path, &PixelBuffer::rgba(MAX_WEBP_DIMENSION, 1, edge)).unwrap();
        assert_eq!(read_webp(&path).unwrap().width(), MAX_WEBP_DIMENSION);
    }

    #[test]
    fn pixel_buffer_size_is_checked() {
        let data = vec![0u8; 15];
        let err = PixelBuffer::rgba(2, 2, &data).to_rgba().unwrap_err();
        assert!(matches!(
            err,
            LibraryError::BufferSize {
                needed: 16,
                got: 15,
                ..
            }
        ));
        let narrow = PixelBuffer {
            bytes_per_row: 4,
            ..PixelBuffer::rgba(2, 1, &data)
        };
        assert!(narrow.to_rgba().is_err());
    }

    #[test]
    fn pixel_buffer_size_overflow_is_an_error() {
        let data = vec![0u8; 16];
        let huge_stride = PixelBuffer {
            bytes_per_row: usize::MAX,
            ..PixelBuffer::rgba(1, 3, &data)
        };
        assert!(matches!(
            huge_stride.to_rgba(),
            Err(LibraryError::BufferSize {
                needed: usize::MAX,
                ..
            })
        ));
    }
}
