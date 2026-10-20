//! Windows.Graphics.Capture stream with a rolling frame buffer, the
//! displays and windows it can capture, and the pointer position: the
//! Windows counterpart of the macOS frames module, with the same API.
//!
//! [`FrameStream`] captures a display, a window or a region of a display
//! with a free-threaded `Direct3D11CaptureFramePool` and keeps the last
//! few frames in the shared [`FrameRing`]. Coordinates are physical pixels
//! of the virtual screen (see the backend's `# Coordinates`), so every
//! display's scale is 1.
//!
//! # Frames
//!
//! Windows delivers a frame whenever the content changes, up to the
//! display's refresh rate; frames closer together than the stream's rate
//! ([`FrameStreamConfig::fps`]) are dropped without being read. Each kept
//! frame is copied once from the GPU, through a staging texture, into a
//! tightly packed BGRA buffer taken from the ring's pool. Its timestamp is
//! the frame's `SystemRelativeTime`, the performance counter in 100 ns
//! units, which [`super::event_tap::now`] reads in nanoseconds: frames and
//! events compare directly. It is when the frame is shown, a couple of
//! milliseconds after it was composed and handed to the pool, so a frame
//! can be stamped just after an event that came after its content; the
//! frame before that event is then the one before it, which shows the
//! screen as it was too. A window target's geometry is read again for
//! every frame, from the window's bounds, so it follows moves; a resize
//! recreates the pool at the new size.
//!
//! # Excluded windows
//!
//! Windows.Graphics.Capture has no content filter. This process's windows
//! are kept out of display and region captures
//! ([`FrameStreamConfig::exclude_own_windows`]) by display affinity
//! (`WDA_EXCLUDEFROMCAPTURE`), set on every top-level window of the
//! process when the stream starts and on new ones as frames arrive, and
//! cleared again when it stops. Other apps' windows cannot be left out:
//! [`FrameStreamConfig::excluded_bundle_ids`] is not applied to frames, and
//! [`FilterHandle`] has nothing to refresh. The recording session still
//! captures nothing while an excluded app is in front.
//!
//! # Lost capture
//!
//! When the captured window closes or the display goes away, the capture
//! item's `Closed` event stops the run, with an error that
//! [`is_source_gone_error`] recognises.
//!
//! # Threading
//!
//! Frames are copied on the frame pool's worker thread. Starting and
//! stopping block only briefly.

use std::ffi::c_void;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use guiderecorder_core::events::Timestamp;
use guiderecorder_core::model::{Point, Rect};
use windows::core::{IInspectable, Interface, BOOL};
use windows::Foundation::{TimeSpan, TypedEventHandler};
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureAccess, GraphicsCaptureAccessKind,
    GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::{HMODULE, HWND, LPARAM, POINT, RECT, TRUE};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_BOX,
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE,
    D3D11_MAP_READ, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MonitorFromPoint, HDC, HMONITOR, MONITORINFO,
    MONITOR_DEFAULTTOPRIMARY,
};
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetCursorPos, GetWindowDisplayAffinity, GetWindowLongW,
    GetWindowRect, GetWindowTextW, IsIconic, IsWindow, IsWindowVisible, SetWindowDisplayAffinity,
    GWL_EXSTYLE, WDA_EXCLUDEFROMCAPTURE, WDA_NONE, WS_EX_APPWINDOW, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
};

pub use crate::frames::*;
pub use crate::ring::*;

use super::event_tap::window_pid;
use super::{apps, PerMonitorAware};

/// Marks a stop error that means the window or display is gone.
const SOURCE_GONE: &str = "(source gone)";
/// Frames the pool holds; each is copied out at once.
const POOL_FRAMES: i32 = 2;
/// How often, at most, new windows of this process are excluded.
const OWN_WINDOWS_EVERY: Duration = Duration::from_millis(500);
/// Window classes that are part of the shell (the desktop and the
/// taskbar), never a capture target.
const SHELL_CLASSES: &[&str] = &[
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
];

impl DisplayInfo {
    /// The primary display.
    pub fn main() -> Option<Self> {
        let _dpi = PerMonitorAware::enter();
        // SAFETY: a plain query.
        let monitor = unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) };
        display(monitor)
    }

    pub fn for_id(id: u32) -> Option<Self> {
        Self::all().into_iter().find(|d| d.id == id)
    }

    /// All active displays.
    pub fn all() -> Vec<Self> {
        let _dpi = PerMonitorAware::enter();
        monitors().into_iter().filter_map(display).collect()
    }

    /// The display whose bounds contain `p` (global points).
    pub fn containing(p: Point) -> Option<Self> {
        Self::all().into_iter().find(|d| rect_contains(d.bounds, p))
    }
}

