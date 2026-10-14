//! Tauri commands over the editor [`Session`]. Library IO runs on the
//! blocking pool, never on the main thread.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use guidesya_core::model::{Guide, Step};
use tauri::ipc::{InvokeBody, Request, Response};
use tauri::{AppHandle, Emitter, Runtime, State};

use crate::editor::{
    AppError, DroppedFiles, ErrorKind, GuideListItem, OpenedGuide, Result, Session, SessionCell,
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

/// Saves nothing: the editor saves first. Forgets the guide and deletes
/// step images its saved `guide.json` no longer uses.
#[tauri::command]
pub async fn close_guide(cell: State<'_, Arc<SessionCell>>, id: String) -> Result<Vec<String>> {
    with_session(cell, move |s| s.close_guide(&id)).await
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

/// Header naming the guide for [`add_image_step`], whose body is the PNG.
pub const GUIDE_ID_HEADER: &str = "guidesya-guide-id";

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
    let png = body_bytes(request.body())?;
    with_session(cell, move |s| s.add_image_step(&guide_id, &png)).await
}

/// The bytes of a command's binary body. Over the custom IPC protocol
/// they arrive raw; after Tauri falls back to postMessage they arrive as a
/// JSON array of numbers (or, from a plain `JSON.stringify`, an object with
/// index keys).
fn body_bytes(body: &InvokeBody) -> Result<Vec<u8>> {
    let invalid = || AppError::new(ErrorKind::Invalid, "expected image bytes");
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn body_bytes_from_raw_and_postmessage_bodies() {
        let png = vec![0x89, b'P', b'N', b'G', 0, 255];
        assert_eq!(body_bytes(&InvokeBody::Raw(png.clone())).unwrap(), png);
        assert_eq!(
            body_bytes(&InvokeBody::Json(json!([137, 80, 78, 71, 0, 255]))).unwrap(),
            png
        );
        assert_eq!(
            body_bytes(&InvokeBody::Json(json!({"1": 80, "0": 137}))).unwrap(),
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
            let err = body_bytes(&InvokeBody::Json(bad.clone())).unwrap_err();
            assert_eq!(err.kind, ErrorKind::Invalid, "{bad}");
        }
    }
}
