//! Motion exports: an MP4 video ([`mp4`]) or an animated GIF ([`gif`]) of a
//! guide, one step after the other, zooming in on each click.
//!
//! # Timeline
//!
//! With [`MotionOptions::captions`] on, the video opens on a title card
//! (the guide's title and summary) and each section gets a short card of
//! its own; notes are left out. Every step is shown for
//! [`MotionOptions::seconds_per_step`], and each scene cross-fades from the
//! last frame of the one before.
//!
//! Within a step, with [`MotionOptions::zoom`] on and a known click point,
//! the full screenshot eases into a close-up around the click (the
//! [`ZOOM`] factor, kept inside the image), holds, and eases back out by
//! the end of the step, so the next screenshot follows on from a full
//! view. A ripple marks the click once the close-up is reached (or soon
//! after the step starts, without zoom), and a cursor arrow points at it
//! throughout. A step without a known point, such as a page of an
//! annotation guide, is shown whole. Zoom shows the full screenshot; without
//! zoom each step shows its own choice of view (close-up or full).
//!
//! # Captions
//!
//! The step number and title sit in a band along the bottom of the frame,
//! in the bundled Lato (as annotations are drawn); the screenshot fills the
//! rest. The band has room for two lines, and a longer title is cut short
//! with an ellipsis.
//!
//! # Images
//!
//! Step images come from [`render_step_image`](super::render_step_image), so
//! annotations are drawn and blurs are burned in before any frame is made.
//! A missing or broken image becomes the renderer's placeholder and a
//! warning in the [`ExportReport`].
//!
//! # Frames
//!
//! [`render`] hands each distinct frame to its sink together with how many
//! frame intervals it stays on screen: a held close-up is one frame, not
//! dozens, which both encoders store as one longer frame. Output depends
//! only on the guide and the options.

pub mod gif;
pub mod mp4;

use std::io;
use std::path::PathBuf;

use resvg::tiny_skia::{
    BlendMode, Color, FillRule, FilterQuality, LineJoin, Mask, Paint, PathBuilder, Pixmap,
    PixmapPaint, Rect, Stroke, Transform,
};
use resvg::usvg;
use serde::{Deserialize, Serialize};

use super::annotate::{self, escape, num, FONT_FAMILY};
use super::ast::plain_text;
use super::md::Labels;
use super::raster::ImageOptions;
use super::render::{RenderGuide, RenderItem, RenderStep};
use super::report::ExportReport;
use crate::model::ImageShow;

/// Seconds per step: default and bounds ([`MotionOptions::seconds_per_step`]
/// is clamped to them).
pub const DEFAULT_SECONDS_PER_STEP: f64 = 3.0;
pub const MIN_SECONDS_PER_STEP: f64 = 1.0;
pub const MAX_SECONDS_PER_STEP: f64 = 30.0;

/// How far the close-up zooms in, relative to the whole screenshot fitted
/// to the frame.
pub const ZOOM: f64 = 2.0;

/// Longest a title card is shown, in seconds.
const TITLE_SECONDS: f64 = 2.5;
/// Longest a section card is shown, in seconds.
const SECTION_SECONDS: f64 = 1.5;
/// Longest cross-fade into a scene, in seconds, and its largest share of
/// the scene.
const FADE_SECONDS: f64 = 0.35;
const FADE_SHARE: f64 = 0.2;
/// Zoom in and out, as shares of the step's time.
const ZOOM_IN: (f64, f64) = (0.12, 0.40);
const ZOOM_OUT: (f64, f64) = (0.82, 1.0);
/// When the click ripple starts without zoom, as a share of the step.
const RIPPLE_START: f64 = 0.10;
const RIPPLE_SECONDS: f64 = 0.7;

const BACKGROUND: [u8; 3] = [0x18, 0x18, 0x1b];
const CAPTION_TEXT: &str = "#ffffff";
const SUBTITLE_TEXT: &str = "#a1a1aa";
const BADGE: &str = "#007aff";
const RIPPLE: [u8; 3] = [0xff, 0x3b, 0x30];

