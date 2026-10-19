//! On-device redaction (#10): sensitive text in a step's screenshot becomes
//! blur suggestions, or blurs, with no API key and no network.
//!
//! - Text recognition is platform code, passed in as a [`Recognizer`] (the
//!   capture crate's `ocr`, Vision on macOS). It gives [`TextLine`]s with
//!   word boxes in full-screenshot pixels.
//! - A [`Detector`] finds the sensitive spans of each line (see [`detect`]).
//! - [`finding_rect`] maps a span to the union of the boxes of the words it
//!   touches (whole words: recognition boxes words, not characters),
//!   padded and clamped to the image.
//! - [`redact_step`] adds them to a step as [`RedactionMode`] says: as
//!   [`RedactionSuggestion`]s from [`SuggestionSource::OnDevice`], for the
//!   editor's accept/reject list, or as blur annotations, which every
//!   export burns into the pixels. A region a blur or a suggestion already
//!   covers is skipped, so running it again adds nothing.
//! - [`redact_guide`] does that for every step of a guide, recognising each
//!   distinct screenshot once (steps often share a frame), a few at a time.
//! - [`burn_in_redactions`] blurs a step's blurs and on-device suggestions
//!   into an image before it leaves the device (the AI pass).
//!
//! Not done: following the same element from step to step. Each step's
//! screenshot is scanned on its own, which finds the same text again
//! wherever it is still on screen.

pub mod detect;

use std::collections::HashMap;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::thread;

use image::RgbaImage;
use serde::{Deserialize, Serialize};
use ulid::Ulid;

pub use detect::{check_pattern, check_word, Detector, Finding, Kind, RulesError};

use crate::export::blur;
use crate::export::raster::region;
use crate::library::GuideHandle;
use crate::model::{
    Annotation, AnnotationStyle, BlurMode, Guide, Item, Point, Rect, RedactionSuggestion, Shape,
    Step, SuggestionSource,
};

/// What happens to sensitive text found in a new recording.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RedactionMode {
    /// Nothing is scanned.
    Off,
    /// Blur suggestions to accept or reject in the editor.
    #[default]
    Suggest,
    /// Blur annotations straight away.
    Apply,
}

/// A line of recognised text, in full-screenshot pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct TextLine {
    pub text: String,
    pub rect: Rect,
    /// The boxes of the words of `text`, by byte range. May be empty: then
    /// a span's box is estimated from where it is in the line.
    pub words: Vec<TextWord>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextWord {
    /// Bytes of [`TextLine::text`], on character boundaries.
    pub range: Range<usize>,
    pub rect: Rect,
}

/// Recognises the text of a screenshot. Called from several threads at
/// once.
pub type Recognizer<'a> = dyn Fn(&RgbaImage) -> Result<Vec<TextLine>, String> + Sync + 'a;

/// Screenshots recognised at the same time by [`redact_guide`].
pub const MAX_WORKERS: usize = 3;

/// Padding around a found span, as a fraction of its height (clamped to
/// [`MIN_PAD`]–[`MAX_PAD`] pixels): recognition boxes are tight around the
/// glyphs, and a blur should also cover their antialiased edges.
const PAD: f64 = 0.2;
const MIN_PAD: f64 = 2.0;
const MAX_PAD: f64 = 12.0;
/// A region counts as already covered when this much of it is under one
/// existing blur or suggestion.
const COVERED: f64 = 0.9;

/// The region to blur for `range` of `line`, in a `size` screenshot.
/// `None` if nothing of it is on the image.
pub fn finding_rect(line: &TextLine, range: &Range<usize>, size: (u32, u32)) -> Option<Rect> {
    let touched = line
        .words
        .iter()
        .filter(|w| w.range.start < range.end && range.start < w.range.end)
        .map(|w| w.rect)
        .reduce(|a, b| union(&a, &b));
    padded(touched.unwrap_or_else(|| estimate(line, range)), size)
}

/// The part of the line's box `range` takes, by character count.
fn estimate(line: &TextLine, range: &Range<usize>) -> Rect {
    let chars = |s: &str| s.chars().count() as f64;
    let n = chars(&line.text).max(1.0);
    let start = chars(&line.text[..range.start]);
    let end = chars(&line.text[..range.end]);
    Rect {
        x: line.rect.x + line.rect.width * start / n,
        y: line.rect.y,
        width: line.rect.width * (end - start) / n,
        height: line.rect.height,
    }
}

