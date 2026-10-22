//! Pages of annotated guides ([`GuideKind::Annotated`]): images the user
//! chose, each a manual step showing its full image, on which the editor
//! draws hotspots. The app's "New annotation guide…" and the MCP server's
//! `create_guide_from_images` both build guides with
//! [`create_annotated_guide`]; the app also turns PDF pages into images
//! (macOS only, in the app), which [`create_from_images`] does not.

use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use image::imageops::{self, FilterType};
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits, RgbaImage};
use ulid::Ulid;

use crate::library::{GuideHandle, Library, LibraryError, PixelBuffer, StepImageVariant};
use crate::model::{AiState, Guide, GuideKind, ImageShow, Item, Step, StepImage, StepKind};

/// Image files that can become pages, by extension.
pub const PAGE_IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp"];

/// Image formats a page may be in, told by the file's content.
pub const PAGE_IMAGE_FORMATS: &[ImageFormat] =
    &[ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::WebP];

/// Largest image a page is made from: phone photos and scans go well past
/// what a step image holds, so they are decoded at up to this size and
/// scaled down to [`MAX_PAGE_SIDE`].
pub const MAX_SOURCE_SIDE: u32 = 30_000;
pub const MAX_SOURCE_PIXELS: u64 = 120_000_000;

/// Longest side of a page image in pixels; larger pages (posters, plans)
/// are scaled down to it.
pub const MAX_PAGE_SIDE: u32 = 4000;

/// Most pages one import adds.
pub const MAX_IMPORT_PAGES: usize = 100;

