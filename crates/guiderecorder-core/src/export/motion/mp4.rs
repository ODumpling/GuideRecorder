//! MP4 (H.264) writer over the motion frames (see the [module
//! docs](super)).
//!
//! Encoded on the device, so nothing beyond the system is needed: with
//! AVFoundation's `AVAssetWriter` on macOS, and with the Media Foundation
//! Sink Writer on Windows (top-down RGB32 frames, which the writer converts
//! for the H.264 encoder). Other platforms, and Windows editions without
//! Media Foundation, get [`MotionError::Unsupported`]. Frames are [`FPS`] a
//! second; a held frame is appended at its first and last tick only. The
//! default frame is [`DEFAULT_SIZE`] pixels on its short side (1920 × 1080
//! when wide).
//!
//! The movie is written into a fresh temporary folder next to `out` and
//! then moved into place, as the other writers do (AVFoundation will not
//! write over a file).

use std::path::Path;

use super::{io_error, MotionOptions, Result};
use crate::export::render::RenderGuide;
use crate::export::report::ExportReport;

/// Frames per second.
pub const FPS: u32 = 30;
/// Default short side of the frame, in pixels.
pub const DEFAULT_SIZE: u32 = 1080;

/// Whether MP4 export works on this platform.
pub const SUPPORTED: bool = cfg!(any(target_os = "macos", windows));

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

#[cfg(not(any(target_os = "macos", windows)))]
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
            "MP4 export needs macOS or Windows; export a GIF instead".into(),
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

#[cfg(windows)]
mod imp {
    use resvg::tiny_skia::Pixmap;
    use windows::core::HSTRING;
    use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
    use windows::Win32::Media::MediaFoundation::{
        IMFMediaBuffer, IMFSinkWriter, MFCreateAttributes, MFCreateMediaType, MFCreateMemoryBuffer,
        MFCreateSample, MFCreateSinkWriterFromURL, MFMediaType_Video, MFShutdown, MFStartup,
        MFTranscodeContainerType_MPEG4, MFVideoFormat_H264, MFVideoFormat_RGB32,
        MFVideoInterlace_Progressive, MFSTARTUP_FULL, MF_MT_AVG_BITRATE, MF_MT_DEFAULT_STRIDE,
        MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE,
        MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE, MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS,
        MF_TRANSCODE_CONTAINERTYPE, MF_VERSION,
    };
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

    use super::*;
    use crate::export::motion::{render, MotionError};
    use crate::export::persist;

    /// Media Foundation times are in 100-nanosecond units.
    const UNITS_PER_SECOND: i64 = 10_000_000;

    fn encode_error(what: &str) -> MotionError {
        MotionError::Encode(what.into())
    }

    /// Maps a failed Media Foundation call, saying what was being done.
    fn mf_error(what: &'static str) -> impl FnOnce(windows::core::Error) -> MotionError {
        move |e| MotionError::Encode(format!("{what}: {}", e.message()))
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
        {
            let _session = Session::start()?;
            let mut encoder = Encoder::new(&tmp, size)?;
            render(guide, opts, size, FPS, &mut report, &mut |pixmap, ticks| {
                encoder.push(pixmap, ticks)
            })?;
            encoder.finish()?;
        }
        // Windows flushes only a handle open for writing.
        std::fs::OpenOptions::new()
            .write(true)
            .open(&tmp)?
            .sync_all()?;
        persist::persist(
            tempfile::TempPath::try_from_path(&tmp)?,
            out,
            opts.overwrite,
        )
        .map_err(|e| io_error(e, out))?;
        report.files.push(out.to_path_buf());
        Ok(report)
    }

    /// COM and Media Foundation, started on this thread while it lives.
    /// Encoders are dropped before it.
    struct Session {
        /// Whether `start` initialised COM, and so drop uninitialises it.
        com: bool,
        /// Keeps the session on the thread that started it.
        _thread: std::marker::PhantomData<*const ()>,
    }

    impl Session {
        fn start() -> Result<Self> {
            // SAFETY: every successful CoInitializeEx (S_OK, or S_FALSE when
            // COM is already up in this apartment) is paired with the
            // CoUninitialize in drop; a thread already in another apartment
            // (RPC_E_CHANGED_MODE) is left as it is, which Media Foundation
            // works in too.
            unsafe {
                let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
                if hr.is_err() && hr != RPC_E_CHANGED_MODE {
                    return Err(MotionError::Encode(format!(
                        "could not start COM: {}",
                        hr.message()
                    )));
                }
                let com = hr.is_ok();
                // The binaries delay-load these: without them (Windows N
                // without the Media Feature Pack) the first call would end
                // the process.
                let missing = ["mfplat.dll", "mfreadwrite.dll"]
                    .into_iter()
                    .find(|dll| !system_library(dll));
                if let Some(dll) = missing {
                    if com {
                        CoUninitialize();
                    }
                    return Err(MotionError::Unsupported(format!(
                        "MP4 export needs Windows Media Foundation, which is not installed \
                         ({dll} is missing); install the Media Feature Pack, or export a GIF \
                         instead"
                    )));
                }
                if let Err(e) = MFStartup(MF_VERSION, MFSTARTUP_FULL) {
                    if com {
                        CoUninitialize();
                    }
                    return Err(MotionError::Unsupported(format!(
                        "MP4 export needs Windows Media Foundation, which is not available \
                         here ({}); install the Media Feature Pack, or export a GIF instead",
                        e.message()
                    )));
                }
                Ok(Self {
                    com,
                    _thread: std::marker::PhantomData,
                })
            }
        }
    }

