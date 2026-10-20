//! Choosing what a recording captures: the windows and displays the user
//! can pick (`list_capture_windows`, `list_capture_displays`), the region
//! overlay (`pick_region`, answered by the overlay page through
//! `finish_region_pick`), and the one mapping from
//! the webview's [`CaptureTargetArg`] to a capture [`CaptureTarget`]
//! ([`capture_target`]), used by `start_recording`.
//!
//! The overlay is the window [`LABEL`] (`region.html`): borderless,
//! transparent and always on top, covering the display under the pointer.
//! It is opened as `region.html?pick=<id>`; the page passes that id back
//! to `finish_region_pick`, so an answer from a stale page is ignored. On
//! macOS it sits above the menu bar and the Dock and joins every Space,
//! a full-screen app's included ([`raise_overlay`]).
//!
//! The page reports a rect in its CSS pixels, which are local points of
//! the overlay; [`to_global`] turns them into global points using the
//! overlay's frame as measured when it answers ([`measured_frame`]), not
//! the display bounds it was asked to cover. At most one pick runs at a
//! time; closing the overlay any other way cancels it.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex, PoisonError};

use guiderecorder_capture::backend::frames::{
    self, CaptureTarget, DisplayInfo, FrameError, WindowInfo,
};
use guiderecorder_core::model::{Point, Rect};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Runtime, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::blocking::blocking;
use crate::editor::{AppError, ErrorKind, Result};
use crate::points;
use crate::settings::SettingsStore;

/// Label of the region overlay window.
pub const LABEL: &str = "region";
const URL: &str = "region.html";

/// Smallest region, in points, `start_recording` accepts (each side).
pub const MIN_REGION_SIDE: f64 = 40.0;

pub const WINDOW_GONE: &str = "That window is no longer open.";
pub const WINDOW_NOT_PICKABLE: &str = "That window can't be recorded.";
pub const DISPLAY_GONE: &str = "That display is no longer connected.";

/// A window the user can record (from `list_capture_windows`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureWindow {
    pub window_id: u32,
    pub app_name: String,
    pub bundle_id: Option<String>,
    pub title: Option<String>,
    /// Points.
    pub width: f64,
    pub height: f64,
}

impl CaptureWindow {
    fn of(window: WindowInfo) -> Self {
        Self {
            window_id: window.window_id,
            app_name: window.app_name,
            bundle_id: window.bundle_id,
            title: window.title,
            width: window.frame.width,
            height: window.frame.height,
        }
    }
}

/// A display the user can record (from `list_capture_displays`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureDisplay {
    pub display_id: u32,
    pub main: bool,
    /// Global points.
    pub bounds: Rect,
}

/// What `start_recording` is asked to capture. Serialized as
/// `{"kind": "display"}` (every display), `{"kind": "display",
/// "displayId": …}`, `{"kind": "window", "windowId": …}` or
/// `{"kind": "region", "displayId": …, "rect": {x, y, width, height}}`
/// (`rect` in global points).
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum CaptureTargetArg {
    Display {
        #[serde(default)]
        display_id: Option<u32>,
    },
    Window {
        window_id: u32,
    },
    Region {
        #[serde(default)]
        display_id: Option<u32>,
        rect: Rect,
    },
}

