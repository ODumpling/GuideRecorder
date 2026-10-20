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
//!
//! The window is the bar ([`HEIGHT`] tall) until the live feed panel opens
//! under it: the webview then asks for the height it needs
//! ([`resize_toolbar`], at most [`MAX_HEIGHT`]), and shrinks it back once
//! the panel has faded out. It stays out of the way: a captured click near
//! it moves it to another top corner of its display ([`dodge`]); a click
//! on the toolbar itself is ours and never captured.
//!
//! With `recording.clickRing` on, each captured click also shows a brief
//! ring where it landed: a click-through window ([`RING_LABEL`],
//! `ring.html`), left out of every frame like the toolbar.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use guiderecorder_core::model::Point;
use tauri::{
    AppHandle, LogicalPosition, LogicalSize, Manager, Runtime, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};

use crate::editor::{AppError, ErrorKind, Result};
use crate::recording::{Recorder, RecordingStatus};

pub const LABEL: &str = "toolbar";
const URL: &str = "toolbar.html";
/// Logical size of the bar.
const WIDTH: f64 = 360.0;
const HEIGHT: f64 = 44.0;
/// Tallest the window grows with the feed panel open.
pub const MAX_HEIGHT: f64 = 520.0;
/// Logical gap between the top of the work area (below the menu bar) and
/// the toolbar.
const TOP_MARGIN: f64 = 8.0;
/// Logical gap between a side of the work area and the toolbar in a
/// corner.
const SIDE_MARGIN: f64 = 12.0;
/// A captured click this close to the toolbar (logical points) moves it.
const DODGE_MARGIN: f64 = 48.0;

/// The click ring window.
pub const RING_LABEL: &str = "click-ring";
const RING_URL: &str = "ring.html";
/// Logical size of the ring window.
const RING_SIZE: f64 = 48.0;
/// How long a ring shows (its animation is as long, see `ring.html`).
const RING_MS: u64 = 600;
/// Counts rings shown, so only the newest one's timer hides the window.
static RING_SHOWN: AtomicU64 = AtomicU64::new(0);

/// A logical rectangle: the toolbar, or a display's work area.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Area {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Area {
    fn contains(&self, p: Point, margin: f64) -> bool {
        p.x >= self.x - margin
            && p.x <= self.x + self.width + margin
            && p.y >= self.y - margin
            && p.y <= self.y + self.height + margin
    }

    fn centre(&self) -> Point {
        Point {
            x: self.x + self.width / 2.0,
            y: self.y + self.height / 2.0,
        }
    }
}

/// The places the toolbar sits in `area` (logical): top centre, top right,
/// top left; always at the top, so the feed panel opens below it.
pub fn anchors(area: Area, width: f64) -> [(f64, f64); 3] {
    let top = (area.y + TOP_MARGIN).round();
    [
        ((area.x + (area.width - width) / 2.0).round(), top),
        ((area.x + area.width - width - SIDE_MARGIN).round(), top),
        ((area.x + SIDE_MARGIN).round(), top),
    ]
}

/// Where the toolbar `bar` should move after a click at `click` in the
/// work area `area` (all logical): the anchor farthest from the click whose
/// bar it is not near, or `None` when the click was not near the toolbar
/// (or nowhere is better).
pub fn dodge(click: Point, bar: Area, area: Area) -> Option<(f64, f64)> {
    if !bar.contains(click, DODGE_MARGIN) {
        return None;
    }
    let distance = |a: &Area| {
        let c = a.centre();
        (c.x - click.x).hypot(c.y - click.y)
    };
    anchors(area, bar.width)
        .into_iter()
        .map(|(x, y)| Area {
            x,
            y,
            width: bar.width,
            height: bar.height,
        })
        .filter(|a| !a.contains(click, DODGE_MARGIN) && (a.x, a.y) != (bar.x, bar.y))
        .max_by(|a, b| distance(a).total_cmp(&distance(b)))
        .map(|a| (a.x, a.y))
}

