//! Step image rendering: load → crop → annotations → burn-in blur → scale →
//! PNG.
//!
//! # Coordinates
//!
//! Annotation geometry is in full-screenshot pixels. The rendered view is a
//! window onto the full screenshot with an `origin` (its top-left, in those
//! pixels), and annotations are shifted by `-origin`.
//!
//! The click crop (`steps/<id>.crop.webp`) is placed by
//! `StepImage::crop_rect`. It is used when its size matches the crop image
//! (±1 px) and the crop equals the full screenshot at that position; if
//! the full screenshot can't be read, the rect is trusted as is. A rect that
//! fails the check is reported ([`RenderWarning::CropRectMismatch`]) and
//! ignored. Older guides lack it, so [`locate_crop`] is the fallback: the
//! crop is assumed to be centred on `step.point` and clamped to the
//! screenshot, and that guess (±1 px for rounding) is verified pixel for
//! pixel against the full screenshot, which lossless WebP makes exact. A
//! crop with no annotations needs no origin and is used as is. If the origin
//! is still unknown, the full screenshot is shown instead (with any crop
//! annotation applied) and a warning is reported, so a blur is never lost.
//!
//! A crop annotation cuts the view down to its rectangle (intersected with
//! the view); if a step has several, the last wins.
//!
//! # Hotspots
//!
//! Hotspots are numbered 1, 2, 3, ... in annotation order among the step's
//! hotspots. By default ([`ImageOptions::hotspot_markers`]) each is drawn as
//! an outlined rectangle with a numbered badge, for formats that can only
//! show pixels and print a numbered legend next to them. Either way,
//! [`RenderedImage::hotspots`] says where each one landed, as fractions of
//! the output image, so a format that can (HTML) overlays its own
//! interactive markers on an image rendered without them.
//!
//! # Order
//!
//! Annotations are drawn first and blur/pixelate is burned in after them,
//! so nothing inside a redacted region survives, including annotation text
//! drawn over it. Hotspot markers come last, over the blur, as the editor
//! shows them: they hold only an outline and a number.
//!
//! # Failure
//!
//! Rendering never fails: an image that is missing or can't be decoded
//! becomes a grey placeholder plus a [`RenderWarning`].

use std::fmt;

use image::imageops::{self, FilterType};
use image::RgbaImage;
use resvg::tiny_skia::{IntSize, Pixmap};

use super::annotate::{self, Layer};
use super::blur::{self, Region};
use crate::library::GuideHandle;
use crate::model::{Annotation, BlurMode, ImageShow, Point, Rect, Shape, Step};

/// How to render a step image.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageOptions {
    /// Overrides the step's [`ImageShow`].
    pub show: Option<ImageShow>,
    /// Scale down (never up) to at most this width, keeping the aspect
    /// ratio.
    pub max_width: Option<u32>,
    /// Size of the placeholder for a missing image, before `max_width`.
    pub placeholder_size: (u32, u32),
    /// Draw hotspots into the pixels as numbered markers (on by default).
    /// Off for a writer that overlays its own markers using
    /// [`RenderedImage::hotspots`].
    pub hotspot_markers: bool,
}

impl Default for ImageOptions {
    fn default() -> Self {
        Self {
            show: None,
            max_width: None,
            placeholder_size: (800, 450),
            hotspot_markers: true,
        }
    }
}

/// A rendered step image.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderedImage {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub warnings: Vec<RenderWarning>,
    /// Every hotspot of the step, in number order, whether or not it is in
    /// view (a placeholder has them all, unplaced).
    pub hotspots: Vec<PlacedHotspot>,
}

/// A hotspot of a rendered step image.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedHotspot {
    /// 1-based, in annotation order among the step's hotspots.
    pub number: u32,
    /// The annotation's id.
    pub id: ulid::Ulid,
    /// Plain text.
    pub title: String,
    /// Plain text; may hold line breaks.
    pub description: String,
    /// Where it sits on the output image (after crop and scaling), as
    /// fractions (0 to 1) of its width and height, clamped to it. `None`
    /// if it lies entirely outside the view, its geometry is unusable, or
    /// the image is a placeholder.
    pub rect: Option<Rect>,
    /// The annotation's colour, if it is a valid hex colour or CSS colour
    /// name (see [`RenderWarning::InvalidColor`]), trimmed; safe to place in
    /// CSS as is.
    pub color: Option<String>,
}