/// The capture target for `arg`. A window must pass `check_window` (see
/// [`check_window`]); a region must be finite
/// and at least [`MIN_REGION_SIDE`] on both sides, else `invalid`. A
/// region without a display gets the one under its centre (`display_at`),
/// else the main display. A display target without a display is every
/// display, as a recording from the stored settings is.
pub fn capture_target(
    arg: CaptureTargetArg,
    check_window: impl FnOnce(u32) -> Result<()>,
    display_at: impl FnOnce(Point) -> Option<u32>,
) -> Result<CaptureTarget> {
    match arg {
        CaptureTargetArg::Display { display_id } => Ok(CaptureTarget::Display { display_id }),
        CaptureTargetArg::Window { window_id } => {
            check_window(window_id)?;
            Ok(CaptureTarget::Window { window_id })
        }
        CaptureTargetArg::Region { display_id, rect } => {
            let finite = [rect.x, rect.y, rect.width, rect.height]
                .iter()
                .all(|v| v.is_finite());
            if !finite || rect.width < MIN_REGION_SIDE || rect.height < MIN_REGION_SIDE {
                return Err(AppError::new(
                    ErrorKind::Invalid,
                    format!(
                        "the region must be at least {MIN_REGION_SIDE}×{MIN_REGION_SIDE} points"
                    ),
                ));
            }
            let display_id = display_id.or_else(|| {
                display_at(Point {
                    x: rect.x + rect.width / 2.0,
                    y: rect.y + rect.height / 2.0,
                })
            });
            Ok(CaptureTarget::Region { display_id, rect })
        }
    }
}

/// Whether window `id` can be recorded, given the windows on screen now
/// (`listed`): `invalid` [`WINDOW_GONE`] when it is not listed, `invalid`
/// [`WINDOW_NOT_PICKABLE`] when it is but `list_capture_windows` would not
/// offer it ([`frames::pickable_windows`] with `own_pid` and `excluded`).
pub fn check_window(
    id: u32,
    listed: Vec<WindowInfo>,
    own_pid: i32,
    excluded: &[String],
) -> Result<()> {
    if !listed.iter().any(|w| w.window_id == id) {
        return Err(AppError::new(ErrorKind::Invalid, WINDOW_GONE));
    }
    if !frames::pickable_windows(listed, own_pid, excluded)
        .iter()
        .any(|w| w.window_id == id)
    {
        return Err(AppError::new(ErrorKind::Invalid, WINDOW_NOT_PICKABLE));
    }
    Ok(())
}

/// Whether display `id` is among the active displays (`listed`):
/// `invalid` [`DISPLAY_GONE`] when not.
pub fn check_display(id: u32, listed: &[DisplayInfo]) -> Result<()> {
    if listed.iter().any(|d| d.id == id) {
        Ok(())
    } else {
        Err(AppError::new(ErrorKind::Invalid, DISPLAY_GONE))
    }
}

/// [`capture_target`] against the system: the window is checked against
/// one ScreenCaptureKit listing ([`check_window`], with the excluded apps
/// `excluded`), a chosen display against the active ones
/// ([`check_display`]), a region's display is found by its bounds.
/// Blocking.
pub fn resolve_target(arg: CaptureTargetArg, excluded: &[String]) -> Result<CaptureTarget> {
    if let CaptureTargetArg::Display {
        display_id: Some(id),
    } = arg
    {
        check_display(id, &DisplayInfo::all())?;
    }
    capture_target(
        arg,
        |id| match frames::list_windows() {
            Ok(listed) => check_window(id, listed, std::process::id() as i32, excluded),
            // Cannot tell: let the session's own lookup decide.
            Err(_) => Ok(()),
        },
        |p| DisplayInfo::containing(p).map(|d| d.id),
    )
}

/// The error for a failed window listing: missing Screen Recording
/// permission reads as `start_recording`'s permission refusal.
fn listing_error(e: FrameError) -> AppError {
    match e {
        FrameError::PermissionDenied => AppError::new(
            ErrorKind::Invalid,
            "recording needs Screen Recording permission",
        ),
        e => AppError::new(ErrorKind::Io, e.to_string()),
    }
}

/// The windows the user can record, frontmost first: on screen, on the
/// normal layer, at least 80 points on both sides, and neither GuideRecorder's
/// nor an excluded app's (settings: `recording.excludedApps`). `invalid`
/// without Screen Recording permission.
#[tauri::command]
pub async fn list_capture_windows(
    store: State<'_, Arc<SettingsStore>>,
) -> Result<Vec<CaptureWindow>> {
    let excluded = store.get().recording.excluded_apps;
    blocking(move || {
        let windows = frames::list_windows().map_err(listing_error)?;
        let own_pid = std::process::id() as i32;
        Ok(frames::pickable_windows(windows, own_pid, &excluded)
            .into_iter()
            .map(CaptureWindow::of)
            .collect())
    })
    .await
}