/// Largest file a page (or an image the editor inserts) is read from.
pub const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// Why a file could not become a page. The messages are for people.
#[derive(Debug, thiserror::Error)]
pub enum PageError {
    /// Not a usable file: the wrong type, unreadable, too large, too many.
    #[error("{0}")]
    Invalid(String),
    /// A chosen file does not exist.
    #[error("{0}")]
    NotFound(String),
    /// Any other error reading a chosen file.
    #[error("{0}")]
    Io(String),
    /// Writing the guide.
    #[error(transparent)]
    Library(#[from] LibraryError),
}

impl PageError {
    /// The same error with `file: ` in front of its message.
    pub fn in_file(self, path: &Path) -> Self {
        let named = |message: String| format!("{}: {message}", file_name(path));
        match self {
            Self::Invalid(m) => Self::Invalid(named(m)),
            Self::NotFound(m) => Self::NotFound(named(m)),
            Self::Io(m) => Self::Io(named(m)),
            e @ Self::Library(_) => e,
        }
    }
}

/// Makes a new annotated guide titled `title` in the library root, has
/// `fill` add its pages, saves it and returns it. `fill` gets the new
/// bundle and adds steps made with [`image_step`]: pages are written one by
/// one into the bundle rather than held, as a hundred would not fit in
/// memory. If `fill` or the save fails, the bundle is removed, so nothing
/// is left in the library.
pub fn create_annotated_guide<E: From<LibraryError>>(
    library: &Library,
    title: &str,
    fill: impl FnOnce(&GuideHandle, &mut Vec<Step>) -> Result<(), E>,
) -> Result<(GuideHandle, Guide), E> {
    let mut guide = Guide::new(title, "en");
    guide.kind = GuideKind::Annotated;
    let handle = library.create(&guide)?;
    let mut steps = Vec::new();
    let filled = fill(&handle, &mut steps).and_then(|()| {
        guide.items.extend(steps.into_iter().map(Item::Step));
        Ok(library.save_to(&handle, &guide)?)
    });
    if let Err(e) = filled {
        // Best effort: the error that matters is the one returned.
        let _ = library.delete(&handle);
        return Err(e);
    }
    Ok((handle, guide))
}

/// Makes a new annotated guide from PNG, JPEG and WebP files, one page
/// each, titled `title` or else after the first file (see
/// [`guide_title`]); pages are titled after their files. At most
/// [`MAX_IMPORT_PAGES`] files. Every file is checked by name before any is
/// read; a file that fails names itself in the error, and leaves no guide.
pub fn create_from_images(
    library: &Library,
    paths: &[PathBuf],
    title: Option<&str>,
) -> Result<(GuideHandle, Guide), PageError> {
    let Some(first) = paths.first() else {
        return Err(PageError::Invalid("no files chosen".into()));
    };
    if paths.len() > MAX_IMPORT_PAGES {
        return Err(PageError::Invalid(format!(
            "{} files chosen; at most {MAX_IMPORT_PAGES} pages can be added at once",
            paths.len()
        )));
    }
    for path in paths {
        let image = extension(path).is_some_and(|e| PAGE_IMAGE_EXTENSIONS.contains(&e.as_str()));
        if !image {
            return Err(PageError::Invalid(format!(
                "{} is not a PNG, JPEG or WebP image",
                file_name(path)
            )));
        }
    }
    let title = title
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map_or_else(|| guide_title(first), str::to_owned);
    create_annotated_guide(library, &title, |handle, steps| {
        for path in paths {
            let image = read_page_image(path)?;
            steps.push(image_step(handle, stem_title(path), &image)?);
        }
        Ok(())
    })
}

/// Reads an image file as a page: decoded (see [`decode_image`]; errors
/// name the file) and scaled down to fit [`MAX_PAGE_SIDE`].
pub fn read_page_image(path: &Path) -> Result<RgbaImage, PageError> {
    let bytes = read_file(path)?;
    let image = decode_image(
        &bytes,
        PAGE_IMAGE_FORMATS,
        "PNG, JPEG or WebP",
        MAX_SOURCE_SIDE,
        MAX_SOURCE_PIXELS,
    )
    .map_err(|e| e.in_file(path))?;
    Ok(fit_page(image))
}

/// Stores `image` as the full image of a new manual step (shown whole,
/// without a crop) titled `title`, and returns the step. The guide itself
/// is not changed.
pub fn image_step(
    handle: &GuideHandle,
    title: String,
    image: &RgbaImage,
) -> Result<Step, LibraryError> {
    let id = Ulid::new();
    let (width, height) = image.dimensions();
    let full = handle.write_step_image(
        id,
        StepImageVariant::Full,
        &PixelBuffer::rgba(width, height, image.as_raw()),
    )?;
    Ok(Step {
        id,
        title,
        description: String::new(),
        kind: StepKind::Manual,
        point: None,
        end_point: None,
        element: None,
        keystrokes: None,
        url: None,
        image: StepImage {
            full,
            crop: None,
            crop_rect: None,
            after: None,
            show: ImageShow::Full,
        },
        annotations: Vec::new(),
        narration: String::new(),
        ai_state: AiState::default(),
    })
}

/// `image` scaled down (never up) so its longer side is at most
/// [`MAX_PAGE_SIDE`].
pub fn fit_page(image: RgbaImage) -> RgbaImage {
    let (width, height) = image.dimensions();
    let longest = width.max(height);
    if longest <= MAX_PAGE_SIDE {
        return image;
    }
    let scale = |side: u32| {
        ((u64::from(side) * u64::from(MAX_PAGE_SIDE) + u64::from(longest) / 2) / u64::from(longest))
            .max(1) as u32
    };
    imageops::resize(&image, scale(width), scale(height), FilterType::Lanczos3)
}

/// Decodes an image in one of `formats` (told by its content, named `what`
/// in errors) as RGBA, turned upright as its EXIF orientation says. The
/// size in its header is checked before any pixels are decoded (at most
/// `max_side` per side and `max_pixels`), and the decoder runs with
/// matching limits.
pub fn decode_image(
    bytes: &[u8],
    formats: &[ImageFormat],
    what: &str,
    max_side: u32,
    max_pixels: u64,
) -> Result<RgbaImage, PageError> {
    let invalid =
        |e: image::ImageError| PageError::Invalid(format!("not a readable {what} image: {e}"));
    let reader = || -> Result<ImageReader<Cursor<&[u8]>>, PageError> {
        let reader = ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .map_err(|e| invalid(e.into()))?;
        match reader.format() {
            Some(format) if !formats.contains(&format) => {
                Err(PageError::Invalid(format!("not a {what} image")))
            }
            _ => Ok(reader),
        }
    };
    let (width, height) = reader()?.into_dimensions().map_err(invalid)?;
    let valid = 1..=max_side;
    if !valid.contains(&width)
        || !valid.contains(&height)
        || u64::from(width) * u64::from(height) > max_pixels
    {
        return Err(PageError::Invalid(format!(
            "image size {width}×{height} is not supported (at most {max_side} \
             per side and {} MP)",
            max_pixels / 1_000_000
        )));
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(max_side);
    limits.max_image_height = Some(max_side);
    // 16-bit RGBA at the pixel limit, plus the 8-bit copy.
    limits.max_alloc = Some(max_pixels * 12);
    let mut reader = reader()?;
    reader.limits(limits.clone());
    let mut decoder = reader.into_decoder().map_err(invalid)?;
    limits.reserve(decoder.total_bytes()).map_err(invalid)?;
    // A photo's camera orientation; PNGs from the webview have none.
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut image = DynamicImage::from_decoder(decoder).map_err(invalid)?;
    image.apply_orientation(orientation);
    Ok(image.into_rgba8())
}

/// Reads a regular file of at most [`MAX_FILE_BYTES`]. The size is checked
/// on the open file and the read is capped, so a file that grows meanwhile
/// is refused too.
pub fn read_file(path: &Path) -> Result<Vec<u8>, PageError> {
    let not_a_file = || PageError::Invalid(format!("{} is not a file", path.display()));
    let file = std::fs::File::open(path).map_err(|e| {
        // Windows refuses to open a folder at all.
        if path.is_dir() {
            not_a_file()
        } else {
            file_error(path, e)
        }
    })?;
    let meta = file.metadata().map_err(|e| file_error(path, e))?;
    if !meta.is_file() {
        return Err(not_a_file());
    }
    let too_large = || {
        PageError::Invalid(format!(
            "{} is too large (over {} MB)",
            path.display(),
            MAX_FILE_BYTES / 1024 / 1024
        ))
    };
    if meta.len() > MAX_FILE_BYTES {
        return Err(too_large());
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| file_error(path, e))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(too_large());
    }
    Ok(bytes)
}

fn file_error(path: &Path, e: std::io::Error) -> PageError {
    let message = format!("{}: {e}", path.display());
    if e.kind() == std::io::ErrorKind::NotFound {
        PageError::NotFound(message)
    } else {
        PageError::Io(message)
    }
}

/// The lowercase extension of `path`, if it has one.
pub fn extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
}

