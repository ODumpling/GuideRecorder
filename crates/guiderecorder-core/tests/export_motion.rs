//! Motion exports (#8): animated GIF and MP4 video.
//!
//! Frames are checked by decoding the GIF: size, total duration (title
//! card, section cards and steps) and that a step's close-up differs from
//! its whole-screenshot frame. The MP4 is checked for its container boxes
//! (macOS and Windows; elsewhere it must report that it is unsupported).

mod support;

use std::fs;
use std::path::{Path, PathBuf};

use guiderecorder_core::export::motion::{self, gif, mp4, Aspect, MotionError, MotionOptions};
use guiderecorder_core::export::{RenderGuide, RenderWarning};
use guiderecorder_core::library::{Library, LoadedGuide};
use image::codecs::gif::GifDecoder;
use image::{AnimationDecoder, RgbaImage};
use support::{copy_dir, fixture_guides_dir};

struct Fixtures {
    dir: tempfile::TempDir,
    library: Library,
}

impl Fixtures {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("library");
        copy_dir(&fixture_guides_dir(), &root);
        Self {
            library: Library::open(&root).unwrap(),
            dir,
        }
    }

    fn guide(&self, name: &str) -> LoadedGuide {
        let path = self.library.root().join(format!("{name}.guiderecorder"));
        self.library.read(&path).unwrap()
    }

    fn out(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }
}

/// Decoded frames and their delays in milliseconds.
fn decode_gif(path: &Path) -> Vec<(RgbaImage, u32)> {
    let decoder = GifDecoder::new(std::io::BufReader::new(fs::File::open(path).unwrap())).unwrap();
    decoder
        .into_frames()
        .map(|f| {
            let f = f.unwrap();
            let (n, d) = f.delay().numer_denom_ms();
            (f.into_buffer(), n / d)
        })
        .collect()
}

fn small(opts: MotionOptions) -> MotionOptions {
    MotionOptions {
        size: Some(180),
        ..opts
    }
}

#[test]
fn gif_shows_title_sections_and_steps_for_their_time() {
    let fixtures = Fixtures::new();
    // 5 steps, 2 sections.
    let loaded = fixtures.guide("Share a calendar event");
    let render = RenderGuide::new(&loaded.guide, &loaded.handle);
    let out = fixtures.out("share.gif");
    let opts = small(MotionOptions {
        seconds_per_step: 2.0,
        ..MotionOptions::default()
    });
    let report = gif::export(&render, &out, &opts).unwrap();
    assert_eq!(report.files, std::slice::from_ref(&out));
    // The fixture's callouts overflow (as in every format); nothing else.
    assert!(
        report
            .warnings
            .iter()
            .all(|(_, w)| matches!(w, RenderWarning::CalloutOverflow { .. })),
        "{:?}",
        report.warnings
    );

    let frames = decode_gif(&out);
    assert!(frames.iter().all(|(f, _)| f.dimensions() == (320, 180)));
    let total: u32 = frames.iter().map(|(_, ms)| ms).sum();
    // Title card 2 s (capped by the step time), sections 1.5 s each.
    assert_eq!(total, 2000 + 2 * 1500 + 5 * 2000);
    // Holds are merged: far fewer frames than 10 a second.
    assert!(frames.len() < (total / 100) as usize);
}

#[test]
fn preview_has_a_still_per_scene() {
    let fixtures = Fixtures::new();
    // 5 steps, 2 sections.
    let loaded = fixtures.guide("Share a calendar event");
    let render = RenderGuide::new(&loaded.guide, &loaded.handle);
    for (aspect, captions, scenes) in [(Aspect::Wide, true, 8), (Aspect::Vertical, false, 5)] {
        let opts = MotionOptions {
            aspect,
            captions,
            ..MotionOptions::default()
        };
        let preview = motion::preview(&render, &opts, 180).unwrap();
        assert_eq!(preview.scene_count, scenes);
        assert_eq!(preview.frames.len(), scenes);
        assert!(preview.report.files.is_empty());
        for frame in &preview.frames {
            let image = image::load_from_memory(frame).unwrap();
            assert_eq!((image.width(), image.height()), aspect.size(180));
        }
    }
}

#[test]
fn without_captions_there_are_no_cards() {
    let fixtures = Fixtures::new();
    let loaded = fixtures.guide("Share a calendar event");
    let render = RenderGuide::new(&loaded.guide, &loaded.handle);
    let out = fixtures.out("plain.gif");
    let opts = small(MotionOptions {
        seconds_per_step: 1.0,
        captions: false,
        zoom: false,
        aspect: Aspect::Square,
        ..MotionOptions::default()
    });
    gif::export(&render, &out, &opts).unwrap();
    let frames = decode_gif(&out);
    assert!(frames.iter().all(|(f, _)| f.dimensions() == (180, 180)));
    let total: u32 = frames.iter().map(|(_, ms)| ms).sum();
    assert_eq!(total, 5 * 1000);
}