impl RenderedImage {
    /// Decodes [`Self::png`] back into straight RGBA, e.g. to re-encode it
    /// as WebP.
    pub fn to_rgba(&self) -> Option<RgbaImage> {
        let pixmap = Pixmap::decode_png(&self.png).ok()?;
        let (w, h) = (pixmap.width(), pixmap.height());
        RgbaImage::from_raw(w, h, pixmap.take_demultiplied())
    }
}

/// Something that degraded a render without failing it.
#[derive(Debug, Clone, PartialEq)]
pub enum RenderWarning {
    /// The image at `path` could not be read; a placeholder (or, for a
    /// crop, the full screenshot) was used instead.
    ImageUnavailable { path: String, reason: String },
    /// The crop's position in the full screenshot is unknown, so the full
    /// screenshot was shown to keep annotations and blur in place.
    CropOffsetUnknown,
    /// The stored `cropRect` does not match the crop image (size, or pixels
    /// at that position in the full screenshot) and was ignored.
    CropRectMismatch,
    /// The annotation's colour is not a hex colour or CSS colour name; the
    /// default was used.
    InvalidColor { id: ulid::Ulid, value: String },
    /// The crop annotation lies outside the image and was ignored.
    CropOutside { id: ulid::Ulid },
    /// The annotation could not be drawn.
    AnnotationSkipped { id: ulid::Ulid, reason: String },
    /// The callout's text does not fit its box and was clipped.
    CalloutOverflow { id: ulid::Ulid },
    /// The annotation layer failed to render.
    Annotations(String),
}

impl fmt::Display for RenderWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ImageUnavailable { path, reason } => {
                write!(f, "image {path} unavailable: {reason}")
            }
            Self::CropOffsetUnknown => {
                f.write_str("crop position unknown; showing the full screenshot")
            }
            Self::CropRectMismatch => {
                f.write_str("stored crop rectangle does not match the crop; ignored")
            }
            Self::InvalidColor { id, value } => {
                write!(
                    f,
                    "annotation {id} has invalid colour {value:?}; using the default"
                )
            }
            Self::CropOutside { id } => write!(f, "crop {id} lies outside the image; ignored"),
            Self::AnnotationSkipped { id, reason } => {
                write!(f, "annotation {id} skipped: {reason}")
            }
            Self::CalloutOverflow { id } => write!(f, "callout {id} text does not fit its box"),
            Self::Annotations(e) => write!(f, "annotations not rendered: {e}"),
        }
    }
}

/// Renders the image of `step` from the bundle at `handle`.
pub fn render_step_image(handle: &GuideHandle, step: &Step, opts: &ImageOptions) -> RenderedImage {
    let mut warnings = Vec::new();
    let Some((source, origin)) = load_view(handle, step, opts, &mut warnings) else {
        let hotspots = place_hotspots(&step.annotations, None);
        return placeholder(opts, warnings, hotspots);
    };
    let (source, origin) = apply_crop(source, origin, step, &mut warnings);
    // Fractions of the view, which scaling keeps.
    let hotspots = place_hotspots(&step.annotations, Some((origin, source.dimensions())));

    // One pixmap from here on; the source buffer is reused, not copied.
    let mut pixmap = into_pixmap(source);
    annotate::draw(
        &step.annotations,
        origin,
        &mut pixmap.as_mut(),
        Layer::Shapes,
        &mut warnings,
    );
    // Redaction comes before any scaling.
    burn_in_redactions(&mut pixmap, origin, step);
    // Markers show only numbers and outlines, and stay readable over a blur.
    if opts.hotspot_markers {
        annotate::draw(
            &step.annotations,
            origin,
            &mut pixmap.as_mut(),
            Layer::Hotspots,
            &mut warnings,
        );
    }

    if let Some(max) = opts
        .max_width
        .filter(|&max| max > 0 && max < pixmap.width())
    {
        let height = scaled_height(pixmap.width(), pixmap.height(), max);
        let rgba = from_pixmap(pixmap);
        pixmap = into_pixmap(imageops::resize(&rgba, max, height, FilterType::Lanczos3));
    }
    encode(&pixmap, warnings, hotspots)
}