/// Frame shape. Serialized as `wide`, `square` or `vertical`, as the app
/// and the CLI name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Aspect {
    /// 16:9 landscape.
    #[default]
    Wide,
    /// 1:1.
    Square,
    /// 9:16 portrait.
    Vertical,
}

impl Aspect {
    /// Width and height of a frame whose short side is `short`, both even
    /// (as H.264 needs).
    pub fn size(self, short: u32) -> (u32, u32) {
        let short = even(short.max(2));
        let long = even(short * 16 / 9);
        match self {
            Self::Wide => (long, short),
            Self::Square => (short, short),
            Self::Vertical => (short, long),
        }
    }
}

fn even(v: u32) -> u32 {
    v & !1
}

/// Options for [`mp4::export`] and [`gif::export`].
#[derive(Debug, Clone, PartialEq)]
pub struct MotionOptions {
    /// Replace an existing file at the output path.
    pub overwrite: bool,
    /// How long each step is shown; clamped to [`MIN_SECONDS_PER_STEP`] to
    /// [`MAX_SECONDS_PER_STEP`].
    pub seconds_per_step: f64,
    pub aspect: Aspect,
    /// Zoom into the click of each step.
    pub zoom: bool,
    /// Title cards and a caption band with each step's number and title.
    pub captions: bool,
    /// Short side of the frame in pixels; `None` uses the format's default.
    pub size: Option<u32>,
    /// Fixed words; `None` uses [`Labels::for_language`] of the guide's
    /// language.
    pub labels: Option<Labels>,
    /// How step images are rendered. `show` (without zoom),
    /// `burn_on_device_suggestions` and `hotspot_markers` apply; size and
    /// point lookup are set here.
    pub image: ImageOptions,
}

impl Default for MotionOptions {
    fn default() -> Self {
        Self {
            overwrite: false,
            seconds_per_step: DEFAULT_SECONDS_PER_STEP,
            aspect: Aspect::default(),
            zoom: true,
            captions: true,
            size: None,
            labels: None,
            image: ImageOptions::default(),
        }
    }
}

/// Why a motion export failed.
#[derive(Debug, thiserror::Error)]
pub enum MotionError {
    #[error("{0} already exists")]
    Exists(PathBuf),
    #[error("the guide has no steps to show")]
    NoSteps,
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("encoding failed: {0}")]
    Encode(String),
    #[error("{0}")]
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, MotionError>;

/// Maps the [`io::ErrorKind::AlreadyExists`] of the persist helpers.
fn io_error(e: io::Error, out: &std::path::Path) -> MotionError {
    if e.kind() == io::ErrorKind::AlreadyExists {
        MotionError::Exists(out.to_path_buf())
    } else {
        MotionError::Io(e)
    }
}

/// Receives each distinct frame and how many frame intervals it stays on
/// screen.
pub(super) type Sink<'a> = dyn FnMut(&Pixmap, u32) -> Result<()> + 'a;