fn union(a: &Rect, b: &Rect) -> Rect {
    let x = a.x.min(b.x);
    let y = a.y.min(b.y);
    Rect {
        x,
        y,
        width: (a.x + a.width).max(b.x + b.width) - x,
        height: (a.y + a.height).max(b.y + b.height) - y,
    }
}

/// `rect` grown by its padding, in whole pixels, clamped to the image.
fn padded(rect: Rect, (w, h): (u32, u32)) -> Option<Rect> {
    if ![rect.x, rect.y, rect.width, rect.height]
        .iter()
        .all(|v| v.is_finite())
    {
        return None;
    }
    let pad = (rect.height * PAD).clamp(MIN_PAD, MAX_PAD);
    let x0 = (rect.x - pad).floor().max(0.0);
    let y0 = (rect.y - pad).floor().max(0.0);
    let x1 = (rect.x + rect.width + pad).ceil().min(f64::from(w));
    let y1 = (rect.y + rect.height + pad).ceil().min(f64::from(h));
    (x1 > x0 && y1 > y0).then_some(Rect {
        x: x0,
        y: y0,
        width: x1 - x0,
        height: y1 - y0,
    })
}

/// Whether one of `others` covers most of `rect`.
fn covered(rect: &Rect, others: &[Rect]) -> bool {
    let area = rect.width * rect.height;
    others.iter().any(|o| {
        let w = (rect.x + rect.width).min(o.x + o.width) - rect.x.max(o.x);
        let h = (rect.y + rect.height).min(o.y + o.height) - rect.y.max(o.y);
        w > 0.0 && h > 0.0 && w * h >= COVERED * area
    })
}

/// Adds what `detector` finds in `lines` (the text of `step`'s `size`
/// screenshot) to `step`, as `mode` says. Returns how many were added.
pub fn redact_step(
    step: &mut Step,
    lines: &[TextLine],
    detector: &Detector,
    mode: RedactionMode,
    size: (u32, u32),
) -> usize {
    if mode == RedactionMode::Off {
        return 0;
    }
    let mut taken: Vec<Rect> = step
        .annotations
        .iter()
        .filter_map(|a| match &a.shape {
            Shape::Blur(rect) => Some(*rect),
            _ => None,
        })
        .chain(step.ai_state.redaction_suggestions.iter().map(|s| s.rect))
        .collect();
    let mut added = 0;
    for line in lines {
        for finding in detector.find(&line.text) {
            let Some(rect) = finding_rect(line, &finding.range, size) else {
                continue;
            };
            if covered(&rect, &taken) {
                continue;
            }
            taken.push(rect);
            if mode == RedactionMode::Apply {
                step.annotations.push(Annotation {
                    id: Ulid::new(),
                    shape: Shape::Blur(rect),
                    style: AnnotationStyle::default(),
                });
            } else {
                step.ai_state
                    .redaction_suggestions
                    .push(RedactionSuggestion {
                        rect,
                        reason: finding.kind.reason().to_owned(),
                        source: SuggestionSource::OnDevice,
                    });
            }
            added += 1;
        }
    }
    added
}

/// The result of [`redact_guide`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RedactSummary {
    /// Steps whose screenshot was scanned.
    pub steps: usize,
    /// Distinct screenshots recognised.
    pub images: usize,
    /// Suggestions or blurs added.
    pub found: usize,
    /// Steps that could not be scanned, with why.
    pub failures: Vec<(Ulid, String)>,
}

