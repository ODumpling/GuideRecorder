//! Tauri commands over the editor [`Session`]. Library IO runs on the
//! blocking pool, never on the main thread.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use guiderecorder_core::edit::DetailsEdit;
use guiderecorder_core::export::docx::Paper;
use guiderecorder_core::history::VersionInfo;
use guiderecorder_core::model::{Guide, NaiveDate, Step, StepImage};
use tauri::ipc::{InvokeBody, Request, Response};
use tauri::{AppHandle, Emitter, Runtime, State, WebviewWindow};
use tauri_plugin_dialog::{
    DialogExt, FileDialogBuilder, FilePath, MessageDialogButtons, MessageDialogKind,
};

use crate::annotated::PAGE_FILE_EXTENSIONS;
use crate::blocking::blocking;
use crate::editor::{
    parse_id, AppError, DroppedFiles, ErrorKind, GuideListItem, OpenedGuide, RenamedGuide, Result,
    ReviewedGuide, Session, SessionCell, MAX_PNG_BYTES,
};
use crate::export::{
    self, with_extension, BrandingRequest, ChosenPaths, ExportPreview, ExportResult, Format,
    MotionRequest, ScormRequest, SkillRequest,
};
use crate::settings::SettingsStore;

/// Runs `f` with the session on the blocking pool.
async fn with_session<T, F>(cell: State<'_, Arc<SessionCell>>, f: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(&Session) -> Result<T> + Send + 'static,
{
    let cell = Arc::clone(&cell);
    tauri::async_runtime::spawn_blocking(move || f(&*cell.get()?))
        .await
        .map_err(|e| AppError::new(ErrorKind::Io, format!("task failed: {e}")))?
}

#[tauri::command]
pub async fn library_root(cell: State<'_, Arc<SessionCell>>) -> Result<String> {
    with_session(cell, |s| Ok(s.root().display().to_string())).await
}

#[tauri::command]
pub async fn list_guides(
    cell: State<'_, Arc<SessionCell>>,
    query: Option<String>,
) -> Result<Vec<GuideListItem>> {
    with_session(cell, move |s| s.list_guides(query)).await
}

#[tauri::command]
pub async fn open_guide(cell: State<'_, Arc<SessionCell>>, id: String) -> Result<OpenedGuide> {
    with_session(cell, move |s| s.open_guide(&id)).await
}

#[tauri::command]
pub async fn save_guide(cell: State<'_, Arc<SessionCell>>, guide: Guide) -> Result<()> {
    with_session(cell, move |s| s.save_guide(guide)).await
}

/// Saves nothing: the editor saves first. Forgets the guide, keeps its
/// saved `guide.json` as a version (pruning the history per the settings)
/// and deletes step images its saved `guide.json` no longer uses.
#[tauri::command]
pub async fn close_guide(
    cell: State<'_, Arc<SessionCell>>,
    store: State<'_, Arc<SettingsStore>>,
    id: String,
) -> Result<Vec<String>> {
    let retention = store.get().history.retention();
    with_session(cell, move |s| s.close_guide(&id, &retention)).await
}

/// A guide's saved versions, newest first.
#[tauri::command]
pub async fn list_versions(
    cell: State<'_, Arc<SessionCell>>,
    guide_id: String,
) -> Result<Vec<VersionInfo>> {
    with_session(cell, move |s| s.list_versions(&guide_id)).await
}

/// A guide as one of its versions saved it.
#[tauri::command]
pub async fn read_version(
    cell: State<'_, Arc<SessionCell>>,
    guide_id: String,
    version_id: String,
) -> Result<Guide> {
    with_session(cell, move |s| s.read_version(&guide_id, &version_id)).await
}

/// Raw WebP bytes of an image as a version saved it.
#[tauri::command]
pub async fn version_image(
    cell: State<'_, Arc<SessionCell>>,
    guide_id: String,
    version_id: String,
    path: String,
) -> Result<Response> {
    let bytes = with_session(cell, move |s| {
        s.version_image(&guide_id, &version_id, &path)
    })
    .await?;
    Ok(Response::new(bytes))
}

/// Saves an open guide as last saved as a version (the editor saves first).
#[tauri::command]
pub async fn save_version(
    cell: State<'_, Arc<SessionCell>>,
    guide_id: String,
    label: Option<String>,
) -> Result<VersionInfo> {
    with_session(cell, move |s| s.save_version(&guide_id, label)).await
}

/// Makes a version an open guide's current state (keeping the current one
/// as a version first) and returns the guide as saved.
#[tauri::command]
pub async fn restore_version(
    cell: State<'_, Arc<SessionCell>>,
    guide_id: String,
    version_id: String,
) -> Result<Guide> {
    with_session(cell, move |s| s.restore_version(&guide_id, &version_id)).await
}

/// Asks to delete the guide at `path` (named `title` in the prompt), then
/// deletes it and its contents. False if the user cancelled.
#[tauri::command]
pub async fn delete_guide<R: Runtime>(
    window: WebviewWindow<R>,
    cell: State<'_, Arc<SessionCell>>,
    id: String,
    path: PathBuf,
    title: String,
) -> Result<bool> {
    let confirmed = blocking(move || {
        Ok(window
            .dialog()
            .message("The guide and all its screenshots are deleted. This can't be undone.")
            .title(format!("Delete “{title}”?"))
            .kind(MessageDialogKind::Warning)
            .buttons(MessageDialogButtons::OkCancelCustom(
                "Delete".into(),
                "Cancel".into(),
            ))
            .parent(&window)
            .blocking_show())
    })
    .await?;
    if !confirmed {
        return Ok(false);
    }
    with_session(cell, move |s| s.delete_guide(&id, &path)).await?;
    Ok(true)
}

/// A listed guide a library action works on: its id and bundle.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct GuideTarget {
    id: String,
    path: PathBuf,
}

