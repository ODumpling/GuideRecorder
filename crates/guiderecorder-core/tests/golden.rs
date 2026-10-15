//! Tests for the golden-file helpers in `support/`.

mod support;

use image::{Rgba, RgbaImage};
use support::{check_golden_image, check_golden_text, image_diff, ImageTolerance};

#[test]
fn text_golden_compares_and_updates() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested/out.golden");

    let missing = check_golden_text(&path, "a\n", false).unwrap_err();
    assert!(missing.contains("UPDATE_GOLDEN=1"), "{missing}");

    check_golden_text(&path, "one\ntwo\nthree\n", true).unwrap();
    check_golden_text(&path, "one\ntwo\nthree\n", false).unwrap();

    let diff = check_golden_text(&path, "one\nTWO\nthree\n", false).unwrap_err();
    assert!(diff.contains("line 2"), "{diff}");
    assert!(diff.contains("- two") && diff.contains("+ TWO"), "{diff}");

    let newline = check_golden_text(&path, "one\ntwo\nthree", false).unwrap_err();
    assert!(newline.contains("trailing newline"), "{newline}");
}

#[test]
fn image_golden_compares_with_tolerance() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shape.golden.webp");
    let base = RgbaImage::from_pixel(8, 4, Rgba([10, 20, 30, 255]));

    check_golden_image(&path, &base, ImageTolerance::EXACT, true).unwrap();
    check_golden_image(&path, &base, ImageTolerance::EXACT, false).unwrap();

    let mut nudged = base.clone();
    nudged.put_pixel(3, 1, Rgba([12, 20, 30, 255]));
    check_golden_image(&path, &nudged, ImageTolerance::channel(2), false).unwrap();
    let err = check_golden_image(&path, &nudged, ImageTolerance::channel(1), false).unwrap_err();
    assert!(
        err.contains("1 of 32 pixels") && err.contains("(3, 1)"),
        "{err}"
    );

    let resized = RgbaImage::from_pixel(4, 4, Rgba([10, 20, 30, 255]));
    let diff = image_diff(&base, &resized, ImageTolerance::channel(255)).unwrap();
    assert!(diff.contains("size differs"), "{diff}");
}

#[test]
fn image_tolerance_allows_a_pixel_budget() {
    let base = RgbaImage::from_pixel(8, 4, Rgba([0, 0, 0, 255]));
    let mut speckled = base.clone();
    speckled.put_pixel(0, 0, Rgba([255, 0, 0, 255]));
    speckled.put_pixel(7, 3, Rgba([0, 0, 0, 0]));

    let two = ImageTolerance {
        channel: 0,
        max_pixels: 2,
    };
    assert_eq!(image_diff(&base, &speckled, two), None);
    let one = ImageTolerance {
        channel: 0,
        max_pixels: 1,
    };
    let diff = image_diff(&base, &speckled, one).unwrap();
    assert!(
        diff.contains("2 of 32 pixels") && diff.contains("1 allowed"),
        "{diff}"
    );
}

#[test]
fn failed_images_keep_their_golden_subfolder() {
    let actual = RgbaImage::from_pixel(2, 2, Rgba([1, 2, 3, 255]));
    let expected = RgbaImage::from_pixel(2, 2, Rgba([9, 9, 9, 255]));
    let temp = tempfile::tempdir().unwrap();
    let mut saved = Vec::new();
    for folder in ["golden-helper-a", "golden-helper-b"] {
        let golden = temp.path().join(folder).join("same.golden.webp");
        check_golden_image(&golden, &expected, ImageTolerance::EXACT, true).unwrap();
        let err = check_golden_image(&golden, &actual, ImageTolerance::EXACT, false).unwrap_err();
        let line = err
            .lines()
            .find(|l| l.starts_with("Actual image: "))
            .unwrap();
        saved.push(line.trim_start_matches("Actual image: ").to_owned());
    }
    assert_ne!(saved[0], saved[1]);
    for (path, folder) in saved.iter().zip(["golden-helper-a", "golden-helper-b"]) {
        assert!(path.contains(folder), "{path}");
        assert!(std::path::Path::new(path).is_file(), "{path}");
    }
}
