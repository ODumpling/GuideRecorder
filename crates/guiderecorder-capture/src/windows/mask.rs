//! Masks excluded apps' windows out of display and region frames, as the
//! macOS content filter leaves them out (see the frames module's
//! `# Excluded windows`).
//!
//! # What is masked
//!
//! Every kept frame of a display or region run, before it enters the ring,
//! is painted over in [`MASK_COLOR`] wherever a window of an excluded app
//! shows. The top-level windows are listed front to back (`EnumWindows`);
//! a window belongs to an excluded app when its process's executable
//! matches [`FrameStreamConfig::excluded_bundle_ids`] by
//! [`bundle_id_matches`], which takes in the app's menus, tooltips and
//! other popups (top-level windows of the same process) too. A Store app
//! shown in an `ApplicationFrameHost.exe` frame is matched by the process
//! of the frame's `CoreWindow`. A hung window of an excluded app is
//! masked although Windows hides it, since it shows a "ghost" copy of the
//! window, owned by another process, in its place. What shows of an
//! excluded window is its bounds less the windows in front of it that hide
//! what is behind them.
//!
//! # Failing closed
//!
//! When unsure, more is masked, never less:
//!
//! - An excluded window is masked whole, grown by [`EXCLUDED_MARGIN`] on
//!   every side, whatever covers it in the app (transparency, rounded
//!   corners).
//! - A window in front only hides what is behind it when it is shown (not
//!   minimised or cloaked), drawn opaque (neither click-through
//!   `WS_EX_TRANSPARENT` nor layered, unless its layered alpha is 255 with
//!   no colour key), in captures (display affinity `WDA_NONE`: this
//!   process's own windows are kept out of captures, so what is under them
//!   shows) and rectangular (no window region). It then hides its bounds
//!   shrunk by [`OCCLUDER_INSET`], which takes in Windows 11's rounded
//!   corners and small moves.
//! - A pid whose executable cannot be read (it just exited) is looked up
//!   again on the next frame.
//!
//! Windows that draw their own transparency through DirectComposition
//! (`WS_EX_NOREDIRECTIONBITMAP`, e.g. a see-through terminal) cannot be
//! told from opaque ones and are taken as opaque: what they show through
//! is not masked.
//!
//! # Timing
//!
//! The windows are listed when the frame is copied, a few milliseconds
//! after it was composed, so the mask is best-effort for a window moving,
//! opening or closing in between; the margins cover a few pixels of
//! movement. Frames of a window target show that one window only and are
//! never masked.
//!
//! # Cost
//!
//! Each pid is looked up once per [`PID_TTL`] (the cache is then cleared,
//! so a reused pid is not mistaken for the process it had before), and the
//! window list, the layers and the rectangles reuse their buffers. With no
//! excluded window shown, a frame costs one pass over the window list.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use windows::core::{w, BOOL, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, RECT, TRUE};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Gdi::{GetWindowRgnBox, RGN_ERROR};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowExW, GetLayeredWindowAttributes, GetWindowDisplayAffinity,
    GetWindowLongW, GetWindowRect, IsHungAppWindow, IsIconic, IsWindowVisible, GWL_EXSTYLE,
    LWA_ALPHA, LWA_COLORKEY, WDA_NONE, WS_EX_LAYERED, WS_EX_TRANSPARENT,
};

use super::event_tap::window_pid;
use super::frames::{bundle_id_matches, is_cloaked, FrameGeometry};

#[cfg(doc)]
use super::frames::FrameStreamConfig;

/// What masked parts are painted with (BGRA): an opaque mid grey, which
/// reads as "hidden" rather than as a dark window.
pub(super) const MASK_COLOR: [u8; 4] = [0x80, 0x80, 0x80, 0xFF];
/// Pixels an excluded window's mask reaches past its bounds.
const EXCLUDED_MARGIN: i32 = 4;
/// Pixels a window in front hides less than its bounds, on every side:
/// Windows 11 rounds window corners with an 8 pixel radius.
const OCCLUDER_INSET: i32 = 8;
/// How long a pid's lookup is trusted.
const PID_TTL: Duration = Duration::from_secs(1);
/// Pieces an excluded window may break into before it is masked whole.
const MAX_PIECES: usize = 256;
/// The app id of the host of Store apps' frames.
const FRAME_HOST: &str = "applicationframehost.exe";