/// Asks once to delete `guides` (more than one, from the library's
/// selection), then deletes them one by one, stopping at the first that
/// fails. False if the user cancelled.
#[tauri::command]
pub async fn delete_guides<R: Runtime>(
    window: WebviewWindow<R>,
    cell: State<'_, Arc<SessionCell>>,
    guides: Vec<GuideTarget>,
) -> Result<bool> {
    let count = guides.len();
    let confirmed = blocking(move || {
        Ok(window
            .dialog()
            .message("The guides and all their screenshots are deleted. This can't be undone.")
            .title(format!("Delete {count} guides?"))
            .kind(MessageDialogKind::Warning)
            .buttons(MessageDialogButtons::OkCancelCustom(
                "Delete".into(),
                "Cancel".into(),
            ))
            .parent(&window)
            .blocking_show())
    })
    .await?;
    if !confirmed {
        return Ok(false);
    }
    with_session(cell, move |s| {
        guides
            .iter()
            .try_for_each(|g| s.delete_guide(&g.id, &g.path))
    })
    .await?;
    Ok(true)
}

/// The library's folders, relative paths with `/` separators.
#[tauri::command]
pub async fn list_folders(cell: State<'_, Arc<SessionCell>>) -> Result<Vec<String>> {
    with_session(cell, |s| s.list_folders()).await
}

/// Makes folder `name` inside `parent` (`""`: the top of the library).
#[tauri::command]
pub async fn create_folder(
    cell: State<'_, Arc<SessionCell>>,
    parent: String,
    name: String,
) -> Result<String> {
    with_session(cell, move |s| s.create_folder(&parent, &name)).await
}

/// Copies the guide at `path` into a new guide titled `title`; returns its id.
#[tauri::command]
pub async fn duplicate_guide(
    cell: State<'_, Arc<SessionCell>>,
    id: String,
    path: PathBuf,
    title: String,
) -> Result<String> {
    with_session(cell, move |s| s.duplicate_guide(&id, &path, &title)).await
}

/// Retitles the guide at `path`, renaming its folder to match.
#[tauri::command]
pub async fn rename_guide(
    cell: State<'_, Arc<SessionCell>>,
    id: String,
    path: PathBuf,
    title: String,
) -> Result<RenamedGuide> {
    with_session(cell, move |s| s.rename_guide(&id, &path, &title)).await
}

/// Sets the guide's last review day to `today` (the editor's local day).
#[tauri::command]
pub async fn mark_guide_reviewed(
    cell: State<'_, Arc<SessionCell>>,
    id: String,
    path: PathBuf,
    today: NaiveDate,
) -> Result<ReviewedGuide> {
    with_session(cell, move |s| s.mark_guide_reviewed(&id, &path, today)).await
}

/// Sets the status, owner and tags of the guide at `path`; returns it as
/// saved.
#[tauri::command]
pub async fn edit_guide_details(
    cell: State<'_, Arc<SessionCell>>,
    id: String,
    path: PathBuf,
    edit: DetailsEdit,
) -> Result<Guide> {
    with_session(cell, move |s| s.edit_guide_details(&id, &path, &edit)).await
}

/// Moves the guide at `path` into `folder`; returns its new path.
#[tauri::command]
pub async fn move_guide(
    cell: State<'_, Arc<SessionCell>>,
    id: String,
    path: PathBuf,
    folder: String,
) -> Result<PathBuf> {
    with_session(cell, move |s| s.move_guide(&id, &path, &folder)).await
}