/// A display's id: its monitor handle, which stays the same while the
/// display is connected.
fn monitor_id(monitor: HMONITOR) -> u32 {
    monitor.0 as usize as u32
}

/// The monitor a display id stands for.
fn monitor_for(id: u32) -> Option<HMONITOR> {
    monitors().into_iter().find(|&m| monitor_id(m) == id)
}

fn monitors() -> Vec<HMONITOR> {
    unsafe extern "system" fn collect(
        monitor: HMONITOR,
        _dc: HDC,
        _rect: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        // SAFETY: `data` is the `Vec` passed below, alive for the call.
        unsafe { (*(data.0 as *mut Vec<HMONITOR>)).push(monitor) };
        TRUE
    }
    let mut found: Vec<HMONITOR> = Vec::new();
    // SAFETY: the callback only pushes to `found`, which outlives the call.
    let _ = unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(collect),
            LPARAM(&mut found as *mut Vec<HMONITOR> as isize),
        )
    };
    found
}

fn display(monitor: HMONITOR) -> Option<DisplayInfo> {
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: a valid, sized out-pointer.
    unsafe { GetMonitorInfoW(monitor, &mut info) }.ok().ok()?;
    let bounds = rect_from(info.rcMonitor);
    (bounds.width > 0.0 && bounds.height > 0.0).then_some(DisplayInfo {
        id: monitor_id(monitor),
        bounds,
        scale: 1.0,
    })
}

/// Where the mouse pointer is now, in global points (the same space as
/// event points).
pub fn mouse_location() -> Option<Point> {
    let _dpi = PerMonitorAware::enter();
    let mut p = POINT::default();
    // SAFETY: a valid out-pointer.
    unsafe { GetCursorPos(&mut p) }.ok()?;
    Some(Point {
        x: f64::from(p.x),
        y: f64::from(p.y),
    })
}

/// The top-level windows, front to back, for choosing a window target
/// ([`pickable_windows`] then keeps the ones a user can pick). Tool
/// windows, always-on-top panels that never take focus, and the desktop
/// and taskbar are listed on a layer above 0.
pub fn list_windows() -> Result<Vec<WindowInfo>, FrameError> {
    let _dpi = PerMonitorAware::enter();
    let mut apps_by_pid = std::collections::HashMap::new();
    let windows = top_level_windows()
        .into_iter()
        .map(|window| {
            let pid = window_pid(window);
            let app = apps_by_pid
                .entry(pid)
                .or_insert_with(|| apps::app_for_pid(pid))
                .clone();
            // SAFETY: plain queries on a window handle (a stale one fails).
            let (visible, iconic, ex_style) = unsafe {
                (
                    IsWindowVisible(window).as_bool(),
                    IsIconic(window).as_bool(),
                    GetWindowLongW(window, GWL_EXSTYLE) as u32,
                )
            };
            let tool = ex_style & WS_EX_TOOLWINDOW.0 != 0 && ex_style & WS_EX_APPWINDOW.0 == 0;
            let panel = ex_style & WS_EX_TOPMOST.0 != 0 && ex_style & WS_EX_NOACTIVATE.0 != 0;
            let shell = class_name(window).is_some_and(|c| SHELL_CLASSES.contains(&c.as_str()));
            WindowInfo {
                window_id: window_id(window),
                pid,
                app_name: app.name.unwrap_or_default(),
                bundle_id: app.bundle_id,
                title: window_text(window),
                frame: window_bounds(window).unwrap_or(Rect {
                    x: 0.0,
                    y: 0.0,
                    width: 0.0,
                    height: 0.0,
                }),
                layer: if tool || panel || shell { 1 } else { 0 },
                on_screen: visible && !iconic && !is_cloaked(window),
            }
        })
        .collect();
    Ok(windows)
}

/// Whether window `window_id` still exists.
pub fn window_exists(window_id: u32) -> bool {
    // SAFETY: a plain query; any value is safe to ask about.
    unsafe { IsWindow(Some(window_handle(window_id))) }.as_bool()
}

/// Whether a stop error (as [`FrameReader::last_error`] gives it) says the
/// captured window or display is gone.
pub fn is_source_gone_error(error: &str) -> bool {
    error.ends_with(SOURCE_GONE)
}

/// A window's id: its handle, which Windows keeps to 32 bits.
fn window_id(window: HWND) -> u32 {
    window.0 as usize as u32
}

/// The handle a window id stands for (handles are sign-extended).
fn window_handle(window_id: u32) -> HWND {
    HWND(window_id as i32 as isize as *mut c_void)
}