/// Renders the frames of `guide` at `fps` and `size` into `sink`. Image
/// warnings go to `report`. Returns the number of frame intervals.
pub(super) fn render(
    guide: &RenderGuide,
    opts: &MotionOptions,
    size: (u32, u32),
    fps: u32,
    report: &mut ExportReport,
    sink: &mut Sink<'_>,
) -> Result<u64> {
    let labels = opts
        .labels
        .clone()
        .unwrap_or_else(|| Labels::for_language(&guide.language));
    let scenes = scenes(guide, opts);
    if !scenes.iter().any(|s| matches!(s.kind, SceneKind::Step(_))) {
        return Err(MotionError::NoSteps);
    }
    let frame = FrameLayout::new(size, opts.captions);
    let mut out = Deduper::new(sink);
    let mut previous: Option<Pixmap> = None;
    for scene in &scenes {
        let frames = ((scene.seconds * f64::from(fps)).round() as u32).max(1);
        let fade = if previous.is_some() {
            (FADE_SECONDS.min(scene.seconds * FADE_SHARE) * f64::from(fps)).round() as u32
        } else {
            0
        };
        let mut painter: Box<dyn ScenePainter> = match scene.kind {
            SceneKind::Card { title, subtitle } => Box::new(Card::new(&frame, title, subtitle)?),
            SceneKind::Step(step) => {
                Box::new(Shot::new(guide, step, opts, &labels, &frame, report)?)
            }
        };
        let mut last: Option<(State, Pixmap)> = None;
        for i in 0..frames {
            let time = f64::from(i) / f64::from(fps);
            let mut state = painter.state(time / scene.seconds, time, scene.seconds);
            if i < fade {
                state.fade = Some(quantize(ease(f64::from(i) / f64::from(fade))));
            }
            if let Some((last_state, pixmap)) = &last {
                if *last_state == state {
                    out.push(pixmap)?;
                    continue;
                }
            }
            let mut pixmap = painter.paint(&state)?;
            if let (Some(t), Some(prev)) = (state.fade, &previous) {
                let paint = PixmapPaint {
                    opacity: (1.0 - t) as f32,
                    ..PixmapPaint::default()
                };
                pixmap.draw_pixmap(0, 0, prev.as_ref(), &paint, Transform::identity(), None);
            }
            out.push(&pixmap)?;
            last = Some((state, pixmap));
        }
        previous = last.map(|(_, p)| p);
    }
    out.finish()
}

/// Passes runs of identical frames on as one.
struct Deduper<'s, 'a> {
    sink: &'s mut Sink<'a>,
    pending: Option<(Pixmap, u32)>,
    total: u64,
}

impl<'s, 'a> Deduper<'s, 'a> {
    fn new(sink: &'s mut Sink<'a>) -> Self {
        Self {
            sink,
            pending: None,
            total: 0,
        }
    }

    fn push(&mut self, frame: &Pixmap) -> Result<()> {
        self.total += 1;
        if let Some((pending, count)) = &mut self.pending {
            if pending.data() == frame.data() {
                *count += 1;
                return Ok(());
            }
            (self.sink)(pending, *count)?;
        }
        self.pending = Some((frame.clone(), 1));
        Ok(())
    }

    fn finish(mut self) -> Result<u64> {
        if let Some((pending, count)) = self.pending.take() {
            (self.sink)(&pending, count)?;
        }
        Ok(self.total)
    }
}

// ---------------------------------------------------------------------------
// Scenes

struct Scene<'a> {
    kind: SceneKind<'a>,
    seconds: f64,
}

#[derive(Clone, Copy)]
enum SceneKind<'a> {
    Card { title: &'a str, subtitle: &'a str },
    Step(&'a RenderStep),
}

fn scenes<'a>(guide: &'a RenderGuide, opts: &MotionOptions) -> Vec<Scene<'a>> {
    let step_seconds = opts
        .seconds_per_step
        .clamp(MIN_SECONDS_PER_STEP, MAX_SECONDS_PER_STEP);
    let mut scenes = Vec::new();
    if opts.captions && !guide.title.trim().is_empty() {
        scenes.push(Scene {
            kind: SceneKind::Card {
                title: &guide.title,
                subtitle: &guide.summary,
            },
            seconds: step_seconds.min(TITLE_SECONDS),
        });
    }
    for item in &guide.items {
        match item {
            RenderItem::Step(step) => scenes.push(Scene {
                kind: SceneKind::Step(step),
                seconds: step_seconds,
            }),
            RenderItem::Section(section) if opts.captions && !section.title.trim().is_empty() => {
                scenes.push(Scene {
                    kind: SceneKind::Card {
                        title: &section.title,
                        subtitle: "",
                    },
                    seconds: step_seconds.min(SECTION_SECONDS),
                })
            }
            _ => {}
        }
    }
    scenes
}

/// What a frame of a scene shows; equal states give equal frames.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct State {
    /// Zoom progress, 0 (whole screenshot) to 1 (close-up).
    zoom: f64,
    /// Ripple progress, 0 to 1, while it plays.
    ripple: Option<f64>,
    /// Cross-fade progress, 0 (all previous scene) to 1, while it plays.
    fade: Option<f64>,
}