/// Shows a guide's bundle (or, with `folder` set, that library folder) in
/// Finder.
#[tauri::command]
pub async fn reveal_in_library(
    cell: State<'_, Arc<SessionCell>>,
    path: Option<PathBuf>,
    folder: Option<String>,
) -> Result<()> {
    let target = with_session(cell, move |s| match (path, folder) {
        (Some(path), _) => s.check_bundle(&path).map(|()| path),
        (None, Some(folder)) => Ok(s.library().folder_path(&folder)?),
        (None, None) => Ok(s.root().to_path_buf()),
    })
    .await?;
    blocking(move || {
        tauri_plugin_opener::reveal_item_in_dir(&target).map_err(|e| {
            AppError::new(
                ErrorKind::Io,
                format!("cannot reveal {}: {e}", target.display()),
            )
        })
    })
    .await
}

/// Raw WebP bytes (an `ArrayBuffer` in JS) of an image in an opened
/// guide's bundle, by bundle-relative path.
#[tauri::command]
pub async fn step_image(
    cell: State<'_, Arc<SessionCell>>,
    guide_id: String,
    path: String,
) -> Result<Response> {
    let bytes = with_session(cell, move |s| s.step_image(&guide_id, &path)).await?;
    Ok(Response::new(bytes))
}

/// The cover of a guide in the library, as WebP, for its card: the chosen
/// step drawn as asked, else the first screenshot (empty: none). The guide
/// needn't be open.
#[tauri::command]
pub async fn guide_cover(cell: State<'_, Arc<SessionCell>>, guide_id: String) -> Result<Response> {
    let bytes = with_session(cell, move |s| s.guide_cover(&guide_id)).await?;
    Ok(Response::new(bytes))
}

/// Header naming the guide for [`add_image_step`], whose body is the PNG.
pub const GUIDE_ID_HEADER: &str = "guiderecorder-guide-id";