fn top_level_windows() -> Vec<HWND> {
    unsafe extern "system" fn collect(window: HWND, data: LPARAM) -> BOOL {
        // SAFETY: `data` is the `Vec` passed below, alive for the call.
        unsafe { (*(data.0 as *mut Vec<HWND>)).push(window) };
        TRUE
    }
    let mut found: Vec<HWND> = Vec::new();
    // SAFETY: the callback only pushes to `found`, which outlives the call.
    let _ = unsafe { EnumWindows(Some(collect), LPARAM(&mut found as *mut Vec<HWND> as isize)) };
    found
}

fn window_text(window: HWND) -> Option<String> {
    let mut buf = [0u16; 512];
    // SAFETY: a valid buffer.
    let n = unsafe { GetWindowTextW(window, &mut buf) };
    let text = String::from_utf16_lossy(&buf[..usize::try_from(n).ok()?.min(buf.len())]);
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn class_name(window: HWND) -> Option<String> {
    let mut buf = [0u16; 256];
    // SAFETY: a valid buffer.
    let n = unsafe { GetClassNameW(window, &mut buf) };
    Some(String::from_utf16_lossy(
        &buf[..usize::try_from(n).ok()?.min(buf.len())],
    ))
}

/// A cloaked window (on another virtual desktop, or a suspended app's) is
/// not shown even though it is "visible".
fn is_cloaked(window: HWND) -> bool {
    let mut cloaked = 0u32;
    // SAFETY: a valid out-pointer of the attribute's size.
    let read = unsafe {
        DwmGetWindowAttribute(
            window,
            DWMWA_CLOAKED,
            (&mut cloaked as *mut u32).cast(),
            std::mem::size_of::<u32>() as u32,
        )
    };
    read.is_ok() && cloaked != 0
}

/// A window's bounds as seen on screen (without the invisible resize
/// borders), in global points.
fn window_bounds(window: HWND) -> Option<Rect> {
    let mut r = RECT::default();
    // SAFETY: a valid out-pointer of the attribute's size.
    let read = unsafe {
        DwmGetWindowAttribute(
            window,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&mut r as *mut RECT).cast(),
            std::mem::size_of::<RECT>() as u32,
        )
    };
    if read.is_err() {
        // SAFETY: a valid out-pointer.
        unsafe { GetWindowRect(window, &mut r) }.ok()?;
    }
    let rect = rect_from(r);
    (rect.width > 0.0 && rect.height > 0.0).then_some(rect)
}

/// Refreshes a [`FrameStream`]'s content filter from any thread. Windows
/// has no content filter (see `# Excluded windows`): there is never
/// anything to refresh.
#[derive(Debug, Clone)]
pub struct FilterHandle(());

impl FilterHandle {
    /// False: no target's frames leave apps out.
    pub fn filters_apps(&self) -> bool {
        false
    }

    /// False: no app's windows are left out.
    pub fn covers(&self, _pid: i32) -> bool {
        false
    }

    /// Does nothing.
    pub fn refresh(&self) -> Result<(), FrameError> {
        Ok(())
    }
}

/// A running (or paused) Windows.Graphics.Capture capture.
///
/// `pause()` and `resume()` really stop and restart the capture; resume
/// finds the target again, so a window target's current size is picked up.
/// If the target goes away, [`FrameStream::is_capturing`] turns false,
/// [`FrameReader::last_error`] says why, and `resume()` tries again.
pub struct FrameStream {
    cfg: FrameStreamConfig,
    reader: FrameReader,
    running: Option<Running>,
    stopped: bool,
}

impl std::fmt::Debug for FrameStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrameStream")
            .field("cfg", &self.cfg)
            .field("capturing", &self.is_capturing())
            .field("stopped", &self.stopped)
            .finish()
    }
}

impl FrameStream {
    /// Starts capturing.
    pub fn start(cfg: FrameStreamConfig) -> Result<Self, FrameError> {
        cfg.validate()?;
        let reader = FrameReader::with_pool(cfg.ring_capacity, cfg.reuse_buffers);
        let generation = reader.begin_run();
        let running =
            Running::start(&cfg, &reader, generation).inspect_err(|_| reader.end_run())?;
        Ok(Self {
            cfg,
            reader,
            running: Some(running),
            stopped: false,
        })
    }

    pub fn config(&self) -> &FrameStreamConfig {
        &self.cfg
    }

    /// A handle for reading frames from other threads.
    pub fn reader(&self) -> FrameReader {
        self.reader.clone()
    }

    /// True while frames are being captured: not paused, not stopped, and
    /// the target still there.
    pub fn is_capturing(&self) -> bool {
        self.running.is_some() && !self.reader.stopped_by_system()
    }