/// Rounds an animation value to 1/1000, so equal-looking frames compare
/// equal.
fn quantize(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// Cubic ease-in-out of `t` (clamped to 0..1).
fn ease(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

/// Zoom progress at `share` of a step's time.
fn zoom_progress(share: f64) -> f64 {
    let (in0, in1) = ZOOM_IN;
    let (out0, out1) = ZOOM_OUT;
    if share < in0 {
        0.0
    } else if share < in1 {
        ease((share - in0) / (in1 - in0))
    } else if share < out0 {
        1.0
    } else {
        1.0 - ease((share - out0) / (out1 - out0))
    }
}

trait ScenePainter {
    /// The state at `share` (0 to 1) of the scene, `time` seconds in, of
    /// `seconds` in all.
    fn state(&self, share: f64, time: f64, seconds: f64) -> State;
    fn paint(&mut self, state: &State) -> Result<Pixmap>;
}

/// Where things go in a frame.
struct FrameLayout {
    width: u32,
    height: u32,
    /// Font size of captions, in pixels.
    font: f64,
    /// Height of the caption band (0 without captions).
    band: u32,
}

impl FrameLayout {
    fn new((width, height): (u32, u32), captions: bool) -> Self {
        let short = f64::from(width.min(height));
        let font = (short * 0.042).max(8.0);
        let band = if captions {
            (font * (2.0 * 1.25 + 1.4)).round() as u32
        } else {
            0
        };
        Self {
            width,
            height,
            font,
            band: band.min(height / 3),
        }
    }

    fn blank(&self) -> Result<Pixmap> {
        let mut pixmap = Pixmap::new(self.width, self.height)
            .ok_or_else(|| MotionError::Encode("frame size out of range".into()))?;
        let [r, g, b] = BACKGROUND;
        pixmap.fill(Color::from_rgba8(r, g, b, 255));
        Ok(pixmap)
    }

    /// The area the screenshot is fitted into: `(x, y, width, height)`.
    fn image_area(&self) -> (f64, f64, f64, f64) {
        (
            0.0,
            0.0,
            f64::from(self.width),
            f64::from(self.height - self.band),
        )
    }
}

/// Draws `svg` (an SVG document of the frame's size) onto `pixmap`.
fn draw_svg(svg: &str, pixmap: &mut Pixmap) -> Result<()> {
    let tree = usvg::Tree::from_str(svg, &annotate::options())
        .map_err(|e| MotionError::Encode(format!("caption: {e}")))?;
    resvg::render(&tree, Transform::identity(), &mut pixmap.as_mut());
    Ok(())
}

/// `text` wrapped to `max_width`, at most `max_lines` lines, the last cut
/// short with an ellipsis if more would be needed.
fn wrap_lines(text: &str, font: f64, max_width: f64, max_lines: usize) -> Vec<String> {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut lines = annotate::wrap(&flat, font, max_width);
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        let last = lines.last_mut().expect("max_lines > 0");
        loop {
            let candidate = format!("{last}…");
            if annotate::measure(&candidate, font) <= max_width || last.is_empty() {
                *last = candidate;
                break;
            }
            last.pop();
            let trimmed = last.trim_end().len();
            last.truncate(trimmed);
        }
    }
    lines
}

/// SVG `<text>` lines centred on `cx`, the first baseline at `y`.
fn text_lines(
    lines: &[String],
    x: f64,
    y: f64,
    font: f64,
    anchor: &str,
    weight: &str,
    fill: &str,
) -> String {
    let mut svg = String::new();
    for (i, line) in lines.iter().enumerate() {
        svg.push_str(&format!(
            r#"<text x="{}" y="{}" text-anchor="{anchor}" font-family="{FONT_FAMILY}" font-weight="{weight}" font-size="{}" fill="{fill}">{}</text>"#,
            num(x),
            num(y + i as f64 * font * 1.25),
            num(font),
            escape(line),
        ));
    }
    svg
}

// ---------------------------------------------------------------------------
// Cards

/// A title or section card: centred text on the background.
struct Card {
    frame: Pixmap,
}

impl Card {
    fn new(layout: &FrameLayout, title: &str, subtitle: &str) -> Result<Self> {
        let (w, h) = (f64::from(layout.width), f64::from(layout.height));
        let max_width = w * 0.8;
        let title_font = layout.font * 1.6;
        let title_lines = wrap_lines(title, title_font, max_width, 3);
        let sub_lines = wrap_lines(subtitle, layout.font, max_width, 3);
        let gap = if sub_lines.is_empty() {
            0.0
        } else {
            layout.font
        };
        let height = title_lines.len() as f64 * title_font * 1.25
            + gap
            + sub_lines.len() as f64 * layout.font * 1.25;
        let top = (h - height) / 2.0;
        let mut svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{}" height="{}">"#,
            layout.width, layout.height
        );
        svg.push_str(&text_lines(
            &title_lines,
            w / 2.0,
            top + title_font,
            title_font,
            "middle",
            "bold",
            CAPTION_TEXT,
        ));
        svg.push_str(&text_lines(
            &sub_lines,
            w / 2.0,
            top + title_lines.len() as f64 * title_font * 1.25 + gap + layout.font,
            layout.font,
            "middle",
            "normal",
            SUBTITLE_TEXT,
        ));
        svg.push_str("</svg>");
        let mut frame = layout.blank()?;
        draw_svg(&svg, &mut frame)?;
        Ok(Self { frame })
    }
}