/// Stores an image as a new manual step's screenshot and returns the step
/// (not yet part of the guide; the editor inserts it). The body is a PNG
/// the webview encoded from the user's file, so any format it can read
/// works.
#[tauri::command]
pub async fn add_image_step(
    cell: State<'_, Arc<SessionCell>>,
    request: Request<'_>,
) -> Result<Step> {
    let guide_id = request
        .headers()
        .get(GUIDE_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .ok_or_else(|| {
            AppError::new(
                ErrorKind::Invalid,
                format!("missing {GUIDE_ID_HEADER} header"),
            )
        })?;
    let png = body_bytes(request.body(), MAX_PNG_BYTES)?;
    with_session(cell, move |s| s.add_image_step(&guide_id, &png)).await
}

/// Asks for images or PDFs to make pages of, in the order the dialog gives
/// them. Empty if the user cancelled. Blocking.
fn choose_pages<R: Runtime>(window: &WebviewWindow<R>) -> Result<Vec<PathBuf>> {
    let Some(paths) = window
        .dialog()
        .file()
        .set_parent(window)
        .set_title("Choose images or a PDF")
        .add_filter("Images or PDF", PAGE_FILE_EXTENSIONS)
        .blocking_pick_files()
    else {
        return Ok(Vec::new());
    };
    paths.into_iter().map(dialog_path).collect()
}

/// Asks for images or PDFs and makes a new annotated guide of them, one
/// page per image and per PDF page (see
/// [`Session::create_annotated_guide`]); returns its id for `open_guide`.
/// `None` if the user cancelled.
#[tauri::command]
pub async fn new_annotated_guide<R: Runtime>(
    window: WebviewWindow<R>,
    cell: State<'_, Arc<SessionCell>>,
) -> Result<Option<String>> {
    let paths = blocking(move || choose_pages(&window)).await?;
    if paths.is_empty() {
        return Ok(None);
    }
    with_session(cell, move |s| s.create_annotated_guide(&paths).map(Some)).await
}

/// Asks for images or PDFs and writes their pages into an open guide's
/// bundle (see [`Session::add_annotated_pages`]); returns the new steps,
/// not yet part of the guide (the editor inserts them, as for
/// `add_image_step`). Empty if the user cancelled.
#[tauri::command]
pub async fn add_annotated_pages<R: Runtime>(
    window: WebviewWindow<R>,
    cell: State<'_, Arc<SessionCell>>,
    guide_id: String,
) -> Result<Vec<Step>> {
    // Refused before the dialog when the guide can't take pages.
    let id = parse_id(&guide_id)?;
    let session = Arc::clone(&cell);
    blocking(move || session.get()?.writable_handle(id).map(drop)).await?;
    let paths = blocking(move || choose_pages(&window)).await?;
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    with_session(cell, move |s| s.add_annotated_pages(&guide_id, &paths)).await
}

/// The bytes of a command's binary body. Over the custom IPC protocol
/// they arrive raw; after Tauri falls back to postMessage they arrive as a
/// JSON array of numbers (or, from a plain `JSON.stringify`, an object with
/// index keys).
///
/// Bodies longer than `max` bytes are refused before any copy is made.
fn body_bytes(body: &InvokeBody, max: usize) -> Result<Vec<u8>> {
    let invalid = || AppError::new(ErrorKind::Invalid, "expected image bytes");
    let len = match body {
        InvokeBody::Raw(bytes) => bytes.len(),
        InvokeBody::Json(serde_json::Value::Array(items)) => items.len(),
        InvokeBody::Json(serde_json::Value::Object(map)) => map.len(),
        InvokeBody::Json(_) => 0,
    };
    if len > max {
        return Err(AppError::new(
            ErrorKind::Invalid,
            format!("the image is too large ({} MB)", len / 1024 / 1024),
        ));
    }
    let byte = |value: &serde_json::Value| {
        value
            .as_u64()
            .and_then(|n| u8::try_from(n).ok())
            .ok_or_else(invalid)
    };
    match body {
        InvokeBody::Raw(bytes) => Ok(bytes.clone()),
        InvokeBody::Json(serde_json::Value::Array(items)) => items.iter().map(byte).collect(),
        InvokeBody::Json(serde_json::Value::Object(map)) => (0..map.len())
            .map(|i| map.get(&i.to_string()).ok_or_else(invalid).and_then(byte))
            .collect(),
        InvokeBody::Json(_) => Err(invalid()),
    }
}

/// Copies a step's image files for a new step (see
/// [`Session::copy_step_images`]); returns the new step's image.
#[tauri::command]
pub async fn copy_step_images(
    cell: State<'_, Arc<SessionCell>>,
    guide_id: String,
    image: StepImage,
    new_step_id: String,
) -> Result<StepImage> {
    with_session(cell, move |s| {
        s.copy_step_images(&guide_id, &image, &new_step_id)
    })
    .await
}

/// The bytes of an image file the user dropped on the window (Tauri gives
/// the webview paths, not files), for the webview to decode. Only paths
/// from a recent drop, each once (see [`DroppedFiles`]).
#[tauri::command]
pub async fn read_image_file(
    dropped: State<'_, Arc<DroppedFiles>>,
    path: String,
) -> Result<Response> {
    let dropped = Arc::clone(&dropped);
    let bytes = tauri::async_runtime::spawn_blocking(move || dropped.read(path.as_ref()))
        .await
        .map_err(|e| AppError::new(ErrorKind::Io, format!("task failed: {e}")))??;
    Ok(Response::new(bytes))
}

/// Starts a dialog in `directory` when it is an existing folder (only where
/// the dialog opens; the user still picks).
fn start_in<R: Runtime>(
    dialog: FileDialogBuilder<R>,
    directory: Option<PathBuf>,
) -> FileDialogBuilder<R> {
    match directory.filter(|d| d.is_dir()) {
        Some(directory) => dialog.set_directory(directory),
        None => dialog,
    }
}

fn dialog_path(path: FilePath) -> Result<PathBuf> {
    path.into_path()
        .map_err(|e| AppError::new(ErrorKind::Invalid, format!("not a file path: {e}")))
}

/// Asks for the folder a Markdown or HTML export goes into (as a new
/// folder inside it). `None` if the user cancelled.
#[tauri::command]
pub async fn choose_export_folder<R: Runtime>(
    window: WebviewWindow<R>,
    chosen: State<'_, Arc<ChosenPaths>>,
    directory: Option<PathBuf>,
) -> Result<Option<PathBuf>> {
    let chosen = Arc::clone(&chosen);
    blocking(move || {
        let Some(path) = start_in(window.dialog().file(), directory)
            .set_parent(&window)
            .set_title("Export into folder")
            .blocking_pick_folder()
        else {
            return Ok(None);
        };
        let path = dialog_path(path)?;
        chosen.add_folder(path.clone());
        Ok(Some(path))
    })
    .await
}

/// Asks where a DOCX or PDF export is saved, suggesting `file_name`.
/// `None` if the user cancelled.
#[tauri::command]
pub async fn choose_export_file<R: Runtime>(
    window: WebviewWindow<R>,
    chosen: State<'_, Arc<ChosenPaths>>,
    format: Format,
    file_name: String,
    directory: Option<PathBuf>,
) -> Result<Option<PathBuf>> {
    let extension = format.extension().ok_or_else(|| {
        AppError::new(
            ErrorKind::Invalid,
            "Markdown, HTML and skills export into a folder",
        )
    })?;
    let chosen = Arc::clone(&chosen);
    blocking(move || {
        let label = extension.to_ascii_uppercase();
        let Some(path) = start_in(window.dialog().file(), directory)
            .set_parent(&window)
            .set_title(format!("Export as {label}"))
            .set_file_name(with_extension(PathBuf::from(file_name), extension).to_string_lossy())
            .add_filter(label, &[extension])
            .blocking_save_file()
        else {
            return Ok(None);
        };
        let path = with_extension(dialog_path(path)?, extension);
        chosen.add_file(path.clone());
        Ok(Some(path))
    })
    .await
}

/// Asks for a logo image (PNG or JPEG). `None` if the user cancelled.
#[tauri::command]
pub async fn choose_logo<R: Runtime>(
    window: WebviewWindow<R>,
    chosen: State<'_, Arc<ChosenPaths>>,
) -> Result<Option<PathBuf>> {
    let chosen = Arc::clone(&chosen);
    blocking(move || {
        let Some(path) = window
            .dialog()
            .file()
            .set_parent(&window)
            .set_title("Choose a logo")
            .add_filter("PNG or JPEG image", &["png", "jpg", "jpeg"])
            .blocking_pick_file()
        else {
            return Ok(None);
        };
        let path = dialog_path(path)?;
        chosen.add_logo(path.clone());
        Ok(Some(path))
    })
    .await
}

/// Exports a guide from its saved bundle (see [`export::export_guide`]).
/// The editor saves its changes first.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn export_guide(
    cell: State<'_, Arc<SessionCell>>,
    chosen: State<'_, Arc<ChosenPaths>>,
    guide_id: String,
    format: Format,
    out: PathBuf,
    overwrite: bool,
    branding: BrandingRequest,
    paper: Paper,
    review_details: bool,
    motion: Option<MotionRequest>,
    skill: Option<SkillRequest>,
    scorm: Option<ScormRequest>,
) -> Result<ExportResult> {
    let chosen = Arc::clone(&chosen);
    let motion = motion.unwrap_or_default();
    let skill = skill.unwrap_or_default();
    let scorm = scorm.unwrap_or_default();
    with_session(cell, move |s| {
        export::export_guide(
            s,
            &chosen,
            &guide_id,
            format,
            &out,
            overwrite,
            &branding,
            paper,
            review_details,
            &motion,
            &skill,
            &scorm,
        )
    })
    .await
}

