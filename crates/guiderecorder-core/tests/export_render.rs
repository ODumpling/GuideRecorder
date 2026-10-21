//! Export render model and step image rendering (#364, design #443).
//!
//! Image goldens are under `tests/golden/render/`; regenerate them with
//! `UPDATE_GOLDEN=1 cargo test -p guiderecorder-core --test export_render` and
//! look at them before committing. The committed goldens were generated on
//! aarch64 (Apple silicon); tiny-skia's antialiasing differs slightly by
//! CPU, which `TOLERANCE` absorbs. If x86 CI exceeds it (most likely on the
//! callout's text edges), widen `max_pixels` for that golden only.

mod support;

use std::fs;

use guiderecorder_core::export::ast::{Block, Inline};
use guiderecorder_core::export::blur::{effective_region, Region};
use guiderecorder_core::export::{
    render_cover_image, render_step_image, ImageOptions, PlacedHotspot, RenderGuide, RenderItem,
    RenderWarning, RenderedImage,
};
use guiderecorder_core::library::{
    step_image_path, GuideHandle, Library, PixelBuffer, StepImageVariant,
};
use guiderecorder_core::model::*;
use guiderecorder_core::Ulid;
use image::{imageops, Rgba, RgbaImage};
use support::{assert_golden_image, assert_golden_text, fixture_guides_dir, ImageTolerance};

/// Small allowance for antialiasing differences between platforms.
const TOLERANCE: ImageTolerance = ImageTolerance {
    channel: 4,
    max_pixels: 40,
};
const W: u32 = 160;
const H: u32 = 100;

// ---------------------------------------------------------------------------
// Helpers

/// A screenshot stand-in with structure: a gradient, a dark title bar and a
/// "button" so annotations and crops are easy to check by eye.
fn screenshot() -> RgbaImage {
    RgbaImage::from_fn(W, H, |x, y| {
        if y < 12 {
            Rgba([60, 60, 70, 255])
        } else if (60..110).contains(&x) && (40..60).contains(&y) {
            Rgba([0, 122, 255, 255])
        } else {
            Rgba([200 + (x * 50 / W) as u8, 210 + (y * 40 / H) as u8, 230, 255])
        }
    })
}

/// Deterministic noise: the worst case for redaction (every pixel differs).
fn noise(w: u32, h: u32) -> RgbaImage {
    let mut state = 0x9e37_79b9_u32;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state as u8
    };
    RgbaImage::from_fn(w, h, |_, _| Rgba([next(), next(), next(), 255]))
}

fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

fn point(x: f64, y: f64) -> Point {
    Point { x, y }
}

fn annotation(shape: Shape, style: AnnotationStyle) -> Annotation {
    Annotation {
        id: Ulid::new(),
        shape,
        style,
    }
}

fn blur_style(mode: BlurMode, amount: f64) -> AnnotationStyle {
    AnnotationStyle {
        blur_mode: Some(mode),
        blur_amount: Some(amount),
        ..Default::default()
    }
}

/// A temp library with one empty guide to write step images into.
struct Bundle {
    _dir: tempfile::TempDir,
    handle: GuideHandle,
}

impl Bundle {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let library = Library::open(dir.path()).unwrap();
        let handle = library.create(&Guide::new("Render test", "en")).unwrap();
        Self { _dir: dir, handle }
    }

    fn write(&self, id: Ulid, variant: StepImageVariant, image: &RgbaImage) -> String {
        let pixels = PixelBuffer::rgba(image.width(), image.height(), image.as_raw());
        self.handle.write_step_image(id, variant, &pixels).unwrap()
    }

    /// A step showing `full` (and `crop`, if given) with `annotations`.
    fn step(
        &self,
        full: &RgbaImage,
        crop: Option<&RgbaImage>,
        point: Option<Point>,
        annotations: Vec<Annotation>,
    ) -> Step {
        let id = Ulid::new();
        Step {
            id,
            title: "Click **Save**".into(),
            description: String::new(),
            kind: StepKind::Click,
            point,
            end_point: None,
            element: None,
            keystrokes: None,
            url: None,
            image: StepImage {
                full: self.write(id, StepImageVariant::Full, full),
                crop: crop.map(|crop| self.write(id, StepImageVariant::Crop, crop)),
                crop_rect: None,
                after: None,
                show: if crop.is_some() {
                    ImageShow::Crop
                } else {
                    ImageShow::Full
                },
            },
            annotations,
            narration: String::new(),
            ai_state: AiState::default(),
        }
    }

    fn render(&self, step: &Step) -> RenderedImage {
        render_step_image(&self.handle, step, &ImageOptions::default())
    }
}

fn decode(image: &RenderedImage) -> RgbaImage {
    let rgba = image.to_rgba().expect("valid PNG");
    assert_eq!(rgba.dimensions(), (image.width, image.height));
    rgba
}

