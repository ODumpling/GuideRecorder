//! On-device redaction of a new recording (#10; settings `redaction`).
//!
//! Before the editor opens a stopped recording, the text of every step's
//! screenshot is recognised with the capture crate's `ocr` (Vision: no API
//! key, nothing leaves the Mac), and what core's detectors find
//! (`guiderecorder_core::redact`) becomes blur suggestions or blurs, which
//! are saved with the guide. A failure here never loses the recording: the
//! guide stays as it was saved, the reason is logged, and the steps that
//! were not scanned are counted, so the user is told to check them.

use guiderecorder_capture::ocr;
use guiderecorder_core::library::{GuideHandle, Library};
use guiderecorder_core::model::Guide;
use guiderecorder_core::redact::{self, Detector, Recognizer, RedactionMode};
use image::RgbaImage;

use crate::settings::Redaction;

/// What [`redact_recording`] added.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Redacted {
    pub suggested: usize,
    pub blurred: usize,
    /// Steps whose screenshot could not be scanned (or whose findings
    /// could not be saved), so may still show sensitive text.
    pub unscanned: usize,
}

/// Scans `guide` (just saved in `handle`) as `settings` say and saves what
/// was found. Blocks while the screenshots are recognised.
pub fn redact_recording(
    library: &Library,
    handle: &GuideHandle,
    guide: &mut Guide,
    settings: &Redaction,
) -> Redacted {
    if settings.mode == RedactionMode::Off {
        return Redacted::default();
    }
    if !ocr::is_supported() {
        return Redacted {
            unscanned: guide.steps().count(),
            ..Redacted::default()
        };
    }
    let recognize = |image: &RgbaImage| {
        ocr::recognize_text(image.as_raw(), image.width(), image.height())
            .map_err(|e| e.to_string())
    };
    redact_with(library, handle, guide, settings, &recognize)
}

/// [`redact_recording`] with `recognize` for the text recognition.
fn redact_with(
    library: &Library,
    handle: &GuideHandle,
    guide: &mut Guide,
    settings: &Redaction,
    recognize: &Recognizer<'_>,
) -> Redacted {
    let detector = detector(settings);
    let mut scanned = guide.clone();
    let summary = redact::redact_guide(handle, &mut scanned, &detector, settings.mode, recognize);
    for (step, error) in &summary.failures {
        eprintln!("guiderecorder: redaction: step {step}: {error}");
    }
    let unscanned = summary.failures.len();
    // Nothing found and nothing scanned: no change to save. (A scan that
    // found nothing still marks its steps scanned.)
    if summary.found == 0 && summary.steps == 0 {
        return Redacted {
            unscanned,
            ..Redacted::default()
        };
    }
    if let Err(e) = library.save_to(handle, &scanned) {
        eprintln!(
            "guiderecorder: redaction: cannot save {}: {e}",
            handle.path.display()
        );
        // What was found is lost: as good as not scanned.
        return Redacted {
            unscanned: guide.steps().count(),
            ..Redacted::default()
        };
    }
    *guide = scanned;
    let (suggested, blurred) = match settings.mode {
        RedactionMode::Apply => (0, summary.found),
        _ => (summary.found, 0),
    };
    Redacted {
        suggested,
        blurred,
        unscanned,
    }
}

