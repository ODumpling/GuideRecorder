//! The editor's library session: opening the library, listing, opening,
//! saving and closing guides, and reading and adding step images. Blocking
//! file IO only; the Tauri commands in `commands.rs` run it on the blocking
//! pool.

use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use guidesya_core::index::{self, Index, IndexedGuide, ListFilter};
use guidesya_core::library::{
    step_image_path, write_atomic, Durability, GuideHandle, Library, LibraryError, PixelBuffer,
    StepImageVariant, MAX_WEBP_DIMENSION, STEPS_DIR,
};
use guidesya_core::model::{AiState, Guide, ImageShow, Step, StepImage, StepKind};
use guidesya_core::Ulid;
use image::{ImageFormat, ImageReader, Limits};
use serde::Serialize;

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
    /// The export target already exists and overwrite is off.
    Exists,
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

/// Largest PNG `add_image_step` accepts. A 40 MP screenshot is far
/// smaller; this only bounds what the decoder is given.
pub const MAX_PNG_BYTES: usize = 256 * 1024 * 1024;

/// Image files `read_image_file` accepts, by extension. The webview decodes
/// them; Rust only stores WebP.
pub const IMAGE_FILE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "webp", "gif", "heic", "heif", "tif", "tiff", "bmp",
];

/// Largest image file `read_image_file` reads.
pub const MAX_IMAGE_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// Most pixels in a manual step's image (the editor scales larger ones
/// down to fit): 40 MP is 160 MB as RGBA.
pub const MAX_IMAGE_PIXELS: u64 = 40_000_000;

