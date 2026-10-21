//! ScreenCaptureKit stream with a rolling frame buffer. See design
//! scratchpad #440 ("Screen frames").
//!
//! [`FrameStream`] runs an `SCStream` at a low frame rate (default 12 fps)
//! and keeps the last few frames in a [`FrameRing`]. Step capture then picks
//! the newest frame taken before a click ([`FrameStream::frame_before`]) and,
//! optionally, the first frame some time after it
//! ([`FrameStream::frame_after`]).
//!
//! # Timestamps
//! Each frame's [`Timestamp`] is the sample buffer's presentation time.
//! ScreenCaptureKit stamps buffers on the host time clock (mach absolute
//! time), so `seconds * 1e9` is the same host monotonic ns clock as
//! [`crate::macos::event_tap`] events, and frames and clicks compare directly.
//!
//! ScreenCaptureKit only delivers a new image when the screen content
//! changes, so on a static screen the newest frame can be several seconds
//! old. That is still the correct "before" frame. Only frames whose
//! `SCStreamFrameInfoStatus` is `Complete` or `Started` enter the ring;
//! idle, blank, suspended and stopped buffers are skipped.
//!
//! # Copy costs
//! Every delivered frame is copied once, on the stream's dispatch queue, from
//! the IOSurface-backed `CVPixelBuffer` into a tightly packed BGRA `Vec<u8>`
//! (row padding dropped; a single `memcpy` when the buffer has none). A
//! 2880×1800 Retina display is ~20.7 MB per frame (~59 MB on 5K); at 12 fps
//! that is ~250 MB/s of copying and ~62 MB resident for a ring of 3.
//!
//! With [`FrameStreamConfig::reuse_buffers`] (the default), buffers of
//! frames evicted from the ring are kept in a small pool and reused, so
//! steady-state capture does no fresh multi-MB allocations (which would
//! page-fault and zero-fill each time). A buffer is only reused if nobody
//! else still holds the frame.
//!
//! We copy rather than retain the pixel buffer because ScreenCaptureKit's
//! surface pool is only `queueDepth` deep, and holding surfaces in the ring
//! would starve it and stall the stream. [`Frame`] shares its pixels through
//! an `Arc`, so cloning a frame out of the ring is free. The WebP encoder
//! reads `pixels` as BGRA directly.
//!
//! # Coordinates
//! Every frame carries a [`FrameGeometry`] that maps global display points
//! (the coordinate space of [`guiderecorder_core::events`]) to frame pixels, and
//! [`DisplayInfo`] gives the same mapping for a whole display. For window
//! targets the geometry is recomputed per frame from the window's current
//! position and the frame's `ContentRect` / `ContentScale` / `ScaleFactor`
//! attachments, so it stays right when the window moves or resizes.
//!
//! # Excluded apps
//! Display and region captures leave out, besides this process (see
//! [`FrameStreamConfig::exclude_own_windows`]), every application matching
//! [`FrameStreamConfig::excluded_bundle_ids`] by
//! [`bundle_id_matches`]: an exact id, or the id followed by `.` and more
//! (its helpers). ScreenCaptureKit then never puts their windows in a frame,
//! whether they are in front, behind another app, or a non-activating panel
//! such as 1Password's Quick Access. The filter is built from the
//! applications listed when the run starts; one that launches (or opens its
//! first window) later is only left out after [`FrameStream::refresh_filter`]
//! or [`FilterHandle::refresh`] (the recording session does this on launch
//! and activation). [`FrameStream::excluded_pids`] tells which are covered.
//! Window captures are of one window only and need no exclusion.
//!
//! # Threading
//! [`FrameStream::start`], [`FrameStream::pause`], [`FrameStream::resume`]
//! and [`FrameStream::stop`] block until ScreenCaptureKit answers (normally
//! well under a second, at most ~5 s). Completion handlers run on an
//! internal SCK queue, so this cannot deadlock, but call them off the UI
//! thread to keep it responsive.

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchRetained};
use guiderecorder_core::events::Timestamp;
use guiderecorder_core::model::{Point, Rect};
use objc2::encode::{Encoding, RefEncode};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGDisplayBounds, CGDisplayCopyDisplayMode, CGDisplayMode, CGEvent, CGGetActiveDisplayList,
    CGMainDisplayID, CGWindowListCopyWindowInfo, CGWindowListOption,
};
use objc2_foundation::{
    NSArray, NSDictionary, NSError, NSNumber, NSObject, NSObjectProtocol, NSString,
};
use objc2_screen_capture_kit::{
    SCContentFilter, SCDisplay, SCFrameStatus, SCShareableContent, SCStream, SCStreamConfiguration,
    SCStreamDelegate, SCStreamErrorCode, SCStreamFrameInfoContentRect,
    SCStreamFrameInfoContentScale, SCStreamFrameInfoScaleFactor, SCStreamFrameInfoStatus,
    SCStreamOutput, SCStreamOutputType, SCWindow,
};

pub use crate::frames::*;
use crate::permissions::{self, Permission};
pub use crate::ring::*;

/// How long to wait for ScreenCaptureKit's asynchronous calls.
const SCK_TIMEOUT: Duration = Duration::from_secs(5);
/// `kCVPixelFormatType_32BGRA` ('BGRA').
const PIXEL_FORMAT_BGRA: u32 = u32::from_be_bytes(*b"BGRA");

impl DisplayInfo {
    pub fn main() -> Option<Self> {
        Self::for_id(CGMainDisplayID())
    }

    pub fn for_id(id: u32) -> Option<Self> {
        let bounds = rect_from_cg(CGDisplayBounds(id));
        if bounds.width <= 0.0 || bounds.height <= 0.0 {
            return None;
        }
        let mode = CGDisplayCopyDisplayMode(id);
        let points = CGDisplayMode::width(mode.as_deref());
        let pixels = CGDisplayMode::pixel_width(mode.as_deref());
        let scale = if points > 0 && pixels > 0 {
            pixels as f64 / points as f64
        } else {
            1.0
        };
        Some(Self { id, bounds, scale })
    }

    /// All active displays.
    pub fn all() -> Vec<Self> {
        let mut ids = [0u32; 32];
        let mut count = 0u32;
        // SAFETY: `ids` has room for 32 entries and `count` is a valid out pointer.
        let err = unsafe { CGGetActiveDisplayList(32, ids.as_mut_ptr(), &mut count) };
        if err.0 != 0 {
            return Vec::new();
        }
        ids[..count.min(32) as usize]
            .iter()
            .filter_map(|&id| Self::for_id(id))
            .collect()
    }

    /// The display whose bounds contain `p` (global points).
    pub fn containing(p: Point) -> Option<Self> {
        Self::all().into_iter().find(|d| rect_contains(d.bounds, p))
    }
}