impl ScenePainter for Card {
    fn state(&self, _share: f64, _time: f64, _seconds: f64) -> State {
        State::default()
    }

    fn paint(&mut self, _state: &State) -> Result<Pixmap> {
        Ok(self.frame.clone())
    }
}

// ---------------------------------------------------------------------------
// Steps

/// A step: its screenshot, zoomed and marked, over its caption.
struct Shot {
    /// The frame without the screenshot: background and caption band.
    base: Pixmap,
    image: Pixmap,
    /// The click, in `image` pixels.
    point: Option<(f64, f64)>,
    zoom: bool,
    area: (f64, f64, f64, f64),
    /// Keeps a zoomed screenshot out of the caption band.
    clip: Option<Mask>,
    /// Cursor and ripple size, in frame pixels.
    marker: f64,
}

impl Shot {
    fn new(
        guide: &RenderGuide,
        step: &RenderStep,
        opts: &MotionOptions,
        labels: &Labels,
        layout: &FrameLayout,
        report: &mut ExportReport,
    ) -> Result<Self> {
        let area = layout.image_area();
        // Enough pixels for the close-up to stay sharp.
        let scale = if opts.zoom { ZOOM } else { 1.0 };
        let image_opts = ImageOptions {
            show: if opts.zoom {
                Some(ImageShow::Full)
            } else {
                opts.image.show
            },
            max_width: Some((area.2 * scale).ceil() as u32),
            locate_point: true,
            ..opts.image.clone()
        };
        let rendered = guide.step_image(step, &image_opts);
        report.add_warnings(step.step.id, rendered.warnings.iter().cloned());
        let image = Pixmap::decode_png(&rendered.png)
            .map_err(|e| MotionError::Encode(format!("step image: {e}")))?;
        let point = rendered.point.map(|p| {
            (
                p.x * f64::from(image.width()),
                p.y * f64::from(image.height()),
            )
        });

        let mut base = layout.blank()?;
        if layout.band > 0 {
            let mut title = plain_text(&step.title);
            if title.trim().is_empty() {
                title = format!("{} {}", labels.step_kind(guide.kind), step.number);
            }
            draw_svg(&caption_svg(layout, step.number, &title), &mut base)?;
        }
        let (ax, ay, aw, ah) = area;
        let clip = Rect::from_xywh(ax as f32, ay as f32, aw as f32, ah as f32).and_then(|r| {
            let mut mask = Mask::new(layout.width, layout.height)?;
            mask.fill_path(
                &PathBuilder::from_rect(r),
                FillRule::Winding,
                false,
                Transform::identity(),
            );
            Some(mask)
        });
        Ok(Self {
            base,
            image,
            point,
            zoom: opts.zoom,
            area,
            clip,
            marker: f64::from(layout.width.min(layout.height)) * 0.035,
        })
    }