/// Renders one annotation on [`screenshot`] and compares with its golden.
fn golden_annotation(name: &str, shape: Shape, style: AnnotationStyle) {
    let bundle = Bundle::new();
    let step = bundle.step(&screenshot(), None, None, vec![annotation(shape, style)]);
    let rendered = bundle.render(&step);
    assert_eq!(rendered.warnings, []);
    assert_golden_image(
        &format!("render/{name}.golden.webp"),
        &decode(&rendered),
        TOLERANCE,
    );
}

// ---------------------------------------------------------------------------
// Annotation goldens

#[test]
fn golden_arrow() {
    golden_annotation(
        "arrow",
        Shape::Arrow(ArrowGeometry {
            from: point(20.0, 85.0),
            to: point(85.0, 45.0),
        }),
        AnnotationStyle::default(),
    );
}

#[test]
fn golden_box() {
    golden_annotation(
        "box",
        Shape::Box(rect(56.0, 36.0, 58.0, 28.0)),
        AnnotationStyle {
            color: Some("#34c759".into()),
            stroke_width: Some(3.0),
            ..Default::default()
        },
    );
}

#[test]
fn golden_highlight() {
    golden_annotation(
        "highlight",
        Shape::Highlight(rect(10.0, 66.0, 120.0, 16.0)),
        AnnotationStyle::default(),
    );
}

#[test]
fn golden_callout() {
    golden_annotation(
        "callout",
        Shape::Callout(CalloutGeometry {
            rect: rect(6.0, 16.0, 76.0, 46.0),
            anchor: Some(point(85.0, 50.0)),
            text: "Click Save to finish".into(),
        }),
        AnnotationStyle {
            font_size: Some(12.0),
            ..Default::default()
        },
    );
}

#[test]
fn golden_blur() {
    golden_annotation(
        "blur",
        Shape::Blur(rect(50.0, 30.0, 70.0, 40.0)),
        blur_style(BlurMode::Blur, 6.0),
    );
}

#[test]
fn golden_pixelate() {
    golden_annotation(
        "pixelate",
        Shape::Blur(rect(50.0, 30.0, 70.0, 40.0)),
        blur_style(BlurMode::Pixelate, 8.0),
    );
}

#[test]
fn golden_hotspot() {
    golden_annotation(
        "hotspot",
        Shape::Hotspot(HotspotGeometry {
            rect: rect(56.0, 36.0, 58.0, 28.0),
            title: "Save".into(),
            description: "Saves the file".into(),
        }),
        AnnotationStyle::default(),
    );
}

fn hotspot(rect: Rect, title: &str) -> Annotation {
    annotation(
        Shape::Hotspot(HotspotGeometry {
            rect,
            title: title.into(),
            description: format!("About {title}\nSecond line"),
        }),
        AnnotationStyle::default(),
    )
}

fn placed(image: &RenderedImage) -> Vec<(u32, String, Option<[f64; 4]>)> {
    let round = |v: f64| (v * 1e6).round() / 1e6;
    image
        .hotspots
        .iter()
        .map(|h: &PlacedHotspot| {
            let rect = h
                .rect
                .map(|r| [round(r.x), round(r.y), round(r.width), round(r.height)]);
            (h.number, h.title.clone(), rect)
        })
        .collect()
}

#[test]
fn hotspots_are_placed_through_crop_and_scaling() {
    let bundle = Bundle::new();
    let annotations = vec![
        hotspot(rect(60.0, 40.0, 50.0, 20.0), "Button"),
        annotation(
            Shape::Crop(rect(40.0, 20.0, 100.0, 50.0)),
            Default::default(),
        ),
        // Half outside the crop, at its top-left.
        hotspot(rect(20.0, 10.0, 40.0, 20.0), "Corner"),
        // Entirely outside the crop: keeps its number, gets no rect.
        hotspot(rect(0.0, 80.0, 20.0, 10.0), "Hidden"),
    ];
    let step = bundle.step(&screenshot(), None, None, annotations);
    let rendered = bundle.render(&step);
    assert_eq!(rendered.warnings, []);
    assert_eq!((rendered.width, rendered.height), (100, 50));
    let expected = vec![
        (1, "Button".to_owned(), Some([0.2, 0.4, 0.5, 0.4])),
        (2, "Corner".to_owned(), Some([0.0, 0.0, 0.2, 0.2])),
        (3, "Hidden".to_owned(), None),
    ];
    assert_eq!(placed(&rendered), expected);
    assert_eq!(
        rendered.hotspots[0].description,
        "About Button\nSecond line"
    );
    assert_eq!(rendered.hotspots[0].id, step.annotations[0].id);

    // Scaling keeps the fractions.
    let scaled = render_step_image(
        &bundle.handle,
        &step,
        &ImageOptions {
            max_width: Some(40),
            ..Default::default()
        },
    );
    assert_eq!((scaled.width, scaled.height), (40, 20));
    assert_eq!(placed(&scaled), expected);

    // Without markers: the same placement, and the pixels of a step with no
    // hotspots at all.
    let unmarked_opts = ImageOptions {
        hotspot_markers: false,
        ..Default::default()
    };
    let unmarked = render_step_image(&bundle.handle, &step, &unmarked_opts);
    assert_eq!(placed(&unmarked), expected);
    let mut bare = step.clone();
    bare.annotations
        .retain(|a| !matches!(a.shape, Shape::Hotspot(_)));
    assert_eq!(decode(&unmarked), decode(&bundle.render(&bare)));
    assert_ne!(decode(&rendered), decode(&unmarked));
}