/// The step's hotspots, numbered, with their rectangles as fractions of
/// the view at `origin` with size `(w, h)` (full-screenshot pixels), clamped
/// to it. Without a view (a placeholder), none is placed.
fn place_hotspots(
    annotations: &[Annotation],
    view: Option<(Point, (u32, u32))>,
) -> Vec<PlacedHotspot> {
    annotations
        .iter()
        .filter_map(|a| match &a.shape {
            Shape::Hotspot(geometry) => Some((a, geometry)),
            _ => None,
        })
        .zip(1..)
        .map(|((annotation, geometry), number)| PlacedHotspot {
            number,
            id: annotation.id,
            title: geometry.title.clone(),
            description: geometry.description.clone(),
            rect: view.and_then(|(origin, size)| fraction_rect(&geometry.rect, origin, size)),
            color: annotation
                .style
                .color
                .as_deref()
                .map(str::trim)
                .filter(|c| annotate::is_valid_color(c))
                .map(str::to_owned),
        })
        .collect()
}

/// `rect` (full-screenshot pixels) as fractions of the view at `origin`
/// with size `(w, h)`, clamped to it; `None` if nothing of it is in view or
/// it is not a finite, non-empty rectangle.
fn fraction_rect(rect: &Rect, origin: Point, (w, h): (u32, u32)) -> Option<Rect> {
    let finite = [rect.x, rect.y, rect.width, rect.height]
        .iter()
        .all(|v| v.is_finite());
    if !finite || rect.width <= 0.0 || rect.height <= 0.0 || w == 0 || h == 0 {
        return None;
    }
    let (w, h) = (f64::from(w), f64::from(h));
    let x0 = (rect.x - origin.x).clamp(0.0, w);
    let y0 = (rect.y - origin.y).clamp(0.0, h);
    let x1 = (rect.x + rect.width - origin.x).clamp(0.0, w);
    let y1 = (rect.y + rect.height - origin.y).clamp(0.0, h);
    (x1 > x0 && y1 > y0).then(|| Rect {
        x: x0 / w,
        y: y0 / h,
        width: (x1 - x0) / w,
        height: (y1 - y0) / h,
    })
}

/// The image to show and its origin in full-screenshot pixels.
fn load_view(
    handle: &GuideHandle,
    step: &Step,
    opts: &ImageOptions,
    warnings: &mut Vec<RenderWarning>,
) -> Option<(RgbaImage, Point)> {
    let show = opts.show.unwrap_or(step.image.show);
    if let (ImageShow::Crop, Some(crop_path)) = (show, &step.image.crop) {
        match read(handle, crop_path, warnings) {
            Some(crop) if !needs_origin(step) => return Some((crop, ORIGIN)),
            Some(crop) => {
                let full = handle.read_image(&step.image.full);
                if let Some(rect) = step.image.crop_rect {
                    match (stored_origin(rect, &crop), &full) {
                        // Verified against the full screenshot.
                        (Some((x, y)), Ok(full)) if crop_matches_at(full, &crop, x, y) => {
                            return Some((crop, point(x, y)));
                        }
                        // Nothing to verify against: trust the stored rect,
                        // which capture cut the crop with.
                        (Some((x, y)), Err(_)) => return Some((crop, point(x, y))),
                        _ => warnings.push(RenderWarning::CropRectMismatch),
                    }
                }
                let full = match full {
                    Ok(full) => full,
                    // Without the full screenshot the crop can't be placed,
                    // so show nothing rather than an unredacted crop.
                    Err(e) => {
                        warnings.push(unavailable(&step.image.full, &e));
                        return None;
                    }
                };
                if let Some(origin) = locate_crop(&full, &crop, step.point) {
                    return Some((crop, origin));
                }
                warnings.push(RenderWarning::CropOffsetUnknown);
                return Some((full, ORIGIN));
            }
            // Fall back to the full screenshot (warning already recorded).
            None => {}
        }
    }
    let full = read(handle, &step.image.full, warnings)?;
    Some((full, ORIGIN))
}

const ORIGIN: Point = Point { x: 0.0, y: 0.0 };

