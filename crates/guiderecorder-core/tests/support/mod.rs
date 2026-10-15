//! Shared helpers for integration tests. See design scratchpad #439 ("Testing").
//!
//! Golden files live next to the tests that use them, under `tests/golden/`.
//! Text goldens end in `.golden`; image goldens are lossless WebP and end in
//! `.golden.webp`. Run with `UPDATE_GOLDEN=1` to (re)write them from the
//! current output, then review the diff before committing.

// Each test crate uses a different subset of these helpers.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use guiderecorder_core::library::{read_webp, write_webp, PixelBuffer};
use image::RgbaImage;

/// Environment variable that makes the `assert_golden_*` helpers rewrite
/// golden files instead of comparing against them.
pub const UPDATE_ENV: &str = "UPDATE_GOLDEN";

/// `crates/guiderecorder-core`.
pub fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The repo's `fixtures/guides` folder.
pub fn fixture_guides_dir() -> PathBuf {
    crate_dir().join("../../fixtures/guides")
}

/// `tests/golden/<name>`.
pub fn golden_path(name: &str) -> PathBuf {
    crate_dir().join("tests/golden").join(name)
}

/// Whether `UPDATE_GOLDEN` is set to something other than empty or `0`.
pub fn update_requested() -> bool {
    std::env::var_os(UPDATE_ENV).is_some_and(|v| !v.is_empty() && v != "0")
}

/// Compares `actual` with `tests/golden/<name>`, or rewrites the file when
/// `UPDATE_GOLDEN` is set. Panics with a line diff on mismatch.
pub fn assert_golden_text(name: &str, actual: &str) {
    if let Err(message) = check_golden_text(&golden_path(name), actual, update_requested()) {
        panic!("{message}");
    }
}

/// How far an image may drift from its golden before it fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageTolerance {
    /// Largest allowed difference in any channel (alpha included) for a
    /// pixel to count as matching.
    pub channel: u8,
    /// How many pixels may still differ by more than `channel`, e.g. for
    /// antialiased text edges.
    pub max_pixels: usize,
}

impl ImageTolerance {
    /// Every pixel identical.
    pub const EXACT: Self = Self {
        channel: 0,
        max_pixels: 0,
    };

    /// Every pixel within `channel`, with no outliers allowed.
    pub const fn channel(channel: u8) -> Self {
        Self {
            channel,
            max_pixels: 0,
        }
    }
}

/// Compares `actual` with `tests/golden/<name>` (a `.golden.webp`) within
/// `tolerance`, or rewrites the file when `UPDATE_GOLDEN` is set. On
/// mismatch the actual image is written under the test target dir and the
/// panic names it.
pub fn assert_golden_image(name: &str, actual: &RgbaImage, tolerance: ImageTolerance) {
    let path = golden_path(name);
    if let Err(message) = check_golden_image(&path, actual, tolerance, update_requested()) {
        panic!("{message}");
    }
}

/// Text golden check. With `update`, writes `actual` to `path` (creating
/// parent folders) and succeeds.
pub fn check_golden_text(path: &Path, actual: &str, update: bool) -> Result<(), String> {
    if update {
        write_creating_dirs(path, actual.as_bytes());
        return Ok(());
    }
    let expected = match fs::read_to_string(path) {
        Ok(expected) => expected,
        Err(e) => return Err(missing(path, &e)),
    };
    if expected == actual {
        return Ok(());
    }
    Err(format!(
        "golden mismatch: {}\n{}\nRe-run with {UPDATE_ENV}=1 to accept the new output.",
        path.display(),
        line_diff(&expected, actual)
    ))
}