#[test]
fn hotspots_through_a_click_crop_and_on_placeholders() {
    let bundle = Bundle::new();
    let full = screenshot();
    // A 64×40 crop with its top-left at (53, 30).
    let crop = imageops::crop_imm(&full, 53, 30, 64, 40).to_image();
    let mut step = bundle.step(
        &full,
        Some(&crop),
        Some(point(85.0, 50.0)),
        vec![hotspot(rect(69.0, 40.0, 16.0, 10.0), "Save")],
    );
    let rendered = bundle.render(&step);
    assert_eq!(rendered.warnings, []);
    assert_eq!(
        placed(&rendered),
        [(1, "Save".to_owned(), Some([0.25, 0.25, 0.25, 0.25]))]
    );

    // A placeholder still lists the hotspot, unplaced.
    step.image.full = "steps/missing.webp".into();
    step.image.crop = None;
    let rendered = bundle.render(&step);
    assert_eq!((rendered.width, rendered.height), (800, 450));
    assert_eq!(placed(&rendered), [(1, "Save".to_owned(), None)]);
}

#[test]
fn click_point_is_placed_through_crop_and_scaling() {
    let bundle = Bundle::new();
    let full = screenshot();
    // A 64×40 crop with its top-left at (53, 30); the click is at its
    // centre (85, 50), and nothing is drawn on it.
    let crop = imageops::crop_imm(&full, 53, 30, 64, 40).to_image();
    let mut step = bundle.step(&full, Some(&crop), Some(point(69.0, 40.0)), Vec::new());
    // The default skips the lookup: the crop shows, the point does not.
    let rendered = bundle.render(&step);
    assert_eq!(rendered.width, 64);
    assert_eq!(rendered.point, None);

    let locate = ImageOptions {
        locate_point: true,
        max_width: Some(32),
        ..ImageOptions::default()
    };
    // Off-centre point: only the stored rect places the crop.
    step.image.crop_rect = Some(rect(53.0, 30.0, 64.0, 40.0));
    let rendered = render_step_image(&bundle.handle, &step, &locate);
    assert_eq!((rendered.width, rendered.height), (32, 20));
    assert_eq!(rendered.warnings, []);
    assert_eq!(rendered.point, Some(point(0.25, 0.25)));
    // Without it, the crop is located around the point.
    step.image.crop_rect = None;
    step.point = Some(point(85.0, 50.0));
    let rendered = render_step_image(&bundle.handle, &step, &locate);
    assert_eq!(rendered.point, Some(point(0.5, 0.5)));

    // On the full screenshot; outside the view or on a placeholder, none.
    step.image.show = ImageShow::Full;
    let rendered = render_step_image(&bundle.handle, &step, &ImageOptions::default());
    assert_eq!(rendered.point, Some(point(85.0 / 160.0, 0.5)));
    step.point = Some(point(400.0, 50.0));
    assert_eq!(bundle.render(&step).point, None);
    step.point = Some(point(85.0, 50.0));
    step.image.full = "steps/missing.webp".into();
    assert_eq!(bundle.render(&step).point, None);
}

#[test]
fn unusable_hotspots_warn_with_or_without_markers() {
    let bundle = Bundle::new();
    let empty = hotspot(rect(10.0, 10.0, 0.0, 5.0), "Empty");
    let id = empty.id;
    let step = bundle.step(&screenshot(), None, None, vec![empty]);
    for hotspot_markers in [true, false] {
        let opts = ImageOptions {
            hotspot_markers,
            ..Default::default()
        };
        let rendered = render_step_image(&bundle.handle, &step, &opts);
        assert_eq!(
            rendered.warnings,
            [RenderWarning::AnnotationSkipped {
                id,
                reason: "empty rectangle".into()
            }]
        );
        assert_eq!(placed(&rendered), [(1, "Empty".to_owned(), None)]);
    }
}

#[test]
fn golden_crop_annotation_maps_other_annotations() {
    let bundle = Bundle::new();
    let step = bundle.step(
        &screenshot(),
        None,
        None,
        vec![
            annotation(
                Shape::Crop(rect(40.0, 20.0, 90.0, 60.0)),
                Default::default(),
            ),
            annotation(Shape::Box(rect(56.0, 36.0, 58.0, 28.0)), Default::default()),
        ],
    );
    let rendered = bundle.render(&step);
    assert_eq!(rendered.warnings, []);
    assert_eq!((rendered.width, rendered.height), (90, 60));
    assert_golden_image("render/crop.golden.webp", &decode(&rendered), TOLERANCE);
}