/// The window height for a panel that needs `height`: from the bar alone
/// up to [`MAX_HEIGHT`].
pub fn clamp_height(height: f64) -> f64 {
    if height.is_finite() {
        height.clamp(HEIGHT, MAX_HEIGHT)
    } else {
        HEIGHT
    }
}

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
            if let Some(ring) = app.get_webview_window(RING_LABEL) {
                let _ = ring.destroy();
            }
        }
        _ => {}
    }
}

/// A captured click at `point` (global display points): moves the toolbar
/// away if it is near ([`dodge`]) and shows the click ring when `ring` is
/// on. Runs on the main thread; never blocks the caller (the session's
/// element thread).
pub fn on_click<R: Runtime>(app: &AppHandle<R>, point: Point, ring: bool) {
    let handle = app.clone();
    let result = app.run_on_main_thread(move || {
        if let Some(window) = handle.get_webview_window(LABEL) {
            if let Err(e) = move_away(&window, point) {
                eprintln!("guiderecorder: cannot move the toolbar: {e}");
            }
        }
        if ring {
            if let Err(e) = show_ring(&handle, point) {
                eprintln!("guiderecorder: cannot show the click ring: {e}");
            }
        }
    });
    if let Err(e) = result {
        eprintln!("guiderecorder: cannot handle a click: {e}");
    }
}

fn move_away<R: Runtime>(window: &WebviewWindow<R>, click: Point) -> tauri::Result<()> {
    let Some(monitor) = window.current_monitor()? else {
        return Ok(());
    };
    let scale = window.scale_factor()?;
    let scale = if scale > 0.0 { scale } else { 1.0 };
    let position = window.outer_position()?;
    let size = window.outer_size()?;
    let bar = Area {
        x: f64::from(position.x) / scale,
        y: f64::from(position.y) / scale,
        width: f64::from(size.width) / scale,
        // Only the bar counts: the panel under it comes and goes.
        height: HEIGHT,
    };
    let work = monitor.work_area();
    let monitor_scale = monitor.scale_factor();
    let monitor_scale = if monitor_scale > 0.0 {
        monitor_scale
    } else {
        1.0
    };
    let area = Area {
        x: f64::from(work.position.x) / monitor_scale,
        y: f64::from(work.position.y) / monitor_scale,
        width: f64::from(work.size.width) / monitor_scale,
        height: f64::from(work.size.height) / monitor_scale,
    };
    if let Some((x, y)) = dodge(click, bar, area) {
        window.set_position(LogicalPosition::new(x, y))?;
    }
    Ok(())
}

/// Shows the ring centred on `point`, creating its window the first time,
/// and hides it [`RING_MS`] later unless another ring showed meanwhile.
fn show_ring<R: Runtime>(app: &AppHandle<R>, point: Point) -> tauri::Result<()> {
    let x = point.x - RING_SIZE / 2.0;
    let y = point.y - RING_SIZE / 2.0;
    let window = match app.get_webview_window(RING_LABEL) {
        Some(window) => {
            window.set_position(LogicalPosition::new(x, y))?;
            // Restarts the animation (`ring.html` has no script of its own).
            window.eval(
                "document.body.classList.remove('on');void document.body.offsetWidth;document.body.classList.add('on')",
            )?;
            window
        }
        None => WebviewWindowBuilder::new(app, RING_LABEL, WebviewUrl::App(RING_URL.into()))
            .title("GuideRecorder Click")
            .inner_size(RING_SIZE, RING_SIZE)
            .position(x, y)
            .decorations(false)
            .transparent(true)
            .shadow(false)
            .always_on_top(true)
            .resizable(false)
            .skip_taskbar(true)
            .visible_on_all_workspaces(true)
            .focused(false)
            .focusable(false)
            .build()?,
    };
    window.set_ignore_cursor_events(true)?;
    window.show()?;
    let shown = RING_SHOWN.fetch_add(1, Ordering::SeqCst) + 1;
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(RING_MS));
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || {
            if RING_SHOWN.load(Ordering::SeqCst) == shown {
                if let Some(window) = handle.get_webview_window(RING_LABEL) {
                    let _ = window.hide();
                }
            }
        });
    });
    Ok(())
}

