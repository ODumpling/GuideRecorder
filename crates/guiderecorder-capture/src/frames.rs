//! Frame types every capture backend shares: what to capture
//! ([`CaptureTarget`], [`FrameStreamConfig`]), how global display points
//! map to frame pixels ([`FrameGeometry`], [`DisplayInfo`]), and which
//! windows a user can pick ([`pickable_windows`]).
//!
//! Each backend adds the platform parts in its own `frames` module: the
//! display list (`DisplayInfo::all`, `main`, `for_id`, `containing`),
//! `list_windows`, `mouse_location` and the frame stream itself.

use guiderecorder_core::model::{Point, Rect};

/// Default frames per second (the design calls for 10–15).
pub const DEFAULT_FPS: u32 = 12;
/// Default number of frames kept in memory.
pub const DEFAULT_RING_CAPACITY: usize = 3;

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("Screen Recording permission is not granted")]
    PermissionDenied,
    #[error("invalid frame stream config: {0}")]
    InvalidConfig(String),
    #[error("display {0} not found")]
    DisplayNotFound(u32),
    #[error("window {0} not found")]
    WindowNotFound(u32),
    #[error("could not list shareable content: {0}")]
    ShareableContent(String),
    #[error("screen capture stream failed: {0}")]
    Stream(String),
    #[error("timed out waiting for screen capture")]
    Timeout,
    #[error("frame stream already stopped")]
    Stopped,
    #[error("screen capture is not supported on this platform")]
    Unsupported,
}

/// What to capture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CaptureTarget {
    /// A whole display; `None` is the main display.
    Display { display_id: Option<u32> },
    /// One window (follows it as it moves; other windows never cover it).
    Window { window_id: u32 },
    /// A region of a display. `rect` is in global display points and is
    /// clipped to the display; [`FrameStream::geometry`] reports the
    /// clipped rect.
    Region { display_id: Option<u32>, rect: Rect },
}

impl Default for CaptureTarget {
    fn default() -> Self {
        CaptureTarget::Display { display_id: None }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FrameStreamConfig {
    pub target: CaptureTarget,
    /// Maximum frames per second, 1–60. ScreenCaptureKit sends fewer when
    /// the screen is static.
    pub fps: u32,
    /// Frames kept in memory, at least 1.
    pub ring_capacity: usize,
    /// Draw the cursor into frames. Off by default: step screenshots get
    /// their own click marker.
    pub shows_cursor: bool,
    /// Leave this process's windows (toolbar, menu bar popover) out of
    /// display and region captures. See [`FrameStream::own_app_excluded`].
    pub exclude_own_windows: bool,
    /// Applications whose windows are left out of display and region
    /// captures, matched by [`bundle_id_matches`] (so their helpers too).
    /// See `# Excluded apps` in the module docs.
    pub excluded_bundle_ids: Vec<String>,
    /// Reuse pixel buffers of evicted frames instead of allocating a new
    /// one per frame.
    pub reuse_buffers: bool,
}

impl Default for FrameStreamConfig {
    fn default() -> Self {
        Self {
            target: CaptureTarget::default(),
            fps: DEFAULT_FPS,
            ring_capacity: DEFAULT_RING_CAPACITY,
            shows_cursor: false,
            exclude_own_windows: true,
            excluded_bundle_ids: Vec::new(),
            reuse_buffers: true,
        }
    }
}

impl FrameStreamConfig {
    #[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
    pub(crate) fn validate(&self) -> Result<(), FrameError> {
        if !(1..=60).contains(&self.fps) {
            return Err(FrameError::InvalidConfig(format!(
                "fps must be 1-60, got {}",
                self.fps
            )));
        }
        if self.ring_capacity == 0 {
            return Err(FrameError::InvalidConfig(
                "ring_capacity must be at least 1".into(),
            ));
        }
        if let CaptureTarget::Region { rect, .. } = self.target {
            if !(rect.width > 0.0 && rect.height > 0.0) {
                return Err(FrameError::InvalidConfig(format!(
                    "region must have a positive size, got {}x{}",
                    rect.width, rect.height
                )));
            }
        }
        Ok(())
    }
}

/// Maps global display points to pixels of a captured image.
///
/// `origin` is the global point shown at pixel (0, 0); `scale_x` and
/// `scale_y` are pixels per point (2.0 on Retina displays).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameGeometry {
    pub origin: Point,
    pub scale_x: f64,
    pub scale_y: f64,
    pub width_px: u32,
    pub height_px: u32,
}

impl FrameGeometry {
    /// Geometry of an image of `width_px`×`height_px` showing `rect`
    /// (global points).
    pub fn from_rect(rect: Rect, width_px: u32, height_px: u32) -> Self {
        let ratio = |px: u32, pt: f64| if pt > 0.0 { f64::from(px) / pt } else { 1.0 };
        Self {
            origin: Point {
                x: rect.x,
                y: rect.y,
            },
            scale_x: ratio(width_px, rect.width),
            scale_y: ratio(height_px, rect.height),
            width_px,
            height_px,
        }
    }

