//! Text recognition with Vision (`VNRecognizeTextRequest`), for redaction.
//!
//! - Accurate level, without language correction: it would "correct" codes
//!   and keys into words. Text down to about [`MIN_TEXT_PX`] high.
//! - Synchronous, on the calling thread. Each call makes its own request
//!   and handler, so calls may run in parallel. Vision runs locally.
//! - Vision's boxes are normalised, origin bottom left; they come back in
//!   image pixels, origin top left.
//! - Each line comes with the box of each whitespace-separated word
//!   (`boundingBoxForRange:error:`, not in the generated bindings), so a
//!   match blurs only its own words. A word Vision can't box is left out;
//!   redaction then estimates from the line.

use guiderecorder_core::model::Rect;
use guiderecorder_core::redact::{TextLine, TextWord};
use objc2::rc::{autoreleasepool, Retained};
use objc2::{msg_send, AllocAnyThread};
use objc2_core_foundation::{CFData, CFRetained, CGRect};
use objc2_core_graphics::{
    kCGColorSpaceSRGB, CGBitmapInfo, CGColorRenderingIntent, CGColorSpace, CGDataProvider, CGImage,
    CGImageAlphaInfo, CGImageByteOrderInfo,
};
use objc2_foundation::{NSArray, NSDictionary, NSError, NSRange};
use objc2_vision::{
    VNImageRequestHandler, VNRecognizeTextRequest, VNRecognizedText, VNRectangleObservation,
    VNRequest, VNRequestTextRecognitionLevel,
};

use crate::ocr::OcrError;

/// Smallest text recognised, in image pixels (Vision takes a fraction of
/// the image height). Small UI text on a non-Retina display is about 9 px.
const MIN_TEXT_PX: f32 = 8.0;

/// See [`crate::ocr::recognize_text`], which checks the buffer size.
pub fn recognize(rgba: &[u8], width: u32, height: u32) -> Result<Vec<TextLine>, OcrError> {
    autoreleasepool(|_| {
        let image = cg_image(rgba, width, height)?;
        let request = VNRecognizeTextRequest::new();
        request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
        request.setUsesLanguageCorrection(false);
        request.setMinimumTextHeight((MIN_TEXT_PX / height as f32).min(1.0));
        // SAFETY: an empty options dictionary is valid.
        let handler = unsafe {
            VNImageRequestHandler::initWithCGImage_options(
                VNImageRequestHandler::alloc(),
                &image,
                &NSDictionary::new(),
            )
        };
        let as_request: &VNRequest = &request;
        handler
            .performRequests_error(&NSArray::from_slice(&[as_request]))
            .map_err(|e| OcrError::Failed(e.localizedDescription().to_string()))?;
        let size = (f64::from(width), f64::from(height));
        let mut lines = Vec::new();
        for observation in request.results().iter().flat_map(|r| r.iter()) {
            let Some(text) = observation.topCandidates(1).firstObject() else {
                continue;
            };
            let string = text.string().to_string();
            if string.trim().is_empty() {
                continue;
            }
            // SAFETY: a plain property read on a finished observation.
            let rect = to_pixels(unsafe { observation.boundingBox() }, size);
            let words = word_boxes(&text, &string, size);
            lines.push(TextLine {
                text: string,
                rect,
                words,
            });
        }
        Ok(lines)
    })
}

/// An sRGB `CGImage` of the pixels (copied), alpha skipped.
fn cg_image(rgba: &[u8], width: u32, height: u32) -> Result<CFRetained<CGImage>, OcrError> {
    let failed = || OcrError::Failed("cannot make an image for Vision".into());
    let data = CFData::from_bytes(rgba);
    let provider = CGDataProvider::with_cf_data(Some(&data)).ok_or_else(failed)?;
    // SAFETY: a static CFString, valid for the whole program.
    let srgb = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB })).ok_or_else(failed)?;
    let info = CGBitmapInfo(CGImageAlphaInfo::NoneSkipLast.0 | CGImageByteOrderInfo::Order32Big.0);
    // SAFETY: `provider` holds `height` rows of `width * 4` bytes (checked
    // by the caller), 8 bits a component, 32 a pixel; `decode` may be null.
    unsafe {
        CGImage::new(
            width as usize,
            height as usize,
            8,
            32,
            width as usize * 4,
            Some(&srgb),
            info,
            Some(&provider),
            std::ptr::null(),
            false,
            CGColorRenderingIntent::RenderingIntentDefault,
        )
    }
    .ok_or_else(failed)
}

/// The box of each whitespace-separated word of `string` (the text of
/// `text`), by byte range.
fn word_boxes(text: &VNRecognizedText, string: &str, size: (f64, f64)) -> Vec<TextWord> {
    let mut words = Vec::new();
    let mut utf16 = 0;
    let mut start: Option<(usize, usize)> = None;
    for (i, c) in string.char_indices().chain([(string.len(), ' ')]) {
        if c.is_whitespace() {
            if let Some((byte, unit)) = start.take() {
                let range = NSRange::new(unit, utf16 - unit);
                // SAFETY: `range` is inside the string Vision returned
                // (UTF-16 units, as NSString counts).
                let boxed: Result<Retained<VNRectangleObservation>, Retained<NSError>> =
                    unsafe { msg_send![text, boundingBoxForRange: range, error: _] };
                if let Ok(observation) = boxed {
                    // SAFETY: a plain property read.
                    let rect = to_pixels(unsafe { observation.boundingBox() }, size);
                    words.push(TextWord {
                        range: byte..i,
                        rect,
                    });
                }
            }
        } else if start.is_none() {
            start = Some((i, utf16));
        }
        utf16 += c.len_utf16();
    }
    words
}

/// A normalised Vision box (origin bottom left) in image pixels (origin
/// top left).
fn to_pixels(b: CGRect, (w, h): (f64, f64)) -> Rect {
    Rect {
        x: b.origin.x * w,
        y: (1.0 - b.origin.y - b.size.height) * h,
        width: b.size.width * w,
        height: b.size.height * h,
    }
}