/// The active displays, the main one first, then left to right.
pub fn capture_displays(displays: Vec<DisplayInfo>, main: Option<u32>) -> Vec<CaptureDisplay> {
    let mut listed: Vec<CaptureDisplay> = displays
        .into_iter()
        .map(|d| CaptureDisplay {
            display_id: d.id,
            main: Some(d.id) == main,
            bounds: d.bounds,
        })
        .collect();
    listed.sort_by(|a, b| {
        b.main
            .cmp(&a.main)
            .then(a.bounds.x.total_cmp(&b.bounds.x))
            .then(a.bounds.y.total_cmp(&b.bounds.y))
    });
    listed
}

/// The displays the user can record ([`capture_displays`]).
#[tauri::command]
pub async fn list_capture_displays() -> Result<Vec<CaptureDisplay>> {
    blocking(|| {
        Ok(capture_displays(
            DisplayInfo::all(),
            DisplayInfo::main().map(|d| d.id),
        ))
    })
    .await
}

/// A picked region (from `pick_region`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PickedRegion {
    pub display_id: Option<u32>,
    /// Global points.
    pub rect: Rect,
}

/// What the overlay page answers through `finish_region_pick`.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct RegionAnswer {
    /// The overlay's CSS pixels: display-local points.
    pub rect: Rect,
}

/// An answer as `pick_region` receives it: the page's rect and the
/// overlay's frame in global points when it answered (`None` if it could
/// not be read).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Picked {
    local: Rect,
    frame: Option<Rect>,
}

/// A window's frame in global points from its physical inner position and
/// size (as Tauri reports them) and the physical pixels per point
/// ([`points::pixels_per_point`] of its scale); `None` for a scale that is
/// not positive and finite.
pub fn measured_frame(x: i32, y: i32, width: u32, height: u32, scale: f64) -> Option<Rect> {
    (scale.is_finite() && scale > 0.0).then(|| Rect {
        x: f64::from(x) / scale,
        y: f64::from(y) / scale,
        width: f64::from(width) / scale,
        height: f64::from(height) / scale,
    })
}

/// True when `a` and `b` differ by more than a point on any edge.
fn differs(a: Rect, b: Rect) -> bool {
    [
        a.x - b.x,
        a.y - b.y,
        (a.x + a.width) - (b.x + b.width),
        (a.y + a.height) - (b.y + b.height),
    ]
    .iter()
    .any(|d| d.abs() > 1.0)
}

/// `local` (points from the overlay's top-left) in global points, clipped
/// to the overlay's `bounds` (global points); `None` when not finite or
/// nothing is left.
pub fn to_global(local: Rect, bounds: Rect) -> Option<Rect> {
    if ![local.x, local.y, local.width, local.height]
        .iter()
        .all(|v| v.is_finite())
    {
        return None;
    }
    // A rect dragged up or left has a negative size.
    let (x0, x1) = ordered(local.x, local.x + local.width);
    let (y0, y1) = ordered(local.y, local.y + local.height);
    let x0 = (bounds.x + x0).max(bounds.x);
    let y0 = (bounds.y + y0).max(bounds.y);
    let x1 = (bounds.x + x1).min(bounds.x + bounds.width);
    let y1 = (bounds.y + y1).min(bounds.y + bounds.height);
    (x1 > x0 && y1 > y0).then_some(Rect {
        x: x0,
        y: y0,
        width: x1 - x0,
        height: y1 - y0,
    })
}

fn ordered(a: f64, b: f64) -> (f64, f64) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

/// The region pick in progress (managed state): the channel its command
/// waits on, under a number so a late close of an earlier overlay cannot
/// answer a newer pick.
#[derive(Default)]
pub struct RegionPicker {
    pending: Mutex<Option<(u64, SyncSender<Option<Picked>>)>>,
    next: AtomicU64,
}

