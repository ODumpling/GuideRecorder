//! MP4 (H.264) writer over the motion frames (see the [module
//! docs](super)).
//!
//! Encoded on the device with AVFoundation's `AVAssetWriter`, so nothing
//! beyond the system is needed. Other platforms get
//! [`MotionError::Unsupported`]. Frames are [`FPS`] a second; a held frame
//! is appended at its first and last tick only. The default frame
//! is [`DEFAULT_SIZE`] pixels on its short side (1920 × 1080 when wide).
//!
//! AVFoundation will not write over a file, so the movie is written into a
//! fresh temporary folder next to `out` and then moved into place, as the
//! other writers do.

use std::path::Path;

use super::{io_error, MotionOptions, Result};
use crate::export::render::RenderGuide;
use crate::export::report::ExportReport;

/// Frames per second.
pub const FPS: u32 = 30;
/// Default short side of the frame, in pixels.
pub const DEFAULT_SIZE: u32 = 1080;

/// Whether MP4 export works on this platform.
pub const SUPPORTED: bool = cfg!(target_os = "macos");

/// Writes `guide` as an MP4 video to `out_path`.
pub fn export(
    guide: &RenderGuide,
    out_path: impl AsRef<Path>,
    opts: &MotionOptions,
) -> Result<ExportReport> {
    let out = out_path.as_ref();
    crate::export::persist::check_free(out, opts.overwrite).map_err(|e| io_error(e, out))?;
    let size = opts.aspect.size(opts.size.unwrap_or(DEFAULT_SIZE));
    imp::export(guide, out, opts, size)
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::*;
    use crate::export::motion::MotionError;

    pub fn export(
        _guide: &RenderGuide,
        _out: &Path,
        _opts: &MotionOptions,
        _size: (u32, u32),
    ) -> Result<ExportReport> {
        Err(MotionError::Unsupported(
            "MP4 export needs macOS; export a GIF instead".into(),
        ))
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::ptr::NonNull;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use block2::RcBlock;
    use objc2::rc::autoreleasepool;
    use objc2::runtime::AnyObject;
    use objc2_av_foundation::{
        AVAssetWriter, AVAssetWriterInput, AVAssetWriterInputPixelBufferAdaptor,
        AVAssetWriterStatus, AVFileTypeMPEG4, AVMediaTypeVideo, AVVideoAverageBitRateKey,
        AVVideoCodecKey, AVVideoCodecTypeH264, AVVideoCompressionPropertiesKey, AVVideoHeightKey,
        AVVideoWidthKey,
    };
    use objc2_core_foundation::CFRetained;
    use objc2_core_media::CMTime;
    use objc2_core_video::{
        kCVPixelFormatType_32BGRA, kCVReturnSuccess, CVPixelBuffer, CVPixelBufferCreate,
        CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow, CVPixelBufferLockBaseAddress,
        CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
    };
    use objc2_foundation::{NSDictionary, NSNumber, NSString, NSURL};
    use resvg::tiny_skia::Pixmap;

    use super::*;
    use crate::export::motion::{render, MotionError};
    use crate::export::persist;

    /// Longest wait for the encoder to take another frame.
    const READY_TIMEOUT: Duration = Duration::from_secs(30);

    fn encode_error(what: &str) -> MotionError {
        MotionError::Encode(what.into())
    }

    pub fn export(
        guide: &RenderGuide,
        out: &Path,
        opts: &MotionOptions,
        size: (u32, u32),
    ) -> Result<ExportReport> {
        let dir = tempfile::Builder::new()
            .prefix(".guiderecorder-export-")
            .tempdir_in(persist::parent_dir(out))?;
        let tmp = dir.path().join("video.mp4");
        let mut report = ExportReport::default();
        autoreleasepool(|_| write(guide, opts, size, &tmp, &mut report))?;
        std::fs::File::open(&tmp)?.sync_all()?;
        persist::persist(
            tempfile::TempPath::try_from_path(&tmp)?,
            out,
            opts.overwrite,
        )
        .map_err(|e| io_error(e, out))?;
        report.files.push(out.to_path_buf());
        Ok(report)
    }

    fn write(
        guide: &RenderGuide,
        opts: &MotionOptions,
        (width, height): (u32, u32),
        path: &Path,
        report: &mut ExportReport,
    ) -> Result<()> {
        let path = path
            .to_str()
            .ok_or_else(|| encode_error("the output path is not valid UTF-8"))?;
        // SAFETY: the statics are AVFoundation's constants, the settings
        // dictionaries hold the value types AVFoundation documents for
        // their keys, and every call follows AVAssetWriter's documented
        // order (inputs added before startWriting, frames appended only
        // while the input is ready, finish after markAsFinished).
        unsafe {
            let url = NSURL::fileURLWithPath(&NSString::from_str(path));
            let file_type = AVFileTypeMPEG4.ok_or_else(|| encode_error("no MPEG-4 file type"))?;
            let writer = AVAssetWriter::assetWriterWithURL_fileType_error(&url, file_type)
                .map_err(|e| MotionError::Encode(e.localizedDescription().to_string()))?;

            let bitrate = NSNumber::new_u64(u64::from(width) * u64::from(height) * 3);
            let compression = NSDictionary::<NSString, AnyObject>::from_slices(
                &[AVVideoAverageBitRateKey.ok_or_else(|| encode_error("no bitrate key"))?],
                &[&*bitrate as &AnyObject],
            );
            let w = NSNumber::new_u32(width);
            let h = NSNumber::new_u32(height);
            let codec = AVVideoCodecTypeH264.ok_or_else(|| encode_error("no H.264 codec"))?;
            let settings = NSDictionary::<NSString, AnyObject>::from_slices(
                &[
                    AVVideoCodecKey.ok_or_else(|| encode_error("no codec key"))?,
                    AVVideoWidthKey.ok_or_else(|| encode_error("no width key"))?,
                    AVVideoHeightKey.ok_or_else(|| encode_error("no height key"))?,
                    AVVideoCompressionPropertiesKey
                        .ok_or_else(|| encode_error("no compression key"))?,
                ],
                &[
                    codec as &AnyObject,
                    &*w as &AnyObject,
                    &*h as &AnyObject,
                    &*compression as &AnyObject,
                ],
            );
            let media = AVMediaTypeVideo.ok_or_else(|| encode_error("no video media type"))?;
            let input = AVAssetWriterInput::assetWriterInputWithMediaType_outputSettings(
                media,
                Some(&settings),
            );
            input.setExpectsMediaDataInRealTime(false);
            if !writer.canAddInput(&input) {
                return Err(encode_error("the writer refused the video settings"));
            }
            writer.addInput(&input);
            let adaptor = AVAssetWriterInputPixelBufferAdaptor::assetWriterInputPixelBufferAdaptorWithAssetWriterInput_sourcePixelBufferAttributes(&input, None);
            if !writer.startWriting() {
                return Err(writer_error(&writer));
            }
            writer.startSessionAtSourceTime(CMTime::new(0, FPS as i32));

            let mut tick: i64 = 0;
            let mut sink = |pixmap: &Pixmap, ticks: u32| -> Result<()> {
                autoreleasepool(|_| {
                    // A held frame goes in again at its last tick, or the
                    // movie's final frame would last a single tick.
                    let last = tick + i64::from(ticks) - 1;
                    let times = if last > tick {
                        vec![tick, last]
                    } else {
                        vec![tick]
                    };
                    for time in times {
                        wait_ready(&writer, &input)?;
                        let buffer = pixel_buffer(pixmap)?;
                        if !adaptor.appendPixelBuffer_withPresentationTime(
                            &buffer,
                            CMTime::new(time, FPS as i32),
                        ) {
                            return Err(writer_error(&writer));
                        }
                    }
                    tick += i64::from(ticks);
                    Ok(())
                })
            };
            let result = render(guide, opts, (width, height), FPS, report, &mut sink);
            if let Err(e) = result {
                writer.cancelWriting();
                return Err(e);
            }
            input.markAsFinished();
            writer.endSessionAtSourceTime(CMTime::new(tick, FPS as i32));
            let (tx, rx) = mpsc::channel();
            let done = RcBlock::new(move || {
                let _ = tx.send(());
            });
            writer.finishWritingWithCompletionHandler(&done);
            rx.recv()
                .map_err(|_| encode_error("the writer stopped without finishing"))?;
            if writer.status() != AVAssetWriterStatus::Completed {
                return Err(writer_error(&writer));
            }
        }
        Ok(())
    }

    /// Waits until `input` takes another frame, or the writer fails.
    unsafe fn wait_ready(writer: &AVAssetWriter, input: &AVAssetWriterInput) -> Result<()> {
        let start = Instant::now();
        while !input.isReadyForMoreMediaData() {
            if writer.status() != AVAssetWriterStatus::Writing {
                return Err(writer_error(writer));
            }
            if start.elapsed() > READY_TIMEOUT {
                return Err(encode_error("the encoder stopped taking frames"));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(())
    }

    unsafe fn writer_error(writer: &AVAssetWriter) -> MotionError {
        match writer.error() {
            Some(e) => MotionError::Encode(e.localizedDescription().to_string()),
            None => encode_error("the video writer failed"),
        }
    }

    /// `pixmap` (opaque RGBA) as a BGRA pixel buffer.
    unsafe fn pixel_buffer(pixmap: &Pixmap) -> Result<CFRetained<CVPixelBuffer>> {
        let (width, height) = (pixmap.width() as usize, pixmap.height() as usize);
        let mut raw: *mut CVPixelBuffer = std::ptr::null_mut();
        let status = CVPixelBufferCreate(
            None,
            width,
            height,
            kCVPixelFormatType_32BGRA,
            None,
            NonNull::from(&mut raw),
        );
        let raw = NonNull::new(raw)
            .filter(|_| status == kCVReturnSuccess)
            .ok_or_else(|| MotionError::Encode(format!("no pixel buffer (CoreVideo {status})")))?;
        let buffer = CFRetained::from_raw(raw);
        if CVPixelBufferLockBaseAddress(&buffer, CVPixelBufferLockFlags(0)) != kCVReturnSuccess {
            return Err(encode_error("could not lock a pixel buffer"));
        }
        let base = CVPixelBufferGetBaseAddress(&buffer).cast::<u8>();
        let stride = CVPixelBufferGetBytesPerRow(&buffer);
        if !base.is_null() && stride >= width * 4 {
            let src = pixmap.data();
            for y in 0..height {
                let row = std::slice::from_raw_parts_mut(base.add(y * stride), width * 4);
                let from = &src[y * width * 4..(y + 1) * width * 4];
                for (dst, px) in row
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .zip(from.as_chunks::<4>().0)
                {
                    *dst = [px[2], px[1], px[0], 255];
                }
            }
        }
        CVPixelBufferUnlockBaseAddress(&buffer, CVPixelBufferLockFlags(0));
        if base.is_null() || stride < width * 4 {
            return Err(encode_error("unusable pixel buffer"));
        }
        Ok(buffer)
    }
}