/// The file name of `path`, for messages.
pub fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

/// A title from a file name: its stem, whitespace and control characters
/// run together into single spaces.
pub fn stem_title(path: &Path) -> String {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    stem.split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// A new guide's title: the first file's (`Untitled` if it has none).
pub fn guide_title(path: &Path) -> String {
    let title = stem_title(path);
    if title.is_empty() {
        "Untitled".to_owned()
    } else {
        title
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{read_webp, MAX_WEBP_DIMENSION};
    use std::fs;

    /// A `width`×`height` image, red above blue, as `format` at `path`.
    fn image_file(path: &Path, width: u32, height: u32, format: ImageFormat) {
        let image = RgbaImage::from_fn(width, height, |_, y| {
            image::Rgba(if y < height / 2 {
                [255, 0, 0, 255]
            } else {
                [0, 0, 255, 255]
            })
        });
        let image = DynamicImage::ImageRgba8(image);
        let image = if format == ImageFormat::Jpeg {
            DynamicImage::ImageRgb8(image.to_rgb8())
        } else {
            image
        };
        image.save_with_format(path, format).unwrap();
    }

    fn library() -> (tempfile::TempDir, Library) {
        let dir = tempfile::tempdir().unwrap();
        let library = Library::open(dir.path().join("Library")).unwrap();
        (dir, library)
    }

    #[test]
    fn large_images_are_scaled_down_not_refused() {
        // Past both the 40 MP and the per-side limit of a step image.
        let wide = RgbaImage::new(MAX_WEBP_DIMENSION + 1, 3);
        assert_eq!(fit_page(wide).dimensions(), (MAX_PAGE_SIDE, 1));
        let photo = RgbaImage::new(MAX_PAGE_SIDE * 2, MAX_PAGE_SIDE * 3 / 2);
        assert_eq!(
            fit_page(photo).dimensions(),
            (MAX_PAGE_SIDE, MAX_PAGE_SIDE * 3 / 4)
        );
        let small = RgbaImage::new(5, 7);
        assert_eq!(fit_page(small).dimensions(), (5, 7));
    }

    #[test]
    fn creates_a_guide_from_images() {
        let (dir, library) = library();
        let paths = vec![
            dir.path().join("  Quarterly   report .PNG"),
            dir.path().join("photo.jpeg"),
            dir.path().join("shot.webp"),
        ];
        image_file(&paths[0], 6, 4, ImageFormat::Png);
        image_file(&paths[1], 8, 8, ImageFormat::Jpeg);
        image_file(&paths[2], 3, 2, ImageFormat::WebP);
        let (handle, guide) = create_from_images(&library, &paths, None).unwrap();
        assert_eq!(guide.kind, GuideKind::Annotated);
        assert_eq!(guide.title, "Quarterly report");
        assert!(handle.path.ends_with("Quarterly report.guiderecorder"));
        assert_eq!(library.read(&handle.path).unwrap().guide, guide);
        let steps: Vec<&Step> = guide.steps().collect();
        let titles: Vec<&str> = steps.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, ["Quarterly report", "photo", "shot"]);
        for (step, size) in steps.iter().zip([(6, 4), (8, 8), (3, 2)]) {
            assert_eq!(step.kind, StepKind::Manual);
            assert_eq!(step.image.show, ImageShow::Full);
            assert_eq!(step.image.crop, None);
            let image = read_webp(&handle.resolve(&step.image.full).unwrap()).unwrap();
            assert_eq!(image.dimensions(), size);
        }
        let png = read_webp(&handle.resolve(&steps[0].image.full).unwrap()).unwrap();
        assert_eq!(png.get_pixel(0, 0).0, [255, 0, 0, 255]);
        assert_eq!(png.get_pixel(5, 3).0, [0, 0, 255, 255]);

        let (_, titled) = create_from_images(&library, &paths[..1], Some(" Plan ")).unwrap();
        assert_eq!(titled.title, "Plan");
    }

    #[test]
    fn a_bad_file_leaves_no_guide() {
        let (dir, library) = library();
        let good = dir.path().join("good.png");
        image_file(&good, 2, 2, ImageFormat::Png);
        let broken = dir.path().join("broken.jpg");
        fs::write(&broken, b"not a jpeg").unwrap();
        let pdf = dir.path().join("doc.pdf");
        fs::write(&pdf, b"%PDF-1.4").unwrap();
        let big = dir.path().join("big.png");
        fs::File::create(&big)
            .unwrap()
            .set_len(MAX_FILE_BYTES + 1)
            .unwrap();
        let cases = [
            (broken, "broken.jpg: not a readable PNG, JPEG or WebP image"),
            (pdf, "doc.pdf is not a PNG, JPEG or WebP image"),
            (big, "is too large (over 64 MB)"),
            (dir.path().join("missing.png"), "missing.png"),
        ];
        for (bad, message) in cases {
            let err = create_from_images(&library, &[good.clone(), bad.clone()], None).unwrap_err();
            assert!(err.to_string().contains(message), "{message}: {err}");
            assert!(library.list().unwrap().is_empty(), "{}", bad.display());
        }
        let missing = create_from_images(&library, &[dir.path().join("nope.png")], None);
        assert!(matches!(missing, Err(PageError::NotFound(_))));
        assert!(matches!(
            create_from_images(&library, &[], None),
            Err(PageError::Invalid(_))
        ));
        let many = vec![good; MAX_IMPORT_PAGES + 1];
        let err = create_from_images(&library, &many, None).unwrap_err();
        assert_eq!(
            err.to_string(),
            "101 files chosen; at most 100 pages can be added at once"
        );
        // Nothing half-made either.
        let names: Vec<_> = fs::read_dir(library.root()).unwrap().collect();
        assert!(names.is_empty(), "{names:?}");
    }

    #[test]
    fn titles_fall_back_to_untitled() {
        assert_eq!(guide_title(Path::new("/x/ \t.png")), "Untitled");
        assert_eq!(guide_title(Path::new("/x/a\nb  c.pdf")), "a b c");
        assert_eq!(guide_title(Path::new("/x/.pdf")), ".pdf");
        assert_eq!(stem_title(Path::new("Scan 2.final.jpg")), "Scan 2.final");
    }
}
