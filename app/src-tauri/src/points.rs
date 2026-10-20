//! Capture points and window pixels. The capture backend's global points
//! are logical pixels on macOS and physical pixels on Windows (which has
//! no logical space shared by displays of different scales), so placing a
//! window at a point, or reading a window's frame as points, goes through
//! here.

use tauri::{PhysicalPosition, PhysicalSize, Position, Size};

/// Physical pixels per point on a display (or in a window) of `scale`:
/// the scale on macOS, 1 on Windows.
pub fn pixels_per_point(scale: f64) -> f64 {
    if cfg!(windows) || !(scale.is_finite() && scale > 0.0) {
        1.0
    } else {
        scale
    }
}

/// Logical (CSS) pixels per point on a display of `scale`: 1 on macOS,
/// `1 / scale` on Windows.
pub fn css_per_point(scale: f64) -> f64 {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    pixels_per_point(scale) / scale
}

/// The window position of global point (`x`, `y`).
pub fn position(x: f64, y: f64) -> Position {
    if cfg!(windows) {
        PhysicalPosition::new(x.round() as i32, y.round() as i32).into()
    } else {
        tauri::LogicalPosition::new(x, y).into()
    }
}

/// The window size of `width` × `height` points.
pub fn size(width: f64, height: f64) -> Size {
    if cfg!(windows) {
        PhysicalSize::new(
            width.round().max(1.0) as u32,
            height.round().max(1.0) as u32,
        )
        .into()
    } else {
        tauri::LogicalSize::new(width, height).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn points_follow_the_platform() {
        if cfg!(windows) {
            assert_eq!(pixels_per_point(1.5), 1.0);
            assert_eq!(css_per_point(2.0), 0.5);
            assert!(matches!(position(1.4, 2.6), Position::Physical(p) if (p.x, p.y) == (1, 3)));
        } else {
            assert_eq!(pixels_per_point(2.0), 2.0);
            assert_eq!(css_per_point(2.0), 1.0);
            assert!(matches!(position(1.5, 2.5), Position::Logical(_)));
        }
        assert_eq!(pixels_per_point(f64::NAN), 1.0);
        assert_eq!(css_per_point(0.0), 1.0);
    }
}