impl RegionPicker {
    /// Starts a pick; `invalid` while another one is open.
    fn begin(&self) -> Result<(u64, Receiver<Option<Picked>>)> {
        let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        if pending.is_some() {
            return Err(AppError::new(
                ErrorKind::Invalid,
                "a region is already being picked",
            ));
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::sync_channel(1);
        *pending = Some((id, tx));
        Ok((id, rx))
    }

    /// Answers pick `id` (any pick when `None`) with `answer`; false when
    /// that pick is not open (already answered, or another one).
    fn finish(&self, id: Option<u64>, answer: Option<Picked>) -> bool {
        let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        match pending.take() {
            Some((open, tx)) if id.is_none_or(|id| id == open) => {
                let _ = tx.send(answer);
                true
            }
            other => {
                *pending = other;
                false
            }
        }
    }

    fn is_open(&self) -> bool {
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }
}

/// The display the overlay covers: the one under the pointer, else the
/// main display.
fn overlay_display() -> Option<DisplayInfo> {
    frames::mouse_location()
        .and_then(DisplayInfo::containing)
        .or_else(DisplayInfo::main)
}

/// Opens the region overlay over the display under the pointer and waits
/// for the user to draw a region (or cancel). Returns the region in global
/// points, or `null` when cancelled or the overlay was closed. `invalid`
/// while another pick is open.
#[tauri::command]
pub async fn pick_region<R: Runtime>(
    app: AppHandle<R>,
    picker: State<'_, Arc<RegionPicker>>,
) -> Result<Option<PickedRegion>> {
    let picker = Arc::clone(&picker);
    let (id, rx) = picker.begin()?;
    let display = match overlay_display() {
        Some(display) => display,
        None => {
            picker.finish(Some(id), None);
            return Err(AppError::new(
                ErrorKind::Io,
                "no display to pick a region on",
            ));
        }
    };
    if let Err(e) = open_overlay(&app, &picker, id, display.bounds) {
        picker.finish(Some(id), None);
        return Err(AppError::new(
            ErrorKind::Io,
            format!("cannot open the region picker: {e}"),
        ));
    }
    // A dropped sender (never expected) is a cancel.
    let answer = blocking(move || Ok(rx.recv().ok().flatten())).await?;
    let Some(Picked { local, frame }) = answer else {
        return Ok(None);
    };
    let frame = frame.unwrap_or(display.bounds);
    let display_id = if differs(frame, display.bounds) {
        eprintln!(
            "guiderecorder: the region picker covers {frame:?}, not display {} at {:?}",
            display.id, display.bounds
        );
        DisplayInfo::containing(Point {
            x: frame.x + frame.width / 2.0,
            y: frame.y + frame.height / 2.0,
        })
        .map_or(display.id, |d| d.id)
    } else {
        display.id
    };
    Ok(to_global(local, frame).map(|rect| PickedRegion {
        display_id: Some(display_id),
        rect,
    }))
}

fn open_overlay<R: Runtime>(
    app: &AppHandle<R>,
    picker: &Arc<RegionPicker>,
    id: u64,
    bounds: Rect,
) -> tauri::Result<()> {
    // A leftover overlay (never expected) would take the label.
    if let Some(old) = app.get_webview_window(LABEL) {
        let _ = old.destroy();
    }
    let url = WebviewUrl::App(format!("{URL}?pick={id}").into());
    let window = WebviewWindowBuilder::new(app, LABEL, url)
        .title("GuideRecorder Region")
        .position(bounds.x, bounds.y)
        .inner_size(bounds.width, bounds.height)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .skip_taskbar(true)
        .focused(true)
        .accept_first_mouse(true)
        .build()?;
    if cfg!(windows) {
        // The builder takes logical pixels; the bounds are physical there.
        window.set_position(points::position(bounds.x, bounds.y))?;
        window.set_size(points::size(bounds.width, bounds.height))?;
    }
    let raised = window.clone();
    app.run_on_main_thread(move || raise_overlay(&raised))?;
    let picker = Arc::clone(picker);
    window.on_window_event(move |event| {
        if matches!(
            event,
            tauri::WindowEvent::CloseRequested { .. } | tauri::WindowEvent::Destroyed
        ) {
            picker.finish(Some(id), None);
        }
    });
    Ok(())
}

/// The overlay's frame in global points, as Tauri reports it now.
fn overlay_frame<R: Runtime>(window: &WebviewWindow<R>) -> Option<Rect> {
    let position = window.inner_position().ok()?;
    let size = window.inner_size().ok()?;
    let scale = window.scale_factor().ok()?;
    measured_frame(
        position.x,
        position.y,
        size.width,
        size.height,
        points::pixels_per_point(scale),
    )
}

/// The overlay's answer: a rect in its CSS pixels, or `null` to cancel,
/// for the pick `pick` (the `pick` query parameter of its URL; omitted, the
/// open pick). Closes the overlay, unless `pick` names another pick than
/// the open one: then the answer is ignored and nothing happens. Only the
/// overlay window may call it; with no pick open it only closes the
/// overlay.
#[tauri::command]
pub fn finish_region_pick<R: Runtime>(
    window: WebviewWindow<R>,
    picker: State<'_, Arc<RegionPicker>>,
    result: Option<RegionAnswer>,
    pick: Option<u64>,
) -> Result<()> {
    if window.label() != LABEL {
        return Err(AppError::new(
            ErrorKind::Invalid,
            "only the region picker can finish a pick",
        ));
    }
    // The overlay draws in CSS pixels; points may be smaller (Windows).
    let per_point = points::css_per_point(window.scale_factor().unwrap_or(1.0));
    let picked = result.map(|answer| Picked {
        local: Rect {
            x: answer.rect.x / per_point,
            y: answer.rect.y / per_point,
            width: answer.rect.width / per_point,
            height: answer.rect.height / per_point,
        },
        frame: overlay_frame(&window),
    });
    if !picker.finish(pick, picked) && picker.is_open() {
        eprintln!("guiderecorder: ignored an answer for region pick {pick:?}, not the open one");
        return Ok(());
    }
    if let Err(e) = window.destroy() {
        eprintln!("guiderecorder: cannot close the region picker: {e}");
    }
    Ok(())
}

/// Puts the overlay above the menu bar and the Dock (`NSStatusWindowLevel`)
/// and lets it join every Space, a full-screen app's included
/// (`canJoinAllSpaces` + `fullScreenAuxiliary`, added to its current
/// behaviour). Main thread only. A window without an `NSWindow` is left as
/// it is.
#[cfg(target_os = "macos")]
fn raise_overlay<R: Runtime>(window: &WebviewWindow<R>) {
    use std::ffi::{c_char, c_void};

    #[link(name = "objc", kind = "dylib")]
    extern "C" {
        fn sel_registerName(name: *const c_char) -> *const c_void;
        fn objc_msgSend();
    }
    /// `kCGStatusWindowLevel`: above the menu bar (24) and the Dock (20).
    const NS_STATUS_WINDOW_LEVEL: isize = 25;
    const CAN_JOIN_ALL_SPACES: usize = 1 << 0;
    const FULL_SCREEN_AUXILIARY: usize = 1 << 8;

    let ns_window = match window.ns_window() {
        Ok(w) if !w.is_null() => w,
        _ => return,
    };
    // SAFETY: `ns_window` is the live NSWindow of `window`, used on the main
    // thread. `objc_msgSend` is called through the exact signatures of
    // `-setLevel:(NSInteger)`, `-collectionBehavior` (returns NSUInteger)
    // and `-setCollectionBehavior:(NSUInteger)`, which NSWindow implements;
    // the selector names are NUL-terminated literals.
    unsafe {
        let send = objc_msgSend as unsafe extern "C" fn();
        let set_isize: unsafe extern "C" fn(*mut c_void, *const c_void, isize) =
            std::mem::transmute(send);
        let get_usize: unsafe extern "C" fn(*mut c_void, *const c_void) -> usize =
            std::mem::transmute(send);
        let set_usize: unsafe extern "C" fn(*mut c_void, *const c_void, usize) =
            std::mem::transmute(send);
        set_isize(
            ns_window,
            sel_registerName(c"setLevel:".as_ptr()),
            NS_STATUS_WINDOW_LEVEL,
        );
        let behavior = get_usize(ns_window, sel_registerName(c"collectionBehavior".as_ptr()));
        set_usize(
            ns_window,
            sel_registerName(c"setCollectionBehavior:".as_ptr()),
            behavior | CAN_JOIN_ALL_SPACES | FULL_SCREEN_AUXILIARY,
        );
    }
}

#[cfg(not(target_os = "macos"))]
fn raise_overlay<R: Runtime>(_window: &WebviewWindow<R>) {}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    fn arg(value: serde_json::Value) -> CaptureTargetArg {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn target_args_deserialize_from_the_contract_shapes() {
        assert_eq!(
            arg(json!({"kind": "display"})),
            CaptureTargetArg::Display { display_id: None }
        );
        assert_eq!(
            arg(json!({"kind": "display", "displayId": 4})),
            CaptureTargetArg::Display {
                display_id: Some(4)
            }
        );
        assert_eq!(
            arg(json!({"kind": "window", "windowId": 42})),
            CaptureTargetArg::Window { window_id: 42 }
        );
        assert_eq!(
            arg(json!({"kind": "region", "displayId": null,
                "rect": {"x": -10.5, "y": 20, "width": 300, "height": 200}})),
            CaptureTargetArg::Region {
                display_id: None,
                rect: rect(-10.5, 20.0, 300.0, 200.0)
            }
        );
        assert_eq!(
            arg(json!({"kind": "region", "displayId": 3,
                "rect": {"x": 0, "y": 0, "width": 40, "height": 40}})),
            CaptureTargetArg::Region {
                display_id: Some(3),
                rect: rect(0.0, 0.0, 40.0, 40.0)
            }
        );
        for bad in [
            json!({"kind": "window"}),
            json!({"kind": "screen"}),
            json!({"kind": "region", "displayId": 1}),
        ] {
            assert!(
                serde_json::from_value::<CaptureTargetArg>(bad.clone()).is_err(),
                "{bad}"
            );
        }
    }

    fn check(id: u32, open: u32) -> Result<()> {
        if id == open {
            Ok(())
        } else {
            Err(AppError::new(ErrorKind::Invalid, WINDOW_GONE))
        }
    }

    #[test]
    fn targets_map_and_validate() {
        let never = |_: Point| -> Option<u32> { panic!("no display lookup") };
        assert_eq!(
            capture_target(
                CaptureTargetArg::Display { display_id: None },
                |_| unreachable!(),
                never
            )
            .unwrap(),
            CaptureTarget::Display { display_id: None }
        );
        assert_eq!(
            capture_target(
                CaptureTargetArg::Display {
                    display_id: Some(4)
                },
                |_| unreachable!(),
                never
            )
            .unwrap(),
            CaptureTarget::Display {
                display_id: Some(4)
            }
        );
        assert_eq!(
            capture_target(
                CaptureTargetArg::Window { window_id: 7 },
                |id| check(id, 7),
                never
            )
            .unwrap(),
            CaptureTarget::Window { window_id: 7 }
        );
        let gone = capture_target(
            CaptureTargetArg::Window { window_id: 8 },
            |id| check(id, 7),
            never,
        )
        .unwrap_err();
        assert_eq!(gone.kind, ErrorKind::Invalid);
        assert_eq!(gone.message, WINDOW_GONE);

        // A given display is kept; a missing one is found under the centre.
        let region = |display_id, r| CaptureTargetArg::Region {
            display_id,
            rect: r,
        };
        let r = rect(100.0, 50.0, 40.0, 40.0);
        assert_eq!(
            capture_target(region(Some(2), r), |_| unreachable!(), never).unwrap(),
            CaptureTarget::Region {
                display_id: Some(2),
                rect: r
            }
        );
        let r = rect(-1000.0, 10.0, 200.0, 100.0);
        let centre = |p: Point| {
            assert_eq!((p.x, p.y), (-900.0, 60.0));
            Some(5)
        };
        assert_eq!(
            capture_target(region(None, r), |_| unreachable!(), centre).unwrap(),
            CaptureTarget::Region {
                display_id: Some(5),
                rect: r
            }
        );
        // No display there: the main display (`None`).
        assert_eq!(
            capture_target(region(None, r), |_| unreachable!(), |_| None).unwrap(),
            CaptureTarget::Region {
                display_id: None,
                rect: r
            }
        );
        for bad in [
            rect(0.0, 0.0, 39.9, 200.0),
            rect(0.0, 0.0, 200.0, 39.0),
            rect(0.0, 0.0, -100.0, 100.0),
            rect(f64::NAN, 0.0, 100.0, 100.0),
            rect(0.0, 0.0, f64::INFINITY, 100.0),
        ] {
            let e = capture_target(region(Some(1), bad), |_| unreachable!(), never).unwrap_err();
            assert_eq!(e.kind, ErrorKind::Invalid, "{bad:?}");
        }
    }

    fn display(id: u32, x: f64) -> DisplayInfo {
        DisplayInfo {
            id,
            bounds: rect(x, 0.0, 1920.0, 1080.0),
            scale: 2.0,
        }
    }

    #[test]
    fn displays_list_main_first_then_left_to_right() {
        let listed = capture_displays(
            vec![display(3, 1920.0), display(1, 0.0), display(2, -1920.0)],
            Some(1),
        );
        let ids: Vec<(u32, bool)> = listed.iter().map(|d| (d.display_id, d.main)).collect();
        assert_eq!(ids, [(1, true), (2, false), (3, false)]);
        assert_eq!(listed[1].bounds, rect(-1920.0, 0.0, 1920.0, 1080.0));
    }

    #[test]
    fn a_chosen_display_must_still_be_connected() {
        let listed = [display(1, 0.0), display(2, 1920.0)];
        assert!(check_display(2, &listed).is_ok());
        let e = check_display(5, &listed).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Invalid);
        assert_eq!(e.message, DISPLAY_GONE);
    }

