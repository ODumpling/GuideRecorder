//! Guide library: `.guiderecorder` bundles, image IO, atomic writes. See design
//! scratchpad #444 ("Library", "Guide bundle", "Writes").
//!
//! A library is a user-chosen folder of bundles:
//!
//! ```text
//! <root>/
//!   My Guide.guiderecorder/
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
//! visible `.guiderecorder` folder is always complete. [`Library::recover`],
//! which only the app calls, sweeps leftovers older than an hour (by the
//! ULID timestamp in the name): `.create-*` folders and stale temp files are
//! removed, and a `.rename-*` folder holding a guide is put back as a
//! bundle. `.delete-*` folders are left alone: a failed removal leaves the
//! whole guide there, and only the user should throw it away.
//!
//! Readers and the app: [`Library::open`], `list`, `scan`, `find` and
//! [`Library::read`] never write, so MCP and CLI readers share a library
//! with the app using only those. The app opens guides with
//! [`Library::load`] (which may migrate and rewrite the file), saves,
//! creates, renames, deletes and calls [`Library::recover`]. The SQLite
//! index (#340) is a separate layer.
//!
//! Schema versions (#444 "Versioning", [`crate::migrate`]): every read
//! migrates older `guide.json` files in memory. `load` also backs up the
//! original to `.backup/guide.v{N}.json` and rewrites the file. Guides from
//! a newer app are listed and open read-only; saves refuse to overwrite them.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Utc};
use image::codecs::webp::WebPEncoder;
use image::{ExtendedColorType, ImageFormat, RgbaImage};
use serde::Deserialize;
use serde_json::Value;
use ulid::Ulid;

use crate::migrate::{self, MigrationError, Schema};
use crate::model::Guide;

/// Extension of a guide bundle folder.
pub const BUNDLE_EXTENSION: &str = "guiderecorder";
/// Extension of bundles made before the rename to GuideRecorder (GuidesYa).
/// Still listed, opened and saved in place; new bundles use [`BUNDLE_EXTENSION`].
pub const LEGACY_BUNDLE_EXTENSION: &str = "guidesya";
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
const WINDOWS_RESERVED: [&str; 32] = [
    "CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$", "COM0", "COM1", "COM2", "COM3", "COM4",
    "COM5", "COM6", "COM7", "COM8", "COM9", "COM¹", "COM²", "COM³", "LPT0", "LPT1", "LPT2", "LPT3",
    "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9", "LPT¹", "LPT²", "LPT³",
];
/// Age after which [`Library::recover`] sweeps leftover temp files and
/// folders.
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
    #[error("{0}: not a .guiderecorder bundle")]
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
    #[error("{path}: {source}")]
    Migration {
        path: PathBuf,
        source: MigrationError,
    },
    /// The file on disk is at an older schema that [`Library::load`] has not
    /// backed up and rewritten yet, so a save would lose the original.
    #[error("{path}: guide.json schema version {found} has not been migrated to {current}")]
    NotMigrated {
        path: PathBuf,
        found: u32,
        current: u32,
    },
    /// `guide.json` changed on disk while [`Library::load`] was migrating
    /// it (e.g. a sync); the migration was not written back.
    #[error("{0}: changed on disk during migration")]
    ChangedOnDisk(PathBuf),
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
    /// Absolute path of the `.guiderecorder` folder.
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
    /// Version as stored on disk. Lower than [`crate::SCHEMA_VERSION`]: the
    /// other fields come from a migration in memory, and `load` will rewrite
    /// the file. Greater: the guide is from a newer app and loads read-only.
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

/// The result of [`Library::load`].
#[derive(Debug, Clone)]
pub struct LoadedGuide {
    pub handle: GuideHandle,
    pub guide: Guide,
    /// Show the guide, but do not offer editing. Set when:
    ///
    /// - it is from a newer app (its `schemaVersion` is above
    ///   [`crate::SCHEMA_VERSION`]): `save` and `save_to` refuse with
    ///   [`LibraryError::NewerSchema`], and fields this app does not know
    ///   are missing from `guide`;
    /// - it was migrated in memory but the backup or rewrite failed (see
    ///   [`LoadedGuide::migration_error`]): `save` and `save_to` refuse with
    ///   [`LibraryError::NotMigrated`] until a later `load` succeeds.
    pub read_only: bool,
    /// The version on disk before `load` migrated the guide. The file was
    /// rewritten unless [`LoadedGuide::migration_error`] is set.
    pub migrated_from: Option<u32>,
    /// Where the original file was copied before the rewrite.
    pub backup: Option<PathBuf>,
    /// Why the migrated guide could not be written back (e.g. a read-only
    /// folder). `guide.json` is untouched; `guide` is the migrated copy in
    /// memory. A warning for the user, not a failed load.
    pub migration_error: Option<Arc<LibraryError>>,
}

/// What [`Library::recover`] cleaned up.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Recovery {
    /// Stale temp files and folders that were deleted.
    pub removed: Vec<PathBuf>,
    /// Bundles put back from a stranded `.rename-*` folder (new paths).
    pub restored: Vec<PathBuf>,
}

/// A library folder. Cheap to share by reference; methods take `&self`.
#[derive(Debug)]
pub struct Library {
    root: PathBuf,
    /// The schema guides are read at; [`migrate::CURRENT`] outside tests.
    schema: Schema,
    /// Guide id → bundle path, filled by `scan`, `create` and `load`.
    /// Only a hint: entries are re-checked before use.
    known: RwLock<HashMap<Ulid, PathBuf>>,
}