/// Where the mouse pointer is now, in global points (top-left origin, the
/// same space as event points). `None` if no event can be created.
pub fn mouse_location() -> Option<Point> {
    let event = CGEvent::new(None)?;
    let p = CGEvent::location(Some(&event));
    Some(Point { x: p.x, y: p.y })
}

/// A running (or paused) ScreenCaptureKit capture.
///
/// `pause()` and `resume()` really stop and restart the `SCStream`; resume
/// rebuilds the content filter, so windows this process opened while paused
/// are excluded and a window target's current position is picked up. If the
/// system stops the stream, [`FrameStream::is_capturing`] turns false,
/// [`FrameReader::last_error`] says why, and `resume()` starts a new one.
pub struct FrameStream {
    cfg: FrameStreamConfig,
    reader: FrameReader,
    running: Option<Running>,
    stopped: bool,
    filter: FilterHandle,
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
    /// Checks Screen Recording permission and starts capturing. Blocks until
    /// ScreenCaptureKit has started the stream (see the module docs on
    /// threading).
    pub fn start(cfg: FrameStreamConfig) -> Result<Self, FrameError> {
        cfg.validate()?;
        if !permissions::check(Permission::ScreenRecording).is_granted() {
            return Err(FrameError::PermissionDenied);
        }
        let reader = FrameReader::with_pool(cfg.ring_capacity, cfg.reuse_buffers);
        let generation = reader.begin_run();
        let filter = FilterHandle::new(cfg.clone());
        let running =
            Running::start(&cfg, &reader, generation).inspect_err(|_| reader.end_run())?;
        filter.attach(&running);
        Ok(Self {
            cfg,
            reader,
            running: Some(running),
            stopped: false,
            filter,
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
    /// not stopped by the system.
    pub fn is_capturing(&self) -> bool {
        self.running.is_some() && !self.reader.stopped_by_system()
    }

    /// Geometry of the current run (`None` while paused). For window
    /// targets each frame carries its own, more current, geometry.
    pub fn geometry(&self) -> Option<FrameGeometry> {
        self.running.as_ref().map(|r| r.geometry)
    }

    /// Whether this process is excluded from the current run's capture.
    /// False when [`FrameStreamConfig::exclude_own_windows`] is off, for
    /// window targets, and when the process had no windows at all when the
    /// filter was built; in that last case call
    /// [`FrameStream::refresh_filter`] once our windows exist.
    pub fn own_app_excluded(&self) -> bool {
        self.filter.coverage().own_app_excluded
    }

    /// Pids of the applications matching
    /// [`FrameStreamConfig::excluded_bundle_ids`] that the current run's
    /// filter leaves out (empty while paused and for window targets).
    pub fn excluded_pids(&self) -> Vec<i32> {
        self.filter.coverage().excluded_pids
    }

    /// Rebuilds the content filter without restarting the stream (e.g.
    /// after this app opens its first window, or an excluded app launches,
    /// so they can be excluded). Returns [`FrameStream::own_app_excluded`].
    /// The captured area and geometry are unchanged.
    pub fn refresh_filter(&mut self) -> Result<bool, FrameError> {
        if self.stopped {
            return Err(FrameError::Stopped);
        }
        Ok(self.filter.refresh()?.is_some_and(|c| c.own_app_excluded))
    }

    /// A handle that refreshes this stream's content filter from any
    /// thread, for as long as the stream runs; see [`FilterHandle`].
    pub fn filter_handle(&self) -> FilterHandle {
        self.filter.clone()
    }

    /// Stops the `SCStream`. Buffered frames stay readable. No-op if
    /// already paused. On error the stream is abandoned (stopped in the
    /// background) and the stream counts as paused.
    pub fn pause(&mut self) -> Result<(), FrameError> {
        if self.stopped {
            return Err(FrameError::Stopped);
        }
        self.halt()
    }

    /// Restarts capture after [`FrameStream::pause`] or after the system
    /// stopped the stream. Clears the buffer, since the screen may have
    /// changed meanwhile. No-op if capturing.
    pub fn resume(&mut self) -> Result<(), FrameError> {
        if self.stopped {
            return Err(FrameError::Stopped);
        }
        if self.is_capturing() {
            return Ok(());
        }
        // A stream the system stopped is dead; release it without stopping.
        if let Some(dead) = self.running.take() {
            dead.release();
        }
        let generation = self.reader.begin_run();
        match Running::start(&self.cfg, &self.reader, generation) {
            Ok(r) => {
                self.filter.attach(&r);
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
        self.halt()
    }

    fn halt(&mut self) -> Result<(), FrameError> {
        // First, so no refresh touches a stream being stopped (this waits
        // for one in progress).
        self.filter.detach();
        let system_stopped = self.reader.stopped_by_system();
        self.reader.end_run();
        match self.running.take() {
            Some(r) if system_stopped => {
                r.release();
                Ok(())
            }
            Some(r) => r.stop(),
            None => Ok(()),
        }
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
        let _ = self.halt();
    }
}

/// One `SCStream` run.
struct Running {
    stream: Retained<SCStream>,
    // Kept alive for the stream's lifetime: it is the stream's output and
    // delegate.
    output: Retained<FrameOutput>,
    queue: DispatchRetained<DispatchQueue>,
    geometry: FrameGeometry,
    coverage: FilterCoverage,
}

// SAFETY: SCStream is documented as usable from any thread, and we only
// call its start/stop/update methods, never concurrently (they take
// `&mut FrameStream` or consume it). `FrameOutput` is messaged by
// ScreenCaptureKit on `queue` while we hold a retain on the owner's thread;
// that is fine because its ivars are `Send + Sync` (a `FrameReader`, plain
// data and a `Mutex`). `DispatchQueue` is thread-safe.
unsafe impl Send for Running {}
// SAFETY: as above; `&Running` only exposes `geometry` and `coverage`.
unsafe impl Sync for Running {}

impl Running {
    fn start(
        cfg: &FrameStreamConfig,
        reader: &FrameReader,
        generation: u64,
    ) -> Result<Self, FrameError> {
        let content = shareable_content()?;
        let plan = plan_capture(cfg, &content)?;
        let (rect_points, scale) = (plan.rect_points, plan.scale);

        let width = (rect_points.width * scale).round().max(1.0) as usize;
        let height = (rect_points.height * scale).round().max(1.0) as usize;
        let geometry = FrameGeometry::from_rect(rect_points, width as u32, height as u32);

        // SAFETY: plain property setters on a fresh configuration object.
        let config = unsafe {
            let config = SCStreamConfiguration::new();
            config.setWidth(width);
            config.setHeight(height);
            config.setPixelFormat(PIXEL_FORMAT_BGRA);
            let mut interval = config.minimumFrameInterval();
            write_cmtime(&mut interval, frame_interval(cfg.fps));
            config.setMinimumFrameInterval(interval);
            // Pool size: frames in flight plus slack; we copy out of each
            // surface immediately, so the ring does not hold any of these.
            config.setQueueDepth(5);
            config.setShowsCursor(cfg.shows_cursor);
            if let Some(src) = plan.source_rect {
                config.setSourceRect(src);
            }
            config
        };

        let window = match cfg.target {
            CaptureTarget::Window { window_id } => Some(WindowTracking {
                id: window_id,
                last_rect: Mutex::new(rect_points),
            }),
            _ => None,
        };
        let output = FrameOutput::new(OutputIvars {
            reader: reader.clone(),
            generation,
            rect_points,
            window,
        });
        let queue = DispatchQueue::new("dev.dufflebear.guiderecorder.capture.frames", None);
        let delegate = ProtocolObject::<dyn SCStreamDelegate>::from_ref(&*output);
        // SAFETY: filter and config are valid; the delegate is retained by
        // `Running` for as long as the stream exists.
        let stream = unsafe {
            SCStream::initWithFilter_configuration_delegate(
                SCStream::alloc(),
                &plan.filter,
                &config,
                Some(delegate),
            )
        };
        let as_output = ProtocolObject::<dyn SCStreamOutput>::from_ref(&*output);
        // SAFETY: the queue is a serial queue we own, kept alive in `Running`.
        unsafe {
            stream.addStreamOutput_type_sampleHandlerQueue_error(
                as_output,
                SCStreamOutputType::Screen,
                Some(&queue),
            )
        }
        .map_err(|e| FrameError::Stream(describe(&e)))?;

        let running = Self {
            stream,
            output,
            queue,
            geometry,
            coverage: plan.coverage,
        };
        let stream = &running.stream;
        // SAFETY: valid stream; the handler only sends on a channel.
        let done = wait_for_completion(|handler| unsafe {
            stream.startCaptureWithCompletionHandler(Some(handler))
        });
        match done {
            Completion::Done => Ok(running),
            Completion::Failed { message, .. } => Err(FrameError::Stream(message)),
            Completion::TimedOut => {
                // SCK may still be starting; make sure it ends up stopped.
                running.abandon();
                Err(FrameError::Timeout)
            }
        }
    }

    /// Stops the stream and waits for it. On failure the stream is
    /// abandoned so it cannot keep running unowned.
    fn stop(self) -> Result<(), FrameError> {
        let stream = &self.stream;
        // SAFETY: valid stream; the handler only sends on a channel.
        let done = wait_for_completion(|handler| unsafe {
            stream.stopCaptureWithCompletionHandler(Some(handler))
        });
        let already_stopped = SCStreamErrorCode::AttemptToStopStreamState.0;
        match done {
            Completion::Done => {
                self.release();
                Ok(())
            }
            Completion::Failed { code, .. } if code == already_stopped => {
                self.release();
                Ok(())
            }
            Completion::Failed { message, .. } => {
                self.abandon();
                Err(FrameError::Stream(message))
            }
            Completion::TimedOut => {
                self.abandon();
                Err(FrameError::Timeout)
            }
        }
    }

    /// Drops a stopped stream after draining callbacks already queued on
    /// its sample queue (they are ignored anyway: their generation is over).
    fn release(self) {
        // SAFETY: valid stream and output; failure only means the output
        // was already removed.
        let _ = unsafe {
            self.stream.removeStreamOutput_type_error(
                ProtocolObject::from_ref(&*self.output),
                SCStreamOutputType::Screen,
            )
        };
        self.queue.exec_sync(|| {});
    }

    /// Asks the stream to stop without waiting, keeping every object alive
    /// until ScreenCaptureKit answers.
    fn abandon(self) {
        let stream = self.stream.clone();
        let keep = Mutex::new(Some(self));
        let handler = RcBlock::new(move |_error: *mut NSError| {
            if let Some(r) = keep.lock().unwrap_or_else(|e| e.into_inner()).take() {
                drop(r.output);
            }
        });
        // SAFETY: valid stream; the handler owns everything it touches.
        unsafe { stream.stopCaptureWithCompletionHandler(Some(&handler)) };
    }
}

/// Overwrites a `CMTime` (from `objc2-core-media`, which cannot be named
/// here as it is only a transitive dependency) with `raw`.
///
/// # Safety
/// `T` must be `CMTime`.
unsafe fn write_cmtime<T>(time: &mut T, raw: CMTimeRaw) {
    assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<CMTimeRaw>());
    // SAFETY: `CMTime` and `CMTimeRaw` are both repr(C) {i64, i32, u32,
    // i64}; the size is checked above.
    unsafe { std::ptr::write((time as *mut T).cast::<CMTimeRaw>(), raw) }
}

/// `1/fps` with the Valid flag set.
fn frame_interval(fps: u32) -> CMTimeRaw {
    CMTimeRaw {
        value: 1,
        timescale: fps as i32,
        flags: CMTIME_VALID,
        epoch: 0,
    }
}

/// Filter plus the global rect it shows and its pixels per point.
struct CapturePlan {
    filter: Retained<SCContentFilter>,
    rect_points: Rect,
    scale: f64,
    /// Crop in display-local points, for region targets.
    source_rect: Option<CGRect>,
    coverage: FilterCoverage,
}

fn plan_capture(
    cfg: &FrameStreamConfig,
    content: &SCShareableContent,
) -> Result<CapturePlan, FrameError> {
    match cfg.target {
        CaptureTarget::Display { display_id } => {
            let (display, info) = find_display(content, display_id)?;
            let (filter, coverage) = display_filter(content, &display, cfg);
            Ok(CapturePlan {
                filter,
                rect_points: info.bounds,
                scale: info.scale,
                source_rect: None,
                coverage,
            })
        }
        CaptureTarget::Region { display_id, rect } => {
            let (display, info) = find_display(content, display_id)?;
            let clipped = intersect(rect, info.bounds).ok_or_else(|| {
                FrameError::InvalidConfig(format!(
                    "region {rect:?} is outside display {} ({:?})",
                    info.id, info.bounds
                ))
            })?;
            let (filter, coverage) = display_filter(content, &display, cfg);
            let local = CGRect::new(
                CGPoint::new(clipped.x - info.bounds.x, clipped.y - info.bounds.y),
                CGSize::new(clipped.width, clipped.height),
            );
            Ok(CapturePlan {
                filter,
                rect_points: clipped,
                scale: info.scale,
                source_rect: Some(local),
                coverage,
            })
        }
        CaptureTarget::Window { window_id } => {
            // SAFETY: read-only accessors on immutable content objects.
            let window = unsafe { content.windows() }
                .iter()
                .find(|w| unsafe { w.windowID() } == window_id)
                .ok_or(FrameError::WindowNotFound(window_id))?;
            let frame = rect_from_cg(unsafe { window.frame() });
            let center = Point {
                x: frame.x + frame.width / 2.0,
                y: frame.y + frame.height / 2.0,
            };
            let scale = DisplayInfo::containing(center)
                .or_else(DisplayInfo::main)
                .map_or(1.0, |d| d.scale);
            // SAFETY: `window` comes from the current shareable content.
            let filter = unsafe {
                SCContentFilter::initWithDesktopIndependentWindow(SCContentFilter::alloc(), &window)
            };
            Ok(CapturePlan {
                filter,
                rect_points: frame,
                scale,
                source_rect: None,
                coverage: FilterCoverage::default(),
            })
        }
    }
}

fn find_display(
    content: &SCShareableContent,
    display_id: Option<u32>,
) -> Result<(Retained<SCDisplay>, DisplayInfo), FrameError> {
    let id = display_id.unwrap_or_else(|| CGMainDisplayID());
    // SAFETY: read-only accessors on immutable content objects.
    let display = unsafe { content.displays() }
        .iter()
        .find(|d| unsafe { d.displayID() } == id)
        .ok_or(FrameError::DisplayNotFound(id))?;
    let info = DisplayInfo::for_id(id).ok_or(FrameError::DisplayNotFound(id))?;
    Ok((display, info))
}

/// Display filter that leaves out this process and the excluded
/// applications ([`excluded_apps`]), and what it left out. Excluding an
/// application (rather than its current windows) also hides windows it
/// opens later, such as our recording toolbar or a password manager's
/// Quick Access panel. The content is listed with off-screen windows, so a
/// hidden window is enough for an app to be found; a process with no
/// windows at all is not listed, and then there is nothing to exclude yet.
fn display_filter(
    content: &SCShareableContent,
    display: &SCDisplay,
    cfg: &FrameStreamConfig,
) -> (Retained<SCContentFilter>, FilterCoverage) {
    let own_pid = std::process::id() as i32;
    // SAFETY: read-only accessors on immutable content objects; the filter
    // initialisers take valid objects from the same content.
    unsafe {
        let apps = content.applications();
        // An app without a bundle id has an empty one here.
        let listed: Vec<(Option<String>, i32)> = apps
            .iter()
            .map(|a| {
                let id = a.bundleIdentifier().to_string();
                ((!id.is_empty()).then_some(id), a.processID())
            })
            .collect();
        let pick = excluded_apps(
            listed.iter().map(|(id, pid)| (id.as_deref(), *pid)),
            own_pid,
            cfg.exclude_own_windows,
            &cfg.excluded_bundle_ids,
        );
        let coverage = FilterCoverage {
            own_app_excluded: pick.iter().any(|&i| listed[i].1 == own_pid),
            excluded_pids: pick
                .iter()
                .map(|&i| listed[i].1)
                .filter(|&pid| pid != own_pid)
                .collect(),
        };
        let filter = if pick.is_empty() {
            SCContentFilter::initWithDisplay_excludingWindows(
                SCContentFilter::alloc(),
                display,
                &NSArray::<SCWindow>::new(),
            )
        } else {
            let excluded: Vec<_> = pick.iter().map(|&i| apps.objectAtIndex(i)).collect();
            SCContentFilter::initWithDisplay_excludingApplications_exceptingWindows(
                SCContentFilter::alloc(),
                display,
                &NSArray::from_retained_slice(&excluded),
                &NSArray::new(),
            )
        };
        (filter, coverage)
    }
}

/// The windows on screen now, front to back, as ScreenCaptureKit lists
/// them (unfiltered: see [`pickable_windows`]). Checks Screen Recording
/// permission first. Blocks until ScreenCaptureKit answers.
pub fn list_windows() -> Result<Vec<WindowInfo>, FrameError> {
    if !permissions::check(Permission::ScreenRecording).is_granted() {
        return Err(FrameError::PermissionDenied);
    }
    let content = shareable_content_with(true)?;
    let non_empty = |s: Retained<NSString>| Some(s.to_string()).filter(|s| !s.is_empty());
    // SAFETY: read-only accessors on immutable content objects.
    let windows = unsafe { content.windows() }
        .iter()
        .map(|w| unsafe {
            let app = w.owningApplication();
            WindowInfo {
                window_id: w.windowID(),
                pid: app.as_ref().map_or(0, |a| a.processID()),
                app_name: app
                    .as_ref()
                    .map(|a| a.applicationName().to_string())
                    .unwrap_or_default(),
                bundle_id: app.as_ref().and_then(|a| non_empty(a.bundleIdentifier())),
                title: w.title().and_then(non_empty),
                frame: rect_from_cg(w.frame()),
                layer: w.windowLayer(),
                on_screen: w.isOnScreen(),
            }
        })
        .collect();
    Ok(windows)
}

/// What a run's display filter leaves out (nothing for a window target).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FilterCoverage {
    /// This process is left out.
    pub own_app_excluded: bool,
    /// Pids of the excluded applications (not this process) left out.
    pub excluded_pids: Vec<i32>,
}

/// Refreshes a [`FrameStream`]'s content filter from any thread (see
/// `# Excluded apps`), e.g. when an excluded app launches. Clones share the
/// stream. While the stream is paused or stopped a refresh does nothing;
/// after a resume it applies to the new run.
#[derive(Clone)]
pub struct FilterHandle(Arc<FilterSlot>);

struct FilterSlot {
    cfg: FrameStreamConfig,
    live: Mutex<Option<LiveFilter>>,
}

/// The running stream and what its filter covers.
struct LiveFilter {
    stream: Retained<SCStream>,
    coverage: FilterCoverage,
}

// SAFETY: SCStream is documented as usable from any thread; the slot only
// calls `updateContentFilter` on it, under the mutex, so never concurrently
// with another update, and `FrameStream::halt` detaches it (taking the same
// mutex) before stopping the stream.
unsafe impl Send for LiveFilter {}

impl std::fmt::Debug for FilterHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FilterHandle")
            .field("coverage", &self.coverage())
            .finish()
    }
}

impl FilterHandle {
    fn new(cfg: FrameStreamConfig) -> Self {
        Self(Arc::new(FilterSlot {
            cfg,
            live: Mutex::new(None),
        }))
    }