    #[test]
    fn local_rects_become_global_and_clipped() {
        // A display left of and above the main one.
        let bounds = rect(-1920.0, -200.0, 1920.0, 1080.0);
        assert_eq!(
            to_global(rect(100.0, 50.0, 300.0, 200.0), bounds),
            Some(rect(-1820.0, -150.0, 300.0, 200.0))
        );
        // Dragged up and left: normalised.
        assert_eq!(
            to_global(rect(400.0, 250.0, -300.0, -200.0), bounds),
            Some(rect(-1820.0, -150.0, 300.0, 200.0))
        );
        // Spilling over the edges: clipped to the display.
        assert_eq!(
            to_global(rect(1800.0, -20.0, 400.0, 100.0), bounds),
            Some(rect(-120.0, -200.0, 120.0, 80.0))
        );
        let main = rect(0.0, 0.0, 1512.0, 982.0);
        assert_eq!(to_global(rect(0.0, 0.0, 1512.0, 982.0), main), Some(main));
        assert_eq!(to_global(rect(10.0, 10.0, 0.0, 50.0), main), None);
        assert_eq!(to_global(rect(2000.0, 10.0, 50.0, 50.0), main), None);
        assert_eq!(to_global(rect(f64::NAN, 10.0, 50.0, 50.0), main), None);
    }