    /// Geometry of the current run (`None` while paused). For window
    /// targets each frame carries its own, more current, geometry.
    pub fn geometry(&self) -> Option<FrameGeometry> {
        self.running.as_ref().map(|r| r.geometry)
    }

    /// Whether this process's windows are kept out of the current run's
    /// frames (see `# Excluded windows`): for display and region targets
    /// with [`FrameStreamConfig::exclude_own_windows`].
    pub fn own_app_excluded(&self) -> bool {
        self.running.is_some() && excludes_own_windows(&self.cfg)
    }

    /// Always empty: no other app's windows are left out.
    pub fn excluded_pids(&self) -> Vec<i32> {
        Vec::new()
    }

    /// Covers windows this process opened since the last frame. Returns
    /// [`FrameStream::own_app_excluded`].
    pub fn refresh_filter(&mut self) -> Result<bool, FrameError> {
        if self.stopped {
            return Err(FrameError::Stopped);
        }
        if let Some(running) = &self.running {
            running.own_windows.refresh();
        }
        Ok(self.own_app_excluded())
    }

    /// See [`FilterHandle`].
    pub fn filter_handle(&self) -> FilterHandle {
        FilterHandle(())
    }

    /// Stops the capture. Buffered frames stay readable. No-op if already
    /// paused.
    pub fn pause(&mut self) -> Result<(), FrameError> {
        if self.stopped {
            return Err(FrameError::Stopped);
        }
        self.halt();
        Ok(())
    }

    /// Restarts capture after [`FrameStream::pause`] or after the target
    /// went away. Clears the buffer, since the screen may have changed
    /// meanwhile. No-op if capturing.
    pub fn resume(&mut self) -> Result<(), FrameError> {
        if self.stopped {
            return Err(FrameError::Stopped);
        }
        if self.is_capturing() {
            return Ok(());
        }
        self.running = None;
        let generation = self.reader.begin_run();
        match Running::start(&self.cfg, &self.reader, generation) {
            Ok(r) => {
                self.running = Some(r);
                self.reader.clear_error();
                Ok(())
            }
            Err(e) => {
                self.reader.end_run();
                Err(e)
            }
        }
    }

    /// Stops capture for good. Buffered frames stay readable through
    /// existing [`FrameReader`]s.
    pub fn stop(mut self) -> Result<(), FrameError> {
        self.stopped = true;
        self.halt();
        Ok(())
    }

    fn halt(&mut self) {
        self.reader.end_run();
        self.running = None;
    }

    /// See [`FrameReader::frame_before`].
    pub fn frame_before(&self, ts: Timestamp) -> Option<Frame> {
        self.reader.frame_before(ts)
    }

    /// See [`FrameReader::frame_after`].
    pub fn frame_after(
        &self,
        ts: Timestamp,
        min_delay_ms: u64,
        timeout: Duration,
    ) -> Option<Frame> {
        self.reader.frame_after(ts, min_delay_ms, timeout)
    }
}

impl Drop for FrameStream {
    fn drop(&mut self) {
        self.stopped = true;
        self.halt();
    }
}

fn excludes_own_windows(cfg: &FrameStreamConfig) -> bool {
    cfg.exclude_own_windows && !matches!(cfg.target, CaptureTarget::Window { .. })
}

/// What a run captures, resolved.
#[derive(Debug, Clone, Copy)]
enum Source {
    /// A display, or a region of one: the monitor, its bounds and the
    /// part captured (both global points).
    Display {
        monitor: HMONITOR,
        bounds: Rect,
        area: Rect,
    },
    Window(HWND),
}

// SAFETY: monitor and window handles are plain values, usable from any
// thread.
unsafe impl Send for Source {}

fn resolve(target: CaptureTarget) -> Result<Source, FrameError> {
    let _dpi = PerMonitorAware::enter();
    let display = |id: Option<u32>| -> Result<(HMONITOR, Rect), FrameError> {
        let monitor = match id {
            Some(id) => monitor_for(id).ok_or(FrameError::DisplayNotFound(id))?,
            // SAFETY: a plain query.
            None => unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) },
        };
        let info = display(monitor).ok_or(FrameError::DisplayNotFound(id.unwrap_or(0)))?;
        Ok((monitor, info.bounds))
    };
    match target {
        CaptureTarget::Display { display_id } => {
            let (monitor, bounds) = display(display_id)?;
            Ok(Source::Display {
                monitor,
                bounds,
                area: bounds,
            })
        }
        CaptureTarget::Region { display_id, rect } => {
            let (monitor, bounds) = display(display_id)?;
            let area = intersect(rect, bounds).ok_or_else(|| {
                FrameError::InvalidConfig("the region is not on the display".into())
            })?;
            Ok(Source::Display {
                monitor,
                bounds,
                area,
            })
        }
        CaptureTarget::Window { window_id } => {
            let window = window_handle(window_id);
            if !window_exists(window_id) {
                return Err(FrameError::WindowNotFound(window_id));
            }
            Ok(Source::Window(window))
        }
    }
}