/// A preview of exporting a guide from its saved bundle, writing nothing
/// (see [`export::preview_guide`]).
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn preview_export(
    cell: State<'_, Arc<SessionCell>>,
    chosen: State<'_, Arc<ChosenPaths>>,
    guide_id: String,
    format: Format,
    branding: BrandingRequest,
    paper: Paper,
    review_details: bool,
    motion: Option<MotionRequest>,
    skill: Option<SkillRequest>,
    scorm: Option<ScormRequest>,
) -> Result<ExportPreview> {
    let chosen = Arc::clone(&chosen);
    let motion = motion.unwrap_or_default();
    let skill = skill.unwrap_or_default();
    let scorm = scorm.unwrap_or_default();
    with_session(cell, move |s| {
        export::preview_guide(
            s,
            &chosen,
            &guide_id,
            format,
            &branding,
            paper,
            review_details,
            &motion,
            &skill,
            &scorm,
        )
    })
    .await
}

/// The chosen logo as a PNG data URI, for the Export dialog (see
/// [`export::logo_preview`]).
#[tauri::command]
pub async fn logo_preview(chosen: State<'_, Arc<ChosenPaths>>, path: PathBuf) -> Result<String> {
    let chosen = Arc::clone(&chosen);
    blocking(move || export::logo_preview(&chosen, &path)).await
}

/// Shows a file an export wrote in Finder.
#[tauri::command]
pub async fn reveal_export(chosen: State<'_, Arc<ChosenPaths>>, path: PathBuf) -> Result<()> {
    chosen.check_exported(&path)?;
    blocking(move || {
        tauri_plugin_opener::reveal_item_in_dir(&path).map_err(|e| {
            AppError::new(
                ErrorKind::Io,
                format!("cannot reveal {}: {e}", path.display()),
            )
        })
    })
    .await
}

/// Whether a step's `cropRect` places its crop exactly (see
/// [`export::verify_crop`]); the annotation layer shows the crop view only
/// then.
#[tauri::command]
pub async fn verify_crop(
    cell: State<'_, Arc<SessionCell>>,
    guide_id: String,
    image: StepImage,
) -> Result<bool> {
    with_session(cell, move |s| export::verify_crop(s, &guide_id, &image)).await
}

/// Emitted on a user quit (Cmd+Q, Dock Quit, logout, last window closed);
/// see `QuitGate`.
pub const QUIT_REQUESTED_EVENT: &str = "guiderecorder://quit-requested";