/// A rectangle in whole pixels, right and bottom exclusive: screen pixels
/// (see the backend's `# Coordinates`) or frame pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct IntRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl IntRect {
    pub fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    fn from_rect(r: RECT) -> Self {
        Self::new(r.left, r.top, r.right, r.bottom)
    }

    pub fn is_empty(&self) -> bool {
        self.right <= self.left || self.bottom <= self.top
    }

    /// The overlap of `self` and `other`; `None` if empty.
    pub fn intersect(&self, other: &Self) -> Option<Self> {
        let r = Self::new(
            self.left.max(other.left),
            self.top.max(other.top),
            self.right.min(other.right),
            self.bottom.min(other.bottom),
        );
        (!r.is_empty()).then_some(r)
    }

    /// Grown by `by` on every side (shrunk for a negative `by`).
    pub fn inflate(&self, by: i32) -> Self {
        Self::new(
            self.left.saturating_sub(by),
            self.top.saturating_sub(by),
            self.right.saturating_add(by),
            self.bottom.saturating_add(by),
        )
    }
}

/// A top-level window that matters to the mask, in front-to-back order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Layer {
    /// An excluded app's window, by its bounds.
    Excluded(IntRect),
    /// A window that hides what is behind it, by its bounds.
    Opaque(IntRect),
}

/// Buffers [`masked_parts`] reuses from frame to frame.
#[derive(Debug, Default)]
pub(super) struct MaskScratch {
    occluders: Vec<IntRect>,
    pieces: Vec<IntRect>,
    next: Vec<IntRect>,
    out: Vec<IntRect>,
}

/// The parts of `layers` (front to back) to mask, inside `clip`: each
/// excluded window grown by [`EXCLUDED_MARGIN`], less every opaque window
/// in front of it shrunk by [`OCCLUDER_INSET`]. Excluded windows do not
/// hide what is behind them, so overlapping parts may repeat.
pub(super) fn masked_parts<'a>(
    layers: &[Layer],
    clip: IntRect,
    scratch: &'a mut MaskScratch,
) -> &'a [IntRect] {
    let MaskScratch {
        occluders,
        pieces,
        next,
        out,
    } = scratch;
    occluders.clear();
    out.clear();
    for layer in layers {
        match *layer {
            Layer::Opaque(r) => {
                let r = r.inflate(-OCCLUDER_INSET);
                if !r.is_empty() {
                    occluders.push(r);
                }
            }
            Layer::Excluded(r) => {
                let Some(r) = r.inflate(EXCLUDED_MARGIN).intersect(&clip) else {
                    continue;
                };
                pieces.clear();
                pieces.push(r);
                for hole in occluders.iter() {
                    next.clear();
                    for piece in pieces.iter() {
                        subtract(piece, hole, next);
                    }
                    std::mem::swap(pieces, next);
                    if pieces.is_empty() {
                        break;
                    }
                    if pieces.len() > MAX_PIECES {
                        // Too broken up to be worth it: mask it whole.
                        pieces.clear();
                        pieces.push(r);
                        break;
                    }
                }
                out.extend_from_slice(pieces);
            }
        }
    }
    out
}

/// Pushes `r` less `hole` onto `out`, as up to four rectangles.
fn subtract(r: &IntRect, hole: &IntRect, out: &mut Vec<IntRect>) {
    let Some(h) = r.intersect(hole) else {
        out.push(*r);
        return;
    };
    let mut push = |piece: IntRect| {
        if !piece.is_empty() {
            out.push(piece);
        }
    };
    // Above and below the hole, full width; beside it, its height.
    push(IntRect::new(r.left, r.top, r.right, h.top));
    push(IntRect::new(r.left, h.bottom, r.right, r.bottom));
    push(IntRect::new(r.left, h.top, h.left, h.bottom));
    push(IntRect::new(h.right, h.top, r.right, h.bottom));
}

/// The screen pixels a frame with `geometry` shows, rounded outwards.
pub(super) fn frame_screen_rect(geometry: &FrameGeometry) -> IntRect {
    let r = geometry.rect_points();
    IntRect::new(
        r.x.floor() as i32,
        r.y.floor() as i32,
        (r.x + r.width).ceil() as i32,
        (r.y + r.height).ceil() as i32,
    )
}