fn point(x: u32, y: u32) -> Point {
    Point {
        x: f64::from(x),
        y: f64::from(y),
    }
}

/// The whole-pixel origin from a stored `crop_rect`, if its size matches the
/// crop image (±1 px).
/// Whether a stored `crop_rect` places `crop` exactly in `full`: the size
/// matches (±1 px), the origin is whole pixels, and the crop equals the
/// full screenshot there. This is the check [`render_step_image`] applies
/// before trusting `crop_rect`.
pub fn verify_crop_rect(full: &RgbaImage, crop: &RgbaImage, rect: Rect) -> bool {
    stored_origin(rect, crop).is_some_and(|(x, y)| crop_matches_at(full, crop, x, y))
}

fn stored_origin(r: Rect, crop: &RgbaImage) -> Option<(u32, u32)> {
    let whole = |v: f64| {
        (v.is_finite() && v >= 0.0 && (v - v.round()).abs() < 1e-6).then(|| v.round() as u32)
    };
    let (w, h) = crop.dimensions();
    let fits = (r.width - f64::from(w)).abs() <= 1.0 && (r.height - f64::from(h)).abs() <= 1.0;
    if !fits {
        return None;
    }
    Some((whole(r.x)?, whole(r.y)?))
}

/// Whether anything is drawn at a position, so the crop's origin matters.
fn needs_origin(step: &Step) -> bool {
    !step.annotations.is_empty()
}

fn read(handle: &GuideHandle, path: &str, warnings: &mut Vec<RenderWarning>) -> Option<RgbaImage> {
    handle
        .read_image(path)
        .map_err(|e| warnings.push(unavailable(path, &e)))
        .ok()
}

fn unavailable(path: &str, e: &crate::library::LibraryError) -> RenderWarning {
    RenderWarning::ImageUnavailable {
        path: path.to_owned(),
        reason: e.to_string(),
    }
}

/// Whether `crop` equals `full` at `(x, y)`, pixel for pixel.
fn crop_matches_at(full: &RgbaImage, crop: &RgbaImage, x: u32, y: u32) -> bool {
    let (cw, ch) = crop.dimensions();
    let fits = u64::from(x) + u64::from(cw) <= u64::from(full.width())
        && u64::from(y) + u64::from(ch) <= u64::from(full.height());
    fits && (0..ch).all(|row| {
        let start = (((y + row) as usize) * full.width() as usize + x as usize) * 4;
        let theirs = &crop.as_raw()[row as usize * cw as usize * 4..][..cw as usize * 4];
        &full.as_raw()[start..start + cw as usize * 4] == theirs
    })
}

/// Finds the crop's top-left in `full`: centred on `point` and clamped to
/// the screenshot, ±1 px, verified by exact pixel comparison.
pub fn locate_crop(full: &RgbaImage, crop: &RgbaImage, point: Option<Point>) -> Option<Point> {
    let point = point.filter(|p| p.x.is_finite() && p.y.is_finite())?;
    let (fw, fh) = full.dimensions();
    let (cw, ch) = crop.dimensions();
    if cw > fw || ch > fh {
        return None;
    }
    let guess = |p: f64, c: u32, f: u32| {
        (p - f64::from(c) / 2.0)
            .round()
            .clamp(0.0, f64::from(f - c)) as i64
    };
    let (gx, gy) = (guess(point.x, cw, fw), guess(point.y, ch, fh));
    let candidates = [0, -1, 1]
        .into_iter()
        .flat_map(|dy| [0, -1, 1].map(|dx| (gx + dx, gy + dy)));
    let matches =
        |(x, y): (i64, i64)| x >= 0 && y >= 0 && crop_matches_at(full, crop, x as u32, y as u32);
    let mut seen = Vec::new();
    candidates
        .filter(|c| {
            let new = !seen.contains(c);
            seen.push(*c);
            new
        })
        .find(|&c| matches(c))
        .map(|(x, y)| Point {
            x: x as f64,
            y: y as f64,
        })
}