#[test]
fn zoom_moves_in_on_the_click() {
    let fixtures = Fixtures::new();
    let loaded = fixtures.guide("Rotate an API key");
    let render = RenderGuide::new(&loaded.guide, &loaded.handle);
    let frames_of = |zoom: bool, name: &str| {
        let out = fixtures.out(name);
        let opts = small(MotionOptions {
            seconds_per_step: 3.0,
            captions: false,
            zoom,
            aspect: Aspect::Vertical,
            ..MotionOptions::default()
        });
        gif::export(&render, &out, &opts).unwrap();
        decode_gif(&out)
    };
    let zoomed = frames_of(true, "zoom.gif");
    let flat = frames_of(false, "flat.gif");
    assert!(zoomed.iter().all(|(f, _)| f.dimensions() == (180, 320)));
    // Zooming adds in-between frames.
    assert!(
        zoomed.len() > flat.len() + 10,
        "{} vs {}",
        zoomed.len(),
        flat.len()
    );
    let total = |frames: &[(RgbaImage, u32)]| frames.iter().map(|(_, ms)| ms).sum::<u32>();
    assert_eq!(total(&zoomed), total(&flat));
}

#[test]
fn seconds_per_step_is_clamped() {
    let fixtures = Fixtures::new();
    let loaded = fixtures.guide("Kurzanleitung Drucker");
    let render = RenderGuide::new(&loaded.guide, &loaded.handle);
    let out = fixtures.out("short.gif");
    let opts = small(MotionOptions {
        seconds_per_step: 0.01,
        captions: false,
        ..MotionOptions::default()
    });
    gif::export(&render, &out, &opts).unwrap();
    let total: u32 = decode_gif(&out).iter().map(|(_, ms)| ms).sum();
    assert_eq!(total, (motion::MIN_SECONDS_PER_STEP * 1000.0) as u32);
}

#[test]
fn existing_files_are_kept_unless_overwriting() {
    let fixtures = Fixtures::new();
    let loaded = fixtures.guide("Kurzanleitung Drucker");
    let render = RenderGuide::new(&loaded.guide, &loaded.handle);
    let out = fixtures.out("exists.gif");
    fs::write(&out, b"keep").unwrap();
    let opts = small(MotionOptions::default());
    assert!(matches!(
        gif::export(&render, &out, &opts),
        Err(MotionError::Exists(p)) if p == out
    ));
    assert_eq!(fs::read(&out).unwrap(), b"keep");
    gif::export(
        &render,
        &out,
        &MotionOptions {
            overwrite: true,
            ..opts
        },
    )
    .unwrap();
    assert_eq!(&fs::read(&out).unwrap()[..6], b"GIF89a");
    // No temporary files are left behind.
    let names: Vec<_> = fs::read_dir(fixtures.dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert!(names.iter().all(|n| !n.to_string_lossy().contains(".tmp")));
}

#[test]
fn a_guide_without_steps_is_refused() {
    let fixtures = Fixtures::new();
    let mut loaded = fixtures.guide("Kurzanleitung Drucker");
    loaded
        .guide
        .items
        .retain(|i| !matches!(i, guiderecorder_core::model::Item::Step(_)));
    let render = RenderGuide::new(&loaded.guide, &loaded.handle);
    let out = fixtures.out("empty.gif");
    assert!(matches!(
        gif::export(&render, &out, &small(MotionOptions::default())),
        Err(MotionError::NoSteps)
    ));
    assert!(!out.exists());
}

#[test]
fn annotation_guides_export_without_a_click() {
    let fixtures = Fixtures::new();
    let loaded = fixtures.guide("Office floor plan");
    let render = RenderGuide::new(&loaded.guide, &loaded.handle);
    let out = fixtures.out("plan.gif");
    gif::export(&render, &out, &small(MotionOptions::default())).unwrap();
    assert!(!decode_gif(&out).is_empty());
}

#[cfg(any(target_os = "macos", windows))]
#[test]
fn mp4_is_an_mpeg4_movie() {
    let fixtures = Fixtures::new();
    let loaded = fixtures.guide("Rotate an API key");
    let render = RenderGuide::new(&loaded.guide, &loaded.handle);
    let out = fixtures.out("rotate.mp4");
    let opts = MotionOptions {
        seconds_per_step: 1.0,
        size: Some(360),
        ..MotionOptions::default()
    };
    let report = mp4::export(&render, &out, &opts).unwrap();
    assert_eq!(report.files, std::slice::from_ref(&out));
    let bytes = fs::read(&out).unwrap();
    assert_eq!(&bytes[4..8], b"ftyp");
    assert!(bytes.windows(4).any(|w| w == b"moov"));
    assert!(bytes.windows(4).any(|w| w == b"avc1"));
    assert!(matches!(
        mp4::export(&render, &out, &opts),
        Err(MotionError::Exists(_))
    ));
    mp4::export(
        &render,
        &out,
        &MotionOptions {
            overwrite: true,
            ..opts
        },
    )
    .unwrap();
}

#[cfg(not(any(target_os = "macos", windows)))]
#[test]
fn mp4_is_unsupported_elsewhere() {
    let fixtures = Fixtures::new();
    let loaded = fixtures.guide("Rotate an API key");
    let render = RenderGuide::new(&loaded.guide, &loaded.handle);
    assert!(matches!(
        mp4::export(&render, fixtures.out("x.mp4"), &MotionOptions::default()),
        Err(MotionError::Unsupported(_))
    ));
}