/// Screen rect `r` in the pixels of a frame with `geometry`, rounded
/// outwards and clipped to the frame; `None` if off it.
pub(super) fn to_frame_pixels(r: &IntRect, geometry: &FrameGeometry) -> Option<IntRect> {
    let x = |v: i32| (f64::from(v) - geometry.origin.x) * geometry.scale_x;
    let y = |v: i32| (f64::from(v) - geometry.origin.y) * geometry.scale_y;
    let w = i32::try_from(geometry.width_px).unwrap_or(i32::MAX);
    let h = i32::try_from(geometry.height_px).unwrap_or(i32::MAX);
    IntRect::new(
        x(r.left).floor() as i32,
        y(r.top).floor() as i32,
        x(r.right).ceil() as i32,
        y(r.bottom).ceil() as i32,
    )
    .intersect(&IntRect::new(0, 0, w, h))
}

/// Paints frame-pixel rect `r` of a tightly packed BGRA image `width`
/// pixels wide in [`MASK_COLOR`]. Parts outside the image are left.
pub(super) fn fill(pixels: &mut [u8], width: u32, r: &IntRect) {
    let row = width as usize * 4;
    if row == 0 {
        return;
    }
    let height = pixels.len() / row;
    let image = IntRect::new(
        0,
        0,
        i32::try_from(width).unwrap_or(i32::MAX),
        i32::try_from(height).unwrap_or(i32::MAX),
    );
    let Some(r) = r.intersect(&image) else {
        return;
    };
    let (left, right) = (r.left as usize * 4, r.right as usize * 4);
    for y in r.top as usize..r.bottom as usize {
        for px in pixels[y * row + left..y * row + right]
            .as_chunks_mut::<4>()
            .0
        {
            *px = MASK_COLOR;
        }
    }
}

/// What a pid is to the mask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PidKind {
    Excluded,
    /// `ApplicationFrameHost.exe`, whose frames show Store apps.
    FrameHost,
    Other,
}

/// Finds the excluded apps' windows for each frame of a display or region
/// run and masks them (see the module docs). Lives in the run's copier.
pub(super) struct Masker {
    pids: PidKinds,
    /// The top-level windows, front to back.
    windows: Vec<HWND>,
    /// The visible windows, front to back, and whether each is excluded.
    shown: Vec<(HWND, bool)>,
    layers: Vec<Layer>,
    scratch: MaskScratch,
}

// SAFETY: window handles are plain values, usable from any thread.
unsafe impl Send for Masker {}

impl Masker {
    /// `None` when there is nothing to mask.
    pub fn new(excluded: &[String]) -> Option<Self> {
        (!excluded.is_empty()).then(|| Self {
            pids: PidKinds {
                excluded: excluded.to_vec(),
                kinds: HashMap::new(),
                since: Instant::now(),
            },
            windows: Vec::new(),
            shown: Vec::new(),
            layers: Vec::new(),
            scratch: MaskScratch::default(),
        })
    }

    /// Masks the excluded windows shown in `pixels`, a tightly packed BGRA
    /// frame with `geometry`. The calling thread must be per-monitor aware.
    pub fn mask(&mut self, pixels: &mut [u8], geometry: &FrameGeometry) {
        self.pids.expire();
        fill_top_level_windows(&mut self.windows);
        // First the cheap pass: which windows are visible and excluded, and
        // the last excluded one. Nothing behind it matters.
        self.shown.clear();
        let mut last_excluded = None;
        for &window in &self.windows {
            // SAFETY: a plain query (a stale handle gives false).
            let visible = unsafe { IsWindowVisible(window) }.as_bool();
            let excluded = match self.pids.kind(window_pid(window)) {
                // A hung window is hidden and a "ghost" of another process
                // shown in its place, with its last content.
                // SAFETY: a plain query.
                PidKind::Excluded => visible || unsafe { IsHungAppWindow(window) }.as_bool(),
                PidKind::FrameHost => visible && self.pids.hosts_excluded(window),
                PidKind::Other => false,
            };
            if !visible && !excluded {
                continue;
            }
            if excluded {
                last_excluded = Some(self.shown.len());
            }
            self.shown.push((window, excluded));
        }
        let Some(last) = last_excluded else {
            return;
        };
        self.layers.clear();
        for &(window, excluded) in &self.shown[..=last] {
            let layer = if excluded {
                shows(window)
                    .then(|| frame_bounds(window).or_else(|| window_rect(window)))
                    .flatten()
                    .map(Layer::Excluded)
            } else {
                hides_behind(window)
                    .then(|| frame_bounds(window))
                    .flatten()
                    .map(Layer::Opaque)
            };
            self.layers.extend(layer);
        }
        let clip = frame_screen_rect(geometry);
        for part in masked_parts(&self.layers, clip, &mut self.scratch) {
            if let Some(px) = to_frame_pixels(part, geometry) {
                fill(pixels, geometry.width_px, &px);
            }
        }
    }
}

