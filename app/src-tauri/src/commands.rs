//! Tauri commands over the editor [`Session`]. Library IO runs on the
//! blocking pool, never on the main thread.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use guidesya_core::model::Guide;
use tauri::ipc::Response;
use tauri::{AppHandle, Emitter, Runtime, State};

use crate::editor::{
    AppError, ErrorKind, GuideListItem, ImageVariant, OpenedGuide, Result, Session, SessionCell,
};

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

/// Raw WebP bytes (an `ArrayBuffer` in JS).
#[tauri::command]
pub async fn step_image(
    cell: State<'_, Arc<SessionCell>>,
    guide_id: String,
    step_id: String,
    variant: ImageVariant,
) -> Result<Response> {
    let bytes = with_session(cell, move |s| s.step_image(&guide_id, &step_id, variant)).await?;
    Ok(Response::new(bytes))
}

/// Emitted on a user quit (Cmd+Q, last window closed); see `QuitGate`.
pub const QUIT_REQUESTED_EVENT: &str = "guidesya://quit-requested";

/// How long the frontend has to answer [`QUIT_REQUESTED_EVENT`] before
/// the app quits anyway (e.g. a hung or unloaded webview).
const QUIT_ACK_TIMEOUT: Duration = Duration::from_secs(3);

/// Holds a user quit until the editor has saved: the frontend acks the
/// event, saves (or asks the user), then calls `quit_app`.
#[derive(Default)]
pub struct QuitGate {
    allowed: AtomicBool,
    acked: AtomicBool,
}

impl QuitGate {
    /// Handles `RunEvent::ExitRequested`. Returns true to prevent the exit.
    pub fn on_exit_requested<R: Runtime>(
        self: &Arc<Self>,
        app: &AppHandle<R>,
        code: Option<i32>,
    ) -> bool {
        // `code` is set for `AppHandle::exit`, i.e. `quit_app` or a restart.
        if code.is_some() || self.allowed.load(Ordering::SeqCst) {
            return false;
        }
        self.acked.store(false, Ordering::SeqCst);
        if app.emit(QUIT_REQUESTED_EVENT, ()).is_err() {
            return false;
        }
        let gate = Arc::clone(self);
        let app = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(QUIT_ACK_TIMEOUT);
            if !gate.acked.load(Ordering::SeqCst) {
                eprintln!("guidesya: no answer to the quit request, quitting");
                gate.allowed.store(true, Ordering::SeqCst);
                app.exit(0);
            }
        });
        true
    }
}

#[tauri::command]
pub fn quit_ack(gate: State<'_, Arc<QuitGate>>) {
    gate.acked.store(true, Ordering::SeqCst);
}

/// Exits the app; the frontend calls this once its changes are saved.
#[tauri::command]
pub fn quit_app<R: Runtime>(app: AppHandle<R>, gate: State<'_, Arc<QuitGate>>) {
    gate.allowed.store(true, Ordering::SeqCst);
    app.exit(0);
}