#[test]
fn golden_click_crop_with_located_origin() {
    let bundle = Bundle::new();
    let full = screenshot();
    // A 64×40 crop centred on the click at (85, 50): origin (53, 30).
    let crop = imageops::crop_imm(&full, 53, 30, 64, 40).to_image();
    let step = bundle.step(
        &full,
        Some(&crop),
        Some(point(85.0, 50.0)),
        vec![
            annotation(Shape::Box(rect(56.0, 36.0, 58.0, 28.0)), Default::default()),
            // Partly outside the crop.
            annotation(
                Shape::Blur(rect(100.0, 40.0, 40.0, 10.0)),
                blur_style(BlurMode::Pixelate, 4.0),
            ),
        ],
    );
    let rendered = bundle.render(&step);
    assert_eq!(rendered.warnings, []);
    assert_eq!((rendered.width, rendered.height), (64, 40));
    let image = decode(&rendered);
    assert_golden_image("render/click_crop.golden.webp", &image, TOLERANCE);

    // Same pixels as rendering the full view and cutting out the crop.
    let full_view = render_step_image(
        &bundle.handle,
        &step,
        &ImageOptions {
            show: Some(ImageShow::Full),
            ..Default::default()
        },
    );
    let expected = imageops::crop_imm(&decode(&full_view), 53, 30, 64, 40).to_image();
    // The pixelate grid is aligned to the region, which the crop clips, so
    // compare outside the redacted rows.
    for (x, y, px) in image.enumerate_pixels() {
        if !(10..20).contains(&y) {
            assert_eq!(px, expected.get_pixel(x, y), "({x}, {y})");
        }
    }
}

#[test]
fn stored_crop_rect_places_an_off_centre_crop() {
    let bundle = Bundle::new();
    let full = screenshot();
    // Not centred on the point, so only `crop_rect` can place it.
    let crop = imageops::crop_imm(&full, 40, 20, 80, 50).to_image();
    let blur = annotation(
        Shape::Blur(rect(40.0, 30.0, 50.0, 20.0)),
        blur_style(BlurMode::Pixelate, 5.0),
    );
    let boxed = annotation(Shape::Box(rect(56.0, 36.0, 58.0, 28.0)), Default::default());
    let mut step = bundle.step(
        &full,
        Some(&crop),
        Some(point(150.0, 90.0)),
        vec![boxed, blur],
    );
    // Without the rect the guess fails and the full view is shown.
    assert_eq!(
        bundle.render(&step).warnings,
        [RenderWarning::CropOffsetUnknown]
    );

    step.image.crop_rect = Some(rect(40.0, 20.0, 80.0, 50.0));
    // The full screenshot isn't needed once the rect is stored.
    fs::remove_file(bundle.handle.resolve(&step.image.full).unwrap()).unwrap();
    let rendered = bundle.render(&step);
    assert_eq!(rendered.warnings, []);
    assert_eq!((rendered.width, rendered.height), (80, 50));
    let image = decode(&rendered);
    assert_golden_image("render/crop_rect.golden.webp", &image, TOLERANCE);

    // (40, 30)–(90, 50) in the full screenshot is (0, 10)–(50, 30) in the
    // crop: gradient and button edge there are now uniform 5×5 blocks, and
    // the row above is untouched.
    assert_eq!(image.get_pixel(2, 9), crop.get_pixel(2, 9));
    assert_ne!(image.get_pixel(0, 10), image.get_pixel(0, 9));
    for by in (10..30).step_by(5) {
        for bx in (0..50).step_by(5) {
            let first = image.get_pixel(bx, by);
            for y in by..by + 5 {
                for x in bx..bx + 5 {
                    assert_eq!(image.get_pixel(x, y), first, "({x}, {y})");
                }
            }
        }
    }

    // A rect whose size doesn't match the crop is not trusted.
    step.image.crop_rect = Some(rect(40.0, 20.0, 40.0, 25.0));
    let rendered = bundle.render(&step);
    assert_eq!((rendered.width, rendered.height), (800, 450));
}

