//! On-device redaction of a new recording (#10; settings `redaction`).
//!
//! Before the editor opens a stopped recording, the text of every step's
//! screenshot is recognised with the capture crate's `ocr` (Vision: no API
//! key, nothing leaves the Mac), and what core's detectors find
//! (`guiderecorder_core::redact`) becomes blur suggestions or blurs, which
//! are saved with the guide. A failure here never loses the recording: the
//! guide stays as it was saved, and the reason is logged.

use guiderecorder_capture::ocr;
use guiderecorder_core::library::{GuideHandle, Library};
use guiderecorder_core::model::Guide;
use guiderecorder_core::redact::{self, Detector, RedactionMode};
use image::RgbaImage;

use crate::settings::Redaction;

/// What [`redact_recording`] added.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Redacted {
    pub suggested: usize,
    pub blurred: usize,
}

/// Scans `guide` (just saved in `handle`) as `settings` say and saves what
/// was found. Blocks while the screenshots are recognised.
pub fn redact_recording(
    library: &Library,
    handle: &GuideHandle,
    guide: &mut Guide,
    settings: &Redaction,
) -> Redacted {
    if settings.mode == RedactionMode::Off || !ocr::is_supported() {
        return Redacted::default();
    }
    let detector = detector(settings);
    let recognize = |image: &RgbaImage| {
        ocr::recognize_text(image.as_raw(), image.width(), image.height())
            .map_err(|e| e.to_string())
    };
    let mut scanned = guide.clone();
    let summary = redact::redact_guide(handle, &mut scanned, &detector, settings.mode, &recognize);
    for (step, error) in &summary.failures {
        eprintln!("guiderecorder: redaction: step {step}: {error}");
    }
    if summary.found == 0 {
        return Redacted::default();
    }
    if let Err(e) = library.save_to(handle, &scanned) {
        eprintln!(
            "guiderecorder: redaction: cannot save {}: {e}",
            handle.path.display()
        );
        return Redacted::default();
    }
    *guide = scanned;
    match settings.mode {
        RedactionMode::Apply => Redacted {
            blurred: summary.found,
            ..Redacted::default()
        },
        _ => Redacted {
            suggested: summary.found,
            ..Redacted::default()
        },
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