/// Applies the last crop annotation, returning the cut image and its
/// origin.
fn apply_crop(
    source: RgbaImage,
    origin: Point,
    step: &Step,
    warnings: &mut Vec<RenderWarning>,
) -> (RgbaImage, Point) {
    let Some((id, rect)) = step.annotations.iter().rev().find_map(|a| match a.shape {
        Shape::Crop(rect) => Some((a.id, rect)),
        _ => None,
    }) else {
        return (source, origin);
    };
    match region(&rect, origin, source.dimensions()) {
        Some(r) => {
            let cut = imageops::crop_imm(&source, r.x, r.y, r.width, r.height).to_image();
            let origin = Point {
                x: origin.x + f64::from(r.x),
                y: origin.y + f64::from(r.y),
            };
            (cut, origin)
        }
        None => {
            warnings.push(RenderWarning::CropOutside { id });
            (source, origin)
        }
    }
}

/// `rect` (full-screenshot pixels) as whole view pixels, grown outward to
/// cover every partly covered pixel and clamped to the view. `None` if
/// nothing is left.
fn region(rect: &Rect, origin: Point, (w, h): (u32, u32)) -> Option<Region> {
    if ![rect.x, rect.y, rect.width, rect.height]
        .iter()
        .all(|v| v.is_finite())
        || rect.width <= 0.0
        || rect.height <= 0.0
    {
        return None;
    }
    let x0 = (rect.x - origin.x).floor().clamp(0.0, f64::from(w));
    let y0 = (rect.y - origin.y).floor().clamp(0.0, f64::from(h));
    let x1 = (rect.x + rect.width - origin.x)
        .ceil()
        .clamp(0.0, f64::from(w));
    let y1 = (rect.y + rect.height - origin.y)
        .ceil()
        .clamp(0.0, f64::from(h));
    (x1 > x0 && y1 > y0).then_some(Region {
        x: x0 as u32,
        y: y0 as u32,
        width: (x1 - x0) as u32,
        height: (y1 - y0) as u32,
    })
}

/// Burns every blur annotation into the pixels, in order.
fn burn_in_redactions(pixmap: &mut Pixmap, origin: Point, step: &Step) {
    let (w, h) = (pixmap.width(), pixmap.height());
    for annotation in &step.annotations {
        let Shape::Blur(rect) = &annotation.shape else {
            continue;
        };
        let mode = annotation.style.blur_mode.unwrap_or(BlurMode::Blur);
        if let Some(r) = region(rect, origin, (w, h)) {
            blur::burn_in(
                pixmap.data_mut(),
                (w, h),
                r,
                mode,
                annotation.style.blur_amount,
            );
        }
    }
}

fn scaled_height(width: u32, height: u32, new_width: u32) -> u32 {
    let h = (f64::from(height) * f64::from(new_width) / f64::from(width)).round();
    (h as u32).max(1)
}

/// Straight RGBA → premultiplied pixmap, reusing the buffer.
fn into_pixmap(image: RgbaImage) -> Pixmap {
    let size =
        IntSize::from_wh(image.width(), image.height()).expect("decoded images are non-empty");
    let mut data = image.into_raw();
    for px in data.as_chunks_mut::<4>().0 {
        let a = u16::from(px[3]);
        if a < 255 {
            for c in &mut px[..3] {
                *c = ((u16::from(*c) * a + 127) / 255) as u8;
            }
        }
    }
    Pixmap::from_vec(data, size).expect("buffer matches size")
}

/// Premultiplied pixmap → straight RGBA, reusing the buffer.
fn from_pixmap(pixmap: Pixmap) -> RgbaImage {
    let (w, h) = (pixmap.width(), pixmap.height());
    RgbaImage::from_raw(w, h, pixmap.take_demultiplied()).expect("buffer matches size")
}

fn encode(
    pixmap: &Pixmap,
    warnings: Vec<RenderWarning>,
    hotspots: Vec<PlacedHotspot>,
) -> RenderedImage {
    let png = pixmap
        .encode_png()
        .expect("encoding a valid pixmap into memory cannot fail");
    RenderedImage {
        png,
        width: pixmap.width(),
        height: pixmap.height(),
        warnings,
        hotspots,
    }
}

const PLACEHOLDER_BG: [u8; 4] = [0xee, 0xee, 0xf0, 0xff];
const PLACEHOLDER_FG: [u8; 4] = [0xb0, 0xb0, 0xb8, 0xff];

