//! PDF pages as images, for the pages of an annotated guide (see
//! `annotated.rs`). macOS draws them with CoreGraphics; elsewhere a PDF is
//! refused.
//!
//! A page is drawn whole: its crop box, turned by its `/Rotate`, on opaque
//! white, at [`PDF_SCALE`] pixels per point unless that makes it longer than
//! [`MAX_PAGE_SIDE`].

use std::fmt;

use guiderecorder_core::library::MAX_WEBP_DIMENSION;
use image::RgbaImage;

/// Pixels per PDF point: 144 dpi, sharp on a Retina screen.
pub const PDF_SCALE: f64 = 2.0;

/// Longest side of a rendered page in pixels; larger pages (posters,
/// plans) are scaled down to it.
pub const MAX_PAGE_SIDE: u32 = 4000;

/// Why a PDF or one of its pages can't be shown. Says nothing of the file:
/// the caller names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PdfError {
    /// Not macOS.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    Unsupported,
    /// Not a PDF CoreGraphics can read.
    Unreadable,
    /// Encrypted with a password.
    Locked,
    NoPages,
    /// The page (1-based) is missing or has no area.
    Page(usize),
}

impl fmt::Display for PdfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PdfError::Unsupported => write!(f, "PDF import needs macOS for now"),
            PdfError::Unreadable => write!(f, "not a readable PDF"),
            PdfError::Locked => write!(f, "the PDF is password-protected"),
            PdfError::NoPages => write!(f, "the PDF has no pages"),
            PdfError::Page(n) => write!(f, "page {n} of the PDF can't be drawn"),
        }
    }
}

/// The pixel size of a page `width`×`height` points: [`PDF_SCALE`] pixels
/// per point, scaled down so the longer side is at most [`MAX_PAGE_SIDE`]
/// (and [`MAX_WEBP_DIMENSION`]), each side at least 1. `None` for a page
/// without area.
pub fn page_pixels(width: f64, height: f64) -> Option<(u32, u32)> {
    let valid = |side: f64| side.is_finite() && side > 0.0;
    if !valid(width) || !valid(height) {
        return None;
    }
    let longest = f64::from(MAX_PAGE_SIDE.min(MAX_WEBP_DIMENSION));
    let scale = PDF_SCALE.min(longest / width.max(height));
    let pixels = |side: f64| (side * scale).round().clamp(1.0, longest) as u32;
    Some((pixels(width), pixels(height)))
}

#[cfg(target_os = "macos")]
pub use macos::PdfDocument;

#[cfg(target_os = "macos")]
mod macos {
    use objc2_core_foundation::{CFData, CFRetained, CGPoint, CGRect, CGSize};
    use objc2_core_graphics::{
        kCGColorSpaceSRGB, CGBitmapContextCreate, CGColorSpace, CGContext, CGDataProvider,
        CGImageAlphaInfo, CGImageByteOrderInfo, CGInterpolationQuality, CGPDFBox, CGPDFDocument,
        CGPDFPage,
    };

    use super::*;

    /// An open PDF. Holds a copy of its bytes; pages are drawn one at a
    /// time, so a long PDF is never in memory as images all at once.
    pub struct PdfDocument {
        document: CFRetained<CGPDFDocument>,
    }

    impl PdfDocument {
        /// Opens a PDF from its bytes. Refuses one that is not a PDF, is
        /// locked with a password or has no pages.
        pub fn open(bytes: &[u8]) -> Result<Self, PdfError> {
            let data = CFData::from_bytes(bytes);
            let provider = CGDataProvider::with_cf_data(Some(&data)).ok_or(PdfError::Unreadable)?;
            let document =
                CGPDFDocument::with_provider(Some(&provider)).ok_or(PdfError::Unreadable)?;
            let doc = Some(&*document);
            if CGPDFDocument::is_encrypted(doc) && !CGPDFDocument::is_unlocked(doc) {
                // Only an owner password (print or copy limits): an empty
                // user password opens it, as in Preview.
                let empty: &std::ffi::c_char = &0;
                // SAFETY: `empty` is a NUL-terminated (empty) C string that
                // outlives the call.
                let unlocked = unsafe {
                    CGPDFDocument::unlock_with_password(doc, std::ptr::NonNull::from(empty))
                };
                if !unlocked {
                    return Err(PdfError::Locked);
                }
            }
            if CGPDFDocument::number_of_pages(doc) == 0 {
                return Err(PdfError::NoPages);
            }
            Ok(Self { document })
        }

        pub fn page_count(&self) -> usize {
            CGPDFDocument::number_of_pages(Some(&self.document))
        }

