//! The floating recording toolbar (`toolbar.html`, window label
//! [`LABEL`]): open while a recording runs or is paused, closed once the
//! status is idle. Kept in line with every status sent (see
//! `recording::emit`) and once at startup.
//!
//! It is a window of this app, so a recording's own-window exclusion
//! (`exclude_own_windows`, which leaves out the whole app) keeps it out of
//! every frame.
//!
//! Known limitation: clicking the toolbar still activates GuideRecorder, taking
//! the active app away from the one being recorded until the user clicks
//! back. `focusable(false)` only stops the toolbar becoming the key or main
//! window (tao's `TaoWindow` answers `canBecomeKeyWindow` with NO); it is
//! still a plain `NSWindow`, and only an `NSPanel` with the non-activating
//! style mask takes clicks without activating its app. A manual step is
//! aimed at no app (pid 0), so the activation does not misattribute it.

use std::sync::Arc;

use tauri::{AppHandle, Manager, Runtime, WebviewUrl, WebviewWindowBuilder};

use crate::recording::{Recorder, RecordingStatus};

pub const LABEL: &str = "toolbar";
const URL: &str = "toolbar.html";
/// Logical size.
const WIDTH: f64 = 360.0;
const HEIGHT: f64 = 44.0;
/// Logical gap between the top of the work area (below the menu bar) and
/// the toolbar.
const TOP_MARGIN: f64 = 8.0;

/// What the toolbar should be for `status`: `Some(true)` open, `Some(false)`
/// closed, `None` left as it is (saving: it closes at idle).
pub fn wanted(status: &RecordingStatus) -> Option<bool> {
    match status {
        RecordingStatus::Recording { .. } | RecordingStatus::Paused { .. } => Some(true),
        RecordingStatus::Idle => Some(false),
        RecordingStatus::Saving { .. } => None,
    }
}

/// The toolbar's logical top-left: centred horizontally in the work area
/// (physical `x`, `y`, `width`, at `scale`), just below its top.
pub fn top_centre(x: i32, y: i32, width: u32, scale: f64) -> (f64, f64) {
    let scale = if scale > 0.0 { scale } else { 1.0 };
    let left = f64::from(x) / scale + (f64::from(width) / scale - WIDTH) / 2.0;
    (left.round(), (f64::from(y) / scale + TOP_MARGIN).round())
}

/// Opens or closes the toolbar for the stored status, on the main thread
/// (never blocking the caller, which may hold the recorder's locks).
pub fn sync<R: Runtime>(app: &AppHandle<R>) {
    let handle = app.clone();
    if let Err(e) = app.run_on_main_thread(move || apply(&handle)) {
        eprintln!("guiderecorder: cannot update the toolbar: {e}");
    }
}

fn apply<R: Runtime>(app: &AppHandle<R>) {
    let Some(recorder) = app.try_state::<Arc<Recorder>>() else {
        return;
    };
    let existing = app.get_webview_window(LABEL);
    match (wanted(&recorder.status().status), existing) {
        (Some(true), None) => {
            if let Err(e) = open(app) {
                eprintln!("guiderecorder: cannot open the toolbar: {e}");
            }
        }
        (Some(false), Some(window)) => {
            if let Err(e) = window.destroy() {
                eprintln!("guiderecorder: cannot close the toolbar: {e}");
            }
        }
        _ => {}
    }
}

fn open<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let mut builder = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App(URL.into()))
        .title("GuideRecorder Recording")
        .inner_size(WIDTH, HEIGHT)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .skip_taskbar(true)
        .visible_on_all_workspaces(true)
        // Don't take focus from the app being recorded: not focused when
        // created, and never the key or main window (see the module docs
        // for what that does not cover). A click on it while inactive
        // still lands on its buttons.
        .focused(false)
        .focusable(false)
        .accept_first_mouse(true);
    if let Some(monitor) = app.primary_monitor()? {
        let area = monitor.work_area();
        let (x, y) = top_centre(
            area.position.x,
            area.position.y,
            area.size.width,
            monitor.scale_factor(),
        );
        builder = builder.position(x, y);
    }
    builder.build().map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_while_recording_or_paused_closed_when_idle() {
        assert_eq!(wanted(&RecordingStatus::Idle), Some(false));
        let recording = RecordingStatus::Recording {
            started_at: "2026-10-15T01:00:00.000Z".into(),
            title: None,
            capture_area_fallback: false,
            paused_ms: 0,
            excluded_app: None,
            capture_lost: None,
        };
        assert_eq!(wanted(&recording), Some(true));
        let paused = RecordingStatus::Paused {
            started_at: "2026-10-15T01:00:00.000Z".into(),
            title: None,
            capture_area_fallback: false,
            paused_ms: 0,
            paused_at: "2026-10-15T01:01:00.000Z".into(),
            excluded_app: None,
            capture_lost: None,
        };
        assert_eq!(wanted(&paused), Some(true));
        let saving = RecordingStatus::Saving {
            started_at: "2026-10-15T01:00:00.000Z".into(),
            title: None,
        };
        assert_eq!(wanted(&saving), None);
    }

    #[test]
    fn toolbar_sits_top_centre_of_the_work_area() {
        // A 1512×949-point Retina work area below a 33-point menu bar.
        assert_eq!(top_centre(0, 66, 3024, 2.0), (576.0, 41.0));
        // A display left of the main one, at 1×.
        assert_eq!(top_centre(-1920, 25, 1920, 1.0), (-1140.0, 33.0));
        assert_eq!(top_centre(0, 0, 360, 0.0), (0.0, 8.0));
    }
}