/// What each pid is, looked up once per [`PID_TTL`].
struct PidKinds {
    excluded: Vec<String>,
    kinds: HashMap<u32, PidKind>,
    since: Instant,
}

impl PidKinds {
    /// Forgets every pid once the cache is [`PID_TTL`] old.
    fn expire(&mut self) {
        if self.since.elapsed() >= PID_TTL {
            self.kinds.clear();
            self.since = Instant::now();
        }
    }

    fn kind(&mut self, pid: i32) -> PidKind {
        let Ok(pid) = u32::try_from(pid) else {
            return PidKind::Other;
        };
        if pid == 0 {
            return PidKind::Other;
        }
        if let Some(&kind) = self.kinds.get(&pid) {
            return kind;
        }
        // Not cached when unreadable: asked again next frame.
        let Some(id) = super::apps::process_image(pid).and_then(|p| super::apps::app_id(&p)) else {
            return PidKind::Other;
        };
        let kind = if bundle_id_matches(&id, &self.excluded) {
            PidKind::Excluded
        } else if id == FRAME_HOST {
            PidKind::FrameHost
        } else {
            PidKind::Other
        };
        self.kinds.insert(pid, kind);
        kind
    }

    /// Whether a Store app frame shows an excluded app: the process of its
    /// `CoreWindow` child.
    fn hosts_excluded(&mut self, frame: HWND) -> bool {
        // SAFETY: a plain query; a stale handle fails.
        let core = unsafe {
            FindWindowExW(
                Some(frame),
                None,
                w!("Windows.UI.Core.CoreWindow"),
                PCWSTR::null(),
            )
        };
        core.is_ok_and(|core| self.kind(window_pid(core)) == PidKind::Excluded)
    }
}

/// Lists the top-level windows, front to back, into `found`.
fn fill_top_level_windows(found: &mut Vec<HWND>) {
    unsafe extern "system" fn collect(window: HWND, data: LPARAM) -> BOOL {
        // SAFETY: `data` is the `Vec` passed below, alive for the call.
        unsafe { (*(data.0 as *mut Vec<HWND>)).push(window) };
        TRUE
    }
    found.clear();
    // SAFETY: the callback only pushes to `found`, which outlives the call.
    let _ = unsafe { EnumWindows(Some(collect), LPARAM(found as *mut Vec<HWND> as isize)) };
}

/// Whether an excluded (visible) window can be on screen: not minimised
/// or cloaked.
fn shows(window: HWND) -> bool {
    // SAFETY: a plain query.
    !unsafe { IsIconic(window) }.as_bool() && !is_cloaked(window)
}

/// Whether a visible window certainly hides what is behind it in a
/// capture (see `# Failing closed`).
fn hides_behind(window: HWND) -> bool {
    if !shows(window) {
        return false;
    }
    // SAFETY: a plain query.
    let ex_style = unsafe { GetWindowLongW(window, GWL_EXSTYLE) } as u32;
    if ex_style & WS_EX_TRANSPARENT.0 != 0 {
        return false;
    }
    if ex_style & WS_EX_LAYERED.0 != 0 {
        let (mut key, mut alpha, mut flags) = (COLORREF(0), 0u8, Default::default());
        // SAFETY: valid out-pointers. Fails for a window drawn with
        // UpdateLayeredWindow (per-pixel alpha).
        let read = unsafe {
            GetLayeredWindowAttributes(window, Some(&mut key), Some(&mut alpha), Some(&mut flags))
        };
        let opaque = read.is_ok() && flags & LWA_ALPHA == LWA_ALPHA && alpha == 255;
        if !opaque || flags & LWA_COLORKEY == LWA_COLORKEY {
            return false;
        }
    }
    let mut affinity = u32::MAX;
    // SAFETY: a valid out-pointer.
    let read = unsafe { GetWindowDisplayAffinity(window, &mut affinity) };
    if read.is_err() || affinity != WDA_NONE.0 {
        return false;
    }
    let mut region = RECT::default();
    // SAFETY: a valid out-pointer; `RGN_ERROR` means no window region.
    unsafe { GetWindowRgnBox(window, &mut region) == RGN_ERROR }
}