#[test]
fn offset_crop_rect_is_rejected() {
    let bundle = Bundle::new();
    let full = screenshot();
    let crop = imageops::crop_imm(&full, 40, 20, 80, 50).to_image();
    let blur = annotation(
        Shape::Blur(rect(40.0, 30.0, 50.0, 20.0)),
        blur_style(BlurMode::Pixelate, 5.0),
    );
    // Right size, wrong place: would move the blur 3 px off target.
    let mut step = bundle.step(&full, Some(&crop), Some(point(150.0, 90.0)), vec![blur]);
    step.image.crop_rect = Some(rect(43.0, 20.0, 80.0, 50.0));
    let rendered = bundle.render(&step);
    assert_eq!(
        rendered.warnings,
        [
            RenderWarning::CropRectMismatch,
            RenderWarning::CropOffsetUnknown
        ]
    );
    assert_eq!((rendered.width, rendered.height), (W, H));

    // With the click at the crop's centre, locate_crop recovers it.
    step.point = Some(point(80.0, 45.0));
    let rendered = bundle.render(&step);
    assert_eq!(rendered.warnings, [RenderWarning::CropRectMismatch]);
    assert_eq!((rendered.width, rendered.height), (80, 50));
    let correct = {
        let mut step = step.clone();
        step.image.crop_rect = Some(rect(40.0, 20.0, 80.0, 50.0));
        bundle.render(&step)
    };
    assert_eq!(correct.warnings, []);
    assert_eq!(decode(&rendered), decode(&correct));

    // A fractional or negative origin can't be verified either.
    for bad in [rect(40.5, 20.0, 80.0, 50.0), rect(-1.0, 20.0, 80.0, 50.0)] {
        step.image.crop_rect = Some(bad);
        assert_eq!(
            bundle.render(&step).warnings,
            [RenderWarning::CropRectMismatch]
        );
    }
}

// ---------------------------------------------------------------------------
// Redaction (privacy-critical)

/// Luminance variance of `region` in `image`.
fn variance(image: &RgbaImage, (x0, y0, w, h): (u32, u32, u32, u32)) -> f64 {
    let lum: Vec<f64> = (y0..y0 + h)
        .flat_map(|y| (x0..x0 + w).map(move |x| (x, y)))
        .map(|(x, y)| {
            let [r, g, b, _] = image.get_pixel(x, y).0;
            0.299 * f64::from(r) + 0.587 * f64::from(g) + 0.114 * f64::from(b)
        })
        .collect();
    let mean = lum.iter().sum::<f64>() / lum.len() as f64;
    lum.iter().map(|l| (l - mean).powi(2)).sum::<f64>() / lum.len() as f64
}

/// Redacts `blur` on a noise image and checks that no pixel of the
/// effective (grown) region keeps its original value, nothing outside it
/// changes, and the variance drops.
fn assert_redacted(mode: BlurMode, amount: Option<f64>, blur: Rect) {
    let (w, h) = (120, 80);
    let source = noise(w, h);
    let bundle = Bundle::new();
    let style = AnnotationStyle {
        blur_mode: Some(mode),
        blur_amount: amount,
        ..Default::default()
    };
    let step = bundle.step(
        &source,
        None,
        None,
        vec![annotation(Shape::Blur(blur), style)],
    );
    let out = decode(&bundle.render(&step));
    // Fractional edges cover every partly covered pixel.
    let x0 = blur.x.floor() as u32;
    let y0 = blur.y.floor() as u32;
    let covered = Region {
        x: x0,
        y: y0,
        width: (blur.x + blur.width).ceil() as u32 - x0,
        height: (blur.y + blur.height).ceil() as u32 - y0,
    };
    let g = effective_region(covered, w, h).unwrap();
    let inside =
        |x: u32, y: u32| (g.x..g.x + g.width).contains(&x) && (g.y..g.y + g.height).contains(&y);
    let label = format!("{mode:?} {amount:?} {blur:?}");

    let mut kept = 0;
    for (x, y, px) in out.enumerate_pixels() {
        let same = px == source.get_pixel(x, y);
        if inside(x, y) {
            kept += usize::from(same);
        } else {
            assert!(
                same,
                "{label}: pixel outside the region changed at ({x}, {y})"
            );
        }
    }
    assert_eq!(kept, 0, "{label}: original pixels kept");
    let region = (g.x, g.y, g.width, g.height);
    let (before, after) = (variance(&source, region), variance(&out, region));
    assert!(
        after < before * 0.25,
        "{label}: variance {before:.0} → {after:.0}"
    );
}

/// Region sizes that used to leave pixels: 1×1, thin and 1-px remainders.
const REDACT_SIZES: [(f64, f64); 5] = [
    (1.0, 1.0),
    (3.0, 3.0),
    (21.0, 21.0),
    (11.0, 31.0),
    (31.0, 11.0),
];

fn assert_redacted_all_sizes(mode: BlurMode) {
    for (w, h) in REDACT_SIZES {
        for (x, y) in [(13.0, 9.0), (0.0, 0.0), (120.0 - w, 80.0 - h)] {
            for amount in [None, Some(1.0), Some(6.0), Some(10.0)] {
                assert_redacted(mode, amount, rect(x, y, w, h));
            }
        }
    }
    // Sub-pixel and fractional rects.
    assert_redacted(mode, None, rect(40.3, 20.6, 0.2, 0.1));
    assert_redacted(mode, Some(6.0), rect(13.4, 9.5, 60.2, 36.0));
}

#[test]
fn blur_destroys_original_pixels() {
    assert_redacted_all_sizes(BlurMode::Blur);
}

#[test]
fn pixelate_destroys_original_pixels() {
    assert_redacted_all_sizes(BlurMode::Pixelate);
}