/// A guide opened for editing: the bundle it came from and the latest copy
/// (as opened or last saved).
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

    pub(crate) fn guide_lock(&self, id: Ulid) -> Arc<Mutex<()>> {
        Arc::clone(self.guide_locks.lock().unwrap().entry(id).or_default())
    }

    pub fn root(&self) -> &Path {
        self.library.root()
    }

    /// Whether the UI has a guide open (a library switch waits for none).
    pub fn has_open_guides(&self) -> bool {
        !self.open.lock().unwrap().is_empty()
    }

    pub(crate) fn library(&self) -> &Library {
        &self.library
    }

    /// Adds or refreshes a bundle written outside the editor (a recording)
    /// in the index, as [`Session::save_guide`] does. A failed update is
    /// only logged: listing falls back to a scan.
    pub(crate) fn index_bundle(&self, handle: &GuideHandle) {
        if let Some(index) = self.index.lock().unwrap().as_mut() {
            if let Err(e) = index.update(&self.library, &handle.path) {
                eprintln!("guidesya: index update failed: {e}");
            }
        }
    }

    /// The bundle of an opened guide.
    pub(crate) fn open_handle(&self, id: Ulid) -> Result<GuideHandle> {
        let open = self.open.lock().unwrap();
        let entry = open
            .get(&id)
            .ok_or_else(|| AppError::new(ErrorKind::NotOpen, format!("guide {id} is not open")))?;
        Ok(entry.handle.clone())
    }

    /// The bundle of a guide: the one it was opened from, else the one the
    /// library finds.
    pub(crate) fn bundle(&self, id: Ulid) -> Result<GuideHandle> {
        match self.open_handle(id) {
            Ok(handle) => Ok(handle),
            Err(_) => Ok(self.library.find(id)?.ok_or(LibraryError::NotFound(id))?),
        }
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

    /// An opened guide as last opened or saved, with its bundle: the
    /// snapshot an AI run works on (see `ai.rs`). Refuses read-only guides,
    /// whose results the editor could not save.
    pub(crate) fn saved_guide(&self, id: Ulid) -> Result<(Guide, GuideHandle)> {
        let open = self.open.lock().unwrap();
        let entry = open
            .get(&id)
            .ok_or_else(|| AppError::new(ErrorKind::NotOpen, format!("guide {id} is not open")))?;
        if entry.read_only {
            return Err(AppError::new(
                ErrorKind::ReadOnly,
                format!("guide {id} is read-only"),
            ));
        }
        Ok((entry.guide.clone(), entry.handle.clone()))
    }

    /// The bundle of an opened guide that may be written.
    fn writable_handle(&self, id: Ulid) -> Result<GuideHandle> {
        let open = self.open.lock().unwrap();
        let entry = open
            .get(&id)
            .ok_or_else(|| AppError::new(ErrorKind::NotOpen, format!("guide {id} is not open")))?;
        if entry.read_only {
            return Err(AppError::new(
                ErrorKind::ReadOnly,
                format!("guide {id} is read-only"),
            ));
        }
        Ok(entry.handle.clone())
    }

    /// The raw WebP bytes of an image in an opened guide's bundle, by its
    /// bundle-relative path (`StepImage::full` and friends).
    ///
    /// The path comes from the editor's copy of the guide, not the saved
    /// one, so images of steps that are not saved yet (inserted, split,
    /// restored by undo) load too. Only WebP files under `steps/` or
    /// `assets/` are served, through [`GuideHandle::resolve`], which keeps
    /// the path inside the bundle.
    pub fn step_image(&self, guide_id: &str, path: &str) -> Result<Vec<u8>> {
        let guide_id = parse_id(guide_id)?;
        check_image_path(path)?;
        let handle = self.open_handle(guide_id)?;
        let path = handle.resolve(path)?;
        std::fs::read(&path).map_err(|e| file_error(&path, e))
    }

    /// Stores a PNG (made by the webview from whatever file the user chose)
    /// as the full image of a new manual step and returns the step. The
    /// guide itself is not changed: the editor inserts the step as an
    /// undoable edit, and saves it with the guide.
    pub fn add_image_step(&self, guide_id: &str, png: &[u8]) -> Result<Step> {
        let guide_id = parse_id(guide_id)?;
        // Cheap refusals first; the decode is not.
        self.writable_handle(guide_id)?;
        let image = decode_png(png)?;
        let (width, height) = image.dimensions();
        let rgba = image.as_raw();
        let lock = self.guide_lock(guide_id);
        let _guard = lock.lock().unwrap();
        let handle = self.writable_handle(guide_id)?;
        let id = Ulid::new();
        let full = handle.write_step_image(
            id,
            StepImageVariant::Full,
            &PixelBuffer::rgba(width, height, rgba),
        )?;
        Ok(Step {
            id,
            title: String::new(),
            description: String::new(),
            kind: StepKind::Manual,
            point: None,
            end_point: None,
            element: None,
            keystrokes: None,
            image: StepImage {
                full,
                crop: None,
                crop_rect: None,
                after: None,
                show: ImageShow::Full,
            },
            annotations: Vec::new(),
            ai_state: AiState::default(),
        })
    }

    /// Copies a step's image files (the paths in `image`) to the files of
    /// the new step `new_step`, and returns the new step's `StepImage`.
    ///
    /// Splitting a step uses this, so the two steps never share files: a
    /// later change to one step's images (a re-capture, a new crop) can't
    /// change the other's. Sources must be WebP files under `steps/` or
    /// `assets/`, read through [`GuideHandle::resolve`]; existing files of
    /// `new_step` are never overwritten.
    pub fn copy_step_images(
        &self,
        guide_id: &str,
        image: &StepImage,
        new_step: &str,
    ) -> Result<StepImage> {
        let guide_id = parse_id(guide_id)?;
        let new_step = parse_id(new_step)?;
        let lock = self.guide_lock(guide_id);
        let _guard = lock.lock().unwrap();
        let handle = self.writable_handle(guide_id)?;
        let copy = |source: &str, variant: StepImageVariant| -> Result<String> {
            let bytes = read_bundle_image(&handle, source)?;
            let target = step_image_path(new_step, variant);
            let path = handle.resolve(&target)?;
            if std::fs::symlink_metadata(&path).is_ok() {
                return Err(AppError::new(
                    ErrorKind::Conflict,
                    format!("{target} already exists"),
                ));
            }
            // `resolve` refused a symlinked `steps/`; a missing one is made.
            let steps = handle.steps_dir();
            match std::fs::create_dir(&steps) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(file_error(&steps, e)),
            }
            write_atomic(&path, &bytes, Durability::RenameOnly)?;
            Ok(target)
        };
        Ok(StepImage {
            full: copy(&image.full, StepImageVariant::Full)?,
            crop: image
                .crop
                .as_deref()
                .map(|crop| copy(crop, StepImageVariant::Crop))
                .transpose()?,
            crop_rect: image.crop_rect,
            after: image
                .after
                .as_deref()
                .map(|after| copy(after, StepImageVariant::After))
                .transpose()?,
            show: image.show,
        })
    }

    /// Forgets an opened guide and deletes the step images that its saved
    /// `guide.json` no longer uses. Editing keeps every image (undo may
    /// bring a deleted step back); closing is the point where they can go.
    /// The editor saves (or discards) its changes first. Returns the
    /// removed files' bundle-relative paths.
    pub fn close_guide(&self, guide_id: &str) -> Result<Vec<String>> {
        let guide_id = parse_id(guide_id)?;
        let lock = self.guide_lock(guide_id);
        let _guard = lock.lock().unwrap();
        let Some(entry) = self.open.lock().unwrap().remove(&guide_id) else {
            return Ok(Vec::new());
        };
        if entry.read_only {
            return Ok(Vec::new());
        }
        // The file on disk decides, not the cached copy.
        let saved = self.library.read(&entry.handle.path)?;
        if saved.read_only || saved.guide.id != guide_id {
            return Ok(Vec::new());
        }
        sweep_step_images(&entry.handle)
    }
}

/// Step image paths the editor may name: WebP files under `steps/` or
/// `assets/` (then resolved inside the bundle).
pub(crate) fn check_image_path(path: &str) -> Result<()> {
    let allowed =
        (path.starts_with("steps/") || path.starts_with("assets/")) && path.ends_with(".webp");
    if allowed {
        Ok(())
    } else {
        Err(AppError::new(
            ErrorKind::Invalid,
            format!("not a step image path: {path:?}"),
        ))
    }
}