/// Sets the toolbar window's height for its feed panel: `height` logical
/// points, clamped from the bar alone to [`MAX_HEIGHT`]; the top edge
/// stays put. Only the toolbar window may call it.
#[tauri::command]
pub fn resize_toolbar<R: Runtime>(window: WebviewWindow<R>, height: f64) -> Result<()> {
    if window.label() != LABEL {
        return Err(AppError::new(ErrorKind::Invalid, "not the toolbar"));
    }
    window
        .set_size(LogicalSize::new(WIDTH, clamp_height(height)))
        .map_err(|e| AppError::new(ErrorKind::Io, e.to_string()))
}

/// Lets the toolbar take the keyboard while a capture is renamed in its
/// feed (`typing`), and gives it back after. A non-focusable window gets
/// no key events, so renaming makes it focusable and focuses it, which
/// activates GuideRecorder: the user clicks back into the recorded app
/// afterwards. Keys typed into it are ours, so never recorded. Only the
/// toolbar window may call it.
#[tauri::command]
pub fn set_toolbar_typing<R: Runtime>(window: WebviewWindow<R>, typing: bool) -> Result<()> {
    if window.label() != LABEL {
        return Err(AppError::new(ErrorKind::Invalid, "not the toolbar"));
    }
    window
        .set_focusable(typing)
        .and_then(|()| if typing { window.set_focus() } else { Ok(()) })
        .map_err(|e| AppError::new(ErrorKind::Io, e.to_string()))
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

    const AREA: Area = Area {
        x: 0.0,
        y: 33.0,
        width: 1512.0,
        height: 949.0,
    };

    fn bar_at((x, y): (f64, f64)) -> Area {
        Area {
            x,
            y,
            width: WIDTH,
            height: HEIGHT,
        }
    }

    #[test]
    fn anchors_are_the_top_centre_and_corners() {
        assert_eq!(
            anchors(AREA, WIDTH),
            [(576.0, 41.0), (1140.0, 41.0), (12.0, 41.0)]
        );
        // The first anchor is where the toolbar opens.
        assert_eq!(anchors(AREA, WIDTH)[0], top_centre(0, 66, 3024, 2.0));
    }

    #[test]
    fn a_click_near_the_toolbar_moves_it_away() {
        let [centre, right, left] = anchors(AREA, WIDTH);
        let bar = bar_at(centre);
        // Far away: it stays.
        assert_eq!(dodge(Point { x: 700.0, y: 600.0 }, bar, AREA), None);
        // Just below its right half: to the far (left) corner.
        assert_eq!(dodge(Point { x: 900.0, y: 100.0 }, bar, AREA), Some(left));
        // Just below its left half: to the right corner.
        assert_eq!(dodge(Point { x: 600.0, y: 100.0 }, bar, AREA), Some(right));
        // In a corner, a click near it moves it to the far corner, never
        // to where the click is.
        assert_eq!(
            dodge(Point { x: 1300.0, y: 90.0 }, bar_at(right), AREA),
            Some(left)
        );
    }

    #[test]
    fn nowhere_better_keeps_it_put() {
        // A work area as narrow as the bar: every anchor is near the click.
        let narrow = Area {
            x: 0.0,
            y: 0.0,
            width: WIDTH,
            height: 400.0,
        };
        let bar = bar_at(anchors(narrow, WIDTH)[0]);
        assert_eq!(dodge(Point { x: 10.0, y: 20.0 }, bar, narrow), None);
    }

    #[test]
    fn the_window_grows_from_the_bar_to_a_cap() {
        assert_eq!(clamp_height(0.0), HEIGHT);
        assert_eq!(clamp_height(300.0), 300.0);
        assert_eq!(clamp_height(5_000.0), MAX_HEIGHT);
        assert_eq!(clamp_height(f64::NAN), HEIGHT);
    }
}