/// How long the frontend has to answer [`QUIT_REQUESTED_EVENT`] before
/// the app quits anyway (e.g. a hung or unloaded webview).
const QUIT_ACK_TIMEOUT: Duration = Duration::from_secs(3);

/// How long a `terminate:` (Dock Quit, logout) is held for the frontend to
/// save and quit. After that the app stays open and a logout is cancelled,
/// e.g. while the leave dialog waits for the user; a later `quit_app`
/// still quits. A user who chooses to stay answers it at once
/// (`quit_cancel`).
#[cfg(target_os = "macos")]
const QUIT_REPLY_TIMEOUT: Duration = Duration::from_secs(30);

/// What the quit gate does to the app: [`AppHandle`] in the app, a fake in
/// the tests.
trait QuitHost {
    /// Sends [`QUIT_REQUESTED_EVENT`]; false if it could not be sent.
    fn request_save(&self) -> bool;
    /// Drops a recording still running; `exit` would skip its destructor.
    fn discard_recording(&self);
    /// Answers the held `terminate:`; false if the reply could not be sent.
    fn reply(&self, quit: bool) -> bool;
    fn exit(&self);
}

impl<R: Runtime> QuitHost for AppHandle<R> {
    fn request_save(&self) -> bool {
        self.emit(QUIT_REQUESTED_EVENT, ()).is_ok()
    }

    fn discard_recording(&self) {
        crate::recording::discard_on_quit(self);
        crate::ai::cancel_on_quit(self);
    }

    fn reply(&self, quit: bool) -> bool {
        crate::terminate::reply(self, quit)
    }

    fn exit(&self) {
        AppHandle::exit(self, 0);
    }
}

/// Holds a user quit until the editor has saved: the frontend acks the
/// event, saves (or asks the user), then calls `quit_app`.
#[derive(Default)]
pub struct QuitGate {
    allowed: AtomicBool,
    acked: AtomicBool,
    /// A `terminate:` held with `NSTerminateLater`; see `terminate`.
    pending: crate::terminate::PendingReply,
    /// Set by [`QuitGate::restart`]: the app starts again once it has quit.
    restart: AtomicBool,
}

impl QuitGate {
    /// Quits like Cmd+Q (the editor saves, a recording is stopped and
    /// saved), then starts the app again: an installed update, or a
    /// Screen Recording grant macOS only applies at launch. A cancelled
    /// quit cancels the restart too.
    pub fn restart<R: Runtime>(self: &Arc<Self>, app: &AppHandle<R>) {
        self.restart.store(true, Ordering::SeqCst);
        self.on_quit_menu(app);
    }

    /// Whether the app should start again as it exits.
    pub fn restart_requested(&self) -> bool {
        self.restart.load(Ordering::SeqCst)
    }

    /// Handles `RunEvent::ExitRequested`. Returns true to prevent the exit.
    pub fn on_exit_requested<R: Runtime>(
        self: &Arc<Self>,
        app: &AppHandle<R>,
        code: Option<i32>,
    ) -> bool {
        if !self.request(app, code) {
            return false;
        }
        self.watch_ack(app);
        true
    }

    /// Handles the app menu's Quit item (Cmd+Q) as a user quit. The
    /// default item sends NSApp `terminate:`, which would reach
    /// `on_terminate` and hold the app in AppKit's terminate wait, so the
    /// menu routes here instead.
    pub fn on_quit_menu<R: Runtime>(self: &Arc<Self>, app: &AppHandle<R>) {
        if !self.on_exit_requested(app, None) {
            self.quit(app);
        }
    }

    /// Handles `applicationShouldTerminate:` (Dock Quit, AppleScript
    /// `quit`, logout, shutdown) as a user quit: AppKit holds the
    /// `terminate:` until [`QuitGate::quit`] or the reply deadline answers.
    #[cfg(target_os = "macos")]
    pub fn on_terminate<R: Runtime>(
        self: &Arc<Self>,
        app: &AppHandle<R>,
    ) -> crate::terminate::TerminateReply {
        let (reply, held) = self.terminate(app);
        if let Some(id) = held {
            self.watch_ack(app);
            let gate = Arc::clone(self);
            let app = app.clone();
            std::thread::spawn(move || {
                std::thread::sleep(QUIT_REPLY_TIMEOUT);
                gate.deadline(&app, id);
            });
        }
        reply
    }

    /// Asks the frontend to save, unless this exit may pass. Returns true
    /// if it was asked.
    fn request(&self, host: &impl QuitHost, code: Option<i32>) -> bool {
        // `code` is set for `AppHandle::exit`, i.e. `quit_app` or a restart.
        if code.is_some() || self.allowed.load(Ordering::SeqCst) {
            return false;
        }
        self.acked.store(false, Ordering::SeqCst);
        host.request_save()
    }