    #[test]
    fn listing_without_permission_reads_as_the_start_refusal() {
        let e = listing_error(FrameError::PermissionDenied);
        assert_eq!(e.kind, ErrorKind::Invalid);
        assert_eq!(e.message, "recording needs Screen Recording permission");
        assert_eq!(listing_error(FrameError::Timeout).kind, ErrorKind::Io);
    }

    #[test]
    fn one_pick_at_a_time_and_late_closes_are_ignored() {
        let picker = RegionPicker::default();
        let (first, rx) = picker.begin().unwrap();
        assert_eq!(picker.begin().unwrap_err().kind, ErrorKind::Invalid);
        let answer = Picked {
            local: rect(1.0, 2.0, 3.0, 4.0),
            frame: None,
        };
        // The page answers (no id), then the overlay's close comes late.
        assert!(picker.finish(None, Some(answer)));
        assert!(!picker.finish(Some(first), None));
        assert_eq!(rx.recv().unwrap(), Some(answer));

        // A new pick is not answered by the old overlay's close.
        let (second, rx) = picker.begin().unwrap();
        assert_ne!(first, second);
        assert!(!picker.finish(Some(first), None));
        assert!(rx.try_recv().is_err());
        // Its own close cancels it.
        assert!(picker.finish(Some(second), None));
        assert_eq!(rx.recv().unwrap(), None);
        assert!(!picker.is_open());

        // A page answering with another pick's id is ignored; its own id
        // answers.
        let (third, rx) = picker.begin().unwrap();
        assert!(!picker.finish(Some(second), Some(answer)));
        assert!(picker.is_open());
        assert!(rx.try_recv().is_err());
        assert!(picker.finish(Some(third), Some(answer)));
        assert_eq!(rx.recv().unwrap(), Some(answer));
        assert!(picker.begin().is_ok());
    }