/// A light grey image with a border and a cross, at
/// `opts.placeholder_size` scaled to `opts.max_width`.
fn placeholder(
    opts: &ImageOptions,
    warnings: Vec<RenderWarning>,
    hotspots: Vec<PlacedHotspot>,
) -> RenderedImage {
    let (mut w, mut h) = opts.placeholder_size;
    (w, h) = (w.clamp(1, 4096), h.clamp(1, 4096));
    if let Some(max) = opts.max_width.filter(|&max| max > 0 && max < w) {
        (w, h) = (max, scaled_height(w, h, max));
    }
    let line = (w.min(h) / 100).max(1);
    let image = RgbaImage::from_fn(w, h, |x, y| {
        let border = x < line || y < line || x >= w - line || y >= h - line;
        // Distance from each diagonal, in pixels along x.
        let fx = f64::from(x) + 0.5;
        let d1 = (fx - (f64::from(y) + 0.5) * f64::from(w) / f64::from(h)).abs();
        let d2 = (fx - (f64::from(h) - f64::from(y) - 0.5) * f64::from(w) / f64::from(h)).abs();
        let cross = d1.min(d2) < f64::from(line) * f64::from(w) / f64::from(h).max(1.0) / 2.0 + 0.5;
        image::Rgba(if border || cross {
            PLACEHOLDER_FG
        } else {
            PLACEHOLDER_BG
        })
    });
    encode(&into_pixmap(image), warnings, hotspots)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| {
            image::Rgba([(x * 7 + y) as u8, (y * 13) as u8, (x ^ y) as u8, 255])
        })
    }

    #[test]
    fn verifies_stored_crop_rect() {
        let full = pattern(64, 40);
        let crop = imageops::crop_imm(&full, 20, 10, 16, 10).to_image();
        let rect = |x, y, width, height| Rect {
            x,
            y,
            width,
            height,
        };
        assert!(verify_crop_rect(&full, &crop, rect(20.0, 10.0, 16.0, 10.0)));
        // Right size, wrong place: the pixels don't match.
        assert!(!verify_crop_rect(
            &full,
            &crop,
            rect(21.0, 10.0, 16.0, 10.0)
        ));
        // Size off by more than a pixel, or a fractional origin.
        assert!(!verify_crop_rect(
            &full,
            &crop,
            rect(20.0, 10.0, 18.0, 10.0)
        ));
        assert!(!verify_crop_rect(
            &full,
            &crop,
            rect(20.5, 10.0, 16.0, 10.0)
        ));
        // Outside the screenshot.
        assert!(!verify_crop_rect(
            &full,
            &crop,
            rect(60.0, 10.0, 16.0, 10.0)
        ));
    }

    #[test]
    fn locates_centred_crop() {
        let full = pattern(64, 40);
        let crop = imageops::crop_imm(&full, 20, 10, 16, 10).to_image();
        let at = |x, y| locate_crop(&full, &crop, Some(Point { x, y }));
        assert_eq!(at(28.0, 15.0), Some(Point { x: 20.0, y: 10.0 }));
        // Off by one from rounding.
        assert_eq!(at(28.6, 15.0), Some(Point { x: 20.0, y: 10.0 }));
        assert_eq!(at(40.0, 15.0), None);
        assert_eq!(locate_crop(&full, &crop, None), None);
        // Clamped at the edge.
        let corner = imageops::crop_imm(&full, 0, 0, 16, 10).to_image();
        assert_eq!(
            locate_crop(&full, &corner, Some(Point { x: 1.0, y: 1.0 })),
            Some(ORIGIN)
        );
    }

    #[test]
    fn regions_cover_partial_pixels_and_clamp() {
        let r = |x, y, width, height| Rect {
            x,
            y,
            width,
            height,
        };
        assert_eq!(
            region(&r(1.5, 2.2, 3.0, 1.0), ORIGIN, (10, 10)),
            Some(Region {
                x: 1,
                y: 2,
                width: 4,
                height: 2
            })
        );
        assert_eq!(
            region(&r(-5.0, 8.0, 20.0, 5.0), Point { x: 2.0, y: 0.0 }, (10, 10)),
            Some(Region {
                x: 0,
                y: 8,
                width: 10,
                height: 2
            })
        );
        assert_eq!(region(&r(20.0, 0.0, 5.0, 5.0), ORIGIN, (10, 10)), None);
        assert_eq!(region(&r(0.0, 0.0, f64::NAN, 5.0), ORIGIN, (10, 10)), None);
    }

    #[test]
    fn hotspot_fractions_follow_the_view_and_clamp() {
        let r = |x, y, width, height| Rect {
            x,
            y,
            width,
            height,
        };
        // A 200×100 view whose top-left is (100, 50) in the screenshot.
        let origin = Point { x: 100.0, y: 50.0 };
        assert_eq!(
            fraction_rect(&r(150.0, 75.0, 50.0, 25.0), origin, (200, 100)),
            Some(r(0.25, 0.25, 0.25, 0.25))
        );
        // Partly outside: clamped to the view.
        assert_eq!(
            fraction_rect(&r(50.0, 125.0, 100.0, 100.0), origin, (200, 100)),
            Some(r(0.0, 0.75, 0.25, 0.25))
        );
        // Entirely outside, touching the edge, or unusable.
        assert_eq!(
            fraction_rect(&r(0.0, 0.0, 100.0, 50.0), origin, (200, 100)),
            None
        );
        assert_eq!(
            fraction_rect(&r(400.0, 60.0, 10.0, 10.0), origin, (200, 100)),
            None
        );
        assert_eq!(
            fraction_rect(&r(150.0, 75.0, 0.0, 5.0), origin, (200, 100)),
            None
        );
        assert_eq!(
            fraction_rect(&r(f64::NAN, 75.0, 5.0, 5.0), origin, (200, 100)),
            None
        );
    }

    #[test]
    fn hotspots_are_numbered_among_themselves() {
        use crate::model::{AnnotationStyle, HotspotGeometry};
        let annotation = |shape| Annotation {
            id: ulid::Ulid::new(),
            shape,
            style: AnnotationStyle::default(),
        };
        let hotspot = |x, title: &str| {
            annotation(Shape::Hotspot(HotspotGeometry {
                rect: Rect {
                    x,
                    y: 0.0,
                    width: 10.0,
                    height: 10.0,
                },
                title: title.into(),
                description: String::new(),
            }))
        };
        let mut annotations = vec![
            annotation(Shape::Box(Rect {
                x: 0.0,
                y: 0.0,
                width: 5.0,
                height: 5.0,
            })),
            hotspot(0.0, "A"),
            hotspot(500.0, "Out of view"),
            annotation(Shape::Blur(Rect {
                x: 0.0,
                y: 0.0,
                width: 5.0,
                height: 5.0,
            })),
            hotspot(20.0, "C"),
        ];
        annotations[4].style.color = Some(" #00ff00 ".into());
        annotations[1].style.color = Some("url(#x)".into());
        let placed = place_hotspots(&annotations, Some((ORIGIN, (40, 20))));
        let summary: Vec<_> = placed
            .iter()
            .map(|h| (h.number, h.title.as_str(), h.rect.is_some()))
            .collect();
        assert_eq!(
            summary,
            [(1, "A", true), (2, "Out of view", false), (3, "C", true)]
        );
        assert_eq!(placed[2].id, annotations[4].id);
        assert_eq!(placed[2].color.as_deref(), Some("#00ff00"));
        assert_eq!(placed[0].color, None);
        // A placeholder keeps the numbers but places nothing.
        let unplaced = place_hotspots(&annotations, None);
        assert_eq!(unplaced.len(), 3);
        assert!(unplaced.iter().all(|h| h.rect.is_none()));
    }

    #[test]
    fn premultiply_round_trips_opaque_pixels() {
        let image = pattern(9, 7);
        assert_eq!(from_pixmap(into_pixmap(image.clone())), image);
    }

    #[test]
    fn placeholder_scales() {
        let opts = ImageOptions {
            max_width: Some(400),
            ..Default::default()
        };
        let image = placeholder(&opts, Vec::new(), Vec::new());
        assert_eq!((image.width, image.height), (400, 225));
        assert_eq!(image.to_rgba().unwrap().dimensions(), (400, 225));
    }
}