    /// Whether `name` loads from the system folder. It stays loaded, which
    /// the delay-loaded calls then use.
    fn system_library(name: &str) -> bool {
        use windows::core::HSTRING;
        use windows::Win32::System::LibraryLoader::{LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};
        // SAFETY: a NUL-terminated name; only System32 is searched.
        unsafe { LoadLibraryExW(&HSTRING::from(name), None, LOAD_LIBRARY_SEARCH_SYSTEM32) }.is_ok()
    }

    impl Drop for Session {
        fn drop(&mut self) {
            // SAFETY: pairs the successful MFStartup and CoInitializeEx of
            // `start`, on the same thread (a session is not Send).
            unsafe {
                let _ = MFShutdown();
                if self.com {
                    CoUninitialize();
                }
            }
        }
    }

    /// An H.264 MP4 being written by the Media Foundation Sink Writer, from
    /// top-down RGB32 frames the writer converts for the encoder.
    struct Encoder {
        writer: IMFSinkWriter,
        stream: u32,
        size: (u32, u32),
        /// Frame intervals written so far.
        tick: i64,
    }

    impl Encoder {
        fn new(path: &Path, (width, height): (u32, u32)) -> Result<Self> {
            let frame_size = (u64::from(width) << 32) | u64::from(height);
            let frame_rate = (u64::from(FPS) << 32) | 1;
            let square = (1 << 32) | 1;
            let progressive = MFVideoInterlace_Progressive.0 as u32;
            // SAFETY: the attribute values are the types Media Foundation
            // documents for their keys, the GUID pointers are to statics
            // that outlive each call, and the writer is set up in its
            // documented order (output stream, input type, BeginWriting).
            unsafe {
                let mut attributes = None;
                MFCreateAttributes(&mut attributes, 2).map_err(mf_error("writer settings"))?;
                let attributes = attributes.ok_or_else(|| encode_error("no writer settings"))?;
                attributes
                    .SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)
                    .and_then(|()| {
                        attributes
                            .SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4)
                    })
                    .map_err(mf_error("writer settings"))?;
                let writer = MFCreateSinkWriterFromURL(&HSTRING::from(path), None, &attributes)
                    .map_err(mf_error("could not create the video file"))?;