/// [`redact_step`] for every step of `guide` (in bundle `handle`), on its
/// full screenshot. Steps whose screenshot files are identical (copies of
/// one frame) share one recognition; up to [`MAX_WORKERS`] screenshots are
/// recognised at a time. A step whose image can't be read or recognised is
/// left as it was and reported. Nothing happens for [`RedactionMode::Off`].
pub fn redact_guide(
    handle: &GuideHandle,
    guide: &mut Guide,
    detector: &Detector,
    mode: RedactionMode,
    recognize: &Recognizer<'_>,
) -> RedactSummary {
    let mut summary = RedactSummary::default();
    if mode == RedactionMode::Off {
        return summary;
    }
    // Each distinct screenshot and the items showing it.
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    let mut by_content: HashMap<(u64, usize), usize> = HashMap::new();
    for (i, item) in guide.items.iter().enumerate() {
        let Item::Step(step) = item else {
            continue;
        };
        let bytes = handle
            .resolve(&step.image.full)
            .map_err(|e| e.to_string())
            .and_then(|path| fs::read(&path).map_err(|e| format!("{}: {e}", path.display())));
        match bytes {
            Ok(bytes) => {
                let mut hasher = DefaultHasher::new();
                bytes.hash(&mut hasher);
                let group = *by_content
                    .entry((hasher.finish(), bytes.len()))
                    .or_insert_with(|| {
                        groups.push((step.image.full.clone(), Vec::new()));
                        groups.len() - 1
                    });
                groups[group].1.push(i);
            }
            Err(e) => summary.failures.push((step.id, e)),
        }
    }

    type Recognized = Result<(Vec<TextLine>, (u32, u32)), String>;
    let results: Mutex<Vec<Option<Recognized>>> = Mutex::new(vec![None; groups.len()]);
    let next = AtomicUsize::new(0);
    thread::scope(|scope| {
        for _ in 0..groups.len().min(MAX_WORKERS) {
            scope.spawn(|| loop {
                let g = next.fetch_add(1, Ordering::Relaxed);
                let Some((path, _)) = groups.get(g) else {
                    break;
                };
                let result = handle
                    .read_image(path)
                    .map_err(|e| e.to_string())
                    .and_then(|image| {
                        let size = image.dimensions();
                        recognize(&image).map(|lines| (lines, size))
                    });
                results.lock().unwrap_or_else(|e| e.into_inner())[g] = Some(result);
            });
        }
    });

    let results = results.into_inner().unwrap_or_else(|e| e.into_inner());
    for ((_, items), result) in groups.iter().zip(results) {
        let result = result.expect("every group was recognised");
        if result.is_ok() {
            summary.images += 1;
        }
        for &i in items {
            let Item::Step(step) = &mut guide.items[i] else {
                continue;
            };
            match &result {
                Ok((lines, size)) => {
                    summary.steps += 1;
                    summary.found += redact_step(step, lines, detector, mode, *size);
                }
                Err(e) => summary.failures.push((step.id, e.clone())),
            }
        }
    }
    summary
}