#[test]
fn redaction_is_burned_in_over_annotations() {
    let bundle = Bundle::new();
    let callout = annotation(
        Shape::Callout(CalloutGeometry {
            rect: rect(10.0, 10.0, 60.0, 30.0),
            anchor: None,
            text: "secret token".into(),
        }),
        Default::default(),
    );
    let pixelate = annotation(
        Shape::Blur(rect(10.0, 10.0, 60.0, 30.0)),
        blur_style(BlurMode::Pixelate, 6.0),
    );
    let step = bundle.step(&noise(80, 60), None, None, vec![callout, pixelate]);
    let out = decode(&bundle.render(&step));
    // Every 6×6 block (aligned to the region) is one colour, so no stroke of
    // the callout text drawn underneath survives.
    for by in (10..40).step_by(6) {
        for bx in (10..70).step_by(6) {
            let first = out.get_pixel(bx, by);
            for y in by..by + 6 {
                for x in bx..bx + 6 {
                    assert_eq!(
                        out.get_pixel(x, y),
                        first,
                        "block ({bx}, {by}) at ({x}, {y})"
                    );
                }
            }
        }
    }
}

#[test]
fn hotspot_markers_are_drawn_over_redaction() {
    let bundle = Bundle::new();
    let marker = annotation(
        Shape::Hotspot(HotspotGeometry {
            rect: rect(10.0, 10.0, 60.0, 40.0),
            title: "Panel".into(),
            description: String::new(),
        }),
        Default::default(),
    );
    let pixelate = annotation(
        Shape::Blur(rect(0.0, 0.0, 80.0, 60.0)),
        blur_style(BlurMode::Pixelate, 6.0),
    );
    // The blur comes after the hotspot, yet the marker stays on top, as in
    // the editor.
    let step = bundle.step(&noise(80, 60), None, None, vec![marker, pixelate]);
    let out = decode(&bundle.render(&step));
    let outline = out.get_pixel(40, 10);
    assert!(
        outline[2] > 200 && outline[0] < 60,
        "hotspot outline is blue: {outline:?}"
    );
}

#[test]
fn cover_draws_annotations_only_when_asked_and_always_redacts() {
    let bundle = Bundle::new();
    let full = screenshot();
    let crop = imageops::crop_imm(&full, 40, 30, 80, 40).to_image();
    let boxed = annotation(Shape::Box(rect(20.0, 20.0, 40.0, 30.0)), Default::default());
    let blur = annotation(
        Shape::Blur(rect(100.0, 60.0, 50.0, 30.0)),
        blur_style(BlurMode::Pixelate, 8.0),
    );
    let mut step = bundle.step(&full, Some(&crop), None, vec![boxed, blur]);
    step.image.crop_rect = Some(rect(40.0, 30.0, 80.0, 40.0));
    let render = |step: &Step, cover: CoverOptions| {
        render_cover_image(&bundle.handle, step, &cover, &ImageOptions::default())
    };

    let plain = render(&step, CoverOptions::default());
    assert_eq!((plain.width, plain.height), (W, H));
    let plain = decode(&plain);
    let drawn = decode(&render(
        &step,
        CoverOptions {
            annotations: true,
            ..CoverOptions::default()
        },
    ));
    // The box's stroke is only drawn when asked; the blur is burned into both.
    assert_eq!(plain.get_pixel(20, 35), full.get_pixel(20, 35));
    assert_ne!(drawn.get_pixel(20, 35), full.get_pixel(20, 35));
    for image in [&plain, &drawn] {
        assert_ne!(
            imageops::crop_imm(image, 100, 60, 50, 30).to_image(),
            imageops::crop_imm(&full, 100, 60, 50, 30).to_image()
        );
    }

    // The close-up, whatever the step itself shows.
    step.image.show = ImageShow::Full;
    let close_up = render(
        &step,
        CoverOptions {
            image: ImageShow::Crop,
            annotations: false,
        },
    );
    assert_eq!((close_up.width, close_up.height), (80, 40));
}

#[test]
fn unlocatable_crop_falls_back_to_redacted_full_view() {
    let bundle = Bundle::new();
    let full = noise(64, 40);
    // Not a crop of `full`, so its origin can't be verified.
    let crop = noise(32, 20);
    let blur = annotation(
        Shape::Blur(rect(0.0, 0.0, 64.0, 40.0)),
        blur_style(BlurMode::Pixelate, 8.0),
    );
    let step = bundle.step(&full, Some(&crop), Some(point(32.0, 20.0)), vec![blur]);
    let rendered = bundle.render(&step);
    assert_eq!(rendered.warnings, [RenderWarning::CropOffsetUnknown]);
    assert_eq!((rendered.width, rendered.height), (64, 40));
    let out = decode(&rendered);
    assert!(
        out.pixels()
            .zip(full.pixels())
            .filter(|(a, b)| a == b)
            .count()
            < 20
    );

    // Without annotations the crop is shown as is.
    let plain = bundle.step(&full, Some(&crop), None, Vec::new());
    let rendered = bundle.render(&plain);
    assert_eq!(rendered.warnings, []);
    assert_eq!(decode(&rendered), crop);
}