    fn lock(&self) -> MutexGuard<'_, Option<LiveFilter>> {
        self.0.live.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn attach(&self, running: &Running) {
        *self.lock() = Some(LiveFilter {
            stream: running.stream.clone(),
            coverage: running.coverage.clone(),
        });
    }

    fn detach(&self) {
        self.lock().take();
    }

    fn coverage(&self) -> FilterCoverage {
        self.lock()
            .as_ref()
            .map(|l| l.coverage.clone())
            .unwrap_or_default()
    }

    /// True for display and region targets, whose filter leaves apps out;
    /// a window target captures one window and has nothing to refresh for.
    pub fn filters_apps(&self) -> bool {
        !matches!(self.0.cfg.target, CaptureTarget::Window { .. })
    }

    /// True while the stream runs and its filter leaves out `pid`.
    pub fn covers(&self, pid: i32) -> bool {
        self.lock()
            .as_ref()
            .is_some_and(|l| l.coverage.excluded_pids.contains(&pid))
    }

    /// Lists the shareable content again and gives the running stream a
    /// new filter built from it (`SCStream.updateContentFilter`, macOS
    /// 12.3+ like the rest of ScreenCaptureKit). Each ScreenCaptureKit call
    /// waits at most 5 s; call it off the main thread.
    /// Returns `Ok(None)` while the stream is not running. The captured
    /// area and geometry are unchanged.
    pub fn refresh(&self) -> Result<Option<FilterCoverage>, FrameError> {
        if self.lock().is_none() {
            return Ok(None);
        }
        let content = shareable_content()?;
        let plan = plan_capture(&self.0.cfg, &content)?;
        let mut live = self.lock();
        let Some(live) = live.as_mut() else {
            return Ok(None);
        };
        let stream = &live.stream;
        // SAFETY: valid stream and filter; the handler only sends on a channel.
        let done = wait_for_completion(|handler| unsafe {
            stream.updateContentFilter_completionHandler(&plan.filter, Some(handler))
        });
        completion_result(done)?;
        live.coverage = plan.coverage;
        Ok(Some(live.coverage.clone()))
    }
}

/// `SCShareableContent` moved out of its completion handler.
struct SendContent(Retained<SCShareableContent>);
// SAFETY: SCShareableContent is an immutable snapshot, documented as usable
// from any thread.
unsafe impl Send for SendContent {}

/// All displays, applications and windows, including off-screen windows
/// (so a hidden window still makes this app excludable).
fn shareable_content() -> Result<Retained<SCShareableContent>, FrameError> {
    shareable_content_with(false)
}

/// Like [`shareable_content`], listing only on-screen windows when
/// `on_screen_only`.
fn shareable_content_with(
    on_screen_only: bool,
) -> Result<Retained<SCShareableContent>, FrameError> {
    let (tx, rx) = mpsc::channel();
    let handler = RcBlock::new(
        move |content: *mut SCShareableContent, error: *mut NSError| {
            // SAFETY: ScreenCaptureKit passes valid (or null) object pointers.
            let result = match unsafe { Retained::retain(content) } {
                Some(c) => Ok(SendContent(c)),
                None => Err(unsafe { error.as_ref() }
                    .map(describe)
                    .unwrap_or_else(|| "no content".into())),
            };
            let _ = tx.send(result);
        },
    );
    // SAFETY: the handler is a valid block; it is copied by the callee.
    unsafe {
        SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(
            true, on_screen_only, &handler,
        )
    };
    match rx.recv_timeout(SCK_TIMEOUT) {
        Ok(Ok(c)) => Ok(c.0),
        Ok(Err(e)) => Err(FrameError::ShareableContent(e)),
        Err(_) => Err(FrameError::Timeout),
    }
}

enum Completion {
    Done,
    Failed { code: isize, message: String },
    TimedOut,
}

fn completion_result(done: Completion) -> Result<(), FrameError> {
    match done {
        Completion::Done => Ok(()),
        Completion::Failed { message, .. } => Err(FrameError::Stream(message)),
        Completion::TimedOut => Err(FrameError::Timeout),
    }
}

/// Calls `f` with a completion handler and blocks until it fires.
fn wait_for_completion(f: impl FnOnce(&block2::DynBlock<dyn Fn(*mut NSError)>)) -> Completion {
    let (tx, rx) = mpsc::channel();
    let handler = RcBlock::new(move |error: *mut NSError| {
        // SAFETY: ScreenCaptureKit passes a valid NSError or null.
        let _ = tx.send(unsafe { error.as_ref() }.map(|e| (e.code(), describe(e))));
    });
    f(&handler);
    match rx.recv_timeout(SCK_TIMEOUT) {
        Ok(None) => Completion::Done,
        Ok(Some((code, message))) => Completion::Failed { code, message },
        Err(_) => Completion::TimedOut,
    }
}

/// Whether a stop error (as [`FrameReader::last_error`] gives it, ending in
/// `(code N)`) says the captured window or display is gone, rather than
/// e.g. a screen lock or a revoked permission.
pub fn is_source_gone_error(error: &str) -> bool {
    let code = error
        .rsplit_once("(code ")
        .and_then(|(_, rest)| rest.strip_suffix(')'))
        .and_then(|n| n.trim().parse::<isize>().ok());
    code.is_some_and(|c| {
        [
            SCStreamErrorCode::NoCaptureSource,
            SCStreamErrorCode::NoWindowList,
            SCStreamErrorCode::NoDisplayList,
        ]
        .iter()
        .any(|gone| gone.0 == c)
    })
}

/// Whether window `window_id` is still listed by the window server.
pub fn window_exists(window_id: u32) -> bool {
    current_window_rect(window_id).is_some()
}

fn describe(error: &NSError) -> String {
    format!("{} (code {})", error.localizedDescription(), error.code())
}

fn rect_from_cg(r: CGRect) -> Rect {
    Rect {
        x: r.origin.x,
        y: r.origin.y,
        width: r.size.width,
        height: r.size.height,
    }
}

fn rect_contains(r: Rect, p: Point) -> bool {
    p.x >= r.x && p.y >= r.y && p.x < r.x + r.width && p.y < r.y + r.height
}

fn intersect(a: Rect, b: Rect) -> Option<Rect> {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = (a.x + a.width).min(b.x + b.width);
    let y1 = (a.y + a.height).min(b.y + b.height);
    (x1 > x0 && y1 > y0).then_some(Rect {
        x: x0,
        y: y0,
        width: x1 - x0,
        height: y1 - y0,
    })
}

/// Geometry of a window-capture frame.
///
/// `window` is the window's current global rect in points. ScreenCaptureKit
/// scales the window to fit the configured buffer: `content_scale` is that
/// fit factor and `scale_factor` the display's backing scale, so content
/// pixels per point are their product. `content_rect` is where the content
/// sits in the buffer. Apple does not document its unit; we take whichever
/// of pixels or points matches the content size (identical at 1x).
fn window_geometry(
    window: Rect,
    content_rect: Option<Rect>,
    content_scale: f64,
    scale_factor: f64,
    width_px: u32,
    height_px: u32,
) -> FrameGeometry {
    let pps = content_scale * scale_factor;
    if !(pps.is_finite() && pps > 0.0) {
        return FrameGeometry::from_rect(window, width_px, height_px);
    }
    let (ox, oy) = match content_rect {
        Some(cr) => {
            let off_if_px = (cr.width - window.width * pps).abs();
            let off_if_pt = (cr.width - window.width * content_scale).abs();
            if off_if_px <= off_if_pt {
                (cr.x, cr.y)
            } else {
                (cr.x * scale_factor, cr.y * scale_factor)
            }
        }
        None => (0.0, 0.0),
    };
    FrameGeometry {
        origin: Point {
            x: window.x - ox / pps,
            y: window.y - oy / pps,
        },
        scale_x: pps,
        scale_y: pps,
        width_px,
        height_px,
    }
}

type Dict = NSDictionary<AnyObject, AnyObject>;

fn dict_get(d: &Dict, key: &NSString) -> Option<Retained<AnyObject>> {
    let key: &AnyObject = key.as_ref();
    d.objectForKey(key)
}

fn dict_f64(d: &Dict, key: &NSString) -> Option<f64> {
    dict_get(d, key)?
        .downcast_ref::<NSNumber>()
        .map(|n| n.doubleValue())
}

/// A `CGRect` dictionary representation (`X`, `Y`, `Width`, `Height`).
fn rect_from_dict(d: &Dict) -> Option<Rect> {
    let get = |k: &str| dict_f64(d, &NSString::from_str(k));
    Some(Rect {
        x: get("X")?,
        y: get("Y")?,
        width: get("Width")?,
        height: get("Height")?,
    })
}

fn dict_rect(d: &Dict, key: &NSString) -> Option<Rect> {
    rect_from_dict(dict_get(d, key)?.downcast_ref::<Dict>()?)
}

/// Current global rect of a window, in points.
fn current_window_rect(window_id: u32) -> Option<Rect> {
    let list = CGWindowListCopyWindowInfo(CGWindowListOption::OptionIncludingWindow, window_id)?;
    // SAFETY: CFArray of CFDictionary is toll-free bridged to NSArray of
    // NSDictionary; `list` outlives the borrow.
    let list: &NSArray<Dict> = unsafe { &*(&*list as *const _ as *const NSArray<Dict>) };
    let info = list.firstObject()?;
    dict_rect(&info, &NSString::from_str("kCGWindowBounds"))
}

/// The `SCStreamFrameInfo` attachments we use.
#[derive(Debug, Default)]
struct FrameInfo {
    status: Option<isize>,
    content_rect: Option<Rect>,
    content_scale: Option<f64>,
    scale_factor: Option<f64>,
}

impl FrameInfo {
    /// Complete and Started frames carry a full image; idle, blank,
    /// suspended and stopped ones must not reach the ring. Frames without a
    /// status attachment are accepted.
    fn has_image(&self) -> bool {
        self.status
            .is_none_or(|s| s == SCFrameStatus::Complete.0 || s == SCFrameStatus::Started.0)
    }
}

/// Reads the frame info attachments.
///
/// # Safety
/// `sample` must be a valid `CMSampleBuffer`.
unsafe fn frame_info(sample: &OpaqueSampleBuffer) -> FrameInfo {
    let array = unsafe { CMSampleBufferGetSampleAttachmentsArray(sample, 0) };
    if array.is_null() {
        return FrameInfo::default();
    }
    // SAFETY: a CFArray of CFDictionary, toll-free bridged, owned by the
    // sample buffer for the duration of the callback.
    let array = unsafe { &*(array as *const NSArray<Dict>) };
    let Some(d) = array.firstObject() else {
        return FrameInfo::default();
    };
    // SAFETY: SCK's exported key constants.
    let (status, rect, content_scale, scale_factor) = unsafe {
        (
            SCStreamFrameInfoStatus,
            SCStreamFrameInfoContentRect,
            SCStreamFrameInfoContentScale,
            SCStreamFrameInfoScaleFactor,
        )
    };
    FrameInfo {
        status: dict_get(&d, status)
            .and_then(|v| v.downcast_ref::<NSNumber>().map(|n| n.integerValue())),
        content_rect: dict_rect(&d, rect),
        content_scale: dict_f64(&d, content_scale),
        scale_factor: dict_f64(&d, scale_factor),
    }
}

/// Presentation time to host ns. `None` if the time is invalid, infinite or
/// indefinite.
fn cmtime_to_timestamp(t: CMTimeRaw) -> Option<Timestamp> {
    const NOT_NUMERIC: u32 = 0x4 | 0x8 | 0x10; // ±infinity, indefinite
    if t.flags & CMTIME_VALID == 0 || t.flags & NOT_NUMERIC != 0 {
        return None;
    }
    if t.timescale <= 0 || t.value < 0 {
        return None;
    }
    let ns = i128::from(t.value) * 1_000_000_000 / i128::from(t.timescale);
    u64::try_from(ns).ok().map(Timestamp)
}

// --- CoreMedia / CoreVideo FFI -------------------------------------------
//
// Declared by hand because `objc2-core-media` / `objc2-core-video` are only
// transitive dependencies. Both frameworks are already linked through
// ScreenCaptureKit.

/// `CMSampleBufferRef` target. Encodes like `objc2-core-media`'s
/// `CMSampleBuffer` so the protocol method signature verifies.
#[repr(C)]
struct OpaqueSampleBuffer {
    _priv: [u8; 0],
}

// SAFETY: matches `^{opaqueCMSampleBuffer=}`, CoreMedia's encoding.
unsafe impl RefEncode for OpaqueSampleBuffer {
    const ENCODING_REF: Encoding =
        Encoding::Pointer(&Encoding::Struct("opaqueCMSampleBuffer", &[]));
}

/// `CMTime`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct CMTimeRaw {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

const CMTIME_VALID: u32 = 1;
const CV_PIXEL_BUFFER_LOCK_READ_ONLY: u64 = 1;

#[link(name = "CoreMedia", kind = "framework")]
extern "C" {
    fn CMSampleBufferGetImageBuffer(sbuf: *const OpaqueSampleBuffer) -> *mut c_void;
    fn CMSampleBufferGetPresentationTimeStamp(sbuf: *const OpaqueSampleBuffer) -> CMTimeRaw;
    fn CMSampleBufferGetSampleAttachmentsArray(
        sbuf: *const OpaqueSampleBuffer,
        create_if_necessary: u8,
    ) -> *const c_void;
}

#[link(name = "CoreVideo", kind = "framework")]
extern "C" {
    fn CVPixelBufferLockBaseAddress(buf: *mut c_void, flags: u64) -> i32;
    fn CVPixelBufferUnlockBaseAddress(buf: *mut c_void, flags: u64) -> i32;
    fn CVPixelBufferGetBaseAddress(buf: *mut c_void) -> *mut c_void;
    fn CVPixelBufferGetBytesPerRow(buf: *mut c_void) -> usize;
    fn CVPixelBufferGetWidth(buf: *mut c_void) -> usize;
    fn CVPixelBufferGetHeight(buf: *mut c_void) -> usize;
    fn CVPixelBufferGetPixelFormatType(buf: *mut c_void) -> u32;
}

/// Copies a BGRA sample buffer into a packed [`Frame`]. Returns `None` for
/// frames without a full image (status other than Complete/Started, or no
/// pixel buffer) and for unexpected formats.
///
/// # Safety
/// `sample` must be a valid `CMSampleBuffer` for the duration of the call.
unsafe fn copy_frame(sample: &OpaqueSampleBuffer, ivars: &OutputIvars) -> Option<Frame> {
    let info = unsafe { frame_info(sample) };
    if !info.has_image() {
        return None;
    }
    let ts = cmtime_to_timestamp(unsafe { CMSampleBufferGetPresentationTimeStamp(sample) })?;
    let pb = NonNull::new(unsafe { CMSampleBufferGetImageBuffer(sample) })?.as_ptr();
    // SAFETY: `pb` is a valid CVPixelBuffer owned by the sample buffer.
    unsafe {
        if CVPixelBufferGetPixelFormatType(pb) != PIXEL_FORMAT_BGRA {
            return None;
        }
        if CVPixelBufferLockBaseAddress(pb, CV_PIXEL_BUFFER_LOCK_READ_ONLY) != 0 {
            return None;
        }
    }
    let (width, height, stride, base) = unsafe {
        (
            CVPixelBufferGetWidth(pb),
            CVPixelBufferGetHeight(pb),
            CVPixelBufferGetBytesPerRow(pb),
            CVPixelBufferGetBaseAddress(pb) as *const u8,
        )
    };
    let row = width * 4;
    let pixels = if base.is_null() || stride < row {
        None
    } else {
        let mut pixels = ivars.reader.take_buffer(row * height);
        // SAFETY: the buffer is locked and holds `height` rows of
        // `stride >= row` bytes.
        unsafe {
            if stride == row {
                pixels.extend_from_slice(std::slice::from_raw_parts(base, row * height));
            } else {
                for y in 0..height {
                    pixels.extend_from_slice(std::slice::from_raw_parts(base.add(y * stride), row));
                }
            }
        }
        Some(pixels)
    };
    // SAFETY: locked above with the same flags.
    unsafe { CVPixelBufferUnlockBaseAddress(pb, CV_PIXEL_BUFFER_LOCK_READ_ONLY) };
    let pixels = pixels?;
    let (width, height) = (u32::try_from(width).ok()?, u32::try_from(height).ok()?);
    let geometry = match &ivars.window {
        Some(w) => w.geometry(&info, width, height),
        None => FrameGeometry::from_rect(ivars.rect_points, width, height),
    };
    Some(Frame {
        ts,
        width,
        height,
        geometry,
        pixels: Arc::new(pixels),
    })
}

/// Follows a window target's position for per-frame geometry.
struct WindowTracking {
    id: u32,
    /// Last known global rect, used if the window list lookup fails.
    last_rect: Mutex<Rect>,
}

impl WindowTracking {
    fn geometry(&self, info: &FrameInfo, width: u32, height: u32) -> FrameGeometry {
        let mut last = self.last_rect.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(r) = current_window_rect(self.id) {
            *last = r;
        }
        match (info.content_scale, info.scale_factor) {
            (Some(cs), Some(sf)) => {
                window_geometry(*last, info.content_rect, cs, sf, width, height)
            }
            _ => FrameGeometry::from_rect(*last, width, height),
        }
    }
}

struct OutputIvars {
    reader: FrameReader,
    /// The run this output belongs to.
    generation: u64,
    /// Global rect the stream shows, in points (display and region targets).
    rect_points: Rect,
    window: Option<WindowTracking>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and FrameOutput does
    // not implement Drop.
    #[unsafe(super(NSObject))]
    #[name = "GuideRecorderFrameOutput"]
    #[ivars = OutputIvars]
    struct FrameOutput;

