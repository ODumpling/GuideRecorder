//! Text recognition with Windows OCR (`Windows.Media.Ocr`), for redaction.
//!
//! - The engine for the user's profile languages, or English (US) if none
//!   of them has an OCR language installed. No engine at all (no language
//!   pack with OCR): unsupported. Windows OCR runs locally.
//! - Synchronous: waits for `RecognizeAsync` on the calling thread. Each
//!   call makes its own engine and bitmap, so calls may run in parallel.
//!   WinRT needs COM: the process keeps an implicit multithreaded
//!   apartment, so any thread works without changing its own apartment.
//! - Windows gives a line as its words, each with a box in image pixels,
//!   origin top left. The line's text is its words joined by single
//!   spaces; its box is the union of theirs. The skew of the text
//!   (`TextAngle`) is ignored: boxes stay upright, as Windows gives them.

use std::sync::OnceLock;

use guiderecorder_core::model::Rect;
use guiderecorder_core::redact::{TextLine, TextWord};
use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::{OcrEngine, OcrLine};
use windows::Security::Cryptography::CryptographicBuffer;
use windows::Win32::System::Com::CoIncrementMTAUsage;

use crate::ocr::OcrError;

/// Language of the engine when none of the user's languages has OCR.
const FALLBACK_LANGUAGE: &str = "en-US";

/// Whether an engine can be made here (checked once).
pub fn is_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| engine().is_ok())
}

/// See [`crate::ocr::recognize_text`], which checks the buffer size.
pub fn recognize(rgba: &[u8], width: u32, height: u32) -> Result<Vec<TextLine>, OcrError> {
    let engine = engine()?;
    let max = OcrEngine::MaxImageDimension().map_err(failed)?;
    if width > max || height > max {
        return Err(OcrError::Failed(format!(
            "the image is larger than {max} px, the most Windows OCR reads"
        )));
    }
    let bitmap = software_bitmap(rgba, width, height)?;
    let result = engine
        .RecognizeAsync(&bitmap)
        .and_then(|op| op.join())
        .map_err(failed)?;
    let mut lines = Vec::new();
    for line in result.Lines().map_err(failed)? {
        if let Some(line) = text_line(&line)? {
            lines.push(line);
        }
    }
    Ok(lines)
}

/// An engine for the user's languages, else for [`FALLBACK_LANGUAGE`].
fn engine() -> Result<OcrEngine, OcrError> {
    ensure_com();
    // Both return null (an error here), not a failure, when no OCR
    // language is installed.
    OcrEngine::TryCreateFromUserProfileLanguages()
        .or_else(|_| {
            let language = Language::CreateLanguage(&FALLBACK_LANGUAGE.into())?;
            OcrEngine::TryCreateFromLanguage(&language)
        })
        .map_err(|_| OcrError::Unsupported)
}

/// Keeps a multithreaded apartment for the whole process, once, so WinRT
/// works on any thread: one with no apartment joins it implicitly, and one
/// already in an apartment (say the UI thread) keeps its own.
fn ensure_com() {
    static MTA: OnceLock<()> = OnceLock::new();
    MTA.get_or_init(|| {
        // SAFETY: no preconditions. The cookie is never released, so the
        // apartment lasts until the process exits. On failure WinRT calls
        // fail and say why.
        let _ = unsafe { CoIncrementMTAUsage() };
    });
}

/// A BGRA `SoftwareBitmap` of the pixels (copied), opaque.
fn software_bitmap(rgba: &[u8], width: u32, height: u32) -> Result<SoftwareBitmap, OcrError> {
    let bgra: Vec<u8> = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|&[r, g, b, _]| [b, g, r, 255])
        .collect();
    let buffer = CryptographicBuffer::CreateFromByteArray(&bgra).map_err(failed)?;
    // The caller checked the size, and `MaxImageDimension` fits an i32.
    SoftwareBitmap::CreateCopyFromBuffer(
        &buffer,
        BitmapPixelFormat::Bgra8,
        width as i32,
        height as i32,
    )
    .map_err(failed)
}

/// `line` as its words joined by single spaces, each boxed by byte range;
/// `None` if it has no words.
fn text_line(line: &OcrLine) -> Result<Option<TextLine>, OcrError> {
    let mut text = String::new();
    let mut words = Vec::new();
    for word in line.Words().map_err(failed)? {
        let string = word.Text().map_err(failed)?.to_string_lossy();
        let string = string.trim();
        if string.is_empty() {
            continue;
        }
        if !text.is_empty() {
            text.push(' ');
        }
        let start = text.len();
        text.push_str(string);
        let b = word.BoundingRect().map_err(failed)?;
        words.push(TextWord {
            range: start..text.len(),
            rect: Rect {
                x: f64::from(b.X),
                y: f64::from(b.Y),
                width: f64::from(b.Width),
                height: f64::from(b.Height),
            },
        });
    }
    let Some(rect) = words.iter().map(|w| w.rect).reduce(union) else {
        return Ok(None);
    };
    Ok(Some(TextLine { text, rect, words }))
}

/// The smallest rect around `a` and `b`.
fn union(a: Rect, b: Rect) -> Rect {
    let (x, y) = (a.x.min(b.x), a.y.min(b.y));
    Rect {
        x,
        y,
        width: (a.x + a.width).max(b.x + b.width) - x,
        height: (a.y + a.height).max(b.y + b.height) - y,
    }
}

fn failed(e: windows::core::Error) -> OcrError {
    OcrError::Failed(e.message())
}