/// Image golden check. With `update`, writes `actual` to `path` as
/// lossless WebP and succeeds.
pub fn check_golden_image(
    path: &Path,
    actual: &RgbaImage,
    tolerance: ImageTolerance,
    update: bool,
) -> Result<(), String> {
    let (width, height) = actual.dimensions();
    let pixels = PixelBuffer::rgba(width, height, actual.as_raw());
    if update {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        write_webp(path, &pixels).unwrap();
        return Ok(());
    }
    if !path.exists() {
        return Err(missing(
            path,
            &std::io::Error::from(std::io::ErrorKind::NotFound),
        ));
    }
    let expected = read_webp(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let Some(diff) = image_diff(&expected, actual, tolerance) else {
        return Ok(());
    };
    let saved = failure_path(path);
    fs::create_dir_all(saved.parent().unwrap()).unwrap();
    write_webp(&saved, &pixels).unwrap();
    Err(format!(
        "golden image mismatch: {}\n{diff}\nActual image: {}\nRe-run with {UPDATE_ENV}=1 to accept it.",
        path.display(),
        saved.display()
    ))
}

/// Describes how `actual` differs from `expected`, or `None` if it is
/// within `tolerance`.
pub fn image_diff(
    expected: &RgbaImage,
    actual: &RgbaImage,
    tolerance: ImageTolerance,
) -> Option<String> {
    if expected.dimensions() != actual.dimensions() {
        return Some(format!(
            "size differs: expected {:?}, got {:?}",
            expected.dimensions(),
            actual.dimensions()
        ));
    }
    let mut differing = 0usize;
    let mut max_delta = 0u8;
    let mut first = None;
    for (x, y, want) in expected.enumerate_pixels() {
        let got = actual.get_pixel(x, y);
        let delta = want
            .0
            .iter()
            .zip(got.0)
            .map(|(a, b)| a.abs_diff(b))
            .max()
            .unwrap_or(0);
        if delta > tolerance.channel {
            differing += 1;
            max_delta = max_delta.max(delta);
            first.get_or_insert((x, y, want.0, got.0));
        }
    }
    let (x, y, want, got) = first?;
    if differing <= tolerance.max_pixels {
        return None;
    }
    let total = expected.width() as usize * expected.height() as usize;
    let ImageTolerance {
        channel,
        max_pixels,
    } = tolerance;
    Some(format!(
        "{differing} of {total} pixels differ by more than {channel} (max {max_delta}, \
         {max_pixels} allowed); first at ({x}, {y}): expected {want:?}, got {got:?}"
    ))
}

/// First differing line with a little context. Enough to find the change;
/// the full new output is one `UPDATE_GOLDEN=1` run away.
fn line_diff(expected: &str, actual: &str) -> String {
    const CONTEXT: usize = 2;
    let want: Vec<&str> = expected.lines().collect();
    let got: Vec<&str> = actual.lines().collect();
    let Some(at) = (0..want.len().max(got.len())).find(|&i| want.get(i) != got.get(i)) else {
        // Same lines, so only the trailing newline differs.
        return format!(
            "trailing newline differs: expected {:?}, got {:?}",
            expected.ends_with('\n'),
            actual.ends_with('\n')
        );
    };
    let mut out = format!("first difference at line {}:\n", at + 1);
    for line in &want[at.saturating_sub(CONTEXT)..at] {
        out += &format!("  {line}\n");
    }
    for (sign, lines) in [('-', &want), ('+', &got)] {
        for line in lines.iter().skip(at).take(CONTEXT + 1) {
            out += &format!("{sign} {line}\n");
        }
    }
    out += &format!("(expected {} lines, got {})", want.len(), got.len());
    out
}

fn missing(path: &Path, e: &std::io::Error) -> String {
    format!(
        "cannot read golden file {}: {e}\nRun with {UPDATE_ENV}=1 to create it.",
        path.display()
    )
}

/// Where the actual image for a failed golden goes: its path relative to
/// `tests/golden` (or, for goldens elsewhere, its whole path) mirrored under
/// `<target tmp>/golden-failures`, so goldens with the same file name in
/// different folders don't overwrite each other.
fn failure_path(golden: &Path) -> PathBuf {
    let relative: PathBuf = match golden.strip_prefix(golden_path("")) {
        Ok(relative) => relative.to_path_buf(),
        Err(_) => golden
            .components()
            .filter(|c| matches!(c, std::path::Component::Normal(_)))
            .collect(),
    };
    PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("golden-failures")
        .join(relative)
}

fn write_creating_dirs(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, bytes).unwrap();
}

/// Recursively copies a folder (files and subfolders only).
pub fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}