    /// Scale and offset of the screenshot at zoom progress `k`, and the
    /// click in frame pixels.
    fn placement(&self, k: f64) -> (f64, f64, f64, Option<(f64, f64)>) {
        let (ax, ay, aw, ah) = self.area;
        let (iw, ih) = (
            f64::from(self.image.width()),
            f64::from(self.image.height()),
        );
        let fit = (aw / iw).min(ah / ih);
        let scale = fit * (1.0 + (ZOOM - 1.0) * k);
        let centre = (iw / 2.0, ih / 2.0);
        let focus = match self.point {
            Some((px, py)) => (
                centre.0 + (px - centre.0) * k,
                centre.1 + (py - centre.1) * k,
            ),
            None => centre,
        };
        let offset = |a: f64, len: f64, size: f64, f: f64| {
            let shown = size * scale;
            if shown <= len {
                a + (len - shown) / 2.0
            } else {
                (a + len / 2.0 - f * scale).clamp(a + len - shown, a)
            }
        };
        let tx = offset(ax, aw, iw, focus.0);
        let ty = offset(ay, ah, ih, focus.1);
        let point = self
            .point
            .map(|(px, py)| (tx + px * scale, ty + py * scale));
        (scale, tx, ty, point)
    }
}

impl ScenePainter for Shot {
    fn state(&self, share: f64, time: f64, seconds: f64) -> State {
        let zooms = self.zoom && self.point.is_some();
        let zoom = if zooms {
            quantize(zoom_progress(share))
        } else {
            0.0
        };
        let start = if zooms { ZOOM_IN.1 } else { RIPPLE_START } * seconds;
        let ripple = self
            .point
            .map(|_| (time - start) / RIPPLE_SECONDS)
            .filter(|p| (0.0..1.0).contains(p))
            .map(quantize);
        State {
            zoom,
            ripple,
            fade: None,
        }
    }

    fn paint(&mut self, state: &State) -> Result<Pixmap> {
        let mut frame = self.base.clone();
        let (scale, tx, ty, point) = self.placement(state.zoom);
        let paint = PixmapPaint {
            quality: FilterQuality::Bicubic,
            blend_mode: BlendMode::SourceOver,
            ..PixmapPaint::default()
        };
        frame.draw_pixmap(
            0,
            0,
            self.image.as_ref(),
            &paint,
            Transform::from_row(scale as f32, 0.0, 0.0, scale as f32, tx as f32, ty as f32),
            self.clip.as_ref(),
        );
        if let Some((x, y)) = point {
            if let Some(p) = state.ripple {
                draw_ripple(&mut frame, x, y, self.marker, p);
            }
            draw_cursor(&mut frame, x, y, self.marker);
        }
        Ok(frame)
    }
}

/// The caption band: a numbered badge and the title, at the bottom.
fn caption_svg(layout: &FrameLayout, number: u32, title: &str) -> String {
    let (w, h) = (f64::from(layout.width), f64::from(layout.height));
    let band = f64::from(layout.band);
    let font = layout.font;
    let top = h - band;
    let pad = font * 0.8;
    let radius = font * 0.8;
    let text_x = pad + radius * 2.0 + pad * 0.75;
    let lines = wrap_lines(title, font, w - text_x - pad, 2);
    let line_height = font * 1.25;
    let block = lines.len() as f64 * line_height;
    let first = top + (band - block) / 2.0 + font * 0.95;
    let cy = top + band / 2.0;
    let label = number.to_string();
    let badge_font = radius * if label.len() > 1 { 0.95 } else { 1.2 };
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{}" height="{}"><rect x="0" y="{}" width="{}" height="{}" fill="black" fill-opacity="0.35"/><circle cx="{}" cy="{}" r="{}" fill="{BADGE}"/><text x="{}" y="{}" text-anchor="middle" font-family="{FONT_FAMILY}" font-weight="bold" font-size="{}" fill="{CAPTION_TEXT}">{label}</text>{}</svg>"#,
        layout.width,
        layout.height,
        num(top),
        num(w),
        num(band),
        num(pad + radius),
        num(cy),
        num(radius),
        num(pad + radius),
        // Digits are about 0.72 em tall in Lato: centre them on the badge.
        num(cy + badge_font * 0.36),
        num(badge_font),
        text_lines(&lines, text_x, first, font, "start", "bold", CAPTION_TEXT),
    )
}