    /// Pixels per point (horizontal; equal to vertical unless the stream
    /// was scaled non-uniformly).
    pub fn scale(&self) -> f64 {
        self.scale_x
    }

    /// The global rect, in points, that the image shows.
    pub fn rect_points(&self) -> Rect {
        Rect {
            x: self.origin.x,
            y: self.origin.y,
            width: f64::from(self.width_px) / self.scale_x,
            height: f64::from(self.height_px) / self.scale_y,
        }
    }

    /// Global point to pixel position. Can fall outside the image.
    pub fn point_to_pixel(&self, p: Point) -> Point {
        Point {
            x: (p.x - self.origin.x) * self.scale_x,
            y: (p.y - self.origin.y) * self.scale_y,
        }
    }

    /// Global point to pixel position, or `None` if it is not in the image.
    pub fn point_to_pixel_checked(&self, p: Point) -> Option<Point> {
        let px = self.point_to_pixel(p);
        let inside = px.x >= 0.0
            && px.y >= 0.0
            && px.x < f64::from(self.width_px)
            && px.y < f64::from(self.height_px);
        inside.then_some(px)
    }

    /// Pixel position to global point.
    pub fn pixel_to_point(&self, px: Point) -> Point {
        Point {
            x: self.origin.x + px.x / self.scale_x,
            y: self.origin.y + px.y / self.scale_y,
        }
    }

    /// Global rect (e.g. an AX element frame) to a pixel rect. Not clipped.
    pub fn rect_to_pixels(&self, r: Rect) -> Rect {
        let tl = self.point_to_pixel(Point { x: r.x, y: r.y });
        Rect {
            x: tl.x,
            y: tl.y,
            width: r.width * self.scale_x,
            height: r.height * self.scale_y,
        }
    }
}

/// A display and its point-to-pixel mapping.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DisplayInfo {
    pub id: u32,
    /// Bounds in global points (origin top-left of the main display).
    pub bounds: Rect,
    /// Backing pixels per point (2.0 on Retina, including scaled modes).
    pub scale: f64,
}

impl DisplayInfo {
    /// Size of a full-display capture, in pixels.
    pub fn pixel_size(&self) -> (u32, u32) {
        (
            (self.bounds.width * self.scale).round() as u32,
            (self.bounds.height * self.scale).round() as u32,
        )
    }

    /// Geometry of a full-display capture.
    pub fn geometry(&self) -> FrameGeometry {
        let (w, h) = self.pixel_size();
        FrameGeometry::from_rect(self.bounds, w, h)
    }

    /// Global point to pixel position within a full-display capture.
    pub fn point_to_pixel(&self, p: Point) -> Point {
        self.geometry().point_to_pixel(p)
    }
}

/// True if an application with `bundle_id` is excluded by `excluded`: its
/// id is one of them, or starts with one of them followed by `.` (a helper
/// bundled with the app, e.g. `com.1password.1password.helper` for
/// `com.1password.1password`). Case-sensitive, like bundle ids on disk.
/// Other look-alike ids do not match (`com.1password.1password-launcher`,
/// `com.bitwarden.desktopx`), nor does an empty entry. On Windows, where an
/// id is an executable's file name, case is ignored, as it is for file
/// names.
pub fn bundle_id_matches(bundle_id: &str, excluded: &[String]) -> bool {
    let fold = |s: &str| {
        if cfg!(windows) {
            s.to_ascii_lowercase()
        } else {
            s.to_owned()
        }
    };
    let bundle_id = fold(bundle_id);
    excluded.iter().any(|e| {
        let e = fold(e);
        !e.is_empty()
            && bundle_id
                .strip_prefix(e.as_str())
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
    })
}

/// Indexes of the applications (`(bundle id, pid)`, as listed by
/// ScreenCaptureKit) a display filter leaves out: this process (`own_pid`)
/// when `exclude_own`, and every app whose bundle id matches `excluded` by
/// [`bundle_id_matches`]. An app without a bundle id is only left out if it
/// is this process.
pub fn excluded_apps<'a>(
    apps: impl IntoIterator<Item = (Option<&'a str>, i32)>,
    own_pid: i32,
    exclude_own: bool,
    excluded: &[String],
) -> Vec<usize> {
    apps.into_iter()
        .enumerate()
        .filter(|(_, (id, pid))| {
            (exclude_own && *pid == own_pid) || id.is_some_and(|id| bundle_id_matches(id, excluded))
        })
        .map(|(i, _)| i)
        .collect()
}