    unsafe impl NSObjectProtocol for FrameOutput {}

    unsafe impl SCStreamOutput for FrameOutput {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn did_output(
            &self,
            _stream: &SCStream,
            sample: &OpaqueSampleBuffer,
            kind: SCStreamOutputType,
        ) {
            if kind != SCStreamOutputType::Screen {
                return;
            }
            let ivars = self.ivars();
            // SAFETY: ScreenCaptureKit passes a valid sample buffer that
            // lives for the duration of this callback.
            if let Some(frame) = unsafe { copy_frame(sample, ivars) } {
                ivars.reader.push_from(ivars.generation, frame);
            }
        }
    }

    unsafe impl SCStreamDelegate for FrameOutput {
        #[unsafe(method(stream:didStopWithError:))]
        fn did_stop(&self, _stream: &SCStream, error: &NSError) {
            let ivars = self.ivars();
            ivars
                .reader
                .system_stopped(ivars.generation, describe(error));
        }
    }
);

impl FrameOutput {
    fn new(ivars: OutputIvars) -> Retained<Self> {
        let this = Self::alloc().set_ivars(ivars);
        // SAFETY: NSObject's init.
        unsafe { msg_send![super(this), init] }
    }
}

#[cfg(test)]
mod tests {
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
    fn source_gone_errors_by_code() {
        assert!(is_source_gone_error(
            "The stream has no capture source. (code -3815)"
        ));
        assert!(is_source_gone_error("x (code -3813)"));
        assert!(is_source_gone_error("x (code -3814)"));
        // A system stop (screen lock, sleep), a user stop, a revoked
        // permission or an unknown text: not known to be gone.
        assert!(!is_source_gone_error(
            "Stream was stopped by the system (code -3821)"
        ));
        assert!(!is_source_gone_error("x (code -3817)"));
        assert!(!is_source_gone_error("x (code -3801)"));
        assert!(!is_source_gone_error("window gone"));
        assert!(!is_source_gone_error("x (code abc)"));
    }