        /// Draws page `number` (1-based); see the module docs.
        pub fn render(&self, number: usize) -> Result<RgbaImage, PdfError> {
            let failed = || PdfError::Page(number);
            let page = CGPDFDocument::page(Some(&self.document), number).ok_or_else(failed)?;
            let page = Some(&*page);
            // The crop box is what viewers show; CoreGraphics clips it to
            // the media box.
            let bounds = CGPDFPage::box_rect(page, CGPDFBox::CropBox);
            let (mut width, mut height) = (bounds.size.width.abs(), bounds.size.height.abs());
            if CGPDFPage::rotation_angle(page).rem_euclid(180) == 90 {
                std::mem::swap(&mut width, &mut height);
            }
            let (pixel_width, pixel_height) = page_pixels(width, height).ok_or_else(failed)?;
            let (w, h) = (pixel_width as usize, pixel_height as usize);
            let mut pixels = vec![0u8; w * h * 4];
            // SAFETY: a static CFString, valid for the whole program.
            let srgb =
                CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB })).ok_or_else(failed)?;
            // Memory order R, G, B, A (32-bit big-endian, alpha last).
            let info = CGImageAlphaInfo::PremultipliedLast.0 | CGImageByteOrderInfo::Order32Big.0;
            {
                // SAFETY: `pixels` holds `h` rows of `w * 4` bytes and
                // outlives `context`, dropped at the end of this block.
                let context = unsafe {
                    CGBitmapContextCreate(
                        pixels.as_mut_ptr().cast(),
                        w,
                        h,
                        8,
                        w * 4,
                        Some(&srgb),
                        info,
                    )
                }
                .ok_or_else(failed)?;
                let c = Some(&*context);
                // Opaque white first: everything drawn over it stays opaque,
                // so the premultiplied pixels are also straight RGBA.
                CGContext::set_rgb_fill_color(c, 1.0, 1.0, 1.0, 1.0);
                CGContext::fill_rect(
                    c,
                    CGRect::new(CGPoint::ZERO, CGSize::new(w as f64, h as f64)),
                );
                CGContext::set_interpolation_quality(c, CGInterpolationQuality::High);
                // Points to pixels. `drawing_transform` never scales up, so
                // it maps the box onto a rect of its own size in points; it
                // moves the box to the origin and applies `/Rotate`. Both
                // the bitmap and PDF space have y up, and the bitmap's first
                // row in memory is its top, so the image comes out upright.
                CGContext::scale_ctm(c, w as f64 / width, h as f64 / height);
                let target = CGRect::new(CGPoint::ZERO, CGSize::new(width, height));
                let transform =
                    CGPDFPage::drawing_transform(page, CGPDFBox::CropBox, target, 0, true);
                CGContext::concat_ctm(c, transform);
                CGContext::clip_to_rect(c, bounds);
                CGContext::draw_pdf_page(c, page);
            }
            RgbaImage::from_raw(pixel_width, pixel_height, pixels).ok_or_else(failed)
        }
    }
}

/// Elsewhere every PDF is refused with [`PdfError::Unsupported`].
#[cfg(not(target_os = "macos"))]
pub struct PdfDocument(());

#[cfg(not(target_os = "macos"))]
impl PdfDocument {
    pub fn open(_bytes: &[u8]) -> Result<Self, PdfError> {
        Err(PdfError::Unsupported)
    }

    pub fn page_count(&self) -> usize {
        0
    }

    pub fn render(&self, _number: usize) -> Result<RgbaImage, PdfError> {
        Err(PdfError::Unsupported)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// One page of [`pdf`]: its media box size in points, an optional crop
    /// box, its `/Rotate` and its content stream.
    pub(crate) struct TestPage<'a> {
        pub size: (u32, u32),
        pub crop: Option<[u32; 4]>,
        pub rotate: u32,
        pub content: &'a str,
    }

    impl<'a> TestPage<'a> {
        pub(crate) fn new(width: u32, height: u32, content: &'a str) -> Self {
            Self {
                size: (width, height),
                crop: None,
                rotate: 0,
                content,
            }
        }
    }