#[test]
fn crop_with_missing_full_screenshot_and_annotations_is_not_shown() {
    let bundle = Bundle::new();
    let full = screenshot();
    let crop = imageops::crop_imm(&full, 0, 0, 40, 20).to_image();
    let blur = annotation(Shape::Blur(rect(0.0, 0.0, 40.0, 20.0)), Default::default());
    let step = bundle.step(&full, Some(&crop), Some(point(1.0, 1.0)), vec![blur]);
    fs::remove_file(bundle.handle.resolve(&step.image.full).unwrap()).unwrap();
    let rendered = bundle.render(&step);
    assert!(matches!(
        &rendered.warnings[..],
        [RenderWarning::ImageUnavailable { path, .. }] if *path == step.image.full
    ));
    assert_eq!((rendered.width, rendered.height), (800, 450));
}

// ---------------------------------------------------------------------------
// Placeholders and scaling

#[test]
fn missing_image_renders_placeholder() {
    let bundle = Bundle::new();
    let mut step = bundle.step(&screenshot(), None, None, Vec::new());
    step.image.full = step_image_path(Ulid::new(), StepImageVariant::Full);
    let opts = ImageOptions {
        placeholder_size: (200, 100),
        ..Default::default()
    };
    let rendered = render_step_image(&bundle.handle, &step, &opts);
    assert_eq!((rendered.width, rendered.height), (200, 100));
    assert!(matches!(
        &rendered.warnings[..],
        [RenderWarning::ImageUnavailable { path, .. }] if *path == step.image.full
    ));
    assert_golden_image(
        "render/placeholder.golden.webp",
        &decode(&rendered),
        ImageTolerance::EXACT,
    );
}

#[test]
fn undecodable_image_renders_placeholder() {
    let bundle = Bundle::new();
    let step = bundle.step(&screenshot(), None, None, Vec::new());
    let path = bundle.handle.resolve(&step.image.full).unwrap();
    // A torn write: empty file, then garbage.
    for bytes in [&b""[..], b"RIFF\0\0\0\0WEBPjunk"] {
        fs::write(&path, bytes).unwrap();
        let rendered = bundle.render(&step);
        assert_eq!((rendered.width, rendered.height), (800, 450));
        assert_eq!(rendered.warnings.len(), 1, "{:?}", rendered.warnings);
        assert!(rendered.warnings[0].to_string().contains(&step.image.full));
        decode(&rendered);
    }
    // An escaping path is refused the same way.
    let mut escaping = step.clone();
    escaping.image.full = "../outside.webp".into();
    assert!(matches!(
        &bundle.render(&escaping).warnings[..],
        [RenderWarning::ImageUnavailable { .. }]
    ));
}

#[test]
fn missing_crop_falls_back_to_full() {
    let bundle = Bundle::new();
    let full = screenshot();
    let crop = imageops::crop_imm(&full, 0, 0, 40, 20).to_image();
    let step = bundle.step(&full, Some(&crop), None, Vec::new());
    fs::remove_file(
        bundle
            .handle
            .resolve(step.image.crop.as_ref().unwrap())
            .unwrap(),
    )
    .unwrap();
    let rendered = bundle.render(&step);
    assert_eq!((rendered.width, rendered.height), (W, H));
    assert_eq!(decode(&rendered), full);
    assert!(matches!(
        &rendered.warnings[..],
        [RenderWarning::ImageUnavailable { path, .. }] if Some(path) == step.image.crop.as_ref()
    ));
}

#[test]
fn scales_down_to_max_width_only() {
    let bundle = Bundle::new();
    let step = bundle.step(&screenshot(), None, None, Vec::new());
    let render = |max_width| {
        render_step_image(
            &bundle.handle,
            &step,
            &ImageOptions {
                max_width,
                ..Default::default()
            },
        )
    };
    let small = render(Some(80));
    assert_eq!((small.width, small.height), (80, 50));
    assert_eq!(decode(&small).dimensions(), (80, 50));
    let same = render(Some(1000));
    assert_eq!((same.width, same.height), (W, H));
    assert_eq!(decode(&same), screenshot());
}