/// Reads a step image file of the bundle: a checked path, a regular file
/// (not a symlink), at most [`MAX_PNG_BYTES`].
fn read_bundle_image(handle: &GuideHandle, relative: &str) -> Result<Vec<u8>> {
    check_image_path(relative)?;
    let path = handle.resolve(relative)?;
    let meta = std::fs::symlink_metadata(&path).map_err(|e| file_error(&path, e))?;
    if !meta.is_file() || meta.len() > MAX_PNG_BYTES as u64 {
        return Err(AppError::new(
            ErrorKind::Invalid,
            format!("{relative} is not a usable image file"),
        ));
    }
    std::fs::read(&path).map_err(|e| file_error(&path, e))
}

/// Decodes a step image PNG. The size in its header is checked before any
/// pixels are decoded (at most [`MAX_WEBP_DIMENSION`] per side and
/// [`MAX_IMAGE_PIXELS`]), and the decoder runs with matching limits.
fn decode_png(png: &[u8]) -> Result<image::RgbaImage> {
    let invalid = |e: image::ImageError| {
        AppError::new(ErrorKind::Invalid, format!("not a readable PNG image: {e}"))
    };
    if png.len() > MAX_PNG_BYTES {
        return Err(AppError::new(
            ErrorKind::Invalid,
            format!("the image is too large ({} MB)", png.len() / 1024 / 1024),
        ));
    }
    let reader = || ImageReader::with_format(Cursor::new(png), ImageFormat::Png);
    let (width, height) = reader().into_dimensions().map_err(invalid)?;
    let valid = 1..=MAX_WEBP_DIMENSION;
    if !valid.contains(&width)
        || !valid.contains(&height)
        || u64::from(width) * u64::from(height) > MAX_IMAGE_PIXELS
    {
        return Err(AppError::new(
            ErrorKind::Invalid,
            format!(
                "image size {width}×{height} is not supported (at most {MAX_WEBP_DIMENSION} \
                 per side and {} MP)",
                MAX_IMAGE_PIXELS / 1_000_000
            ),
        ));
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_WEBP_DIMENSION);
    limits.max_image_height = Some(MAX_WEBP_DIMENSION);
    // 16-bit RGBA at the pixel limit, plus the 8-bit copy.
    limits.max_alloc = Some(MAX_IMAGE_PIXELS * 12);
    let mut reader = reader();
    reader.limits(limits);
    Ok(reader.decode().map_err(invalid)?.into_rgba8())
}

/// Deletes files in `steps/` named like step images (`<ulid>.webp`,
/// `<ulid>.crop.webp`, `<ulid>.after.webp`) that the bundle's `guide.json`
/// and its `.backup/*.json` copies do not mention. Other files are left
/// alone, and so is everything when `steps/` is not a real folder inside
/// the bundle (a symlinked one could point at another guide).
fn sweep_step_images(handle: &GuideHandle) -> Result<Vec<String>> {
    let Some(used) = referenced_step_images(handle)? else {
        return Ok(Vec::new());
    };
    // `resolve` refuses a symlinked `steps`; the check below refuses a
    // `steps` that is a file or that was swapped since.
    let dir = match handle.resolve(STEPS_DIR) {
        Ok(dir) => dir,
        Err(LibraryError::SymlinkInBundle(_)) => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    match std::fs::symlink_metadata(&dir) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => return Ok(Vec::new()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(file_error(&dir, e)),
    }
    let entries = std::fs::read_dir(&dir).map_err(|e| file_error(&dir, e))?;
    let mut removed = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| file_error(&dir, e))?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some(key) = step_image_key(&name) else {
            continue;
        };
        if used.contains(&key) {
            continue;
        }
        let path = entry.path();
        if !std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_file()) {
            continue;
        }
        match std::fs::remove_file(&path) {
            Ok(()) => removed.push(format!("{STEPS_DIR}/{name}")),
            Err(e) => eprintln!("guidesya: cannot remove {}: {e}", path.display()),
        }
    }
    removed.sort();
    Ok(removed)
}

/// Which step image a file is: its step id and variant (0 full, 1 crop,
/// 2 after). Case-insensitive, as macOS file names are.
type StepImageKey = (Ulid, u8);

/// The key of a step image file name, `<ulid>[.crop|.after].webp` in any
/// case; `None` for any other name.
fn step_image_key(name: &str) -> Option<StepImageKey> {
    let lower = name.to_ascii_lowercase();
    let stem = lower.strip_suffix(".webp")?;
    let (id, variant) = if let Some(id) = stem.strip_suffix(".crop") {
        (id, 1)
    } else if let Some(id) = stem.strip_suffix(".after") {
        (id, 2)
    } else {
        (stem, 0)
    };
    if id.len() != 26 {
        return None;
    }
    Ulid::from_string(&id.to_ascii_uppercase())
        .ok()
        .map(|id| (id, variant))
}