/// One capture run.
struct Running {
    session: GraphicsCaptureSession,
    pool: Direct3D11CaptureFramePool,
    item: GraphicsCaptureItem,
    arrived: i64,
    closed: i64,
    geometry: FrameGeometry,
    own_windows: Arc<OwnWindows>,
}

impl Running {
    fn start(
        cfg: &FrameStreamConfig,
        reader: &FrameReader,
        generation: u64,
    ) -> Result<Self, FrameError> {
        super::ensure_com();
        let source = resolve(cfg.target)?;
        if !GraphicsCaptureSession::IsSupported().unwrap_or(false) {
            return Err(FrameError::Unsupported);
        }
        request_borderless();
        let stream = |e: windows::core::Error| FrameError::Stream(e.message());
        let gpu = Gpu::new()?;
        let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
            .map_err(stream)?;
        // SAFETY: a monitor or window handle resolved just now.
        let item: GraphicsCaptureItem = unsafe {
            match source {
                Source::Display { monitor, .. } => interop.CreateForMonitor(monitor),
                Source::Window(window) => interop.CreateForWindow(window),
            }
        }
        .map_err(|e| match (cfg.target, source) {
            (CaptureTarget::Window { window_id }, _) => {
                if window_exists(window_id) {
                    stream(e)
                } else {
                    FrameError::WindowNotFound(window_id)
                }
            }
            _ => stream(e),
        })?;
        let size = item.Size().map_err(stream)?;
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &gpu.winrt,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            POOL_FRAMES,
            size,
        )
        .map_err(stream)?;
        let session = pool.CreateCaptureSession(&item).map_err(stream)?;
        // Each is newer than Windows.Graphics.Capture itself; without it the
        // cursor shows, a border is drawn around the capture, and frames come
        // as often as the screen changes (the copy still drops the extra
        // ones).
        let _ = session.SetIsCursorCaptureEnabled(cfg.shows_cursor);
        let _ = session.SetIsBorderRequired(false);
        let _ = session.SetMinUpdateInterval(TimeSpan {
            Duration: 10_000_000 / i64::from(cfg.fps),
        });

        let geometry = match source {
            Source::Display { area, .. } => {
                FrameGeometry::from_rect(area, area.width as u32, area.height as u32)
            }
            Source::Window(window) => {
                let rect = window_bounds(window).unwrap_or(Rect {
                    x: 0.0,
                    y: 0.0,
                    width: f64::from(size.Width),
                    height: f64::from(size.Height),
                });
                FrameGeometry::from_rect(rect, rect.width as u32, rect.height as u32)
            }
        };
        let own_windows = Arc::new(OwnWindows::new(excludes_own_windows(cfg)));
        own_windows.refresh();

        let copier = Mutex::new(Copier {
            gpu,
            source,
            pool_size: size,
            staging: None,
            min_interval_ns: 1_000_000_000 / u64::from(cfg.fps),
            last_kept: None,
            reader: reader.clone(),
            generation,
            own_windows: Arc::clone(&own_windows),
        });
        let arrived = pool
            .FrameArrived(
                &TypedEventHandler::<Direct3D11CaptureFramePool, IInspectable>::new(
                    move |pool, _| {
                        if let Some(pool) = pool.as_ref() {
                            let mut copier = copier.lock().unwrap_or_else(|e| e.into_inner());
                            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                copier.arrived(pool)
                            }));
                        }
                        Ok(())
                    },
                ),
            )
            .map_err(stream)?;
        let closed_reader = reader.clone();
        let what = match source {
            Source::Display { .. } => "display",
            Source::Window(_) => "window",
        };
        let closed = item
            .Closed(
                &TypedEventHandler::<GraphicsCaptureItem, IInspectable>::new(move |_, _| {
                    closed_reader.system_stopped(
                        generation,
                        format!("the captured {what} went away {SOURCE_GONE}"),
                    );
                    Ok(())
                }),
            )
            .map_err(stream)?;
        let running = Self {
            session,
            pool,
            item,
            arrived,
            closed,
            geometry,
            own_windows,
        };
        running.session.StartCapture().map_err(stream)?;
        Ok(running)
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.pool.RemoveFrameArrived(self.arrived);
        let _ = self.item.RemoveClosed(self.closed);
        let _ = self.session.Close();
        let _ = self.pool.Close();
        self.own_windows.restore();
    }
}