    #[test]
    fn window_geometry_follows_moves_and_resizes() {
        // Window captured at 800x600 pt on a 2x display into a 1600x1200
        // buffer, then moved to (300, 200).
        let moved = rect(300.0, 200.0, 800.0, 600.0);
        let cr = Some(rect(0.0, 0.0, 1600.0, 1200.0));
        let g = window_geometry(moved, cr, 1.0, 2.0, 1600, 1200);
        assert_eq!(g.origin, Point { x: 300.0, y: 200.0 });
        assert_eq!(
            g.point_to_pixel(Point { x: 310.0, y: 210.0 }),
            Point { x: 20.0, y: 20.0 }
        );

        // Then resized to 1600x600 pt: scaled by 0.5 to fit, letterboxed
        // 300 px down (content rect in pixels).
        let wide = rect(300.0, 200.0, 1600.0, 600.0);
        let cr = Some(rect(0.0, 300.0, 1600.0, 600.0));
        let g = window_geometry(wide, cr, 0.5, 2.0, 1600, 1200);
        assert_eq!(g.scale(), 1.0);
        assert_eq!(
            g.point_to_pixel(Point { x: 300.0, y: 200.0 }),
            Point { x: 0.0, y: 300.0 }
        );

        // Same, with the content rect in points.
        let cr = Some(rect(0.0, 150.0, 800.0, 300.0));
        let g = window_geometry(wide, cr, 0.5, 2.0, 1600, 1200);
        assert_eq!(
            g.point_to_pixel(Point { x: 300.0, y: 200.0 }),
            Point { x: 0.0, y: 300.0 }
        );

        // Missing scale info falls back to filling the buffer.
        let g = window_geometry(moved, None, 0.0, 2.0, 1600, 1200);
        assert_eq!(g, FrameGeometry::from_rect(moved, 1600, 1200));
    }