impl Library {
    /// Opens the library at `root`, creating the folder if it is missing.
    /// Writes nothing else; see [`Library::recover`].
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_schema(root, migrate::CURRENT)
    }

    fn open_with_schema(root: impl AsRef<Path>, schema: Schema) -> Result<Self> {
        let root = root.as_ref();
        fs::create_dir_all(root).map_err(io_err(root))?;
        let root = fs::canonicalize(root).map_err(io_err(root))?;
        Ok(Self {
            root,
            schema,
            known: RwLock::new(HashMap::new()),
        })
    }

    /// Cleans up after crashes: removes temp files and folders older than an
    /// hour and puts stranded bundles back (see the module docs). Only the
    /// app calls this, at startup; readers (MCP, CLI) never do. Best-effort:
    /// anything that fails is left for next time.
    pub fn recover(&self) -> Recovery {
        self.recover_older_than(STALE_TEMP_AGE)
    }

    fn recover_older_than(&self, min_age: Duration) -> Recovery {
        let mut report = Recovery::default();
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
            // Real folders only: a symlink is not ours.
            if !is_real_dir(&path) {
                continue;
            }
            if temp_name_is_older_than(&name, "create", min_age) {
                if fs::remove_dir_all(&path).is_ok() {
                    report.removed.push(path);
                }
            } else if temp_name_is_older_than(&name, "rename", min_age) {
                match self.recover_renamed(&path) {
                    Some(restored) => report.restored.push(restored),
                    None if !path.exists() => report.removed.push(path),
                    None => {}
                }
            }
        }
        // After recovery, so restored bundles are swept too. Real folders
        // only: a symlinked bundle points outside the library.
        for bundle in entries().filter(|p| is_bundle_path(p) && is_real_dir(p)) {
            let guide_temp = format!(".{GUIDE_FILE}.");
            sweep_temp_files(&bundle, min_age, &mut report, |name| {
                name.starts_with(&guide_temp)
            });
            sweep_temp_files(&bundle.join(STEPS_DIR), min_age, &mut report, |name| {
                name.starts_with('.') && name.contains(".webp.")
            });
            sweep_temp_files(&bundle.join(BACKUP_DIR), min_age, &mut report, |name| {
                name.starts_with('.')
            });
        }
        report
    }

    /// Puts a bundle stranded under a `.rename-*` name back in the library,
    /// named after its title, and returns its new path; removes the folder
    /// if it holds no guide.
    fn recover_renamed(&self, path: &Path) -> Option<PathBuf> {
        if !path.join(GUIDE_FILE).is_file() {
            let _ = fs::remove_dir_all(path);
            return None;
        }
        // Not `read_summary`: its placeholder for undecodable newer guides
        // is titled after the folder, here a `.rename-<ulid>.tmp` name.
        let title = read_value(path, &self.schema)
            .ok()
            .and_then(|(_, value, _)| RawSummary::deserialize(&value).ok())
            .map(|raw| raw.title)
            .unwrap_or_else(|| "Recovered".to_owned());
        if let Ok(Claim::Fresh(target)) = self.claim_bundle_dir(&sanitize_title(&title), None) {
            if fs::rename(path, &target).is_ok() {
                return Some(target);
            }
            let _ = fs::remove_dir(&target);
        }
        None
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

    /// Every `.guiderecorder` folder in the root, sorted by path, with its summary
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
                let entry = read_summary(&path, &self.schema);
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
    /// so a failure never leaves a partial `.guiderecorder` folder.
    pub fn create(&self, guide: &Guide) -> Result<GuideHandle> {
        let would_be = self.root.join(format!(
            "{}.{BUNDLE_EXTENSION}",
            sanitize_title(&guide.title)
        ));
        self.check_writable_version(&would_be, guide)?;
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

    /// Opens the guide in bundle `path` (the `.guiderecorder` folder). This is
    /// the only read that writes:
    ///
    /// - An older schema is migrated. The original bytes are first copied to
    ///   `.backup/guide.v{N}.json` (`guide.v{N}.2.json`, ... if taken; an
    ///   existing backup is never overwritten), then `guide.json` is
    ///   rewritten atomically. If either write fails, `guide.json` is
    ///   untouched and the migrated guide loads read-only with
    ///   [`LoadedGuide::migration_error`] set. A failed migration step is an
    ///   error ([`LibraryError::Migration`]): there is no guide to show.
    /// - A newer schema loads with [`LoadedGuide::read_only`] set.
    pub fn load(&self, path: impl AsRef<Path>) -> Result<LoadedGuide> {
        let (mut loaded, bytes) = self.open_guide(path.as_ref())?;
        if let Some(version) = loaded.migrated_from {
            if let Err(e) = write_back(&mut loaded, version, &bytes) {
                loaded.read_only = true;
                loaded.migration_error = Some(Arc::new(e));
            }
        }
        self.remember(&loaded.handle);
        Ok(loaded)
    }

    /// Reads the guide in bundle `path` without writing anything: the path
    /// for MCP and CLI readers, which must not change a library the app
    /// owns. An older schema is migrated in memory only
    /// ([`LoadedGuide::migrated_from`] is set, `backup` is not); any guide
    /// not at the current schema is [`LoadedGuide::read_only`].
    ///
    /// Unlike `load`, reading a duplicate copy does not make it the
    /// canonical bundle for its id: readers (and the search index, which
    /// reads every copy) must not change which copy `find` and `save` use.
    pub fn read(&self, path: impl AsRef<Path>) -> Result<LoadedGuide> {
        let (mut loaded, _) = self.open_guide(path.as_ref())?;
        loaded.read_only |= loaded.migrated_from.is_some();
        Ok(loaded)
    }

    /// Reads, migrates in memory and decodes a bundle's guide. Returns the
    /// original bytes for the backup. Pure.
    fn open_guide(&self, path: &Path) -> Result<(LoadedGuide, Vec<u8>)> {
        if !is_bundle_path(path) {
            return Err(LibraryError::NotABundle(path.to_path_buf()));
        }
        let path = fs::canonicalize(path).map_err(io_err(path))?;
        let json = path.join(GUIDE_FILE);
        let bytes = fs::read(&json).map_err(io_err(&json))?;
        let (value, version) = parse_and_upgrade(&json, &bytes, &self.schema)?;
        let guide: Guide = serde_json::from_value(value).map_err(|source| {
            if version > self.schema.version {
                // A newer format this app cannot decode: say so, rather
                // than report it as corrupt.
                LibraryError::NewerSchema {
                    path: json.clone(),
                    found: version,
                    supported: self.schema.version,
                }
            } else {
                LibraryError::Json {
                    path: json.clone(),
                    source,
                }
            }
        })?;
        let loaded = LoadedGuide {
            read_only: version > self.schema.version,
            migrated_from: (version < self.schema.version).then_some(version),
            backup: None,
            migration_error: None,
            handle: GuideHandle { id: guide.id, path },
            guide,
        };
        Ok((loaded, bytes))
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
    /// if the file on disk or `guide` itself is from a newer app (a
    /// read-only [`LoadedGuide`]), [`LibraryError::NotMigrated`] if the file
    /// on disk is older and was never backed up and rewritten by `load`, and
    /// [`LibraryError::Unverifiable`] if the file is missing or unreadable.
    pub fn save_to(&self, handle: &GuideHandle, guide: &Guide) -> Result<()> {
        check_guide_id(handle, guide)?;
        self.check_writable_version(&handle.path, guide)?;
        let header = verify_header(handle)?;
        if let Some(found) = header.schema_version.filter(|v| *v > self.schema.version) {
            return Err(LibraryError::NewerSchema {
                path: handle.guide_json(),
                found,
                supported: self.schema.version,
            });
        }
        if let Some(found) = header.schema_version.filter(|v| *v < self.schema.version) {
            return Err(LibraryError::NotMigrated {
                path: handle.guide_json(),
                found,
                current: self.schema.version,
            });
        }
        write_guide(&handle.path, guide)
    }

    /// Writes `guide.json` without the usual checks: for "Save anyway"
    /// after [`LibraryError::Unverifiable`], e.g. to repair a truncated
    /// file. Overwrites whatever is there, including newer-schema files.
    /// Still requires `guide.id == handle.id`, a `guide` that is not itself
    /// from a newer app, and an existing bundle folder; it never creates one.
    ///
    /// An original at any other schema version (older, or from a newer app)
    /// is never destroyed without a copy: it is backed up to `.backup/`
    /// first (as [`Library::load`] does), and if that fails the save fails.
    pub fn save_to_force(&self, handle: &GuideHandle, guide: &Guide) -> Result<()> {
        check_guide_id(handle, guide)?;
        self.check_writable_version(&handle.path, guide)?;
        let is_dir = fs::symlink_metadata(&handle.path).map_err(io_err(&handle.path))?;
        if !is_dir.is_dir() {
            return Err(LibraryError::NotABundle(handle.path.clone()));
        }
        // A missing file, or one without a valid version (the repair case),
        // has nothing worth a backup. The version is read on its own, so a
        // broken `id` does not hide it.
        if let Ok(original) = fs::read(handle.guide_json()) {
            let other_version = serde_json::from_slice::<Value>(&original)
                .ok()
                .and_then(|value| value.get("schemaVersion").and_then(Value::as_u64))
                .and_then(|v| u32::try_from(v).ok())
                .filter(|v| *v >= 1 && *v != self.schema.version);
            if let Some(version) = other_version {
                write_backup(handle, version, &original)?;
            }
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
    /// `.guiderecorder` folder directly inside the library root, and bundles that
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

    /// Refuses to write a guide decoded from a newer schema: this app has
    /// already dropped the fields it does not know.
    fn check_writable_version(&self, bundle: &Path, guide: &Guide) -> Result<()> {
        if guide.schema_version > self.schema.version {
            return Err(LibraryError::NewerSchema {
                path: bundle.join(GUIDE_FILE),
                found: guide.schema_version,
                supported: self.schema.version,
            });
        }
        Ok(())
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

    /// Picks a bundle folder for `stem`: `Name.guiderecorder`, then
    /// `Name 2.guiderecorder`, `Name 3.guiderecorder`, ...
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

/// A directory, not a symlink to one.
fn is_real_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_dir())
}

fn is_bundle_path(path: &Path) -> bool {
    path.extension().is_some_and(is_bundle_extension)
}

/// Whether `ext` is a bundle folder's extension, current or legacy.
pub fn is_bundle_extension(ext: &std::ffi::OsStr) -> bool {
    ext == BUNDLE_EXTENSION || ext == LEGACY_BUNDLE_EXTENSION
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

/// Whether a file's mtime is at least `min_age` ago. For temp files, which
/// are written as they are created; folders use [`temp_name_is_older_than`].
fn is_older_than(path: &Path, min_age: Duration) -> bool {
    fs::symlink_metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age >= min_age)
}

/// Whether `name` is a `.{purpose}-<ulid>.tmp` temp folder name whose ULID
/// was minted at least `min_age` ago. Renaming a folder does not touch its
/// mtime, so the name is the only reliable age.
fn temp_name_is_older_than(name: &str, purpose: &str, min_age: Duration) -> bool {
    name.strip_prefix('.')
        .and_then(|n| n.strip_prefix(purpose))
        .and_then(|n| n.strip_prefix('-'))
        .and_then(|n| n.strip_suffix(".tmp"))
        .and_then(|ulid| ulid.parse::<Ulid>().ok())
        .and_then(|ulid| SystemTime::now().duration_since(ulid.datetime()).ok())
        .is_some_and(|age| age >= min_age)
}

/// Removes stale `*.tmp` files in `dir` left by an interrupted
/// [`write_atomic`] or backup, for names `matches` accepts.
fn sweep_temp_files(
    dir: &Path,
    min_age: Duration,
    report: &mut Recovery,
    matches: impl Fn(&str) -> bool,
) {
    // `read_dir` follows symlinks: a symlinked `steps/` or `.backup` would
    // let the sweep delete files outside the bundle.
    if !is_real_dir(dir) {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        let is_file = fs::symlink_metadata(&path).is_ok_and(|m| m.is_file());
        if name.ends_with(".tmp")
            && matches(&name)
            && is_file
            && is_older_than(&path, min_age)
            && fs::remove_file(&path).is_ok()
        {
            report.removed.push(path);
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
fn read_value(bundle: &Path, schema: &Schema) -> Result<(PathBuf, Value, u32)> {
    let path = bundle.join(GUIDE_FILE);
    let bytes = fs::read(&path).map_err(io_err(&path))?;
    let (value, version) = parse_and_upgrade(&path, &bytes, schema)?;
    Ok((path, value, version))
}

fn parse_and_upgrade(path: &Path, bytes: &[u8], schema: &Schema) -> Result<(Value, u32)> {
    let value: Value = serde_json::from_slice(bytes).map_err(|source| LibraryError::Json {
        path: path.to_path_buf(),
        source,
    })?;
    upgrade(path, value, schema)
}

/// Migration hook: the single place every `guide.json` read passes through
/// before it is decoded. Pure and in-memory; `path` is only for errors.
///
/// Returns the value migrated to `schema.version` (see [`crate::migrate`])
/// plus the version found on disk. Only [`Library::load`] acts on an older
/// original version, by backing up and rewriting the file. Values from a
/// newer app are returned unchanged.
fn upgrade(path: &Path, value: Value, schema: &Schema) -> Result<(Value, u32)> {
    let version = value
        .get("schemaVersion")
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .filter(|v| *v >= 1)
        .ok_or_else(|| LibraryError::InvalidSchemaVersion {
            path: path.to_path_buf(),
        })?;
    if version >= schema.version {
        return Ok((value, version));
    }
    let (value, _) = schema
        .migrate(value, version)
        .map_err(|source| LibraryError::Migration {
            path: path.to_path_buf(),
            source,
        })?;
    Ok((value, version))
}

/// Copies the original `guide.json` bytes (at schema `version`) to
/// `.backup/guide.v{version}.json`, or `guide.v{version}.{n}.json` for the
/// first free `n >= 2`, and returns its path. An existing backup of that
/// version with identical bytes is reused instead (e.g. a load that keeps
/// failing after the backup). Never overwrites a backup: the copy is
/// written to a temp file, synced, and moved into place with a no-clobber
/// rename.
fn write_backup(handle: &GuideHandle, version: u32, bytes: &[u8]) -> Result<PathBuf> {
    // `resolve` refuses a symlinked `.backup`.
    let dir = handle.resolve(BACKUP_DIR)?;
    match fs::create_dir(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(io_err(&dir)(e)),
    }
    if let Some(existing) = find_backup(handle, &dir, version, bytes) {
        return Ok(existing);
    }
    let first = format!("guide.v{version}.json");
    let mut tmp = temp_file_in(&dir, &first)?;
    tmp.write_all(bytes).map_err(io_err(tmp.path()))?;
    tmp.as_file().sync_all().map_err(io_err(tmp.path()))?;
    for n in 1u32.. {
        let name = if n == 1 {
            first.clone()
        } else {
            format!("guide.v{version}.{n}.json")
        };
        let target = handle.resolve(&format!("{BACKUP_DIR}/{name}"))?;
        match tmp.persist_noclobber(&target) {
            Ok(_) => {
                sync_dir(&dir)?;
                return Ok(target);
            }
            Err(e) if e.error.kind() == io::ErrorKind::AlreadyExists => tmp = e.file,
            Err(e) => return Err(io_err(&target)(e.error)),
        }
    }
    unreachable!("u32 range exhausted")
}

/// A backup of `version` in `dir` holding exactly `bytes`, if any.
fn find_backup(handle: &GuideHandle, dir: &Path, version: u32, bytes: &[u8]) -> Option<PathBuf> {
    let prefix = format!("guide.v{version}.");
    fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| {
            name.strip_prefix(&prefix)
                .and_then(|rest| rest.strip_suffix("json"))
                .is_some_and(|n| {
                    n.is_empty()
                        || n.strip_suffix('.')
                            .is_some_and(|n| n.parse::<u32>().is_ok_and(|n| n >= 2))
                })
        })
        // `resolve` refuses symlinks; only real files count as backups.
        .filter_map(|name| handle.resolve(&format!("{BACKUP_DIR}/{name}")).ok())
        .find(|path| {
            fs::symlink_metadata(path).is_ok_and(|m| m.is_file() && m.len() == bytes.len() as u64)
                && fs::read(path).is_ok_and(|b| b == bytes)
        })
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

/// Backs up the `original` bytes (at `version`) and rewrites `guide.json`
/// with the migrated `loaded.guide`, unless the file changed on disk since
/// it was read. Sets `loaded.backup` once the backup exists.
fn write_back(loaded: &mut LoadedGuide, version: u32, original: &[u8]) -> Result<()> {
    let handle = loaded.handle.clone();
    loaded.backup = Some(write_backup(&handle, version, original)?);
    let json = handle.guide_json();
    #[cfg(test)]
    tests::before_rewrite(&json);
    let current = fs::read(&json).map_err(io_err(&json))?;
    if current != original {
        return Err(LibraryError::ChangedOnDisk(json));
    }
    write_guide(&handle.path, &loaded.guide)
}

/// A guide's listing entry. A newer-schema guide whose fields this app
/// cannot decode still gets an entry if its `id` is readable: titled after
/// the folder, with no summary, tags or steps, so it does not vanish from
/// `list` (opening it fails with [`LibraryError::NewerSchema`]).
fn read_summary(bundle: &Path, schema: &Schema) -> Result<GuideSummary> {
    let (path, value, schema_version) = read_value(bundle, schema)?;
    let raw: RawSummary = match RawSummary::deserialize(&value) {
        Ok(raw) => raw,
        Err(_) if schema_version > schema.version => {
            return newer_placeholder(bundle, &path, &value, schema_version, schema)
        }
        Err(source) => return Err(LibraryError::Json { path, source }),
    };
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

fn newer_placeholder(
    bundle: &Path,
    path: &Path,
    value: &Value,
    schema_version: u32,
    schema: &Schema,
) -> Result<GuideSummary> {
    let newer = || LibraryError::NewerSchema {
        path: path.to_path_buf(),
        found: schema_version,
        supported: schema.version,
    };
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .and_then(|id| id.parse::<Ulid>().ok())
        .ok_or_else(newer)?;
    let updated_at = value
        .get("updatedAt")
        .and_then(|v| DateTime::<Utc>::deserialize(v).ok())
        .or_else(|| {
            fs::metadata(path)
                .and_then(|m| m.modified())
                .ok()
                .map(DateTime::<Utc>::from)
        })
        .unwrap_or(DateTime::<Utc>::UNIX_EPOCH);
    Ok(GuideSummary {
        id,
        path: bundle.to_path_buf(),
        schema_version,
        title: bundle_stem(bundle).unwrap_or(UNTITLED).to_owned(),
        summary: String::new(),
        tags: Vec::new(),
        step_count: 0,
        updated_at,
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
    let mut tmp = temp_file_in(dir, &name)?;
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

/// A `.{name}.<random>.tmp` file in `dir` with normal document permissions.
fn temp_file_in(dir: &Path, name: &str) -> Result<tempfile::NamedTempFile> {
    let prefix = format!(".{name}.");
    let mut builder = tempfile::Builder::new();
    builder.prefix(&prefix).suffix(".tmp");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Umask applies, not tempfile's 0600.
        builder.permissions(fs::Permissions::from_mode(0o666));
    }
    builder.tempfile_in(dir).map_err(io_err(dir))
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
    use crate::migrate::test_schema;
    use crate::model::{Item, Section};
    use crate::SCHEMA_VERSION;
    use std::cell::RefCell;

    type Hook = Box<dyn Fn(&Path)>;

    thread_local! {
        /// Runs in `write_back` between the backup and the re-read, to
        /// simulate another writer. Per test thread.
        static BEFORE_REWRITE: RefCell<Option<Hook>> = const { RefCell::new(None) };
    }

    pub(super) fn before_rewrite(path: &Path) {
        BEFORE_REWRITE.with(|hook| {
            if let Some(hook) = hook.borrow().as_ref() {
                hook(path);
            }
        });
    }

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
        assert_eq!(handle.path, lib.root().join("Export a PDF.guiderecorder"));
        assert_eq!(files_in(&handle.path), ["assets", "guide.json", "steps"]);
        // No staging folder left behind.
        assert_eq!(files_in(lib.root()), ["Export a PDF.guiderecorder"]);
    }

    #[test]
    fn legacy_bundles_are_listed_and_opened_but_new_ones_use_the_new_extension() {
        let (_tmp, lib) = library();
        let guide = Guide::new("Old", "en");
        let made = lib.create(&guide).unwrap();
        let legacy = lib.root().join(format!("Old.{LEGACY_BUNDLE_EXTENSION}"));
        fs::rename(&made.path, &legacy).unwrap();

        let fresh = Library::open(lib.root()).unwrap();
        assert_eq!(fresh.list().unwrap().len(), 1);
        let found = fresh.find(guide.id).unwrap().unwrap();
        assert_eq!(found.path, legacy.canonicalize().unwrap());
        assert_eq!(fresh.load(&legacy).unwrap().guide.id, guide.id);

        let new = fresh.create(&Guide::new("New", "en")).unwrap();
        assert_eq!(new.path, lib.root().join("New.guiderecorder"));
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
        assert_eq!(files_in(lib.root()), ["Once.guiderecorder"]);
    }

    #[test]
    fn save_load_round_trip() {
        let (_tmp, lib) = library();
        let mut guide = sample_guide();
        let handle = lib.create(&guide).unwrap();
        let loaded = lib.load(&handle.path).unwrap();
        assert_eq!(loaded.handle, handle);
        assert_eq!(loaded.guide, guide);
        assert!(!loaded.read_only);
        assert_eq!(loaded.migrated_from, None);
        assert_eq!(loaded.backup, None);

        guide.title = "Renamed".into();
        guide.items.push(Item::Section(Section {
            id: Ulid::new(),
            title: "More".into(),
        }));
        assert_eq!(lib.save(&guide).unwrap(), handle);
        assert_eq!(lib.load(&handle.path).unwrap().guide, guide);
        // The folder name is kept when the title changes.
        assert!(handle.guide_json().is_file());

        // A fresh Library (empty cache) still finds the guide by scanning.
        let fresh = Library::open(lib.root()).unwrap();
        guide.summary = "changed".into();
        assert_eq!(fresh.save(&guide).unwrap(), handle);
        assert_eq!(fresh.load(&handle.path).unwrap().guide.summary, "changed");
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
        assert_eq!(
            files_in(lib.root()),
            ["Moved.guiderecorder", "Name.guiderecorder"]
        );
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
        assert_eq!(lib.load(&handle.path).unwrap().guide, guide);

        fs::remove_file(handle.guide_json()).unwrap();
        assert!(matches!(
            lib.save_to(&handle, &guide),
            Err(LibraryError::Unverifiable { .. })
        ));
        lib.save_to_force(&handle, &guide).unwrap();
        assert_eq!(lib.load(&handle.path).unwrap().guide, guide);

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
        assert_eq!(files_in(lib.root()), ["New.guiderecorder"]);
        assert!(files_in(&new.steps_dir()).is_empty());
    }

    fn temp_dir_name(purpose: &str, minted: SystemTime) -> String {
        format!(".{purpose}-{}.tmp", Ulid::from_datetime(minted))
    }

    fn set_mtime(path: &Path, time: SystemTime) {
        File::open(path).unwrap().set_modified(time).unwrap();
    }

    #[test]
    fn recover_sweeps_stale_temps() {
        let (_tmp, lib) = library();
        let guide = Guide::new("Stranded", "en");
        let handle = lib.create(&guide).unwrap();
        let root = lib.root().to_path_buf();
        let two_hours_ago = SystemTime::now() - Duration::from_secs(2 * 60 * 60);

        // Old leftovers: folders are aged by the ULID in the name, files by
        // mtime.
        let create = root.join(temp_dir_name("create", two_hours_ago));
        fs::create_dir_all(create.join(STEPS_DIR)).unwrap();
        let temp_json = handle.path.join(".guide.json.abc123.tmp");
        fs::write(&temp_json, "partial").unwrap();
        set_mtime(&temp_json, two_hours_ago);
        let temp_webp = handle
            .steps_dir()
            .join(".01J9ZQ3K8V4N6T2W5X7Y8Z9A0D.crop.webp.x1.tmp");
        fs::write(&temp_webp, "partial").unwrap();
        set_mtime(&temp_webp, two_hours_ago);
        let empty_rename = root.join(temp_dir_name("rename", two_hours_ago));
        fs::create_dir(&empty_rename).unwrap();
        // A bundle stranded mid-rename is recovered, not deleted.
        let stranded = root.join(temp_dir_name("rename", two_hours_ago));
        fs::rename(&handle.path, &stranded).unwrap();

        // Kept: deletes, anything recent, and folders that are not ours.
        let doomed = root.join(temp_dir_name("delete", two_hours_ago));
        fs::create_dir(&doomed).unwrap();
        let recent = root.join(temp_dir_name("create", SystemTime::now()));
        fs::create_dir(&recent).unwrap();
        let foreign = root.join(".create-notaulid.tmp");
        fs::create_dir(&foreign).unwrap();
        set_mtime(&foreign, two_hours_ago);

        // Opening never sweeps.
        let snapshot = tree(&root);
        let reopened = Library::open(&root).unwrap();
        assert_eq!(tree(&root), snapshot);

        let report = reopened.recover();
        let bundle = root.join("Stranded.guiderecorder");
        assert_eq!(report.restored, std::slice::from_ref(&bundle));
        let mut removed = report.removed.clone();
        removed.sort();
        let mut expected = vec![
            create,
            empty_rename,
            bundle.join(".guide.json.abc123.tmp"),
            bundle.join(STEPS_DIR).join(temp_webp.file_name().unwrap()),
        ];
        expected.sort();
        assert_eq!(removed, expected);

        let mut names = vec![
            doomed.file_name().unwrap().to_string_lossy().into_owned(),
            recent.file_name().unwrap().to_string_lossy().into_owned(),
            foreign.file_name().unwrap().to_string_lossy().into_owned(),
            "Stranded.guiderecorder".to_owned(),
        ];
        names.sort();
        assert_eq!(files_in(&root), names);
        let recovered = reopened.load(&bundle).unwrap();
        assert_eq!(recovered.guide, guide);
        assert_eq!(files_in(&bundle), ["assets", "guide.json", "steps"]);
        assert!(files_in(&bundle.join(STEPS_DIR)).is_empty());

        // Nothing left to do.
        assert_eq!(reopened.recover(), Recovery::default());
    }

    #[test]
    fn recover_ages_folders_by_name_not_mtime() {
        let (_tmp, lib) = library();
        let handle = lib.create(&Guide::new("Moving", "en")).unwrap();
        let long_ago = SystemTime::now() - Duration::from_secs(24 * 60 * 60);
        set_mtime(&handle.path, long_ago);
        // Mid-rename right now: the folder keeps its old mtime, but the name
        // was just minted, so it must be left alone.
        let in_flight = lib.root().join(temp_dir_name("rename", SystemTime::now()));
        fs::rename(&handle.path, &in_flight).unwrap();
        let staging = lib.root().join(temp_dir_name("create", SystemTime::now()));
        fs::create_dir(&staging).unwrap();
        set_mtime(&staging, long_ago);

        assert_eq!(lib.recover(), Recovery::default());
        assert!(in_flight.is_dir() && staging.is_dir());
    }

    #[test]
    fn temp_names_age_by_ulid() {
        let hour = Duration::from_secs(60 * 60);
        let old = temp_dir_name("rename", SystemTime::now() - 2 * hour);
        assert!(temp_name_is_older_than(&old, "rename", hour));
        assert!(!temp_name_is_older_than(&old, "create", hour));
        let new = temp_dir_name("rename", SystemTime::now());
        assert!(!temp_name_is_older_than(&new, "rename", hour));
        let future = temp_dir_name("create", SystemTime::now() + 2 * hour);
        assert!(!temp_name_is_older_than(&future, "create", hour));
        for bad in [
            ".create-x.tmp",
            "create-01J9ZQ3K8V4N6T2W5X7Y8Z9A0D.tmp",
            ".create-.tmp",
        ] {
            assert!(
                !temp_name_is_older_than(bad, "create", Duration::ZERO),
                "{bad}"
            );
        }
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
        assert_eq!(lib.load(&handle.path).unwrap().guide.summary, "rev 4");

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
        fs::create_dir(lib.root().join("Broken.guiderecorder")).unwrap();

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
            .any(|(p, r)| p.ends_with("Broken.guiderecorder") && r.is_err()));

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
        // " copy" sorts before ".guiderecorder", so path order alone would pick it.
        let copy = duplicate(&handle, lib.root(), "Original copy.guiderecorder");

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
        assert_eq!(fresh.load(&handle.path).unwrap().guide.summary, "edited");
        assert_eq!(read_summary(&copy, &migrate::CURRENT).unwrap().summary, "");
    }

    #[test]
    fn opened_duplicate_becomes_canonical() {
        let (_tmp, lib) = library();
        let guide = Guide::new("Original", "en");
        let handle = lib.create(&guide).unwrap();
        let copy = duplicate(&handle, lib.root(), "Original copy.guiderecorder");

        let fresh = Library::open(lib.root()).unwrap();
        let opened = fresh.load(&copy).unwrap().handle;
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
    fn reading_a_duplicate_keeps_the_canonical_bundle() {
        let (_tmp, lib) = library();
        let guide = Guide::new("Original", "en");
        let handle = lib.create(&guide).unwrap();
        let copy = duplicate(&handle, lib.root(), "Original copy.guiderecorder");

        let fresh = Library::open(lib.root()).unwrap();
        assert_eq!(fresh.find(guide.id).unwrap(), Some(handle.clone()));
        assert_eq!(fresh.read(&copy).unwrap().handle.path, copy);
        assert_eq!(fresh.find(guide.id).unwrap(), Some(handle.clone()));
        let scanned = fresh.scan().unwrap();
        let entry = scanned.iter().find(|(p, _)| *p == copy).unwrap();
        assert_eq!(
            entry.1.as_ref().unwrap().duplicate_of,
            Some(handle.path.clone())
        );
    }

    #[test]
    fn oldest_duplicate_wins_without_name_match() {
        let (_tmp, lib) = library();
        let guide = Guide::new("Title", "en");
        let handle = lib.create(&guide).unwrap();
        let older = duplicate(&handle, lib.root(), "B elsewhere.guiderecorder");
        let newer = duplicate(&handle, lib.root(), "A elsewhere.guiderecorder");
        let delete = Library::open(lib.root()).unwrap();
        delete.delete(&handle).unwrap();

        let fresh = Library::open(lib.root()).unwrap();
        assert_eq!(fresh.find(guide.id).unwrap().unwrap().path, older);
        let scanned = fresh.scan().unwrap();
        let entry = scanned.iter().find(|(p, _)| *p == newer).unwrap();
        assert_eq!(entry.1.as_ref().unwrap().duplicate_of, Some(older));
    }

    #[test]
    fn newer_schema_loads_read_only() {
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

        let loaded = lib.load(&handle.path).unwrap();
        assert!(loaded.read_only);
        assert_eq!(loaded.migrated_from, None);
        assert_eq!(loaded.backup, None);
        assert_eq!(loaded.handle, handle);
        assert_eq!(loaded.guide.title, "Future");
        assert_eq!(loaded.guide.schema_version, SCHEMA_VERSION + 1);

        // Saves refuse, whether by the guide in memory or by what is on disk.
        let mut edited = loaded.guide.clone();
        edited.summary = "edited".into();
        for result in [
            lib.save_to(&handle, &edited),
            lib.save(&edited).map(drop),
            lib.save_to_force(&handle, &edited),
            lib.save_to(&handle, &guide),
            lib.save(&guide).map(drop),
        ] {
            assert!(matches!(
                result,
                Err(LibraryError::NewerSchema { found, .. }) if found == SCHEMA_VERSION + 1
            ));
        }
        // Listing, loading and refused saves never rewrite the file.
        assert_eq!(fs::read_to_string(handle.guide_json()).unwrap(), text);
        assert_eq!(files_in(&handle.path), ["assets", "guide.json", "steps"]);

        assert!(matches!(
            lib.load(lib.root()),
            Err(LibraryError::NotABundle(_))
        ));
    }

    #[test]
    fn create_refuses_newer_schema_guide() {
        let (_tmp, lib) = library();
        let mut guide = Guide::new("Future", "en");
        guide.schema_version = SCHEMA_VERSION + 1;
        let expected = lib.root().join("Future.guiderecorder").join(GUIDE_FILE);
        assert!(matches!(
            lib.create(&guide),
            Err(LibraryError::NewerSchema { path, .. }) if path == expected
        ));
        assert!(files_in(lib.root()).is_empty());
    }

    /// A library on [`test_schema::SCHEMA`] (version 3).
    fn v3_library() -> (tempfile::TempDir, Library) {
        let dir = tempfile::tempdir().unwrap();
        let lib =
            Library::open_with_schema(dir.path().join("Library"), test_schema::SCHEMA).unwrap();
        (dir, lib)
    }

    /// `guide` in the version 1 format of [`test_schema`].
    fn v1_json(guide: &Guide, subtitle: &str) -> String {
        let mut value = serde_json::to_value(guide).unwrap();
        let object = value.as_object_mut().unwrap();
        object.remove("summary");
        object.remove("tags");
        object.insert("subtitle".into(), subtitle.into());
        object.insert("schemaVersion".into(), 1.into());
        serde_json::to_string_pretty(&value).unwrap()
    }

    /// A bundle holding `text` as its guide.json.
    fn old_bundle(lib: &Library, guide: &Guide, text: &str) -> GuideHandle {
        let handle = lib.create(guide).unwrap();
        fs::write(handle.guide_json(), text).unwrap();
        handle
    }

    /// Every file under `dir` with its bytes, by relative path.
    fn tree(dir: &Path) -> Vec<(PathBuf, Option<Vec<u8>>)> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(current) = stack.pop() {
            for entry in fs::read_dir(&current).unwrap() {
                let path = entry.unwrap().path();
                let relative = path.strip_prefix(dir).unwrap().to_path_buf();
                if path.is_dir() {
                    out.push((relative, None));
                    stack.push(path);
                } else {
                    out.push((relative, Some(fs::read(&path).unwrap())));
                }
            }
        }
        out.sort();
        out
    }

    #[test]
    fn load_migrates_older_schema_with_backup() {
        let (_tmp, lib) = v3_library();
        let guide = Guide::new("Legacy", "en");
        let original = v1_json(&guide, "From v1");
        let handle = old_bundle(&lib, &guide, &original);

        let loaded = lib.load(&handle.path).unwrap();
        assert!(!loaded.read_only);
        assert_eq!(loaded.migrated_from, Some(1));
        let backup = handle.backup_dir().join("guide.v1.json");
        assert_eq!(loaded.backup.as_deref(), Some(backup.as_path()));
        assert_eq!(fs::read_to_string(&backup).unwrap(), original);
        assert_eq!(files_in(&handle.backup_dir()), ["guide.v1.json"]);
        assert_eq!(loaded.guide.schema_version, 3);
        assert_eq!(loaded.guide.summary, "From v1");
        assert!(loaded.guide.tags.is_empty());
        assert_eq!(loaded.guide.title, "Legacy");

        // guide.json now holds the migrated guide at version 3.
        let rewritten: Value =
            serde_json::from_slice(&fs::read(handle.guide_json()).unwrap()).unwrap();
        assert_eq!(rewritten["schemaVersion"], 3);
        assert!(rewritten.get("subtitle").is_none());
        assert_eq!(
            serde_json::from_value::<Guide>(rewritten).unwrap(),
            loaded.guide
        );
        assert_eq!(
            files_in(&handle.path),
            [".backup", "assets", "guide.json", "steps"]
        );

        // A second load has nothing to do.
        let again = lib.load(&handle.path).unwrap();
        assert_eq!(again.migrated_from, None);
        assert_eq!(again.backup, None);
        assert_eq!(again.guide, loaded.guide);
        assert_eq!(files_in(&handle.backup_dir()), ["guide.v1.json"]);

        // The migrated guide saves normally.
        let mut edited = again.guide;
        edited.summary = "edited".into();
        lib.save_to(&handle, &edited).unwrap();
    }

    #[test]
    fn repeated_backups_get_unique_names() {
        let (_tmp, lib) = v3_library();
        let guide = Guide::new("Again", "en");
        let handle = old_bundle(&lib, &guide, "{}");
        let mut originals = Vec::new();
        for i in 0..3 {
            // e.g. a sync service bringing back an old copy each time.
            let text = v1_json(&guide, &format!("rev {i}"));
            fs::write(handle.guide_json(), &text).unwrap();
            let loaded = lib.load(&handle.path).unwrap();
            assert_eq!(loaded.guide.summary, format!("rev {i}"));
            originals.push((loaded.backup.unwrap(), text));
        }
        let mut v2 = serde_json::to_value(&guide).unwrap();
        v2["schemaVersion"] = 2.into();
        v2["keywords"] = "a, b".into();
        let v2 = v2.to_string();
        fs::write(handle.guide_json(), &v2).unwrap();
        let loaded = lib.load(&handle.path).unwrap();
        assert_eq!(loaded.migrated_from, Some(2));
        assert_eq!(loaded.guide.tags, ["a", "b"]);
        originals.push((loaded.backup.unwrap(), v2));

        assert_eq!(
            files_in(&handle.backup_dir()),
            [
                "guide.v1.2.json",
                "guide.v1.3.json",
                "guide.v1.json",
                "guide.v2.json"
            ]
        );
        // Every original is kept byte for byte.
        for (path, text) in originals {
            assert_eq!(fs::read_to_string(path).unwrap(), text);
        }
    }

    #[test]
    fn failed_migration_or_backup_leaves_file_untouched() {
        let (_tmp, lib) = v3_library();
        let guide = Guide::new("Broken", "en");
        let mut bad = serde_json::to_value(&guide).unwrap();
        bad["schemaVersion"] = 2.into();
        bad["keywords"] = 5.into();
        let bad = bad.to_string();
        let handle = old_bundle(&lib, &guide, &bad);
        assert!(matches!(
            lib.load(&handle.path),
            Err(LibraryError::Migration {
                source: MigrationError::Step { from: 2, .. },
                ..
            })
        ));
        assert_eq!(fs::read_to_string(handle.guide_json()).unwrap(), bad);
        assert!(!handle.backup_dir().exists());

        // A symlinked .backup is refused before anything is written; the
        // migrated guide still opens, read-only.
        #[cfg(unix)]
        {
            let elsewhere = handle
                .path
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("elsewhere");
            fs::create_dir(&elsewhere).unwrap();
            std::os::unix::fs::symlink(&elsewhere, handle.backup_dir()).unwrap();
            let original = v1_json(&guide, "S");
            fs::write(handle.guide_json(), &original).unwrap();
            let loaded = lib.load(&handle.path).unwrap();
            assert!(loaded.read_only);
            assert_eq!(loaded.guide.summary, "S");
            assert!(matches!(
                loaded.migration_error.as_deref(),
                Some(LibraryError::SymlinkInBundle(_))
            ));
            assert_eq!(fs::read_to_string(handle.guide_json()).unwrap(), original);
            assert!(files_in(&elsewhere).is_empty());
        }
    }

    /// Makes `dir` read-only; false if writes still succeed (e.g. as root),
    /// in which case the caller skips the test.
    #[cfg(unix)]
    fn make_read_only(dir: &Path) -> bool {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o555)).unwrap();
        let probe = dir.join("probe");
        if fs::write(&probe, "x").is_ok() {
            let _ = fs::remove_file(probe);
            return false;
        }
        true
    }

    #[cfg(unix)]
    fn make_writable(dir: &Path) {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn unwritable_migration_loads_read_only() {
        let (_tmp, lib) = v3_library();
        let guide = Guide::new("Locked", "en");
        let original = v1_json(&guide, "From v1");
        let handle = old_bundle(&lib, &guide, &original);

        // 1. The backup cannot be written: nothing is touched.
        fs::create_dir(handle.backup_dir()).unwrap();
        if !make_read_only(&handle.backup_dir()) {
            make_writable(&handle.backup_dir());
            eprintln!("skipped: permissions are not enforced for this user");
            return;
        }
        let loaded = lib.load(&handle.path).unwrap();
        make_writable(&handle.backup_dir());
        assert!(loaded.read_only);
        assert_eq!(loaded.migrated_from, Some(1));
        assert_eq!(loaded.backup, None);
        assert!(matches!(
            loaded.migration_error.as_deref(),
            Some(LibraryError::Io { .. })
        ));
        assert_eq!(loaded.guide.summary, "From v1");
        assert_eq!(loaded.guide.schema_version, 3);
        assert_eq!(fs::read_to_string(handle.guide_json()).unwrap(), original);
        assert!(files_in(&handle.backup_dir()).is_empty());

        // Saving the read-only copy would lose the un-backed-up original.
        for result in [
            lib.save_to(&handle, &loaded.guide),
            lib.save(&loaded.guide).map(drop),
        ] {
            assert!(matches!(
                result,
                Err(LibraryError::NotMigrated {
                    found: 1,
                    current: 3,
                    ..
                })
            ));
        }
        assert_eq!(fs::read_to_string(handle.guide_json()).unwrap(), original);

        // 2. The backup is written but the rewrite fails (read-only bundle
        // folder): guide.json is untouched, the backup is kept, and later
        // attempts reuse it instead of piling up copies.
        assert!(make_read_only(&handle.path));
        let backup = handle.backup_dir().join("guide.v1.json");
        for _ in 0..3 {
            let loaded = lib.load(&handle.path).unwrap();
            assert!(loaded.read_only);
            assert!(loaded.migration_error.is_some());
            assert_eq!(loaded.backup.as_deref(), Some(backup.as_path()));
        }
        make_writable(&handle.path);
        assert_eq!(files_in(&handle.backup_dir()), ["guide.v1.json"]);
        assert_eq!(fs::read_to_string(&backup).unwrap(), original);
        assert_eq!(fs::read_to_string(handle.guide_json()).unwrap(), original);
        assert_eq!(
            files_in(&handle.path),
            [".backup", "assets", "guide.json", "steps"]
        );

        // 3. Once writable, a normal load migrates, reusing the backup.
        let loaded = lib.load(&handle.path).unwrap();
        assert!(!loaded.read_only);
        assert!(loaded.migration_error.is_none());
        assert_eq!(loaded.migrated_from, Some(1));
        assert_eq!(loaded.backup.as_deref(), Some(backup.as_path()));
        assert_eq!(files_in(&handle.backup_dir()), ["guide.v1.json"]);
        let rewritten: Value =
            serde_json::from_slice(&fs::read(handle.guide_json()).unwrap()).unwrap();
        assert_eq!(rewritten["schemaVersion"], 3);
        lib.save_to(&handle, &loaded.guide).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn save_to_force_backs_up_older_original() {
        let (_tmp, lib) = v3_library();
        let guide = Guide::new("Forced", "en");
        let original = v1_json(&guide, "From v1");
        let handle = old_bundle(&lib, &guide, &original);
        fs::create_dir(handle.backup_dir()).unwrap();
        if !make_read_only(&handle.backup_dir()) {
            make_writable(&handle.backup_dir());
            eprintln!("skipped: permissions are not enforced for this user");
            return;
        }
        let loaded = lib.load(&handle.path).unwrap();
        assert!(loaded.read_only && loaded.backup.is_none());

        // No backup possible: "Save anyway" fails and keeps the original.
        assert!(lib.save_to_force(&handle, &loaded.guide).is_err());
        make_writable(&handle.backup_dir());
        assert_eq!(fs::read_to_string(handle.guide_json()).unwrap(), original);

        // With a backup, it goes through and the original is kept.
        lib.save_to_force(&handle, &loaded.guide).unwrap();
        assert_eq!(
            fs::read_to_string(handle.backup_dir().join("guide.v1.json")).unwrap(),
            original
        );
        assert_eq!(lib.load(&handle.path).unwrap().guide, loaded.guide);
        // A current-schema file needs no backup.
        lib.save_to_force(&handle, &loaded.guide).unwrap();
        assert_eq!(files_in(&handle.backup_dir()), ["guide.v1.json"]);
    }

    #[test]
    fn save_to_force_backs_up_newer_original() {
        let (_tmp, lib) = library();
        let guide = Guide::new("Overwritten", "en");
        let handle = lib.create(&guide).unwrap();
        let mut value = serde_json::to_value(&guide).unwrap();
        value["schemaVersion"] = (SCHEMA_VERSION + 1).into();
        value["futureData"] = "keep me".into();
        let newer = value.to_string();
        fs::write(handle.guide_json(), &newer).unwrap();

        lib.save_to_force(&handle, &guide).unwrap();
        let backup = handle
            .backup_dir()
            .join(format!("guide.v{}.json", SCHEMA_VERSION + 1));
        assert_eq!(fs::read_to_string(backup).unwrap(), newer);
        assert_eq!(lib.load(&handle.path).unwrap().guide, guide);
    }

    #[test]
    fn save_to_force_backs_up_despite_invalid_id() {
        let (_tmp, lib) = v3_library();
        let guide = Guide::new("Bad id", "en");
        let mut value: Value = serde_json::from_str(&v1_json(&guide, "S")).unwrap();
        value["id"] = "not-a-ulid".into();
        let original = value.to_string();
        let handle = old_bundle(&lib, &guide, &original);
        assert!(matches!(
            lib.save_to(&handle, &guide),
            Err(LibraryError::Unverifiable { .. })
        ));

        let mut current = guide.clone();
        current.schema_version = 3;
        lib.save_to_force(&handle, &current).unwrap();
        assert_eq!(
            fs::read_to_string(handle.backup_dir().join("guide.v1.json")).unwrap(),
            original
        );
        assert_eq!(lib.load(&handle.path).unwrap().guide, current);
    }

    #[test]
    fn recovered_undecodable_guide_is_named_recovered() {
        let (_tmp, lib) = library();
        let guide = Guide::new("Reshaped", "en");
        let handle = lib.create(&guide).unwrap();
        let mut value = serde_json::to_value(&guide).unwrap();
        value["schemaVersion"] = (SCHEMA_VERSION + 1).into();
        value["title"] = serde_json::json!({ "en": "Reshaped" });
        fs::write(handle.guide_json(), value.to_string()).unwrap();
        let two_hours_ago = SystemTime::now() - Duration::from_secs(2 * 60 * 60);
        let stranded = lib.root().join(temp_dir_name("rename", two_hours_ago));
        fs::rename(&handle.path, &stranded).unwrap();

        let report = lib.recover();
        let restored = lib.root().join("Recovered.guiderecorder");
        assert_eq!(report.restored, std::slice::from_ref(&restored));
        assert_eq!(lib.list().unwrap()[0].title, "Recovered");
    }

    #[test]
    fn identical_original_reuses_backup() {
        let (_tmp, lib) = v3_library();
        let guide = Guide::new("Same", "en");
        let original = v1_json(&guide, "S");
        let handle = old_bundle(&lib, &guide, &original);
        let first = lib.load(&handle.path).unwrap().backup.unwrap();
        // A sync service brings back the same old file.
        fs::write(handle.guide_json(), &original).unwrap();
        let second = lib.load(&handle.path).unwrap();
        assert_eq!(second.migrated_from, Some(1));
        assert_eq!(second.backup, Some(first));
        assert_eq!(files_in(&handle.backup_dir()), ["guide.v1.json"]);
    }

    #[test]
    fn read_migrates_in_memory_only() {
        let (_tmp, lib) = v3_library();
        let old = Guide::new("Old", "en");
        let old_handle = old_bundle(&lib, &old, &v1_json(&old, "Sub"));
        let mut current = Guide::new("Current", "en");
        current.schema_version = 3;
        let current_handle = lib.create(&current).unwrap();
        let future = Guide::new("Future", "en");
        let mut value = serde_json::to_value(&future).unwrap();
        value["schemaVersion"] = 9.into();
        let future_handle = old_bundle(&lib, &future, &value.to_string());
        let before = tree(lib.root());

        let reader = Library::open_with_schema(lib.root(), test_schema::SCHEMA).unwrap();
        let read = reader.read(&old_handle.path).unwrap();
        assert!(read.read_only);
        assert_eq!(read.migrated_from, Some(1));
        assert_eq!(read.backup, None);
        assert!(read.migration_error.is_none());
        assert_eq!(read.guide.summary, "Sub");
        assert_eq!(read.guide.schema_version, 3);

        let read = reader.read(&current_handle.path).unwrap();
        assert!(!read.read_only);
        assert_eq!(read.migrated_from, None);
        assert_eq!(read.guide, current);

        assert!(reader.read(&future_handle.path).unwrap().read_only);
        assert_eq!(tree(lib.root()), before);
    }

    #[test]
    fn undecodable_newer_schema_is_listed_as_placeholder() {
        let (_tmp, lib) = library();
        let guide = Guide::new("Reshaped", "en");
        let handle = lib.create(&guide).unwrap();
        let mut value = serde_json::to_value(&guide).unwrap();
        value["schemaVersion"] = (SCHEMA_VERSION + 1).into();
        value["title"] = serde_json::json!({ "en": "Reshaped" });
        fs::write(handle.guide_json(), value.to_string()).unwrap();

        for result in [lib.load(&handle.path), lib.read(&handle.path)] {
            assert!(matches!(
                result,
                Err(LibraryError::NewerSchema { found, .. }) if found == SCHEMA_VERSION + 1
            ));
        }
        let listed = lib.list().unwrap();
        assert_eq!(listed.len(), 1);
        let entry = &listed[0];
        assert_eq!(entry.id, guide.id);
        assert_eq!(entry.title, "Reshaped");
        assert_eq!(entry.schema_version, SCHEMA_VERSION + 1);
        assert_eq!(entry.step_count, 0);
        assert_eq!(entry.updated_at, guide.updated_at);
        assert_eq!(lib.find(guide.id).unwrap(), Some(handle.clone()));

        // Without a readable id there is nothing to list; scan reports it.
        value.as_object_mut().unwrap().remove("id");
        fs::write(handle.guide_json(), value.to_string()).unwrap();
        assert!(lib.list().unwrap().is_empty());
        assert!(matches!(
            &lib.scan().unwrap()[0].1,
            Err(LibraryError::NewerSchema { .. })
        ));

        // An older or current guide that fails to decode is still corrupt.
        fs::write(
            handle.guide_json(),
            format!(r#"{{"schemaVersion": {SCHEMA_VERSION}, "id": 5}}"#),
        )
        .unwrap();
        assert!(matches!(
            lib.load(&handle.path),
            Err(LibraryError::Json { .. })
        ));
    }

    #[test]
    fn change_during_migration_is_not_overwritten() {
        let (_tmp, lib) = v3_library();
        let guide = Guide::new("Synced", "en");
        let original = v1_json(&guide, "Old");
        let handle = old_bundle(&lib, &guide, &original);
        let synced = v1_json(&guide, "Arrived during load");
        let incoming = synced.clone();
        BEFORE_REWRITE.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |path: &Path| {
                fs::write(path, &incoming).unwrap();
            }));
        });
        let loaded = lib.load(&handle.path).unwrap();
        BEFORE_REWRITE.with(|hook| hook.borrow_mut().take());

        assert!(loaded.read_only);
        assert!(matches!(
            loaded.migration_error.as_deref(),
            Some(LibraryError::ChangedOnDisk(_))
        ));
        assert_eq!(fs::read_to_string(handle.guide_json()).unwrap(), synced);
        assert_eq!(
            fs::read_to_string(loaded.backup.unwrap()).unwrap(),
            original
        );
        // The next load migrates the new content.
        let again = lib.load(&handle.path).unwrap();
        assert!(!again.read_only);
        assert_eq!(again.guide.summary, "Arrived during load");
    }

    #[cfg(unix)]
    #[test]
    fn recover_never_follows_symlinks() {
        use std::os::unix::fs::symlink;
        let (tmp, lib) = library();
        let handle = lib.create(&Guide::new("Linked", "en")).unwrap();
        let two_hours_ago = SystemTime::now() - Duration::from_secs(2 * 60 * 60);
        let outside = |name: &str, file: &str| {
            let dir = tmp.path().join(name);
            fs::create_dir(&dir).unwrap();
            let path = dir.join(file);
            fs::write(&path, "user data").unwrap();
            set_mtime(&path, two_hours_ago);
            (dir, path)
        };
        let (steps_target, photo) = outside("out-steps", ".photo.webp.abc.tmp");
        let (backup_target, notes) = outside("out-backup", ".notes.tmp");
        let (bundle_target, json_temp) = outside("out-bundle", ".guide.json.xyz.tmp");

        fs::remove_dir(handle.steps_dir()).unwrap();
        symlink(&steps_target, handle.steps_dir()).unwrap();
        symlink(&backup_target, handle.backup_dir()).unwrap();
        symlink(&bundle_target, lib.root().join("Elsewhere.guiderecorder")).unwrap();

        let report = lib.recover();
        assert_eq!(report, Recovery::default());
        for path in [photo, notes, json_temp] {
            assert!(path.is_file(), "{path:?} was deleted");
        }
    }

    #[test]
    fn list_scan_find_never_write() {
        let (_tmp, lib) = v3_library();
        let old = Guide::new("Old", "en");
        let old_handle = old_bundle(&lib, &old, &v1_json(&old, "Sub"));
        let future = Guide::new("Future", "en");
        let mut value = serde_json::to_value(&future).unwrap();
        value["schemaVersion"] = 9.into();
        old_bundle(&lib, &future, &value.to_string());
        let mut current = Guide::new("Current", "en");
        current.schema_version = 3;
        lib.create(&current).unwrap();
        fs::create_dir(lib.root().join("Broken.guiderecorder")).unwrap();
        // A stale leftover, which only recover() may remove.
        let leftover = lib.root().join(temp_dir_name(
            "create",
            SystemTime::now() - Duration::from_secs(2 * 60 * 60),
        ));
        fs::create_dir(&leftover).unwrap();
        let before = tree(lib.root());

        let fresh = Library::open_with_schema(lib.root(), test_schema::SCHEMA).unwrap();
        let listed = fresh.list().unwrap();
        assert_eq!(listed.len(), 3);
        let summary = listed.iter().find(|s| s.id == old.id).unwrap();
        // Migrated in memory only.
        assert_eq!(summary.schema_version, 1);
        assert_eq!(summary.summary, "Sub");
        let versions: Vec<_> = listed.iter().map(|s| s.schema_version).collect();
        assert!(versions.contains(&9) && versions.contains(&3));
        assert_eq!(fresh.scan().unwrap().len(), 4);
        for id in [old.id, future.id, current.id] {
            assert!(fresh.find(id).unwrap().is_some());
        }
        assert_eq!(tree(lib.root()), before);
        assert!(!old_handle.backup_dir().exists());
    }

    #[test]
    fn missing_schema_version_is_an_error() {
        let (_tmp, lib) = library();
        let handle = lib.create(&Guide::new("Bad", "en")).unwrap();
        let mut value = serde_json::to_value(lib.load(&handle.path).unwrap().guide).unwrap();
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
                "Same- name.guiderecorder",
                "Same- name 2.guiderecorder",
                "Same- name 3.guiderecorder"
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
        assert_eq!(renamed.path, lib.root().join("Taken 2.guiderecorder"));
        assert!(!handle.path.exists());
        guide.title = "Taken".into();
        assert_eq!(lib.save(&guide).unwrap(), renamed);

        // Same title again is a no-op, not a Taken 2 → Taken 3 → Taken 2 cycle.
        for _ in 0..3 {
            assert_eq!(lib.rename_bundle(&renamed, "Taken").unwrap(), renamed);
        }
        assert_eq!(
            files_in(lib.root()),
            ["Taken 2.guiderecorder", "Taken.guiderecorder"]
        );
    }

    #[test]
    fn rename_case_or_unicode_form_only() {
        let (_tmp, lib) = library();
        let handle = lib.create(&Guide::new("export pdf", "en")).unwrap();
        let renamed = lib.rename_bundle(&handle, "Export PDF").unwrap();
        assert_eq!(renamed.path, lib.root().join("Export PDF.guiderecorder"));
        assert_eq!(files_in(lib.root()), ["Export PDF.guiderecorder"]);

        let nfd = lib.create(&Guide::new("Cafe\u{301}", "en")).unwrap();
        let nfc = lib.rename_bundle(&nfd, "Caf\u{e9}").unwrap();
        assert_eq!(nfc.path, lib.root().join("Caf\u{e9}.guiderecorder"));
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
        let outside = tmp.path().join("Elsewhere.guiderecorder");
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
        assert_eq!(sanitize_title("COM0"), "COM0_");
        assert_eq!(sanitize_title("lpt0.txt"), "lpt0_.txt");
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
            path: PathBuf::from("/nonexistent-lib/G.guiderecorder"),
        };
        assert_eq!(
            handle.resolve("steps/a.webp").unwrap(),
            Path::new("/nonexistent-lib/G.guiderecorder/steps/a.webp")
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