/// The detectors for `settings`; the built-in ones alone if the user's
/// rules can't be used (they are checked when saved, so only for a
/// settings file edited by hand).
fn detector(settings: &Redaction) -> Detector {
    Detector::new(&settings.patterns, &settings.words).unwrap_or_else(|e| {
        eprintln!("guiderecorder: redaction: ignoring your patterns and words: {e}");
        Detector::builtin()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use guiderecorder_core::library::{PixelBuffer, StepImageVariant};
    use guiderecorder_core::model::{Item, Rect, Step};
    use guiderecorder_core::redact::TextLine;
    use guiderecorder_core::Ulid;

    #[test]
    fn off_scans_nothing() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/guiderecorder-app-tests")
            .join(format!("redaction-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let library = Library::open(&dir).unwrap();
        let mut guide = Guide::new("Off", "en");
        let handle = library.create(&guide).unwrap();
        let settings = Redaction {
            mode: RedactionMode::Off,
            ..Redaction::default()
        };
        let before = guide.clone();
        assert_eq!(
            redact_recording(&library, &handle, &mut guide, &settings),
            Redacted::default()
        );
        assert_eq!(guide, before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A library with a guide of `steps` steps, each with a white 64 × 48
    /// screenshot except where `missing` (no image file).
    fn recorded(name: &str, missing: &[bool]) -> (std::path::PathBuf, Library, GuideHandle, Guide) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/guiderecorder-app-tests")
            .join(format!("redaction-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let library = Library::open(&dir).unwrap();
        let mut guide = Guide::new("Scan", "en");
        let handle = library.create(&guide).unwrap();
        let white = vec![255; 64 * 48 * 4];
        for &missing in missing {
            let mut step: Step = serde_json::from_value(serde_json::json!({
                "id": Ulid::new().to_string(),
                "kind": "click",
                "title": "Click Save",
                "image": { "full": "" },
            }))
            .unwrap();
            step.image.full = if missing {
                format!("steps/{}.webp", step.id)
            } else {
                handle
                    .write_step_image(
                        step.id,
                        StepImageVariant::Full,
                        &PixelBuffer::rgba(64, 48, &white),
                    )
                    .unwrap()
            };
            guide.items.push(Item::Step(step));
        }
        library.save_to(&handle, &guide).unwrap();
        (dir, library, handle, guide)
    }

    fn email(image: &RgbaImage) -> Result<Vec<TextLine>, String> {
        Ok(vec![TextLine {
            text: "jane@example.com".into(),
            rect: Rect {
                x: 0.0,
                y: 10.0,
                width: f64::from(image.width()),
                height: 20.0,
            },
            words: Vec::new(),
        }])
    }

    #[test]
    fn steps_that_cannot_be_scanned_are_counted() {
        let (dir, library, handle, mut guide) = recorded("unscanned", &[false, true, false]);
        let settings = Redaction::default();
        let failing = |_: &RgbaImage| Err::<Vec<TextLine>, _>("Vision failed".to_owned());
        assert_eq!(
            redact_with(&library, &handle, &mut guide, &settings, &failing),
            Redacted {
                unscanned: 3,
                ..Redacted::default()
            }
        );
        // One image is missing; the others are scanned and show an address.
        assert_eq!(
            redact_with(&library, &handle, &mut guide, &settings, &email),
            Redacted {
                suggested: 2,
                unscanned: 1,
                ..Redacted::default()
            }
        );
        let scanned: Vec<bool> = guide.steps().map(|s| s.ai_state.scanned).collect();
        assert_eq!(scanned, [true, false, true]);
        let saved = library.read(&handle.path).unwrap().guide;
        assert_eq!(saved.steps().filter(|s| s.ai_state.scanned).count(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn findings_that_cannot_be_saved_count_every_step_unscanned() {
        let (dir, library, handle, mut guide) = recorded("unsaved", &[false, false]);
        // A guide.json that is not one: the save refuses to overwrite it.
        std::fs::write(handle.guide_json(), "not json").unwrap();
        let before = guide.clone();
        let settings = Redaction {
            mode: RedactionMode::Apply,
            ..Redaction::default()
        };
        assert_eq!(
            redact_with(&library, &handle, &mut guide, &settings, &email),
            Redacted {
                unscanned: 2,
                ..Redacted::default()
            }
        );
        assert_eq!(guide, before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unusable_rules_fall_back_to_the_built_in_detectors() {
        let settings = Redaction {
            patterns: vec!["(".into()],
            ..Redaction::default()
        };
        let found = detector(&settings).find("mail jane@example.com");
        assert_eq!(found.len(), 1);
    }
}
