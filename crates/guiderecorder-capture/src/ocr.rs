//! On-device text recognition for redaction (#10): Vision on macOS
//! (`macos::ocr`), Windows OCR on Windows (`windows_ocr`), unsupported
//! elsewhere. Never uses the network.
//!
//! The lines it returns are what `guiderecorder_core::redact` matches
//! against, in image pixels with the origin at the top left.

use guiderecorder_core::redact::TextLine;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OcrError {
    #[error("text recognition is not available on this platform")]
    Unsupported,
    #[error("the image is empty or its pixels do not match its size")]
    BadImage,
    #[error("text recognition failed: {0}")]
    Failed(String),
}

/// Whether [`recognize_text`] can work here. On Windows, whether an OCR
/// language is installed (checked once).
pub fn is_supported() -> bool {
    #[cfg(windows)]
    {
        crate::windows_ocr::is_available()
    }
    #[cfg(not(windows))]
    {
        cfg!(target_os = "macos")
    }
}

/// The text in a `width` × `height` image of tightly packed RGBA pixels
/// (alpha is ignored), line by line with word boxes. Blocks while it
/// runs, on any thread; calls may run in parallel.
pub fn recognize_text(rgba: &[u8], width: u32, height: u32) -> Result<Vec<TextLine>, OcrError> {
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4));
    if width == 0 || height == 0 || expected != Some(rgba.len()) {
        return Err(OcrError::BadImage);
    }
    #[cfg(target_os = "macos")]
    {
        crate::macos::ocr::recognize(rgba, width, height)
    }
    #[cfg(windows)]
    {
        crate::windows_ocr::recognize(rgba, width, height)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        Err(OcrError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bad_images_are_refused() {
        assert_eq!(recognize_text(&[], 0, 0), Err(OcrError::BadImage));
        assert_eq!(recognize_text(&[0; 15], 2, 2), Err(OcrError::BadImage));
        assert_eq!(
            recognize_text(&[0; 4], u32::MAX, u32::MAX),
            Err(OcrError::BadImage)
        );
    }

    #[test]
    fn a_blank_image_has_no_text() {
        let white = vec![255; 64 * 48 * 4];
        let result = recognize_text(&white, 64, 48);
        if is_supported() {
            assert_eq!(result, Ok(Vec::new()));
        } else {
            assert_eq!(result, Err(OcrError::Unsupported));
        }
    }

    /// Renders `text` in black Lato on white, `size` px high, at (20, 40),
    /// with wide gaps between words.
    #[cfg(any(target_os = "macos", windows))]
    fn rendered(text: &str, size: u32) -> (Vec<u8>, u32, u32) {
        use resvg::usvg::{self, fontdb};
        let (w, h) = (1200, 200);
        let mut db = fontdb::Database::new();
        db.load_font_data(
            include_bytes!("../../guiderecorder-core/assets/fonts/Lato-Regular.ttf").to_vec(),
        );
        let opts = usvg::Options {
            font_family: "Lato".into(),
            fontdb: std::sync::Arc::new(db),
            ..usvg::Options::default()
        };
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}">
                 <rect width="{w}" height="{h}" fill="white"/>
                 <text x="20" y="{}" font-size="{size}" word-spacing="{}" fill="black">{text}</text>
               </svg>"#,
            40 + size,
            size / 2
        );
        let tree = usvg::Tree::from_str(&svg, &opts).unwrap();
        let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h).unwrap();
        resvg::render(&tree, Default::default(), &mut pixmap.as_mut());
        // Opaque, so premultiplied is straight RGBA.
        (pixmap.take(), w, h)
    }

    #[cfg(any(target_os = "macos", windows))]
    #[test]
    fn recognises_lines_and_boxes_their_words() {
        let (rgba, w, h) = rendered("Contact jane.doe@example.com today", 32);
        let lines = recognize_text(&rgba, w, h).unwrap();
        let line = lines
            .iter()
            .find(|l| l.text.contains("jane.doe@example.com"))
            .unwrap_or_else(|| panic!("no address in {lines:?}"));
        // The line sits where it was drawn, top-left origin.
        assert!(line.rect.x >= 10.0 && line.rect.x <= 40.0, "{line:?}");
        assert!(
            line.rect.y >= 30.0 && line.rect.y + line.rect.height <= 100.0,
            "{line:?}"
        );
        // One box per word, in order, each inside the line.
        let words: Vec<&str> = line
            .words
            .iter()
            .map(|w| &line.text[w.range.clone()])
            .collect();
        assert_eq!(
            words,
            ["Contact", "jane.doe@example.com", "today"],
            "{line:?}"
        );
        for pair in line.words.windows(2) {
            assert!(pair[0].rect.x < pair[1].rect.x, "{line:?}");
        }
        let address = &line.words[1].rect;
        // Vision may box a word a little wider than the line.
        assert!(
            address.width > 200.0 && address.height > 10.0,
            "{address:?}"
        );
        assert!(address.x >= line.rect.x - 1.0, "{line:?}");
        assert!(
            address.x + address.width <= line.rect.x + line.rect.width + 1.0,
            "{line:?}"
        );
    }
}