    /// Quits anyway if the frontend does not ack in time.
    fn watch_ack<R: Runtime>(self: &Arc<Self>, app: &AppHandle<R>) {
        let gate = Arc::clone(self);
        let app = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(QUIT_ACK_TIMEOUT);
            if !gate.acked.load(Ordering::SeqCst) {
                eprintln!("guiderecorder: no answer to the quit request, quitting");
                gate.quit(&app);
            }
        });
    }

    /// The decision for `applicationShouldTerminate:`, and the id of the
    /// `terminate:` it holds, if a new one. A `terminate:` arriving while
    /// one is held joins it: AppKit keeps a single terminate wait (a test
    /// app showed it never asks the delegate again while waiting), so the
    /// one reply owed to the held request answers it.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    fn terminate(&self, host: &impl QuitHost) -> (crate::terminate::TerminateReply, Option<u64>) {
        use crate::terminate::TerminateReply;
        if self.allowed.load(Ordering::SeqCst) {
            return (TerminateReply::Now, None);
        }
        // Held before the event goes out, so a `quit_app` always finds it.
        let Some(id) = self.pending.begin() else {
            return (TerminateReply::Later, None);
        };
        if !self.request(host, None) {
            self.pending.take();
            self.allowed.store(true, Ordering::SeqCst);
            host.discard_recording();
            return (TerminateReply::Now, None);
        }
        (TerminateReply::Later, Some(id))
    }

    /// The held `terminate:` `id` has waited [`QUIT_REPLY_TIMEOUT`]: if it
    /// is still held, the app stays open (a logout is cancelled).
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    fn deadline(&self, host: &impl QuitHost, id: u64) {
        if self.pending.take_if(id) {
            eprintln!("guiderecorder: the quit did not finish in time; staying open");
            host.reply(false);
        }
    }

    /// The user chose to stay: a held `terminate:` is cancelled now (a
    /// logout too) rather than at the deadline. Nothing else to undo: a
    /// later quit asks the frontend again.
    fn cancel(&self, host: &impl QuitHost) {
        if self.pending.take() {
            host.reply(false);
        }
    }

    /// Quits now: lets a held `terminate:` go on, or exits. A recording
    /// still running is discarded first.
    fn quit(&self, host: &impl QuitHost) {
        self.allowed.store(true, Ordering::SeqCst);
        host.discard_recording();
        // The reply only fails to reach the main thread once the event
        // loop is gone; `exit` then cleans up and ends the process itself,
        // so AppKit is not left waiting in a live app.
        if !(self.pending.take() && host.reply(true)) {
            host.exit();
        }
    }
}

#[tauri::command]
pub fn quit_ack(gate: State<'_, Arc<QuitGate>>) {
    gate.acked.store(true, Ordering::SeqCst);
}

/// Exits the app; the frontend calls this once its changes are saved (and
/// a recording stopped).
#[tauri::command]
pub fn quit_app<R: Runtime>(app: AppHandle<R>, gate: State<'_, Arc<QuitGate>>) {
    gate.quit(&app);
}