/// Burns `step`'s blur annotations and on-device blur suggestions into
/// `image`, a view of its full screenshot whose top-left corner is at
/// `origin` (full-screenshot pixels), as the export burns blurs. For
/// images about to leave the device: text found here as sensitive is never
/// sent, reviewed or not.
pub fn burn_in_redactions(image: &mut RgbaImage, origin: Point, step: &Step) {
    let size = image.dimensions();
    let blurs = step.annotations.iter().filter_map(|a| match &a.shape {
        Shape::Blur(rect) => Some((
            *rect,
            a.style.blur_mode.unwrap_or(BlurMode::Blur),
            a.style.blur_amount,
        )),
        _ => None,
    });
    let suggested = step
        .ai_state
        .redaction_suggestions
        .iter()
        .filter(|s| s.source == SuggestionSource::OnDevice)
        .map(|s| (s.rect, BlurMode::Blur, None));
    for (rect, mode, amount) in blurs.chain(suggested) {
        if let Some(r) = region(&rect, origin, size) {
            blur::burn_in(image, size, r, mode, amount);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::{render_step_image, ImageOptions};
    use crate::library::{Library, PixelBuffer, StepImageVariant};
    use crate::model::{AiState, ImageShow, StepImage, StepKind};

    fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    /// A line whose words are boxed 10 px a character, 20 px high at y 100,
    /// with one space between words.
    fn line(text: &str) -> TextLine {
        let mut words = Vec::new();
        let mut start = None;
        for (i, c) in text.char_indices().chain([(text.len(), ' ')]) {
            match (c == ' ', start) {
                (true, Some(s)) => {
                    let col = text[..s].chars().count() as f64;
                    let len = text[s..i].chars().count() as f64;
                    words.push(TextWord {
                        range: s..i,
                        rect: rect(col * 10.0, 100.0, len * 10.0, 20.0),
                    });
                    start = None;
                }
                (false, None) => start = Some(i),
                _ => {}
            }
        }
        let n = text.chars().count() as f64;
        TextLine {
            text: text.to_owned(),
            rect: rect(0.0, 100.0, n * 10.0, 20.0),
            words,
        }
    }

    fn step() -> Step {
        Step {
            id: Ulid::new(),
            title: "Click Save".into(),
            description: String::new(),
            kind: StepKind::Click,
            point: None,
            end_point: None,
            element: None,
            keystrokes: None,
            image: StepImage {
                full: "steps/x.webp".into(),
                crop: None,
                crop_rect: None,
                after: None,
                show: ImageShow::Full,
            },
            annotations: Vec::new(),
            ai_state: AiState::default(),
        }
    }

    #[test]
    fn a_span_covers_the_words_it_touches_padded() {
        let l = line("Mail jane@example.com now");
        let range = 5..21;
        assert_eq!(&l.text[range.clone()], "jane@example.com");
        // Columns 5..21, padded by 4 (a fifth of 20).
        assert_eq!(
            finding_rect(&l, &range, (1000, 1000)),
            Some(rect(46.0, 96.0, 168.0, 28.0))
        );
        // Part of a word takes the whole word.
        assert_eq!(
            finding_rect(&l, &(7..9), (1000, 1000)),
            Some(rect(46.0, 96.0, 168.0, 28.0))
        );
        // Two words: their union.
        let l = line("call 555 0123 today");
        assert_eq!(
            finding_rect(&l, &(5..13), (1000, 1000)),
            Some(rect(46.0, 96.0, 88.0, 28.0))
        );
    }

    #[test]
    fn without_word_boxes_the_span_is_estimated_from_the_line() {
        let mut l = line("ab 1234");
        l.words.clear();
        assert_eq!(
            finding_rect(&l, &(3..7), (1000, 1000)),
            Some(rect(26.0, 96.0, 48.0, 28.0))
        );
        // By characters, not bytes.
        let mut l = line("üü 1234");
        l.words.clear();
        assert_eq!(
            finding_rect(&l, &(5..9), (1000, 1000)),
            Some(rect(26.0, 96.0, 48.0, 28.0))
        );
    }

    #[test]
    fn regions_are_clamped_to_the_image() {
        let l = line("jane@example.com");
        assert_eq!(
            finding_rect(&l, &(0..16), (100, 110)),
            Some(rect(0.0, 96.0, 100.0, 14.0))
        );
        assert_eq!(finding_rect(&l, &(0..16), (100, 50)), None);
        let mut l = line("x");
        l.words[0].rect.x = f64::NAN;
        assert_eq!(finding_rect(&l, &(0..1), (100, 200)), None);
    }

    #[test]
    fn suggest_adds_on_device_suggestions_once() {
        let detector = Detector::builtin();
        let lines = [
            line("Signed in as jane@example.com"),
            line("Card 4111 1111 1111 1111"),
            line("Nothing here"),
        ];
        let mut s = step();
        assert_eq!(
            redact_step(
                &mut s,
                &lines,
                &detector,
                RedactionMode::Suggest,
                (2000, 2000)
            ),
            2
        );
        let reasons: Vec<_> = s
            .ai_state
            .redaction_suggestions
            .iter()
            .map(|s| (s.reason.as_str(), s.source))
            .collect();
        assert_eq!(
            reasons,
            [
                ("email address", SuggestionSource::OnDevice),
                ("card number", SuggestionSource::OnDevice)
            ]
        );
        assert!(s.annotations.is_empty());
        // Again: everything is covered already.
        assert_eq!(
            redact_step(
                &mut s,
                &lines,
                &detector,
                RedactionMode::Suggest,
                (2000, 2000)
            ),
            0
        );
        assert_eq!(s.ai_state.redaction_suggestions.len(), 2);
    }

    #[test]
    fn apply_adds_blurs_and_skips_what_is_blurred() {
        let detector = Detector::builtin();
        let lines = [line("jane@example.com"), line("x"), line("bob@example.com")];
        let mut s = step();
        // The user already blurred the first address.
        let first = finding_rect(&lines[0], &(0..16), (2000, 2000)).unwrap();
        s.annotations.push(Annotation {
            id: Ulid::new(),
            shape: Shape::Blur(first),
            style: AnnotationStyle::default(),
        });
        let mut lines = lines.to_vec();
        lines[2].rect.y = 300.0;
        lines[2].words[0].rect.y = 300.0;
        assert_eq!(
            redact_step(
                &mut s,
                &lines,
                &detector,
                RedactionMode::Apply,
                (2000, 2000)
            ),
            1
        );
        assert_eq!(s.annotations.len(), 2);
        assert!(matches!(s.annotations[1].shape, Shape::Blur(r) if r.y == 296.0));
        assert!(s.ai_state.redaction_suggestions.is_empty());
        assert_eq!(
            redact_step(&mut s, &lines, &detector, RedactionMode::Off, (2000, 2000)),
            0
        );
    }

    #[test]
    fn user_rules_are_used() {
        let detector = Detector::new(&[r"INC-\d+".into()], &["Falcon".into()]).unwrap();
        let mut s = step();
        redact_step(
            &mut s,
            &[line("INC-42 for project falcon")],
            &detector,
            RedactionMode::Suggest,
            (2000, 2000),
        );
        let reasons: Vec<_> = s
            .ai_state
            .redaction_suggestions
            .iter()
            .map(|s| s.reason.as_str())
            .collect();
        assert_eq!(reasons, ["matches your pattern", "on your word list"]);
    }

    /// A library with one guide; `images[i]` is the full screenshot of step
    /// `i`, `None` for a step whose image is missing.
    fn bundle(images: &[Option<&RgbaImage>]) -> (tempfile::TempDir, GuideHandle, Guide) {
        let dir = tempfile::tempdir().unwrap();
        let library = Library::open(dir.path()).unwrap();
        let mut guide = Guide::new("Redact", "en");
        let handle = library.create(&guide).unwrap();
        for image in images {
            let mut s = step();
            match image {
                Some(image) => {
                    let pixels = PixelBuffer::rgba(image.width(), image.height(), image.as_raw());
                    s.image.full = handle
                        .write_step_image(s.id, StepImageVariant::Full, &pixels)
                        .unwrap();
                }
                None => s.image.full = format!("steps/{}.webp", s.id),
            }
            guide.items.push(Item::Step(s));
        }
        library.save_to(&handle, &guide).unwrap();
        (dir, handle, guide)
    }

    #[test]
    fn redact_guide_recognises_each_screenshot_once() {
        let a = RgbaImage::from_pixel(64, 48, image::Rgba([255, 255, 255, 255]));
        let b = RgbaImage::from_pixel(64, 48, image::Rgba([0, 0, 0, 255]));
        let (_dir, handle, mut guide) = bundle(&[Some(&a), Some(&a), Some(&b), None]);
        let calls = AtomicUsize::new(0);
        let recognize = |image: &RgbaImage| {
            calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(image.dimensions(), (64, 48));
            // White screenshots show an address, black ones fail.
            if image.get_pixel(0, 0).0[0] == 255 {
                let mut l = line("a@example.com");
                l.rect.y = 10.0;
                l.words[0].rect.y = 10.0;
                Ok(vec![l])
            } else {
                Err("no text".to_owned())
            }
        };
        let summary = redact_guide(
            &handle,
            &mut guide,
            &Detector::builtin(),
            RedactionMode::Suggest,
            &recognize,
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(summary.images, 1);
        assert_eq!(summary.steps, 2);
        assert_eq!(summary.found, 2);
        let steps: Vec<&Step> = guide.steps().collect();
        assert_eq!(
            summary.failures.iter().map(|f| f.0).collect::<Vec<_>>(),
            [steps[3].id, steps[2].id]
        );
        assert_eq!(summary.failures[1].1, "no text");
        for s in &steps[..2] {
            let found = &s.ai_state.redaction_suggestions;
            assert_eq!(found.len(), 1);
            // Clamped to the 64 × 48 screenshot.
            assert_eq!(found[0].rect, rect(0.0, 6.0, 64.0, 28.0));
        }
        assert!(steps[2].ai_state.redaction_suggestions.is_empty());

        // Off scans nothing.
        let summary = redact_guide(
            &handle,
            &mut guide,
            &Detector::builtin(),
            RedactionMode::Off,
            &recognize,
        );
        assert_eq!(summary, RedactSummary::default());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    /// A screenshot of vertical stripes, so a blur changes it.
    fn stripes(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, _| {
            let v = if x % 2 == 0 { 0 } else { 255 };
            image::Rgba([v, v, v, 255])
        })
    }

    fn changed(a: &RgbaImage, b: &RgbaImage, area: Rect) -> bool {
        (area.y as u32..(area.y + area.height) as u32).any(|y| {
            (area.x as u32..(area.x + area.width) as u32)
                .any(|x| a.get_pixel(x, y) != b.get_pixel(x, y))
        })
    }

    #[test]
    fn applied_blurs_are_burned_in_on_export() {
        let original = stripes(200, 200);
        let (_dir, handle, mut guide) = bundle(&[Some(&original)]);
        let token = concat!("ghp_", "aBcDeFgHiJkLmNoPqRsTuVwXyZ0123456789");
        let recognize = |_: &RgbaImage| Ok::<_, String>(vec![line(&format!("token {token}"))]);
        let summary = redact_guide(
            &handle,
            &mut guide,
            &Detector::builtin(),
            RedactionMode::Apply,
            &recognize,
        );
        assert_eq!(summary.found, 1);
        let step = guide.steps().next().unwrap();
        let Shape::Blur(blurred) = step.annotations[0].shape else {
            panic!("expected a blur");
        };
        let opts = ImageOptions {
            show: Some(ImageShow::Full),
            ..ImageOptions::default()
        };
        let rendered = render_step_image(&handle, step, &opts);
        assert!(rendered.warnings.is_empty(), "{:?}", rendered.warnings);
        let out = image::load_from_memory(&rendered.png).unwrap().to_rgba8();
        assert_eq!(out.dimensions(), (200, 200));
        assert!(changed(&original, &out, blurred));
        // Outside the blur the pixels are the screenshot's.
        assert!(!changed(&original, &out, rect(0.0, 0.0, 200.0, 90.0)));
    }

    #[test]
    fn on_device_suggestions_are_burned_in_only_when_asked() {
        let original = stripes(100, 100);
        let (_dir, handle, mut guide) = bundle(&[Some(&original)]);
        let Some(Item::Step(s)) = guide.items.first_mut() else {
            panic!("expected a step");
        };
        s.ai_state.redaction_suggestions = vec![
            RedactionSuggestion {
                rect: rect(10.0, 10.0, 20.0, 20.0),
                reason: "API key".into(),
                source: SuggestionSource::OnDevice,
            },
            RedactionSuggestion {
                rect: rect(60.0, 60.0, 20.0, 20.0),
                reason: "name".into(),
                source: SuggestionSource::Ai,
            },
        ];
        let render = |burn_on_device_suggestions| {
            let opts = ImageOptions {
                show: Some(ImageShow::Full),
                burn_on_device_suggestions,
                ..ImageOptions::default()
            };
            let rendered = render_step_image(&handle, s, &opts);
            assert!(rendered.warnings.is_empty(), "{:?}", rendered.warnings);
            image::load_from_memory(&rendered.png).unwrap().to_rgba8()
        };
        // The editor's export: suggestions are reviewed first.
        assert_eq!(render(false), original);
        // The MCP server and the CLI: no review, so the on-device ones go.
        let out = render(true);
        assert!(changed(&original, &out, rect(10.0, 10.0, 20.0, 20.0)));
        assert!(!changed(&original, &out, rect(60.0, 60.0, 20.0, 20.0)));
    }

    #[test]
    fn burn_in_covers_blurs_and_on_device_suggestions_only() {
        let original = stripes(100, 100);
        let mut s = step();
        s.annotations.push(Annotation {
            id: Ulid::new(),
            shape: Shape::Blur(rect(0.0, 0.0, 20.0, 20.0)),
            style: AnnotationStyle::default(),
        });
        s.ai_state.redaction_suggestions = vec![
            RedactionSuggestion {
                rect: rect(40.0, 40.0, 20.0, 20.0),
                reason: "email address".into(),
                source: SuggestionSource::OnDevice,
            },
            RedactionSuggestion {
                rect: rect(70.0, 70.0, 20.0, 20.0),
                reason: "name".into(),
                source: SuggestionSource::Ai,
            },
        ];
        let mut full = original.clone();
        burn_in_redactions(&mut full, Point { x: 0.0, y: 0.0 }, &s);
        assert!(changed(&original, &full, rect(0.0, 0.0, 20.0, 20.0)));
        assert!(changed(&original, &full, rect(40.0, 40.0, 20.0, 20.0)));
        assert!(!changed(&original, &full, rect(70.0, 70.0, 20.0, 20.0)));
        assert!(!changed(&original, &full, rect(20.0, 20.0, 20.0, 20.0)));

        // A crop whose top-left is at (40, 40): the suggestion is at its corner.
        let mut crop = stripes(30, 30);
        burn_in_redactions(&mut crop, Point { x: 40.0, y: 40.0 }, &s);
        assert!(changed(&stripes(30, 30), &crop, rect(0.0, 0.0, 20.0, 20.0)));
        assert!(!changed(
            &stripes(30, 30),
            &crop,
            rect(20.0, 20.0, 10.0, 10.0)
        ));
    }
}