/// An expanding, fading ring around the click, `p` (0 to 1) of the way
/// through.
fn draw_ripple(frame: &mut Pixmap, x: f64, y: f64, size: f64, p: f64) {
    let radius = size * (0.4 + 1.2 * ease(p));
    let alpha = (1.0 - p).clamp(0.0, 1.0);
    let [r, g, b] = RIPPLE;
    let Some(circle) = PathBuilder::from_circle(x as f32, y as f32, radius as f32) else {
        return;
    };
    let mut paint = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    paint.set_color_rgba8(r, g, b, (alpha * 0.25 * 255.0) as u8);
    frame.fill_path(
        &circle,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
    paint.set_color_rgba8(r, g, b, (alpha * 255.0) as u8);
    let stroke = Stroke {
        width: (size * 0.12) as f32,
        ..Stroke::default()
    };
    frame.stroke_path(&circle, &paint, &stroke, Transform::identity(), None);
}

/// A white arrow cursor with a dark outline, its tip at `(x, y)`.
fn draw_cursor(frame: &mut Pixmap, x: f64, y: f64, size: f64) {
    // The classic arrow, 1 unit tall, tip at the origin.
    const ARROW: [(f64, f64); 7] = [
        (0.0, 0.0),
        (0.0, 0.85),
        (0.2, 0.67),
        (0.34, 0.98),
        (0.46, 0.93),
        (0.33, 0.62),
        (0.58, 0.62),
    ];
    let mut builder = PathBuilder::new();
    for (i, (ux, uy)) in ARROW.iter().enumerate() {
        let (px, py) = ((x + ux * size) as f32, (y + uy * size) as f32);
        if i == 0 {
            builder.move_to(px, py);
        } else {
            builder.line_to(px, py);
        }
    }
    builder.close();
    let Some(path) = builder.finish() else {
        return;
    };
    let mut paint = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    paint.set_color_rgba8(255, 255, 255, 255);
    frame.fill_path(
        &path,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
    paint.set_color_rgba8(0, 0, 0, 255);
    let stroke = Stroke {
        width: (size * 0.07).max(1.0) as f32,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    frame.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_even_and_shaped() {
        assert_eq!(Aspect::Wide.size(1080), (1920, 1080));
        assert_eq!(Aspect::Square.size(1080), (1080, 1080));
        assert_eq!(Aspect::Vertical.size(1080), (1080, 1920));
        assert_eq!(Aspect::Wide.size(541), (960, 540));
    }

    #[test]
    fn zoom_eases_in_holds_and_out() {
        assert_eq!(zoom_progress(0.0), 0.0);
        assert_eq!(zoom_progress(0.5), 1.0);
        assert!(zoom_progress(0.26) > 0.0 && zoom_progress(0.26) < 1.0);
        assert!(zoom_progress(0.999) < 0.01);
        assert_eq!(ease(0.5), 0.5);
    }

    #[test]
    fn long_titles_are_cut_with_an_ellipsis() {
        let lines = wrap_lines(&"word ".repeat(200), 20.0, 300.0, 2);
        assert_eq!(lines.len(), 2);
        assert!(lines[1].ends_with('…'));
        assert!(annotate::measure(&lines[1], 20.0) <= 300.0);
        assert!(wrap_lines("", 20.0, 300.0, 2).is_empty());
    }
}