#[test]
fn bad_annotations_warn_but_render() {
    let bundle = Bundle::new();
    let empty = annotation(Shape::Box(rect(10.0, 10.0, 0.0, 5.0)), Default::default());
    let crop_outside = annotation(
        Shape::Crop(rect(500.0, 500.0, 10.0, 10.0)),
        Default::default(),
    );
    let overflow = annotation(
        Shape::Callout(CalloutGeometry {
            rect: rect(10.0, 10.0, 40.0, 20.0),
            anchor: None,
            text: "far too much text for this small box".into(),
        }),
        Default::default(),
    );
    let ids = [empty.id, crop_outside.id, overflow.id];
    const HOSTILE: &str = r#""/><image href="file:///etc/passwd"/><x a=""#;
    let hostile = annotation(
        Shape::Arrow(ArrowGeometry {
            from: point(10.0, 10.0),
            to: point(60.0, 60.0),
        }),
        AnnotationStyle {
            color: Some(HOSTILE.into()),
            ..Default::default()
        },
    );
    let hostile_id = hostile.id;
    let step = bundle.step(
        &screenshot(),
        None,
        None,
        vec![empty, crop_outside, overflow, hostile],
    );
    let rendered = bundle.render(&step);
    assert_eq!((rendered.width, rendered.height), (W, H));
    assert_eq!(
        rendered.warnings,
        [
            RenderWarning::CropOutside { id: ids[1] },
            RenderWarning::AnnotationSkipped {
                id: ids[0],
                reason: "empty rectangle".into()
            },
            RenderWarning::CalloutOverflow { id: ids[2] },
            RenderWarning::InvalidColor {
                id: hostile_id,
                value: HOSTILE.into()
            },
        ]
    );
}

// ---------------------------------------------------------------------------
// Fixtures: render model, AST and every step image

fn fixture_guides() -> Vec<(GuideHandle, Guide)> {
    let library = Library::open(fixture_guides_dir()).unwrap();
    let mut guides: Vec<_> = library
        .list()
        .unwrap()
        .into_iter()
        .map(|summary| {
            let loaded = library.read(&summary.path).unwrap();
            (loaded.handle, loaded.guide)
        })
        .collect();
    guides.sort_by(|a, b| a.1.title.cmp(&b.1.title));
    assert!(!guides.is_empty());
    guides
}

fn dump_blocks(blocks: &[Block], indent: usize, out: &mut String) {
    for block in blocks {
        match block {
            Block::Paragraph(inlines) => {
                out.push_str(&format!("{:indent$}p {}\n", "", dump_inlines(inlines)));
            }
            Block::List { start, items } => {
                out.push_str(&format!("{:indent$}list start={start:?}\n", ""));
                for item in items {
                    out.push_str(&format!("{:indent$}  item\n", ""));
                    dump_blocks(item, indent + 4, out);
                }
            }
        }
    }
}

fn dump_inlines(inlines: &[Inline]) -> String {
    inlines
        .iter()
        .map(|inline| match inline {
            Inline::Text(text) => format!("{text:?}"),
            Inline::Code(code) => format!("code({code:?})"),
            Inline::Strong(children) => format!("strong[{}]", dump_inlines(children)),
            Inline::Emphasis(children) => format!("em[{}]", dump_inlines(children)),
            Inline::Link {
                url,
                title,
                children,
            } => format!("link({url:?} {title:?})[{}]", dump_inlines(children)),
            Inline::Break => "br".into(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn fixture_render_model_and_ast() {
    let mut out = String::new();
    for (handle, guide) in fixture_guides() {
        let render = RenderGuide::new(&guide, &handle);
        let numbers: Vec<u32> = render.steps().map(|s| s.number).collect();
        let expected: Vec<u32> = (1..=guide.steps().count() as u32).collect();
        assert_eq!(numbers, expected, "{}", guide.title);
        assert_eq!(render.items.len(), guide.items.len());

        out.push_str(&format!("# {}\nintro\n", render.title));
        dump_blocks(&render.intro, 2, &mut out);
        out.push_str("prerequisites\n");
        dump_blocks(&render.prerequisites, 2, &mut out);
        for item in &render.items {
            match item {
                RenderItem::Step(step) => {
                    out.push_str(&format!(
                        "step {}: {}\n",
                        step.number,
                        dump_inlines(&step.title)
                    ));
                    dump_blocks(&step.description, 2, &mut out);
                }
                RenderItem::Section(section) => {
                    out.push_str(&format!("section {:?}\n", section.title));
                }
                RenderItem::Note(note) => {
                    out.push_str(&format!("note {:?}\n", note.kind));
                    dump_blocks(&note.body, 2, &mut out);
                }
                RenderItem::Decision(decision) => {
                    out.push_str(&format!("decision: {}\n", dump_inlines(&decision.question)));
                    for option in &decision.options {
                        out.push_str(&format!("  {:?} -> {:?}\n", option.label, option.target));
                    }
                }
            }
        }
        out.push('\n');
    }
    assert_golden_text("render/fixtures_ast.golden", &out);
}

#[test]
fn every_fixture_step_renders() {
    let mut out = String::new();
    for (handle, guide) in fixture_guides() {
        let render = RenderGuide::new(&guide, &handle);
        for step in render.steps() {
            let image = render.step_image(step, &ImageOptions::default());
            decode(&image);
            let warnings: Vec<String> = image.warnings.iter().map(ToString::to_string).collect();
            out.push_str(&format!(
                "{} #{} {}x{} {warnings:?}\n",
                guide.title, step.number, image.width, image.height
            ));
        }
    }
    // The fixture crops are solid stand-ins, not real crops, so steps that
    // show a crop and carry annotations fall back to the full screenshot.
    assert_golden_text("render/fixture_steps.golden", &out);
}
