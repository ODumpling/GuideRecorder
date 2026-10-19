//! Support for syncing guides with the web app: the copies of a guide's
//! images that leave the device, and their content hashes.
//!
//! The server stores images by SHA-256. Each image path the guide names is
//! uploaded with its step's blurs burned in ([`redacted_image`]); a step
//! with nothing to blur uploads its files byte for byte.

use std::fs;

use image::codecs::webp::WebPEncoder;
use image::ExtendedColorType;
use sha2::{Digest, Sha256};

use crate::export::raster::{has_redactions, redacted_image};
use crate::library::{GuideHandle, LibraryError, Result};
use crate::model::{Guide, Step};

/// An image as it will be uploaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadImage {
    /// Bundle-relative path, as in `guide.json`.
    pub path: String,
    /// Lowercase hex SHA-256 of `bytes`.
    pub sha256: String,
    pub bytes: Vec<u8>,
}

/// Every image path the guide names, with the step it belongs to, in item
/// order without duplicates. The same list as `imagePaths` in
/// `packages/sync`.
pub fn image_paths(guide: &Guide) -> Vec<(&Step, &str)> {
    let mut seen = std::collections::HashSet::new();
    guide
        .steps()
        .flat_map(|step| {
            [
                Some(step.image.full.as_str()),
                step.image.crop.as_deref(),
                step.image.after.as_deref(),
            ]
            .into_iter()
            .flatten()
            .filter(|p| !p.is_empty())
            .map(move |p| (step, p))
        })
        .filter(|(_, p)| seen.insert(*p))
        .collect()
}

/// The upload copy of the image at `path` of `step`.
pub fn upload_image(handle: &GuideHandle, step: &Step, path: &str) -> Result<UploadImage> {
    let bytes = if has_redactions(step) {
        let image = redacted_image(handle, step, path)?;
        let mut out = Vec::new();
        WebPEncoder::new_lossless(&mut out)
            .encode(
                image.as_raw(),
                image.width(),
                image.height(),
                ExtendedColorType::Rgba8,
            )
            .map_err(|source| LibraryError::Image {
                path: handle.path.join(path),
                source,
            })?;
        out
    } else {
        let file = handle.resolve(path)?;
        fs::read(&file).map_err(|source| LibraryError::Io { path: file, source })?
    };
    Ok(UploadImage {
        path: path.to_owned(),
        sha256: sha256_hex(&bytes),
        bytes,
    })
}

/// Lowercase hex SHA-256.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::Library;
    use std::path::Path;

    fn fixture(name: &str) -> (tempfile::TempDir, GuideHandle, Guide) {
        let dir = tempfile::tempdir().unwrap();
        let src = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/guides")
            .join(name);
        let dst = dir.path().join(name);
        copy_dir(&src, &dst);
        let library = Library::open(dir.path()).unwrap();
        let loaded = library.read(&dst).unwrap();
        (dir, loaded.handle, loaded.guide)
    }

    fn copy_dir(src: &Path, dst: &Path) {
        fs::create_dir_all(dst).unwrap();
        for entry in fs::read_dir(src).unwrap() {
            let entry = entry.unwrap();
            let to = dst.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_dir(&entry.path(), &to);
            } else {
                fs::copy(entry.path(), to).unwrap();
            }
        }
    }

    #[test]
    fn lists_every_image_once() {
        let (_dir, _handle, guide) = fixture("Rotate an API key.guiderecorder");
        let paths: Vec<_> = image_paths(&guide).into_iter().map(|(_, p)| p).collect();
        assert!(paths.contains(&"steps/01M4WZCE2000000000000002R8.after.webp"));
        let unique: std::collections::HashSet<_> = paths.iter().collect();
        assert_eq!(unique.len(), paths.len());
    }

    #[test]
    fn uploads_unblurred_steps_as_stored() {
        let (_dir, handle, guide) = fixture("Rotate an API key.guiderecorder");
        for (step, path) in image_paths(&guide)
            .into_iter()
            .filter(|(s, _)| !has_redactions(s))
        {
            let upload = upload_image(&handle, step, path).unwrap();
            assert_eq!(
                upload.bytes,
                fs::read(handle.resolve(path).unwrap()).unwrap()
            );
            assert_eq!(upload.sha256, sha256_hex(&upload.bytes));
        }
    }

    #[test]
    fn burns_blurs_into_the_upload_only() {
        let (_dir, handle, guide) = fixture("Rotate an API key.guiderecorder");
        let step = guide.steps().find(|s| has_redactions(s)).unwrap();
        // A checkerboard, so a blur shows. The fixture blurs x 4..28, y 8..14.
        let board = image::RgbaImage::from_fn(64, 32, |x, y| {
            let v = if (x + y) % 2 == 0 { 255 } else { 0 };
            image::Rgba([v, v, v, 255])
        });
        let path = handle.resolve(&step.image.full).unwrap();
        board
            .save_with_format(&path, image::ImageFormat::WebP)
            .unwrap();

        let upload = upload_image(&handle, step, &step.image.full).unwrap();
        let blurred = image::load_from_memory(&upload.bytes).unwrap().into_rgba8();
        assert_eq!(blurred.dimensions(), board.dimensions());
        assert_ne!(blurred.get_pixel(10, 10), board.get_pixel(10, 10));
        assert_eq!(blurred.get_pixel(50, 2), board.get_pixel(50, 2));
        // The bundle keeps the original.
        assert_eq!(handle.read_image(&step.image.full).unwrap(), board);
    }

    #[test]
    fn hashes_are_stable() {
        let (_dir, handle, guide) = fixture("Rotate an API key.guiderecorder");
        let (step, path) = image_paths(&guide)
            .into_iter()
            .find(|(s, _)| has_redactions(s))
            .unwrap();
        assert_eq!(
            upload_image(&handle, step, path).unwrap().sha256,
            upload_image(&handle, step, path).unwrap().sha256
        );
    }
}