    #[test]
    fn windows_are_checked_against_one_listing() {
        let window = |id: u32, pid: i32, bundle: &str| WindowInfo {
            window_id: id,
            pid,
            app_name: "App".into(),
            bundle_id: Some(bundle.into()),
            title: None,
            frame: rect(0.0, 0.0, 800.0, 600.0),
            layer: 0,
            on_screen: true,
        };
        let listed = || {
            vec![
                window(1, 20, "com.apple.Safari"),
                window(2, 99, "dev.dufflebear.guiderecorder"),
                window(3, 30, "com.1password.1password"),
                WindowInfo {
                    frame: rect(0.0, 0.0, 60.0, 600.0),
                    ..window(4, 20, "com.apple.Safari")
                },
            ]
        };
        let excluded = ["com.1password.1password".to_owned()];
        assert!(check_window(1, listed(), 99, &excluded).is_ok());
        for (id, message) in [
            (2, WINDOW_NOT_PICKABLE),
            (3, WINDOW_NOT_PICKABLE),
            (4, WINDOW_NOT_PICKABLE),
            (5, WINDOW_GONE),
        ] {
            let e = check_window(id, listed(), 99, &excluded).unwrap_err();
            assert_eq!(
                (e.kind, e.message.as_str()),
                (ErrorKind::Invalid, message),
                "{id}"
            );
        }
        assert_eq!(WINDOW_NOT_PICKABLE, "That window can't be recorded.");
    }