/// A window's bounds as DWM draws it (without the invisible resize
/// borders).
fn frame_bounds(window: HWND) -> Option<IntRect> {
    let mut r = RECT::default();
    // SAFETY: a valid out-pointer of the attribute's size.
    unsafe {
        DwmGetWindowAttribute(
            window,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&mut r as *mut RECT).cast(),
            std::mem::size_of::<RECT>() as u32,
        )
    }
    .ok()?;
    let r = IntRect::from_rect(r);
    (!r.is_empty()).then_some(r)
}

/// A window's rect, borders included.
fn window_rect(window: HWND) -> Option<IntRect> {
    let mut r = RECT::default();
    // SAFETY: a valid out-pointer.
    unsafe { GetWindowRect(window, &mut r) }.ok()?;
    let r = IntRect::from_rect(r);
    (!r.is_empty()).then_some(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use guiderecorder_core::model::Rect;

    fn r(left: i32, top: i32, right: i32, bottom: i32) -> IntRect {
        IntRect::new(left, top, right, bottom)
    }

    /// The pixels `parts` cover inside `clip`, counted once each.
    fn area(parts: &[IntRect], clip: IntRect) -> usize {
        let mut n = 0;
        for y in clip.top..clip.bottom {
            for x in clip.left..clip.right {
                let inside = |p: &IntRect| x >= p.left && x < p.right && y >= p.top && y < p.bottom;
                n += usize::from(parts.iter().any(inside));
            }
        }
        n
    }

    fn covers(parts: &[IntRect], x: i32, y: i32) -> bool {
        parts
            .iter()
            .any(|p| x >= p.left && x < p.right && y >= p.top && y < p.bottom)
    }

    const SCREEN: IntRect = IntRect {
        left: 0,
        top: 0,
        right: 1000,
        bottom: 1000,
    };

    #[test]
    fn subtracting_leaves_the_rest_in_up_to_four_pieces() {
        let mut out = Vec::new();
        subtract(&r(0, 0, 10, 10), &r(3, 3, 6, 6), &mut out);
        assert_eq!(out.len(), 4);
        assert_eq!(area(&out, r(0, 0, 10, 10)), 100 - 9);
        assert!(!covers(&out, 4, 4));
        out.clear();
        subtract(&r(0, 0, 10, 10), &r(20, 20, 30, 30), &mut out);
        assert_eq!(out, vec![r(0, 0, 10, 10)]);
        out.clear();
        subtract(&r(0, 0, 10, 10), &r(-5, -5, 50, 50), &mut out);
        assert!(out.is_empty());
        out.clear();
        // A hole across one edge leaves no empty pieces.
        subtract(&r(0, 0, 10, 10), &r(-5, 0, 50, 4), &mut out);
        assert_eq!(out, vec![r(0, 4, 10, 10)]);
    }

    #[test]
    fn an_excluded_window_alone_is_masked_with_its_margin() {
        let mut scratch = MaskScratch::default();
        let parts = masked_parts(
            &[Layer::Excluded(r(100, 100, 200, 200))],
            SCREEN,
            &mut scratch,
        );
        let m = EXCLUDED_MARGIN;
        assert_eq!(parts, &[r(100 - m, 100 - m, 200 + m, 200 + m)]);
    }

    #[test]
    fn windows_in_front_hide_their_inset_bounds_only() {
        let mut scratch = MaskScratch::default();
        let front = r(150, 0, 400, 400);
        let parts = masked_parts(
            &[Layer::Opaque(front), Layer::Excluded(r(100, 100, 300, 300))],
            SCREEN,
            &mut scratch,
        )
        .to_vec();
        // The left part shows; under the window in front nothing does,
        // except a band along its edge.
        assert!(covers(&parts, 120, 200));
        assert!(covers(&parts, 150 + OCCLUDER_INSET - 1, 200));
        assert!(!covers(&parts, 150 + OCCLUDER_INSET, 200));
        assert!(!covers(&parts, 250, 200));
        let m = EXCLUDED_MARGIN;
        assert_eq!(
            area(&parts, SCREEN),
            ((150 + OCCLUDER_INSET - (100 - m)) * (200 + 2 * m)) as usize
        );
    }

    #[test]
    fn only_windows_in_front_hide_an_excluded_one() {
        let mut scratch = MaskScratch::default();
        let parts = masked_parts(
            &[
                Layer::Excluded(r(100, 100, 300, 300)),
                Layer::Opaque(r(0, 0, 1000, 1000)),
            ],
            SCREEN,
            &mut scratch,
        );
        assert!(covers(parts, 200, 200));
    }

    #[test]
    fn a_covered_window_is_not_masked_and_excluded_windows_hide_nothing() {
        let mut scratch = MaskScratch::default();
        let parts = masked_parts(
            &[
                Layer::Opaque(r(0, 0, 500, 500)),
                Layer::Excluded(r(100, 100, 200, 200)),
            ],
            SCREEN,
            &mut scratch,
        );
        assert!(parts.is_empty());
        // An excluded window in front of another does not shrink its mask.
        let parts = masked_parts(
            &[
                Layer::Excluded(r(100, 100, 200, 200)),
                Layer::Excluded(r(150, 150, 300, 300)),
            ],
            SCREEN,
            &mut scratch,
        );
        assert!(covers(parts, 250, 250));
        assert!(covers(parts, 120, 120));
    }

    #[test]
    fn masks_are_clipped_to_the_frame() {
        let mut scratch = MaskScratch::default();
        let clip = r(0, 0, 100, 100);
        let parts = masked_parts(&[Layer::Excluded(r(50, 50, 500, 500))], clip, &mut scratch);
        assert_eq!(
            parts,
            &[r(50 - EXCLUDED_MARGIN, 50 - EXCLUDED_MARGIN, 100, 100)]
        );
        let parts = masked_parts(
            &[Layer::Excluded(r(200, 200, 300, 300))],
            clip,
            &mut scratch,
        );
        assert!(parts.is_empty());
    }

    #[test]
    fn many_holes_mask_the_window_whole() {
        let mut scratch = MaskScratch::default();
        // A grid of small windows in front breaks the window up.
        let mut layers: Vec<Layer> = (0..40)
            .flat_map(|i| {
                (0..40).map(move |j| {
                    let (x, y) = (i * 25, j * 25);
                    Layer::Opaque(r(
                        x,
                        y,
                        x + 2 * OCCLUDER_INSET + 4,
                        y + 2 * OCCLUDER_INSET + 4,
                    ))
                })
            })
            .collect();
        layers.push(Layer::Excluded(r(0, 0, 1000, 1000)));
        let parts = masked_parts(&layers, SCREEN, &mut scratch);
        assert_eq!(parts, &[SCREEN]);
    }

    #[test]
    fn screen_rects_map_to_frame_pixels() {
        // A region frame of a display at x = -1920 (scale 1).
        let g = FrameGeometry::from_rect(
            Rect {
                x: -1820.0,
                y: 50.0,
                width: 400.0,
                height: 300.0,
            },
            400,
            300,
        );
        assert_eq!(frame_screen_rect(&g), r(-1820, 50, -1420, 350));
        assert_eq!(
            to_frame_pixels(&r(-1900, 100, -1800, 120), &g),
            Some(r(0, 50, 20, 70))
        );
        assert_eq!(to_frame_pixels(&r(0, 0, 10, 10), &g), None);
        // At a scale of 2, rounded outwards.
        let g = FrameGeometry::from_rect(
            Rect {
                x: 0.0,
                y: 0.0,
                width: 100.5,
                height: 100.0,
            },
            201,
            200,
        );
        assert_eq!(
            to_frame_pixels(&r(10, 10, 20, 20), &g),
            Some(r(20, 20, 40, 40))
        );
        assert_eq!(
            to_frame_pixels(&r(90, 90, 200, 200), &g),
            Some(r(180, 180, 201, 200))
        );
    }

    #[test]
    fn fill_paints_the_rect_only() {
        let (w, h) = (6u32, 4u32);
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        fill(&mut pixels, w, &r(1, 1, 3, 9));
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                let i = (y as usize * w as usize + x as usize) * 4;
                let painted = pixels[i..i + 4] == MASK_COLOR;
                assert_eq!(
                    painted,
                    (1..3).contains(&x) && (1..4).contains(&y),
                    "{x},{y}"
                );
            }
        }
        // Off the image, nothing.
        let before = pixels.clone();
        fill(&mut pixels, w, &r(10, 0, 20, 4));
        assert_eq!(pixels, before);
    }
}