    #[test]
    fn frame_status_filter() {
        let with = |status| FrameInfo {
            status,
            ..Default::default()
        };
        assert!(with(None).has_image());
        assert!(with(Some(SCFrameStatus::Complete.0)).has_image());
        assert!(with(Some(SCFrameStatus::Started.0)).has_image());
        for s in [
            SCFrameStatus::Idle,
            SCFrameStatus::Blank,
            SCFrameStatus::Suspended,
            SCFrameStatus::Stopped,
        ] {
            assert!(!with(Some(s.0)).has_image());
        }
    }

    #[test]
    fn rect_dictionary_parsing() {
        let keys = ["X", "Y", "Width", "Height"].map(NSString::from_str);
        let values = [1.5, -2.0, 30.0, 40.0].map(NSNumber::new_f64);
        let key_refs: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
        let value_refs: Vec<&NSNumber> = values.iter().map(|v| &**v).collect();
        // SAFETY: same object, viewed with erased generics.
        let erase = |d: &NSDictionary<NSString, NSNumber>| -> &Dict {
            unsafe { &*(d as *const NSDictionary<NSString, NSNumber> as *const Dict) }
        };
        let d = NSDictionary::from_slices(&key_refs, &value_refs);
        assert_eq!(rect_from_dict(erase(&d)), Some(rect(1.5, -2.0, 30.0, 40.0)));
        let partial = NSDictionary::from_slices(&key_refs[..2], &value_refs[..2]);
        assert_eq!(rect_from_dict(erase(&partial)), None);
    }