                let output = MFCreateMediaType().map_err(mf_error("H.264 settings"))?;
                output
                    .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                    .and_then(|()| output.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264))
                    .and_then(|()| {
                        output.SetUINT32(&MF_MT_AVG_BITRATE, width.saturating_mul(height) * 3)
                    })
                    .and_then(|()| output.SetUINT32(&MF_MT_INTERLACE_MODE, progressive))
                    .and_then(|()| output.SetUINT64(&MF_MT_FRAME_SIZE, frame_size))
                    .and_then(|()| output.SetUINT64(&MF_MT_FRAME_RATE, frame_rate))
                    .and_then(|()| output.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, square))
                    .map_err(mf_error("H.264 settings"))?;
                let stream = writer
                    .AddStream(&output)
                    .map_err(mf_error("the writer refused the H.264 settings"))?;

                let input = MFCreateMediaType().map_err(mf_error("frame format"))?;
                input
                    .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                    .and_then(|()| input.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32))
                    .and_then(|()| input.SetUINT32(&MF_MT_INTERLACE_MODE, progressive))
                    // A positive stride: rows run top-down, as in a pixmap
                    // (RGB32 is otherwise taken as bottom-up).
                    .and_then(|()| input.SetUINT32(&MF_MT_DEFAULT_STRIDE, width * 4))
                    .and_then(|()| input.SetUINT64(&MF_MT_FRAME_SIZE, frame_size))
                    .and_then(|()| input.SetUINT64(&MF_MT_FRAME_RATE, frame_rate))
                    .and_then(|()| input.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, square))
                    .map_err(mf_error("frame format"))?;
                writer
                    .SetInputMediaType(stream, &input, None)
                    .map_err(mf_error("no H.264 encoder takes these frames"))?;
                writer
                    .BeginWriting()
                    .map_err(mf_error("could not start writing"))?;
                Ok(Self {
                    writer,
                    stream,
                    size: (width, height),
                    tick: 0,
                })
            }
        }

        /// Appends `pixmap`, shown for `ticks` frame intervals.
        fn push(&mut self, pixmap: &Pixmap, ticks: u32) -> Result<()> {
            if (pixmap.width(), pixmap.height()) != self.size {
                return Err(encode_error("a frame has the wrong size"));
            }
            let buffer = frame_buffer(pixmap)?;
            // A held frame goes in again at its last tick, or the movie's
            // final frame would last a single tick; the first copy lasts
            // until the second.
            let end = self.tick + i64::from(ticks);
            let last = end - 1;
            let spans = if last > self.tick {
                vec![(self.tick, last), (last, end)]
            } else {
                vec![(self.tick, end)]
            };
            for (from, to) in spans {
                // SAFETY: each sample takes its own reference to the buffer,
                // which no longer changes once filled.
                unsafe {
                    let sample = MFCreateSample().map_err(mf_error("no video sample"))?;
                    sample
                        .AddBuffer(&buffer)
                        .and_then(|()| sample.SetSampleTime(time(from)))
                        .and_then(|()| sample.SetSampleDuration(time(to) - time(from)))
                        .map_err(mf_error("no video sample"))?;
                    self.writer
                        .WriteSample(self.stream, &sample)
                        .map_err(mf_error("could not write a frame"))?;
                }
            }
            self.tick = end;
            Ok(())
        }

        /// Finishes the file after the last frame.
        fn finish(self) -> Result<()> {
            // SAFETY: called once, after every sample was written.
            unsafe { self.writer.Finalize() }.map_err(mf_error("could not finish the video"))
        }
    }

    /// The start of frame interval `tick`, in 100-nanosecond units.
    fn time(tick: i64) -> i64 {
        tick * UNITS_PER_SECOND / i64::from(FPS)
    }

    /// `pixmap` (opaque RGBA) as a top-down RGB32 (BGRX) buffer.
    fn frame_buffer(pixmap: &Pixmap) -> Result<IMFMediaBuffer> {
        let src = pixmap.data();
        let len = u32::try_from(src.len()).map_err(|_| encode_error("frame size out of range"))?;
        // SAFETY: the buffer holds at least `len` bytes while locked, and
        // is unlocked before anything else uses it.
        unsafe {
            let buffer = MFCreateMemoryBuffer(len).map_err(mf_error("no frame buffer"))?;
            let mut base = std::ptr::null_mut();
            let mut max = 0;
            buffer
                .Lock(&mut base, Some(&mut max), None)
                .map_err(mf_error("could not lock a frame buffer"))?;
            if base.is_null() || max < len {
                let _ = buffer.Unlock();
                return Err(encode_error("unusable frame buffer"));
            }
            let dst = std::slice::from_raw_parts_mut(base, src.len());
            for (dst, px) in dst
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(src.as_chunks::<4>().0)
            {
                *dst = [px[2], px[1], px[0], 255];
            }
            buffer
                .Unlock()
                .and_then(|()| buffer.SetCurrentLength(len))
                .map_err(mf_error("frame buffer"))?;
            Ok(buffer)
        }
    }

    #[cfg(test)]
    mod tests {
        use resvg::tiny_skia::{Color, Paint, Rect, Transform};
        use windows::Win32::Media::MediaFoundation::{
            IMFSourceReader, MFCreateSourceReaderFromURL, MF_SOURCE_READERF_ENDOFSTREAM,
            MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, MF_SOURCE_READER_FIRST_VIDEO_STREAM,
        };

        use super::*;

        /// Not a multiple of 16 high, so the encoder pads the coded frame.
        const SIZE: (u32, u32) = (160, 90);

        /// White, with a red block top left and a blue one bottom left.
        fn marked() -> Pixmap {
            let mut pixmap = Pixmap::new(SIZE.0, SIZE.1).unwrap();
            pixmap.fill(Color::WHITE);
            let mut paint = Paint::default();
            paint.set_color_rgba8(255, 0, 0, 255);
            let rect = Rect::from_xywh(0.0, 0.0, 48.0, 32.0).unwrap();
            pixmap.fill_rect(rect, &paint, Transform::identity(), None);
            paint.set_color_rgba8(0, 0, 255, 255);
            let rect = Rect::from_xywh(0.0, 58.0, 48.0, 32.0).unwrap();
            pixmap.fill_rect(rect, &paint, Transform::identity(), None);
            pixmap
        }

        fn plain(r: u8, g: u8, b: u8) -> Pixmap {
            let mut pixmap = Pixmap::new(SIZE.0, SIZE.1).unwrap();
            pixmap.fill(Color::from_rgba8(r, g, b, 255));
            pixmap
        }

        /// A decoded frame: its time and duration, and RGB pixels top-down.
        struct Decoded {
            time: i64,
            duration: i64,
            rgb: Vec<[u8; 3]>,
        }

        /// Decodes every frame of `path` to RGB, with the frame size.
        unsafe fn decode(path: &Path) -> ((u32, u32), Vec<Decoded>) {
            let mut attributes = None;
            MFCreateAttributes(&mut attributes, 1).unwrap();
            let attributes = attributes.unwrap();
            attributes
                .SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)
                .unwrap();
            let reader: IMFSourceReader =
                MFCreateSourceReaderFromURL(&HSTRING::from(path), &attributes).unwrap();
            let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
            let rgb = MFCreateMediaType().unwrap();
            rgb.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).unwrap();
            rgb.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32).unwrap();
            reader.SetCurrentMediaType(stream, None, &rgb).unwrap();
            let current = reader.GetCurrentMediaType(stream).unwrap();
            let packed = current.GetUINT64(&MF_MT_FRAME_SIZE).unwrap();
            let (width, height) = ((packed >> 32) as u32, packed as u32);
            let stride = current.GetUINT32(&MF_MT_DEFAULT_STRIDE).unwrap() as i32;

            let mut frames = Vec::new();
            loop {
                let mut flags = 0;
                let mut sample = None;
                reader
                    .ReadSample(stream, 0, None, Some(&mut flags), None, Some(&mut sample))
                    .unwrap();
                if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                    break;
                }
                let Some(sample) = sample else { continue };
                let buffer = sample.ConvertToContiguousBuffer().unwrap();
                let mut base = std::ptr::null_mut();
                let mut len = 0;
                buffer.Lock(&mut base, None, Some(&mut len)).unwrap();
                let bytes = std::slice::from_raw_parts(base, len as usize);
                let mut rgb = Vec::new();
                for y in 0..height as usize {
                    // A negative stride stores the rows bottom-up.
                    let row = if stride < 0 {
                        height as usize - 1 - y
                    } else {
                        y
                    };
                    let row = &bytes[row * stride.unsigned_abs() as usize..];
                    for px in row.as_chunks::<4>().0.iter().take(width as usize) {
                        rgb.push([px[2], px[1], px[0]]);
                    }
                }
                buffer.Unlock().unwrap();
                frames.push(Decoded {
                    time: sample.GetSampleTime().unwrap(),
                    duration: sample.GetSampleDuration().unwrap(),
                    rgb,
                });
            }
            ((width, height), frames)
        }

        fn near(px: [u8; 3], want: [u8; 3]) -> bool {
            px.iter().zip(want).all(|(&a, b)| a.abs_diff(b) < 48)
        }

        #[test]
        fn mp4_frames_read_back_upright_and_timed() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("marked.mp4");
            let session = Session::start().unwrap();
            let mut encoder = Encoder::new(&path, SIZE).unwrap();
            encoder.push(&marked(), 15).unwrap();
            encoder.push(&plain(0, 0, 0), 1).unwrap();
            encoder.push(&plain(0, 160, 0), 14).unwrap();
            encoder.finish().unwrap();

            // SAFETY: Media Foundation is running for `session`.
            let (size, frames) = unsafe { decode(&path) };
            drop(session);
            assert_eq!(size, SIZE);
            assert_eq!(frames[0].time, 0);
            let last = frames.last().unwrap();
            // Within a millisecond: encoders round frame durations.
            let end = last.time + last.duration;
            assert!((end - time(30)).abs() < 10_000, "the movie ends at {end}");

            // The encoder may fill a hold with repeats; whatever it does,
            // each tick shows the frame pushed for it.
            let shown = |tick: i64| {
                let t = time(tick);
                let frame = frames
                    .iter()
                    .find(|f| f.time <= t && t < f.time + f.duration)
                    .unwrap_or_else(|| panic!("nothing shown at tick {tick}"));
                &frame.rgb
            };
            for tick in [0, 14] {
                let rgb = shown(tick);
                let at = |x: u32, y: u32| rgb[(y * SIZE.0 + x) as usize];
                assert!(near(at(8, 8), [255, 0, 0]), "top left {:?}", at(8, 8));
                assert!(near(at(8, 82), [0, 0, 255]), "bottom left {:?}", at(8, 82));
                assert!(
                    near(at(120, 45), [255, 255, 255]),
                    "middle {:?}",
                    at(120, 45)
                );
            }
            assert!(near(shown(15)[0], [0, 0, 0]));
            for tick in [16, 29] {
                assert!(near(shown(tick)[0], [0, 160, 0]));
            }
        }
    }
}