/// Smallest width and height, in points, of a window [`pickable_windows`]
/// keeps.
pub const MIN_PICKABLE_WINDOW_SIDE: f64 = 80.0;

/// A window as ScreenCaptureKit lists it, for choosing a window target.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowInfo {
    pub window_id: u32,
    /// Owning process; 0 when ScreenCaptureKit names no owner.
    pub pid: i32,
    pub app_name: String,
    pub bundle_id: Option<String>,
    pub title: Option<String>,
    /// Global points.
    pub frame: Rect,
    /// 0 is the normal window layer; menus, the Dock and overlays are
    /// higher.
    pub layer: isize,
    pub on_screen: bool,
}

/// The windows a user can pick as a capture target, in the order given
/// (ScreenCaptureKit lists front to back): on screen, on the normal layer,
/// owned by an app, at least [`MIN_PICKABLE_WINDOW_SIDE`] on both sides,
/// and neither this process's (`own_pid`) nor an excluded app's (by
/// [`bundle_id_matches`]).
pub fn pickable_windows(
    windows: impl IntoIterator<Item = WindowInfo>,
    own_pid: i32,
    excluded: &[String],
) -> Vec<WindowInfo> {
    windows
        .into_iter()
        .filter(|w| {
            w.on_screen
                && w.layer == 0
                && w.pid != 0
                && w.pid != own_pid
                && w.frame.width >= MIN_PICKABLE_WINDOW_SIDE
                && w.frame.height >= MIN_PICKABLE_WINDOW_SIDE
                && !w
                    .bundle_id
                    .as_deref()
                    .is_some_and(|id| bundle_id_matches(id, excluded))
        })
        .collect()
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

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|&s| s.to_owned()).collect()
    }

    #[test]
    fn retina_point_to_pixel() {
        // Secondary display to the right of a 1512-pt main display, 2x.
        let d = DisplayInfo {
            id: 2,
            bounds: rect(1512.0, -200.0, 1728.0, 1117.0),
            scale: 2.0,
        };
        assert_eq!(d.pixel_size(), (3456, 2234));
        let px = d.point_to_pixel(Point {
            x: 1612.5,
            y: -100.0,
        });
        assert_eq!(px, Point { x: 201.0, y: 200.0 });
        let g = d.geometry();
        assert_eq!(g.scale(), 2.0);
        assert_eq!(
            g.pixel_to_point(px),
            Point {
                x: 1612.5,
                y: -100.0
            }
        );
        assert!(g.point_to_pixel_checked(Point { x: 0.0, y: 0.0 }).is_none());
        assert_eq!(g.rect_points(), d.bounds);
    }

    #[test]
    fn region_geometry_and_rects() {
        let g = FrameGeometry::from_rect(rect(100.0, 50.0, 400.0, 300.0), 800, 600);
        assert_eq!((g.scale_x, g.scale_y), (2.0, 2.0));
        assert_eq!(
            g.point_to_pixel_checked(Point { x: 100.0, y: 50.0 }),
            Some(Point { x: 0.0, y: 0.0 })
        );
        assert!(g
            .point_to_pixel_checked(Point { x: 500.0, y: 50.0 })
            .is_none());
        let r = g.rect_to_pixels(rect(110.0, 60.0, 20.0, 10.0));
        assert_eq!(r, rect(20.0, 20.0, 40.0, 20.0));
    }

    #[test]
    fn non_uniform_scale() {
        let g = FrameGeometry::from_rect(rect(0.0, 0.0, 100.0, 100.0), 150, 200);
        assert_eq!(
            g.point_to_pixel(Point { x: 10.0, y: 10.0 }),
            Point { x: 15.0, y: 20.0 }
        );
    }

    #[test]
    fn bundle_id_matcher_covers_helpers_only() {
        let excluded = ids(&["com.1password.1password", "com.apple.Passwords"]);
        // Exact ids and `<id>.` helpers (real ids from 1Password 8 and
        // macOS 15's Passwords).
        for id in [
            "com.1password.1password",
            "com.1password.1password.helper",
            "com.1password.1password.helper.Renderer",
            "com.apple.Passwords",
            "com.apple.Passwords.MenuBarExtra",
        ] {
            assert!(bundle_id_matches(id, &excluded), "{id}");
        }
        // Look-alikes, other apps, and different case.
        for id in [
            "com.1password.1password-launcher",
            "2BUA8C4S2C.com.1password.browser-helper",
            "com.1password",
            "com.apple.PasswordsX",
            "com.apple.Safari",
            "",
        ] {
            assert!(!bundle_id_matches(id, &excluded), "{id}");
        }
        // Case matters for bundle ids, not for Windows file names.
        assert_eq!(
            bundle_id_matches("com.apple.passwords", &excluded),
            cfg!(windows)
        );
        assert_eq!(
            bundle_id_matches("1Password.exe", &ids(&["1password.exe"])),
            cfg!(windows)
        );
        // An empty entry excludes nothing.
        assert!(!bundle_id_matches("com.apple.Safari", &ids(&[""])));
        assert!(!bundle_id_matches("", &ids(&[""])));
        assert!(!bundle_id_matches("com.apple.Safari", &[]));
    }

    #[test]
    fn filter_excludes_own_and_matching_apps() {
        let apps = [
            (Some("com.apple.Safari"), 10),
            (Some("com.1password.1password"), 20),
            (Some("com.1password.1password.helper"), 21),
            (None, 30),
            (Some("dev.dufflebear.guiderecorder"), 40),
            (Some("com.bitwarden.desktop"), 50),
            (Some("com.bitwarden.desktopx"), 51),
        ];
        let excluded = ids(&["com.1password.1password", "com.bitwarden.desktop"]);
        assert_eq!(excluded_apps(apps, 40, true, &excluded), [1, 2, 4, 5]);
        // Not our own app when that is off.
        assert_eq!(excluded_apps(apps, 40, false, &excluded), [1, 2, 5]);
        // An app without a bundle id only as our own process.
        assert_eq!(excluded_apps(apps, 30, true, &[]), [3]);
        assert!(excluded_apps(apps, 99, true, &[]).is_empty());
        assert!(excluded_apps([], 40, true, &excluded).is_empty());
    }

    #[test]
    fn pickable_windows_keep_order_and_drop_the_unpickable() {
        let window = |id: u32, pid: i32, bundle: Option<&str>| WindowInfo {
            window_id: id,
            pid,
            app_name: format!("App {pid}"),
            bundle_id: bundle.map(str::to_owned),
            title: Some(format!("Window {id}")),
            frame: Rect {
                x: 0.0,
                y: 0.0,
                width: 800.0,
                height: 600.0,
            },
            layer: 0,
            on_screen: true,
        };
        let sized = |id, width, height| WindowInfo {
            frame: Rect {
                x: -1200.0,
                y: 40.0,
                width,
                height,
            },
            ..window(id, 20, Some("com.apple.Safari"))
        };
        let windows = vec![
            window(1, 20, Some("com.apple.Safari")),
            window(2, 99, Some("dev.dufflebear.guiderecorder")),
            window(3, 30, Some("com.1password.1password")),
            window(4, 31, Some("com.1password.1password.helper")),
            WindowInfo {
                layer: 25,
                ..window(5, 20, Some("com.apple.Safari"))
            },
            WindowInfo {
                on_screen: false,
                ..window(6, 20, Some("com.apple.Safari"))
            },
            sized(7, 79.0, 600.0),
            sized(8, 600.0, 79.5),
            sized(9, 80.0, 80.0),
            window(10, 0, None),
            window(11, 40, None),
            window(12, 50, Some("com.apple.finder")),
        ];
        let excluded = vec!["com.1password.1password".to_owned()];
        let ids = |ws: Vec<WindowInfo>| ws.iter().map(|w| w.window_id).collect::<Vec<_>>();
        assert_eq!(
            ids(pickable_windows(windows.clone(), 99, &excluded)),
            [1, 9, 11, 12]
        );
        // Nothing excluded and another own pid: the 1Password and
        // GuideRecorder windows come back, in place.
        assert_eq!(
            ids(pickable_windows(windows, 7, &[])),
            [1, 2, 3, 4, 9, 11, 12]
        );
    }

    #[test]
    fn config_validation() {
        assert!(FrameStreamConfig::default().validate().is_ok());
        let bad_fps = FrameStreamConfig {
            fps: 0,
            ..Default::default()
        };
        assert!(bad_fps.validate().is_err());
        let bad_ring = FrameStreamConfig {
            ring_capacity: 0,
            ..Default::default()
        };
        assert!(bad_ring.validate().is_err());
        let bad_region = FrameStreamConfig {
            target: CaptureTarget::Region {
                display_id: None,
                rect: rect(0.0, 0.0, 0.0, 10.0),
            },
            ..Default::default()
        };
        assert!(bad_region.validate().is_err());
    }
}