    /// A PDF with `pages`, written by hand with a correct cross-reference
    /// table.
    pub(crate) fn pdf(pages: &[TestPage<'_>]) -> Vec<u8> {
        let mut objects = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            format!(
                "<< /Type /Pages /Kids [{}] /Count {} >>",
                (0..pages.len())
                    .map(|i| format!("{} 0 R", 3 + 2 * i))
                    .collect::<Vec<_>>()
                    .join(" "),
                pages.len()
            ),
        ];
        for (i, page) in pages.iter().enumerate() {
            let crop = page
                .crop
                .map(|[x, y, w, h]| format!(" /CropBox [{x} {y} {} {}]", x + w, y + h))
                .unwrap_or_default();
            objects.push(format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}]{crop} /Rotate {} \
                 /Resources << >> /Contents {} 0 R >>",
                page.size.0,
                page.size.1,
                page.rotate,
                4 + 2 * i
            ));
            objects.push(format!(
                "<< /Length {} >>\nstream\n{}\nendstream",
                page.content.len(),
                page.content
            ));
        }
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, object) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend(format!("{} 0 obj\n{object}\nendobj\n", i + 1).bytes());
        }
        let xref = out.len();
        out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
        for offset in offsets {
            out.extend(format!("{offset:010} 00000 n \n").bytes());
        }
        out.extend(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .bytes(),
        );
        out
    }

    /// A black square `side` points wide with its top-left corner at the
    /// top-left of a page `height` points high.
    pub(crate) fn top_left_square(height: u32, side: u32) -> String {
        format!("0 0 0 rg 0 {} {side} {side} re f", height - side)
    }

    #[test]
    fn page_pixels_scale_and_cap() {
        assert_eq!(page_pixels(612.0, 792.0), Some((1224, 1584)));
        assert_eq!(page_pixels(0.4, 0.4), Some((1, 1)));
        // Over the cap: the longer side becomes MAX_PAGE_SIDE.
        assert_eq!(page_pixels(3000.0, 1000.0), Some((4000, 1333)));
        assert_eq!(page_pixels(100.0, 50_000.0), Some((8, 4000)));
        for (w, h) in [
            (0.0, 10.0),
            (10.0, -1.0),
            (f64::NAN, 1.0),
            (f64::INFINITY, 1.0),
        ] {
            assert_eq!(page_pixels(w, h), None, "{w}×{h}");
        }
    }

    #[cfg(target_os = "macos")]
    fn dark(image: &RgbaImage, x: u32, y: u32) -> bool {
        let p = image.get_pixel(x, y).0;
        p[3] == 255 && p[..3].iter().all(|&c| c < 40)
    }

    #[cfg(target_os = "macos")]
    fn white(image: &RgbaImage, x: u32, y: u32) -> bool {
        image.get_pixel(x, y).0 == [255, 255, 255, 255]
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn renders_pages_upright_on_white() {
        let bytes = pdf(&[
            TestPage::new(200, 100, &top_left_square(100, 50)),
            // A different size, the square bottom-right.
            TestPage::new(100, 300, "0 0 0 rg 60 0 40 40 re f"),
        ]);
        let document = PdfDocument::open(&bytes).unwrap();
        assert_eq!(document.page_count(), 2);

        let first = document.render(1).unwrap();
        assert_eq!(first.dimensions(), (400, 200));
        assert!(dark(&first, 5, 5) && dark(&first, 95, 95));
        assert!(white(&first, 105, 5) && white(&first, 5, 105));
        assert!(white(&first, 395, 195));
        // Opaque everywhere: straight and premultiplied agree.
        assert!(first.pixels().all(|p| p.0[3] == 255));

        let second = document.render(2).unwrap();
        assert_eq!(second.dimensions(), (200, 600));
        assert!(dark(&second, 195, 595) && dark(&second, 125, 525));
        assert!(white(&second, 5, 5) && white(&second, 115, 515));

        assert_eq!(document.render(3).unwrap_err(), PdfError::Page(3));
        assert_eq!(document.render(0).unwrap_err(), PdfError::Page(0));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn renders_the_crop_box_turned_by_rotate() {
        let square = top_left_square(100, 50);
        // A 200×100 page turned a quarter clockwise shows 100×200, its
        // top-left corner now top-right.
        let rotated = TestPage {
            rotate: 90,
            ..TestPage::new(200, 100, &square)
        };
        let image = PdfDocument::open(&pdf(&[rotated]))
            .unwrap()
            .render(1)
            .unwrap();
        assert_eq!(image.dimensions(), (200, 400));
        assert!(dark(&image, 195, 5) && dark(&image, 105, 95));
        assert!(white(&image, 5, 5) && white(&image, 195, 395) && white(&image, 95, 5));

        // Only the crop box shows: here the square's top-left quarter.
        let cropped = TestPage {
            crop: Some([0, 75, 100, 25]),
            ..TestPage::new(200, 100, &square)
        };
        let image = PdfDocument::open(&pdf(&[cropped]))
            .unwrap()
            .render(1)
            .unwrap();
        assert_eq!(image.dimensions(), (200, 50));
        assert!(dark(&image, 5, 5) && dark(&image, 95, 45));
        assert!(white(&image, 105, 5) && white(&image, 195, 45));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn caps_the_longest_side() {
        let image = PdfDocument::open(&pdf(&[TestPage::new(3000, 1000, "")]))
            .unwrap()
            .render(1)
            .unwrap();
        assert_eq!(image.dimensions(), (4000, 1333));
        assert!(image.pixels().all(|p| p.0 == [255, 255, 255, 255]));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn refuses_what_is_not_a_usable_pdf() {
        for junk in [&b""[..], b"not a pdf", b"%PDF-1.4\n%%EOF\n"] {
            assert_eq!(
                PdfDocument::open(junk).err(),
                Some(PdfError::Unreadable),
                "{junk:?}"
            );
        }
        // CoreGraphics refuses a page tree without pages outright.
        let empty = PdfDocument::open(&pdf(&[])).err();
        assert!(
            matches!(empty, Some(PdfError::Unreadable | PdfError::NoPages)),
            "{empty:?}"
        );
        // A page without area is refused when drawn.
        let flat = PdfDocument::open(&pdf(&[TestPage::new(0, 100, "")])).unwrap();
        assert_eq!(flat.render(1).unwrap_err(), PdfError::Page(1));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn needs_macos() {
        let err = PdfDocument::open(&pdf(&[TestPage::new(10, 10, "")])).err();
        assert_eq!(err, Some(PdfError::Unsupported));
    }
}