/// Every step image mentioned anywhere in `guide.json` or a JSON file in
/// `.backup/`, in any spelling of its path (`steps//X.webp`, `./steps/x.webp`
/// and so on all count): any string whose last path component is a step
/// image name. `None` when a file can't be read, so nothing is deleted.
fn referenced_step_images(handle: &GuideHandle) -> Result<Option<HashSet<StepImageKey>>> {
    let mut files = vec![handle.guide_json()];
    let backups = handle.backup_dir();
    match std::fs::symlink_metadata(&backups) {
        Ok(meta) if meta.is_dir() => {
            for entry in std::fs::read_dir(&backups).map_err(|e| file_error(&backups, e))? {
                let path = entry.map_err(|e| file_error(&backups, e))?.path();
                if path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("json"))
                {
                    files.push(path);
                }
            }
        }
        Ok(_) => return Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(file_error(&backups, e)),
    }
    let mut used = HashSet::new();
    for path in files {
        let regular = std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_file());
        let json = regular
            .then(|| std::fs::read(&path).ok())
            .flatten()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
        let Some(json) = json else {
            eprintln!(
                "guidesya: not sweeping images: cannot read {}",
                path.display()
            );
            return Ok(None);
        };
        collect_step_images(&json, &mut used);
    }
    Ok(Some(used))
}

fn collect_step_images(value: &serde_json::Value, used: &mut HashSet<StepImageKey>) {
    match value {
        serde_json::Value::String(s) => {
            if let Some(key) = s.rsplit(['/', '\\']).next().and_then(step_image_key) {
                used.insert(key);
            }
        }
        serde_json::Value::Array(items) => items
            .iter()
            .for_each(|item| collect_step_images(item, used)),
        serde_json::Value::Object(map) => map
            .values()
            .for_each(|item| collect_step_images(item, used)),
        _ => {}
    }
}

/// Files the user dropped on the window. `read_image_file` reads only
/// these, each once, for a short while after the drop.
#[derive(Default)]
pub struct DroppedFiles {
    paths: Mutex<Vec<(PathBuf, Instant)>>,
}

impl DroppedFiles {
    /// How long a dropped path stays readable.
    pub const TTL: Duration = Duration::from_secs(120);

    pub fn record(&self, paths: &[PathBuf]) {
        self.record_at(paths, Instant::now());
    }

    fn record_at(&self, paths: &[PathBuf], now: Instant) {
        let mut list = self.paths.lock().unwrap();
        list.retain(|(_, at)| now.duration_since(*at) < Self::TTL);
        list.extend(paths.iter().map(|path| (path.clone(), now)));
    }

    /// Removes `path` from the list; true if it was there and fresh.
    fn take_at(&self, path: &Path, now: Instant) -> bool {
        let mut list = self.paths.lock().unwrap();
        list.retain(|(_, at)| now.duration_since(*at) < Self::TTL);
        match list.iter().position(|(dropped, _)| dropped == path) {
            Some(i) => {
                list.remove(i);
                true
            }
            None => false,
        }
    }

    /// Reads a dropped image file (see [`read_image_file`]); any other path
    /// is refused.
    pub fn read(&self, path: &Path) -> Result<Vec<u8>> {
        if !self.take_at(path, Instant::now()) {
            return Err(AppError::new(
                ErrorKind::Invalid,
                format!("{} was not dropped on the window", path.display()),
            ));
        }
        read_image_file(path)
    }
}