    #[test]
    fn the_overlay_frame_is_measured_in_points() {
        // A 2× display left of the main one: physical pixels to points.
        assert_eq!(
            measured_frame(-3840, -400, 3840, 2160, 2.0),
            Some(rect(-1920.0, -200.0, 1920.0, 1080.0))
        );
        assert_eq!(
            measured_frame(0, 0, 1512, 982, 1.0),
            Some(rect(0.0, 0.0, 1512.0, 982.0))
        );
        assert_eq!(measured_frame(0, 0, 10, 10, 0.0), None);
        assert_eq!(measured_frame(0, 0, 10, 10, f64::NAN), None);

        // Pushed below a 33-point menu bar: the page's rect is local to
        // the measured frame, not to the display.
        let display = rect(0.0, 0.0, 1512.0, 982.0);
        let frame = measured_frame(0, 66, 3024, 1898, 2.0).unwrap();
        assert_eq!(frame, rect(0.0, 33.0, 1512.0, 949.0));
        assert!(differs(frame, display));
        assert_eq!(
            to_global(rect(100.0, 0.0, 200.0, 100.0), frame),
            Some(rect(100.0, 33.0, 200.0, 100.0))
        );
        // Rounding within a point is not a difference.
        assert!(!differs(rect(0.4, -0.5, 1512.0, 982.9), display));
    }
}