    #[test]
    fn intersect_and_contains() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = rect(5.0, -5.0, 10.0, 10.0);
        assert_eq!(intersect(a, b), Some(rect(5.0, 0.0, 5.0, 5.0)));
        let far = Rect { x: 20.0, ..b };
        assert_eq!(intersect(a, far), None);
        assert!(rect_contains(a, Point { x: 0.0, y: 9.9 }));
        assert!(!rect_contains(a, Point { x: 10.0, y: 0.0 }));
    }

    #[test]
    fn cmtime_conversion() {
        let t = |value, timescale, flags| CMTimeRaw {
            value,
            timescale,
            flags,
            epoch: 0,
        };
        assert_eq!(
            cmtime_to_timestamp(t(3, 2, 1)),
            Some(Timestamp(1_500_000_000))
        );
        // 24 MHz host ticks.
        assert_eq!(
            cmtime_to_timestamp(t(24_000_000 * 100, 24_000_000, 1)),
            Some(Timestamp(100_000_000_000))
        );
        assert_eq!(cmtime_to_timestamp(t(3, 2, 0)), None);
        assert_eq!(cmtime_to_timestamp(t(3, 0, 1)), None);
        for special in [0x4, 0x8, 0x10] {
            assert_eq!(cmtime_to_timestamp(t(3, 2, 1 | special)), None);
        }
    }