/// Reads an image file, for the webview to decode: known image extensions
/// and regular files up to [`MAX_IMAGE_FILE_BYTES`] only. The size is
/// checked on the open file and the read is capped, so a file that grows
/// meanwhile is refused too.
fn read_image_file(path: &Path) -> Result<Vec<u8>> {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    if !extension.is_some_and(|e| IMAGE_FILE_EXTENSIONS.contains(&e.as_str())) {
        return Err(AppError::new(
            ErrorKind::Invalid,
            format!("{} is not an image file", path.display()),
        ));
    }
    let file = std::fs::File::open(path).map_err(|e| file_error(path, e))?;
    let meta = file.metadata().map_err(|e| file_error(path, e))?;
    if !meta.is_file() {
        return Err(AppError::new(
            ErrorKind::Invalid,
            format!("{} is not a file", path.display()),
        ));
    }
    let too_large = || {
        AppError::new(
            ErrorKind::Invalid,
            format!(
                "{} is too large (over {} MB)",
                path.display(),
                MAX_IMAGE_FILE_BYTES / 1024 / 1024
            ),
        )
    };
    if meta.len() > MAX_IMAGE_FILE_BYTES {
        return Err(too_large());
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.take(MAX_IMAGE_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| file_error(path, e))?;
    if bytes.len() as u64 > MAX_IMAGE_FILE_BYTES {
        return Err(too_large());
    }
    Ok(bytes)
}

pub(crate) fn file_error(path: &Path, e: std::io::Error) -> AppError {
    let kind = if e.kind() == std::io::ErrorKind::NotFound {
        ErrorKind::NotFound
    } else {
        ErrorKind::Io
    };
    AppError::new(kind, format!("{}: {e}", path.display()))
}

pub(crate) fn parse_id(id: &str) -> Result<Ulid> {
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

    /// The session or its startup error, without waiting; `None` while it
    /// is still starting.
    pub fn current(&self) -> Option<Result<Arc<Session>>> {
        self.state.lock().unwrap().clone()
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
pub(crate) mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    pub(crate) const FIXTURE: &str = "Rotate an API key.guidesya";

    /// A fresh library under `target/` holding a copy of the fixtures.
    pub(crate) fn library() -> PathBuf {
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

    pub(crate) fn fixture_id(root: &Path) -> String {
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
        let bytes = session.step_image(&id, &step.image.full).unwrap();
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WEBP");
        assert_eq!(
            bytes,
            fs::read(root.join(FIXTURE).join(&step.image.full)).unwrap()
        );

        let missing = session.step_image(&id, &format!("steps/{}.webp", Ulid::new()));
        assert_eq!(missing.unwrap_err().kind, ErrorKind::NotFound);
        for bad in [
            "guide.json",
            "steps/../guide.json",
            "steps/../../x.webp",
            "/etc/x.webp",
            "steps/a.png",
            "x/steps/a.webp",
        ] {
            let err = session.step_image(&id, bad).unwrap_err();
            assert_eq!(err.kind, ErrorKind::Invalid, "{bad}");
        }
        let bad_id = session.step_image("nope", &step.image.full);
        assert_eq!(bad_id.unwrap_err().kind, ErrorKind::Invalid);
        let unopened = session.step_image(&Ulid::new().to_string(), &step.image.full);
        assert_eq!(unopened.unwrap_err().kind, ErrorKind::NotOpen);
        fs::remove_dir_all(root).unwrap();
    }

    fn pixels(width: u32, height: u32) -> Vec<u8> {
        (0..width * height)
            .flat_map(|i| [(i % 256) as u8, 40, 200, 255])
            .collect()
    }

    /// `pixels(width, height)` as an RGBA PNG, as the webview sends it.
    fn png(width: u32, height: u32) -> Vec<u8> {
        use image::ImageEncoder;
        let mut out = Vec::new();
        image::codecs::png::PngEncoder::new(&mut out)
            .write_image(
                &pixels(width, height),
                width,
                height,
                image::ExtendedColorType::Rgba8,
            )
            .unwrap();
        out
    }

    /// A 1×1 PNG whose header claims `width`×`height` (with a valid CRC):
    /// only the size check can tell.
    fn png_claiming(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = png(1, 1);
        // Signature (8), IHDR length (4), "IHDR" (4), then width and height.
        bytes[16..20].copy_from_slice(&width.to_be_bytes());
        bytes[20..24].copy_from_slice(&height.to_be_bytes());
        let crc = crc32(&bytes[12..29]);
        bytes[29..33].copy_from_slice(&crc.to_be_bytes());
        bytes
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &byte in data {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    #[test]
    fn decodes_pngs_within_limits_only() {
        assert_eq!(decode_png(&png(6, 4)).unwrap().into_raw(), pixels(6, 4));
        // The header is checked before decoding: these claim sizes that
        // would be refused (their pixel data is missing, so a decode
        // would fail differently).
        for (w, h) in [
            (MAX_WEBP_DIMENSION + 1, 1),
            (10_000, 4_001),
            (16_000, 16_000),
        ] {
            let err = decode_png(&png_claiming(w, h)).unwrap_err();
            assert_eq!(err.kind, ErrorKind::Invalid);
            assert!(
                err.message.contains("not supported"),
                "{w}×{h}: {}",
                err.message
            );
        }
        // Within the limits the claimed size is read, then the data fails.
        let err = decode_png(&png_claiming(2, 2)).unwrap_err();
        assert!(
            err.message.contains("not a readable PNG"),
            "{}",
            err.message
        );
        // 16-bit PNGs become 8-bit RGBA.
        let mut wide = Vec::new();
        {
            use image::ImageEncoder;
            let data: Vec<u8> = [0x12u8, 0x34].repeat(3 * 2 * 4);
            image::codecs::png::PngEncoder::new(&mut wide)
                .write_image(&data, 3, 2, image::ExtendedColorType::Rgba16)
                .unwrap();
        }
        assert_eq!(decode_png(&wide).unwrap().dimensions(), (3, 2));
        for junk in [&b""[..], b"not a png", &png(2, 2)[..20]] {
            assert_eq!(decode_png(junk).unwrap_err().kind, ErrorKind::Invalid);
        }
        // Other formats are not taken as PNG.
        let steps = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/guides")
            .join(FIXTURE)
            .join("steps");
        let webp = fs::read_dir(steps).unwrap().next().unwrap().unwrap().path();
        let err = decode_png(&fs::read(webp).unwrap()).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Invalid);
        // Over the byte cap: refused before reading the header.
        let mut huge = png(1, 1);
        huge.resize(MAX_PNG_BYTES + 1, 0);
        assert!(decode_png(&huge).unwrap_err().message.contains("too large"));
    }

    #[test]
    fn adds_image_steps_readable_before_saving() {
        let root = library();
        let session = Session::start(&root, None).unwrap();
        let id = fixture_id(&root);
        session.open_guide(&id).unwrap();

        let step = session.add_image_step(&id, &png(6, 4)).unwrap();
        assert_eq!(step.kind, StepKind::Manual);
        assert_eq!(step.image.full, format!("steps/{}.webp", step.id));
        assert_eq!(step.image.crop, None);
        assert_eq!(step.image.show, ImageShow::Full);
        // Not in guide.json yet, but its image loads.
        let bytes = session.step_image(&id, &step.image.full).unwrap();
        assert_eq!(&bytes[8..12], b"WEBP");
        let stored =
            guidesya_core::library::read_webp(&root.join(FIXTURE).join(&step.image.full)).unwrap();
        assert_eq!(stored.dimensions(), (6, 4));
        assert_eq!(stored.into_raw(), pixels(6, 4));

        for bad in [b"junk".to_vec(), png_claiming(10_000, 4_001)] {
            let err = session.add_image_step(&id, &bad).unwrap_err();
            assert_eq!(err.kind, ErrorKind::Invalid);
        }
        let unopened = session.add_image_step(&Ulid::new().to_string(), &png(1, 1));
        assert_eq!(unopened.unwrap_err().kind, ErrorKind::NotOpen);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn add_image_step_refuses_read_only_guides() {
        let root = library();
        let session = Session::start(&root, None).unwrap();
        let id = fixture_id(&root);
        let path = root.join(FIXTURE).join("guide.json");
        let mut json: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        json["schemaVersion"] = (guidesya_core::SCHEMA_VERSION + 1).into();
        fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
        session.open_guide(&id).unwrap();
        let before = fs::read_dir(root.join(FIXTURE).join("steps"))
            .unwrap()
            .count();
        let err = session.add_image_step(&id, &png(1, 1)).unwrap_err();
        assert_eq!(err.kind, ErrorKind::ReadOnly);
        // Closing a read-only guide sweeps nothing.
        let orphan = root
            .join(FIXTURE)
            .join(format!("steps/{}.webp", Ulid::new()));
        fs::write(&orphan, b"x").unwrap();
        assert!(session.close_guide(&id).unwrap().is_empty());
        assert!(orphan.exists());
        assert_eq!(
            fs::read_dir(root.join(FIXTURE).join("steps"))
                .unwrap()
                .count(),
            before + 1
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn close_sweeps_unused_step_images_only() {
        let root = library();
        let session = Session::start(&root, None).unwrap();
        let id = fixture_id(&root);
        let bundle = root.join(FIXTURE);
        let mut guide = session.open_guide(&id).unwrap().guide;
        let used: Vec<String> = guide
            .steps()
            .flat_map(|s| [Some(s.image.full.clone()), s.image.crop.clone()])
            .flatten()
            .collect();

        // One new step is saved, one is added and dropped (undone).
        let kept = session.add_image_step(&id, &png(2, 2)).unwrap();
        let dropped = session.add_image_step(&id, &png(2, 2)).unwrap();
        guide
            .items
            .push(guidesya_core::model::Item::Step(kept.clone()));
        session.save_guide(guide).unwrap();
        // A step deleted from the guide leaves its old crop behind.
        let stale = format!("steps/{}.crop.webp", Ulid::new());
        fs::write(bundle.join(&stale), b"x").unwrap();
        // Files that are not step images stay.
        for other in ["steps/notes.txt", "steps/.hidden.webp", "steps/ABC.webp"] {
            fs::write(bundle.join(other), b"x").unwrap();
        }

        let mut removed = session.close_guide(&id).unwrap();
        removed.sort();
        let mut expected = vec![dropped.image.full.clone(), stale.clone()];
        expected.sort();
        assert_eq!(removed, expected);
        assert!(!bundle.join(&dropped.image.full).exists());
        assert!(bundle.join(&kept.image.full).exists());
        for path in used {
            assert!(bundle.join(&path).exists(), "{path}");
        }
        for other in ["steps/notes.txt", "steps/.hidden.webp", "steps/ABC.webp"] {
            assert!(bundle.join(other).exists(), "{other}");
        }
        // Closed: images are no longer served, and closing again is a no-op.
        let err = session.step_image(&id, &kept.image.full).unwrap_err();
        assert_eq!(err.kind, ErrorKind::NotOpen);
        assert!(session.close_guide(&id).unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    /// Opens the fixture with a fresh image step saved into it.
    fn session_with_saved_image_step(root: &Path) -> (Session, String, Step) {
        let session = Session::start(root, None).unwrap();
        let id = fixture_id(root);
        let mut guide = session.open_guide(&id).unwrap().guide;
        let step = session.add_image_step(&id, &png(2, 2)).unwrap();
        guide
            .items
            .push(guidesya_core::model::Item::Step(step.clone()));
        session.save_guide(guide).unwrap();
        (session, id, step)
    }

    #[test]
    fn sweep_counts_any_spelling_of_a_path() {
        let root = library();
        let (session, id, step) = session_with_saved_image_step(&root);
        let bundle = root.join(FIXTURE);
        let json_path = bundle.join("guide.json");
        let json = fs::read_to_string(&json_path).unwrap();
        let lower = format!("steps/{}.webp", step.id.to_string().to_lowercase());
        // A doubled slash and a lowercase name: same files, other spellings.
        let crop = Ulid::new();
        fs::write(bundle.join(format!("steps/{crop}.crop.webp")), b"x").unwrap();
        let json = json.replace(&step.image.full, &lower).replacen(
            "\"show\"",
            &format!("\"after\": \"steps//{crop}.crop.webp\", \"show\""),
            1,
        );
        fs::write(&json_path, json).unwrap();
        // The sweep runs: an unmentioned image goes.
        let orphan = format!("steps/{}.webp", Ulid::new());
        fs::write(bundle.join(&orphan), b"x").unwrap();
        assert_eq!(session.close_guide(&id).unwrap(), vec![orphan]);
        assert!(bundle.join(&step.image.full).exists());
        assert!(bundle.join(format!("steps/{crop}.crop.webp")).exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sweep_keeps_images_of_backups_and_skips_unreadable_ones() {
        let root = library();
        let (session, id, _) = session_with_saved_image_step(&root);
        let bundle = root.join(FIXTURE);
        let old = Ulid::new();
        let orphan = Ulid::new();
        for name in [old, orphan] {
            fs::write(bundle.join(format!("steps/{name}.webp")), b"x").unwrap();
        }
        fs::create_dir_all(bundle.join(".backup")).unwrap();
        fs::write(
            bundle.join(".backup/guide.v0.json"),
            format!(r#"{{"items": [{{"image": {{"full": "steps/{old}.webp"}}}}]}}"#),
        )
        .unwrap();
        session.open_guide(&id).unwrap();
        assert_eq!(
            session.close_guide(&id).unwrap(),
            vec![format!("steps/{orphan}.webp")]
        );
        assert!(bundle.join(format!("steps/{old}.webp")).exists());

        // A backup that can't be read stops the sweep.
        fs::write(bundle.join(format!("steps/{orphan}.webp")), b"x").unwrap();
        fs::write(bundle.join(".backup/guide.v00.json"), b"{not json").unwrap();
        session.open_guide(&id).unwrap();
        assert!(session.close_guide(&id).unwrap().is_empty());
        assert!(bundle.join(format!("steps/{orphan}.webp")).exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn sweep_never_follows_a_symlinked_steps_folder() {
        let root = library();
        let session = Session::start(&root, None).unwrap();
        let id = fixture_id(&root);
        let bundle = root.join(FIXTURE);
        let other = root.join("Share a calendar event.guidesya/steps");
        let before: Vec<_> = fs::read_dir(&other)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert!(!before.is_empty());
        // `steps/` now points at another guide's images, none of which
        // this guide.json mentions.
        fs::remove_dir_all(bundle.join("steps")).unwrap();
        std::os::unix::fs::symlink(&other, bundle.join("steps")).unwrap();
        session.open_guide(&id).unwrap();
        assert!(session.close_guide(&id).unwrap().is_empty());
        for path in before {
            assert!(path.exists(), "{}", path.display());
        }

        // A symlinked image inside a real `steps/` is skipped too.
        fs::remove_file(bundle.join("steps")).unwrap();
        fs::create_dir(bundle.join("steps")).unwrap();
        let target = fs::read_dir(&other)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let link = bundle.join(format!("steps/{}.webp", Ulid::new()));
        std::os::unix::fs::symlink(&target, &link).unwrap();
        // A real orphan beside it proves the sweep ran.
        let orphan = format!("steps/{}.webp", Ulid::new());
        fs::write(bundle.join(&orphan), b"x").unwrap();
        session.open_guide(&id).unwrap();
        assert_eq!(session.close_guide(&id).unwrap(), vec![orphan]);
        assert!(target.exists() && fs::symlink_metadata(&link).is_ok());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn copies_step_images_for_a_split_step() {
        let root = library();
        let session = Session::start(&root, None).unwrap();
        let id = fixture_id(&root);
        let bundle = root.join(FIXTURE);
        let guide = session.open_guide(&id).unwrap().guide;
        let step = guide
            .steps()
            .find(|s| s.image.crop.is_some())
            .unwrap()
            .clone();
        let new = Ulid::new();
        let copied = session
            .copy_step_images(&id, &step.image, &new.to_string())
            .unwrap();
        assert_eq!(copied.full, format!("steps/{new}.webp"));
        assert_eq!(copied.crop, Some(format!("steps/{new}.crop.webp")));
        assert_eq!(copied.crop_rect, step.image.crop_rect);
        assert_eq!(copied.show, step.image.show);
        assert_eq!(
            fs::read(bundle.join(&copied.full)).unwrap(),
            fs::read(bundle.join(&step.image.full)).unwrap()
        );
        assert_eq!(
            fs::read(bundle.join(copied.crop.as_ref().unwrap())).unwrap(),
            fs::read(bundle.join(step.image.crop.as_ref().unwrap())).unwrap()
        );

        // Never over existing files, never from odd paths.
        let again = session.copy_step_images(&id, &step.image, &new.to_string());
        assert_eq!(again.unwrap_err().kind, ErrorKind::Conflict);
        for bad in ["guide.json", "steps/../guide.json", "/etc/x.webp"] {
            let image = StepImage {
                full: bad.into(),
                ..step.image.clone()
            };
            let err = session
                .copy_step_images(&id, &image, &Ulid::new().to_string())
                .unwrap_err();
            assert_eq!(err.kind, ErrorKind::Invalid, "{bad}");
        }
        let missing = StepImage {
            full: format!("steps/{}.webp", Ulid::new()),
            crop: None,
            ..step.image.clone()
        };
        let err = session
            .copy_step_images(&id, &missing, &Ulid::new().to_string())
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::NotFound);
        let unopened = session.copy_step_images(
            &Ulid::new().to_string(),
            &step.image,
            &Ulid::new().to_string(),
        );
        assert_eq!(unopened.unwrap_err().kind, ErrorKind::NotOpen);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn copy_refuses_symlinked_sources() {
        let root = library();
        let session = Session::start(&root, None).unwrap();
        let id = fixture_id(&root);
        let bundle = root.join(FIXTURE);
        let secret = root.join("secret.webp");
        fs::write(&secret, b"outside").unwrap();
        let link = format!("steps/{}.webp", Ulid::new());
        std::os::unix::fs::symlink(&secret, bundle.join(&link)).unwrap();
        let guide = session.open_guide(&id).unwrap().guide;
        let image = StepImage {
            full: link,
            crop: None,
            after: None,
            ..guide.steps().next().unwrap().image.clone()
        };
        let err = session
            .copy_step_images(&id, &image, &Ulid::new().to_string())
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Invalid);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn step_image_names() {
        let id = Ulid::new();
        let lower = id.to_string().to_lowercase();
        assert_eq!(step_image_key(&format!("{id}.webp")), Some((id, 0)));
        assert_eq!(step_image_key(&format!("{id}.crop.webp")), Some((id, 1)));
        assert_eq!(step_image_key(&format!("{id}.after.WEBP")), Some((id, 2)));
        assert_eq!(step_image_key(&format!("{lower}.Crop.webp")), Some((id, 1)));
        for name in [
            format!("{id}.png"),
            format!("{id}.thumb.webp"),
            format!(".{id}.webp.1.tmp"),
            "ABC.webp".to_owned(),
            ".webp".to_owned(),
        ] {
            assert_eq!(step_image_key(&name), None, "{name}");
        }
    }

    #[test]
    fn reads_dropped_files_once() {
        let root = library();
        let png = root.join("shot.png");
        fs::write(&png, b"png bytes").unwrap();
        let dropped = DroppedFiles::default();
        assert_eq!(dropped.read(&png).unwrap_err().kind, ErrorKind::Invalid);
        dropped.record(std::slice::from_ref(&png));
        assert_eq!(dropped.read(&png).unwrap(), b"png bytes");
        // Each drop allows one read.
        assert_eq!(dropped.read(&png).unwrap_err().kind, ErrorKind::Invalid);

        let then = Instant::now();
        dropped.record_at(std::slice::from_ref(&png), then);
        assert!(!dropped.take_at(&png, then + DroppedFiles::TTL));
        dropped.record_at(std::slice::from_ref(&png), then);
        assert!(!dropped.take_at(&root.join("other.png"), then));
        assert!(dropped.take_at(&png, then + Duration::from_secs(1)));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reads_image_files_only() {
        let root = library();
        let png = root.join("shot.PNG");
        fs::write(&png, b"not really a png").unwrap();
        assert_eq!(read_image_file(&png).unwrap(), b"not really a png");
        let text = root.join("notes.txt");
        fs::write(&text, b"x").unwrap();
        assert_eq!(read_image_file(&text).unwrap_err().kind, ErrorKind::Invalid);
        let folder = root.join("folder.png");
        fs::create_dir(&folder).unwrap();
        assert_eq!(
            read_image_file(&folder).unwrap_err().kind,
            ErrorKind::Invalid
        );
        assert_eq!(
            read_image_file(&root.join("missing.jpg")).unwrap_err().kind,
            ErrorKind::NotFound
        );
        let big = root.join("big.webp");
        fs::File::create(&big)
            .unwrap()
            .set_len(MAX_IMAGE_FILE_BYTES + 1)
            .unwrap();
        assert_eq!(read_image_file(&big).unwrap_err().kind, ErrorKind::Invalid);
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