/// Asks, once, to capture without the border Windows draws around a
/// capture. Granted without a prompt to desktop apps where the setting
/// exists (Windows 11); elsewhere it fails and the border stays.
fn request_borderless() {
    static ASKED: OnceLock<()> = OnceLock::new();
    ASKED.get_or_init(|| {
        if let Ok(op) =
            GraphicsCaptureAccess::RequestAccessAsync(GraphicsCaptureAccessKind::Borderless)
        {
            let _ = op.join();
        }
    });
}

/// The Direct3D device frames are captured with and copied on.
struct Gpu {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    winrt: IDirect3DDevice,
}

impl Gpu {
    fn new() -> Result<Self, FrameError> {
        let stream = |e: windows::core::Error| FrameError::Stream(e.message());
        let mut created = Err(FrameError::Stream("no Direct3D device".into()));
        for driver in [D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP] {
            created = create_device(driver);
            if created.is_ok() {
                break;
            }
        }
        let (device, context) = created?;
        let dxgi: IDXGIDevice = device.cast().map_err(stream)?;
        // SAFETY: a live DXGI device.
        let inspectable = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }.map_err(stream)?;
        let winrt: IDirect3DDevice = inspectable.cast().map_err(stream)?;
        Ok(Self {
            device,
            context,
            winrt,
        })
    }
}