    #[test]
    fn frame_interval_is_valid_cmtime() {
        let config = unsafe { SCStreamConfiguration::new() };
        let got = unsafe {
            let mut interval = config.minimumFrameInterval();
            write_cmtime(&mut interval, frame_interval(12));
            config.setMinimumFrameInterval(interval);
            config.minimumFrameInterval()
        };
        assert_eq!((got.value, got.timescale), (1, 12));
        assert_eq!(got.flags.0 & CMTIME_VALID, CMTIME_VALID);
    }

    #[test]
    fn config_excludes_no_apps_by_default() {
        assert!(FrameStreamConfig::default().excluded_bundle_ids.is_empty());
        let handle = FilterHandle::new(FrameStreamConfig::default());
        assert!(handle.filters_apps());
        assert!(!handle.covers(1));
        assert_eq!(handle.coverage(), FilterCoverage::default());
        // Not running: a refresh does nothing (and calls no SCK).
        assert_eq!(handle.refresh().unwrap(), None);
        let window = FilterHandle::new(FrameStreamConfig {
            target: CaptureTarget::Window { window_id: 1 },
            ..Default::default()
        });
        assert!(!window.filters_apps());
    }

    #[test]
    fn output_class_registers_with_protocols() {
        // Registering the class verifies (in debug builds) that our method
        // encodings match the SCStreamOutput / SCStreamDelegate protocols.
        let output = FrameOutput::new(OutputIvars {
            reader: FrameReader::detached(1),
            generation: 0,
            rect_points: rect(0.0, 0.0, 1.0, 1.0),
            window: None,
        });
        let as_output = ProtocolObject::<dyn SCStreamOutput>::from_ref(&*output);
        assert!(as_output.respondsToSelector(objc2::sel!(stream:didOutputSampleBuffer:ofType:)));
        let as_delegate = ProtocolObject::<dyn SCStreamDelegate>::from_ref(&*output);
        assert!(as_delegate.respondsToSelector(objc2::sel!(stream:didStopWithError:)));
    }

    #[test]
    #[ignore = "needs Screen Recording permission (TCC) and a display"]
    fn live_capture_main_display() {
        let mut stream = FrameStream::start(FrameStreamConfig::default()).unwrap();
        std::thread::sleep(Duration::from_millis(500));
        let frame = stream.reader().latest().expect("a frame within 500 ms");
        assert_eq!(
            frame.pixels.len(),
            frame.bytes_per_row() * frame.height as usize
        );
        stream.pause().unwrap();
        assert!(!stream.is_capturing());
        stream.resume().unwrap();
        assert!(stream.is_capturing());
        stream.stop().unwrap();
    }
}