/// The frontend's quit stays open (the user chose to stay, or a recording
/// could not be stopped): a held Dock Quit or logout is cancelled now.
#[tauri::command]
pub fn quit_cancel<R: Runtime>(app: AppHandle<R>, gate: State<'_, Arc<QuitGate>>) {
    gate.restart.store(false, Ordering::SeqCst);
    gate.cancel(&app);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminate::TerminateReply;
    use serde_json::json;
    use std::sync::Mutex;

    /// Records what the gate does; `emit_ok` decides whether the event
    /// reaches the frontend.
    struct FakeHost {
        emit_ok: bool,
        calls: Mutex<Vec<String>>,
    }

    impl FakeHost {
        fn new(emit_ok: bool) -> Self {
            Self {
                emit_ok,
                calls: Mutex::default(),
            }
        }

        fn calls(&self) -> Vec<String> {
            std::mem::take(&mut self.calls.lock().unwrap())
        }

        fn log(&self, call: &str) {
            self.calls.lock().unwrap().push(call.to_owned());
        }
    }

    impl QuitHost for FakeHost {
        fn request_save(&self) -> bool {
            self.log("request_save");
            self.emit_ok
        }

        fn discard_recording(&self) {
            self.log("discard");
        }

        fn reply(&self, quit: bool) -> bool {
            self.log(&format!("reply {quit}"));
            true
        }

        fn exit(&self) {
            self.log("exit");
        }
    }

    #[test]
    fn terminate_passes_once_quitting_is_allowed() {
        let gate = QuitGate::default();
        let host = FakeHost::new(true);
        gate.quit(&host);
        assert_eq!(host.calls(), ["discard", "exit"]);
        assert_eq!(gate.terminate(&host), (TerminateReply::Now, None));
        assert!(host.calls().is_empty(), "no second save request");
    }

    #[test]
    fn terminate_quits_now_when_the_frontend_cannot_be_asked() {
        let gate = QuitGate::default();
        let host = FakeHost::new(false);
        assert_eq!(gate.terminate(&host), (TerminateReply::Now, None));
        assert_eq!(host.calls(), ["request_save", "discard"]);
        assert!(!gate.pending.take(), "nothing left held");
    }

    #[test]
    fn held_terminate_is_answered_by_the_quit() {
        let gate = QuitGate::default();
        let host = FakeHost::new(true);
        let (reply, held) = gate.terminate(&host);
        assert_eq!(reply, TerminateReply::Later);
        let id = held.expect("a new hold");
        assert_eq!(host.calls(), ["request_save"]);
        gate.quit(&host);
        assert_eq!(
            host.calls(),
            ["discard", "reply true"],
            "no exit: AppKit quits"
        );
        gate.deadline(&host, id);
        assert!(
            host.calls().is_empty(),
            "the deadline has nothing to answer"
        );
    }

    #[test]
    fn held_terminate_is_cancelled_at_the_deadline() {
        let gate = QuitGate::default();
        let host = FakeHost::new(true);
        let (_, held) = gate.terminate(&host);
        host.calls();
        gate.deadline(&host, held.unwrap());
        assert_eq!(host.calls(), ["reply false"]);
        // The frontend can still quit later, now through `exit`.
        gate.quit(&host);
        assert_eq!(host.calls(), ["discard", "exit"]);
    }

    #[test]
    fn terminate_while_held_joins_the_held_one() {
        let gate = QuitGate::default();
        let host = FakeHost::new(true);
        let (_, first) = gate.terminate(&host);
        host.calls();
        assert_eq!(gate.terminate(&host), (TerminateReply::Later, None));
        assert!(host.calls().is_empty(), "no second request or hold");
        gate.quit(&host);
        assert_eq!(host.calls(), ["discard", "reply true"], "one reply");
        gate.deadline(&host, first.unwrap());
        assert!(host.calls().is_empty());
    }

    #[test]
    fn staying_cancels_the_held_terminate_at_once() {
        let gate = QuitGate::default();
        let host = FakeHost::new(true);
        let (_, held) = gate.terminate(&host);
        host.calls();
        gate.cancel(&host);
        assert_eq!(host.calls(), ["reply false"]);
        gate.deadline(&host, held.unwrap());
        gate.cancel(&host);
        assert!(host.calls().is_empty(), "answered once");
        // The next Dock Quit asks the frontend again.
        let (reply, held) = gate.terminate(&host);
        assert_eq!(reply, TerminateReply::Later);
        assert!(held.is_some());
        assert_eq!(host.calls(), ["request_save"]);
    }

    #[test]
    fn staying_without_a_held_terminate_does_nothing() {
        let gate = QuitGate::default();
        let host = FakeHost::new(true);
        // Cmd+Q or the window's close button: nothing held.
        assert!(gate.request(&host, None));
        host.calls();
        gate.cancel(&host);
        assert!(host.calls().is_empty());
        gate.quit(&host);
        assert_eq!(host.calls(), ["discard", "exit"], "and still quits later");
    }

    #[test]
    fn body_bytes_from_raw_and_postmessage_bodies() {
        let png = vec![0x89, b'P', b'N', b'G', 0, 255];
        assert_eq!(body_bytes(&InvokeBody::Raw(png.clone()), 100).unwrap(), png);
        assert_eq!(
            body_bytes(&InvokeBody::Json(json!([137, 80, 78, 71, 0, 255])), 100).unwrap(),
            png
        );
        assert_eq!(
            body_bytes(&InvokeBody::Json(json!({"1": 80, "0": 137})), 100).unwrap(),
            vec![137, 80]
        );
        for bad in [
            json!([256]),
            json!([-1]),
            json!(["1"]),
            json!({"0": 1, "2": 2}),
            json!({"guideId": "x"}),
            json!(null),
        ] {
            let err = body_bytes(&InvokeBody::Json(bad.clone()), 100).unwrap_err();
            assert_eq!(err.kind, ErrorKind::Invalid, "{bad}");
        }
        // Too long: refused on the length alone.
        for body in [
            InvokeBody::Raw(vec![0; 101]),
            InvokeBody::Json(json!(vec![0; 101])),
        ] {
            let err = body_bytes(&body, 100).unwrap_err();
            assert!(err.message.contains("too large"), "{}", err.message);
        }
    }
}