fn create_device(
    driver: D3D_DRIVER_TYPE,
) -> Result<(ID3D11Device, ID3D11DeviceContext), FrameError> {
    let mut device = None;
    let mut context = None;
    // SAFETY: valid out-pointers; BGRA support is what capture needs.
    unsafe {
        D3D11CreateDevice(
            None,
            driver,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
    }
    .map_err(|e| FrameError::Stream(e.message()))?;
    device
        .zip(context)
        .ok_or_else(|| FrameError::Stream("no Direct3D device".into()))
}

/// Copies arrived frames into the ring, on the pool's worker thread.
struct Copier {
    gpu: Gpu,
    source: Source,
    /// The size the pool was made for.
    pool_size: SizeInt32,
    /// The CPU-readable texture frames are copied through, with its size.
    staging: Option<(ID3D11Texture2D, u32, u32)>,
    min_interval_ns: u64,
    last_kept: Option<u64>,
    reader: FrameReader,
    generation: u64,
    own_windows: Arc<OwnWindows>,
}

// SAFETY: the Direct3D 11 device is free-threaded; the immediate context
// and the staging texture are only used by the copier, under its mutex.
unsafe impl Send for Copier {}

impl Copier {
    fn arrived(&mut self, pool: &Direct3D11CaptureFramePool) {
        let Ok(frame) = pool.TryGetNextFrame() else {
            return;
        };
        let Ok(time) = frame.SystemRelativeTime() else {
            return;
        };
        let ns = u64::try_from(time.Duration)
            .unwrap_or(0)
            .saturating_mul(100);
        let content = frame.ContentSize().unwrap_or(self.pool_size);
        if self
            .last_kept
            .is_some_and(|last| ns.saturating_sub(last) < self.min_interval_ns * 9 / 10)
        {
            return;
        }
        let _dpi = PerMonitorAware::enter();
        if let Some(copied) = self.copy(&frame, content, ns) {
            self.last_kept = Some(ns);
            self.reader.push_from(self.generation, copied);
        }
        drop(frame);
        if content.Width > 0
            && content.Height > 0
            && (content.Width != self.pool_size.Width || content.Height != self.pool_size.Height)
        {
            // A window was resized: frames come at the new size from now.
            if pool
                .Recreate(
                    &self.gpu.winrt,
                    DirectXPixelFormat::B8G8R8A8UIntNormalized,
                    POOL_FRAMES,
                    content,
                )
                .is_ok()
            {
                self.pool_size = content;
            }
        }
        self.own_windows.refresh_if_due();
    }

    /// The frame's captured area as a [`Frame`], copied off the GPU.
    fn copy(
        &mut self,
        frame: &windows::Graphics::Capture::Direct3D11CaptureFrame,
        content: SizeInt32,
        ns: u64,
    ) -> Option<Frame> {
        let surface = frame.Surface().ok()?;
        let access: IDirect3DDxgiInterfaceAccess = surface.cast().ok()?;
        // SAFETY: a live surface backed by a Direct3D 11 texture.
        let texture: ID3D11Texture2D = unsafe { access.GetInterface() }.ok()?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: a valid out-pointer.
        unsafe { texture.GetDesc(&mut desc) };
        let content_w = u32::try_from(content.Width).ok()?.min(desc.Width);
        let content_h = u32::try_from(content.Height).ok()?.min(desc.Height);
        // The part of the texture to keep, and where it is on screen.
        let (area, rect) = match self.source {
            Source::Display { bounds, area, .. } => {
                let left = ((area.x - bounds.x).max(0.0) as u32).min(content_w);
                let top = ((area.y - bounds.y).max(0.0) as u32).min(content_h);
                let right = (left + area.width as u32).min(content_w);
                let bottom = (top + area.height as u32).min(content_h);
                (
                    (left, top, right, bottom),
                    Rect {
                        x: bounds.x + f64::from(left),
                        y: bounds.y + f64::from(top),
                        width: f64::from(right - left),
                        height: f64::from(bottom - top),
                    },
                )
            }
            Source::Window(window) => {
                let origin = window_bounds(window)?;
                (
                    (0, 0, content_w, content_h),
                    Rect {
                        x: origin.x,
                        y: origin.y,
                        width: f64::from(content_w),
                        height: f64::from(content_h),
                    },
                )
            }
        };
        let (left, top, right, bottom) = area;
        let (width, height) = (right.checked_sub(left)?, bottom.checked_sub(top)?);
        if width == 0 || height == 0 {
            return None;
        }
        let staging = self.staging(&desc, width, height)?;
        let region = D3D11_BOX {
            left,
            top,
            front: 0,
            right,
            bottom,
            back: 1,
        };
        let context = &self.gpu.context;
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: both textures are live and of the same format; the box is
        // inside the source; the staging texture is CPU-readable and
        // unmapped below.
        unsafe {
            context.CopySubresourceRegion(&staging, 0, 0, 0, 0, &texture, 0, Some(&region));
            context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .ok()?;
        }
        let row = width as usize * 4;
        let mut pixels = self.reader.take_buffer(row * height as usize);
        for y in 0..height as usize {
            // SAFETY: the mapped texture has `height` rows of `RowPitch`
            // bytes, each holding at least `row` bytes of pixels.
            let line = unsafe {
                std::slice::from_raw_parts(
                    (mapped.pData as *const u8).add(y * mapped.RowPitch as usize),
                    row,
                )
            };
            pixels.extend_from_slice(line);
        }
        // SAFETY: mapped above.
        unsafe { context.Unmap(&staging, 0) };
        Some(Frame {
            ts: Timestamp(ns),
            width,
            height,
            geometry: FrameGeometry::from_rect(rect, width, height),
            pixels: Arc::new(pixels),
        })
    }

    /// The staging texture for a `width`×`height` copy of a `desc` texture,
    /// made again when the size changes.
    fn staging(
        &mut self,
        desc: &D3D11_TEXTURE2D_DESC,
        width: u32,
        height: u32,
    ) -> Option<ID3D11Texture2D> {
        if let Some((texture, w, h)) = &self.staging {
            if (*w, *h) == (width, height) {
                return Some(texture.clone());
            }
        }
        let staging_desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: desc.Format,
            SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut texture = None;
        // SAFETY: a valid description and out-pointer.
        unsafe {
            self.gpu
                .device
                .CreateTexture2D(&staging_desc, None, Some(&mut texture))
        }
        .ok()?;
        let texture = texture?;
        self.staging = Some((texture.clone(), width, height));
        Some(texture)
    }
}

/// This process's top-level windows kept out of captures by display
/// affinity, to restore when the run ends (see `# Excluded windows`).
struct OwnWindows {
    enabled: bool,
    state: Mutex<OwnState>,
}

#[derive(Default)]
struct OwnState {
    /// Windows whose affinity this run set (handles as integers).
    set: Vec<isize>,
    last_refresh: Option<Instant>,
}

impl OwnWindows {
    fn new(enabled: bool) -> Self {
        Self {
            enabled,
            state: Mutex::default(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, OwnState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn refresh_if_due(&self) {
        let due = self
            .lock()
            .last_refresh
            .is_none_or(|at| at.elapsed() >= OWN_WINDOWS_EVERY);
        if due {
            self.refresh();
        }
    }

    /// Excludes every top-level window of this process not excluded yet.
    /// A window already excluded by its owner is left to it.
    fn refresh(&self) {
        if !self.enabled {
            return;
        }
        let own = i32::try_from(std::process::id()).unwrap_or(-1);
        let mut state = self.lock();
        state.last_refresh = Some(Instant::now());
        for window in top_level_windows() {
            if window_pid(window) != own || state.set.contains(&(window.0 as isize)) {
                continue;
            }
            let mut affinity = 0u32;
            // SAFETY: plain queries and a setter on this process's window.
            let excluded = unsafe {
                GetWindowDisplayAffinity(window, &mut affinity).is_ok()
                    && affinity == WDA_NONE.0
                    && SetWindowDisplayAffinity(window, WDA_EXCLUDEFROMCAPTURE).is_ok()
            };
            if excluded {
                state.set.push(window.0 as isize);
            }
        }
    }

    /// Clears the affinity this run set.
    fn restore(&self) {
        for window in std::mem::take(&mut self.lock().set) {
            let window = HWND(window as *mut c_void);
            // SAFETY: a window of this process; a closed one just fails.
            let _ = unsafe { SetWindowDisplayAffinity(window, WDA_NONE) };
        }
    }
}

fn rect_from(r: RECT) -> Rect {
    Rect {
        x: f64::from(r.left),
        y: f64::from(r.top),
        width: f64::from(r.right - r.left),
        height: f64::from(r.bottom - r.top),
    }
}

fn rect_contains(r: Rect, p: Point) -> bool {
    p.x >= r.x && p.y >= r.y && p.x < r.x + r.width && p.y < r.y + r.height
}

/// The overlap of `a` and `b`, snapped to whole pixels; `None` if empty.
fn intersect(a: Rect, b: Rect) -> Option<Rect> {
    let left = a.x.max(b.x).round();
    let top = a.y.max(b.y).round();
    let right = (a.x + a.width).min(b.x + b.width).round();
    let bottom = (a.y + a.height).min(b.y + b.height).round();
    (right > left && bottom > top).then_some(Rect {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    })
}

#[cfg(test)]
mod tests {
    use super::super::event_tap;
    use super::*;

    fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn displays_are_listed_at_scale_one() {
        let all = DisplayInfo::all();
        assert!(!all.is_empty(), "no display");
        let main = DisplayInfo::main().expect("a primary display");
        // The primary display has the origin at its top left.
        assert_eq!((main.bounds.x, main.bounds.y), (0.0, 0.0));
        assert!(all.iter().all(|d| d.scale == 1.0));
        assert_eq!(DisplayInfo::for_id(main.id), Some(main));
        assert_eq!(
            DisplayInfo::containing(Point { x: 1.0, y: 1.0 }).map(|d| d.id),
            Some(main.id)
        );
        assert!(mouse_location().is_some());
    }

    #[test]
    fn window_ids_round_trip() {
        let windows = list_windows().unwrap();
        assert!(!windows.is_empty());
        for w in windows.iter().take(20) {
            assert!(window_exists(w.window_id) || !w.on_screen);
        }
        assert!(!window_exists(0));
    }

    #[test]
    fn regions_are_clipped_to_whole_pixels() {
        let display = rect(0.0, 0.0, 1920.0, 1080.0);
        assert_eq!(
            intersect(rect(-10.0, 100.4, 500.0, 200.0), display),
            Some(rect(0.0, 100.0, 490.0, 200.0))
        );
        assert_eq!(intersect(rect(2000.0, 0.0, 10.0, 10.0), display), None);
    }

    #[test]
    fn only_gone_sources_are_gone() {
        assert!(is_source_gone_error(&format!(
            "the captured window went away {SOURCE_GONE}"
        )));
        assert!(!is_source_gone_error("screen capture failed"));
    }

    #[test]
    fn filter_handle_has_nothing_to_do() {
        let handle = FilterHandle(());
        assert!(!handle.filters_apps());
        assert!(!handle.covers(1));
        assert!(handle.refresh().is_ok());
    }

    #[test]
    fn captures_the_main_display() {
        let stream = FrameStream::start(FrameStreamConfig::default()).expect("capture starts");
        let main = DisplayInfo::main().unwrap();
        assert_eq!(stream.geometry().unwrap().rect_points(), main.bounds);
        // Wait for a first frame (the screen may be static: one still comes).
        let t0 = event_tap::now();
        let frame = stream
            .reader()
            .frame_after(Timestamp(0), 0, Duration::from_secs(5))
            .expect("a frame");
        assert_eq!((frame.width, frame.height), main.pixel_size());
        assert_eq!(
            frame.pixels.len(),
            frame.width as usize * frame.height as usize * 4
        );
        // Stamped on the event clock: when it is shown, just ahead of now.
        assert!(frame.ts.0 <= event_tap::now().0 + 50_000_000);
        assert!(t0.0.saturating_sub(frame.ts.0) < 60_000_000_000);
        stream.stop().unwrap();
    }
}
