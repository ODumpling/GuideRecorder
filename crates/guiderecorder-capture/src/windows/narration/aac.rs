//! The narration audio kept (`NarrationConfig::audio`): mono AAC in an
//! `.m4a`, written by Media Foundation's Sink Writer.
//!
//! The AAC encoder takes 16-bit PCM at 44.1 or 48 kHz only, so audio at
//! another rate is resampled to 48 kHz first. The app and the command line
//! tool delay-load Media Foundation (their `build.rs`), so they start on
//! Windows N editions without it; [`AacFile::create`] then says it is
//! missing before any call would end the process, and no audio is kept.

use std::mem::ManuallyDrop;
use std::path::Path;

use windows::core::HSTRING;
use windows::Win32::Media::MediaFoundation::{
    IMFSinkWriter, MFAudioFormat_AAC, MFAudioFormat_PCM, MFCreateAttributes, MFCreateMediaType,
    MFCreateMemoryBuffer, MFCreateSample, MFCreateSinkWriterFromURL, MFMediaType_Audio, MFShutdown,
    MFStartup, MFTranscodeContainerType_MPEG4, MFSTARTUP_LITE, MF_MT_AUDIO_AVG_BYTES_PER_SECOND,
    MF_MT_AUDIO_BITS_PER_SAMPLE, MF_MT_AUDIO_BLOCK_ALIGNMENT, MF_MT_AUDIO_NUM_CHANNELS,
    MF_MT_AUDIO_SAMPLES_PER_SECOND, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_TRANSCODE_CONTAINERTYPE,
    MF_VERSION,
};

use super::pcm::{to_i16, Resampler};

/// Media Foundation times are in 100-nanosecond units.
const UNITS_PER_SECOND: i64 = 10_000_000;
/// The encoder's rate when the microphone's is not one it takes.
const FALLBACK_RATE: u32 = 48_000;
/// 96 kbit/s, the lowest mono rate the encoder offers (bytes per second).
const BYTES_PER_SECOND: u32 = 12_000;

fn mf_error(what: &'static str) -> impl FnOnce(windows::core::Error) -> String {
    move |e| format!("{what}: {}", e.message())
}

/// An `.m4a` being written from mono floats. Lives on one thread with COM
/// up (the capture thread); Media Foundation runs while it does.
pub struct AacFile {
    /// Released in drop, before Media Foundation shuts down.
    writer: ManuallyDrop<IMFSinkWriter>,
    stream: u32,
    rate: u32,
    resampler: Resampler,
    /// Frames written so far, at `rate`.
    frames: u64,
    /// Keeps it on the thread that made it.
    _thread: std::marker::PhantomData<*const ()>,
}

impl AacFile {
    /// Starts Media Foundation and a file at `path` for audio at `rate`.
    pub fn create(path: &Path, rate: u32) -> Result<Self, String> {
        // The binaries delay-load these: without them (Windows N without
        // the Media Feature Pack) the first call would end the process.
        if let Some(dll) = ["mfplat.dll", "mfreadwrite.dll"]
            .into_iter()
            .find(|dll| !system_library(dll))
        {
            return Err(format!(
                "Windows Media Foundation is not installed ({dll} is missing)"
            ));
        }
        // SAFETY: COM is up on this thread; paired with the MFShutdown in
        // drop (or below, on failure).
        unsafe { MFStartup(MF_VERSION, MFSTARTUP_LITE) }.map_err(mf_error("Media Foundation"))?;
        let encoded = if matches!(rate, 44_100 | 48_000) {
            rate
        } else {
            FALLBACK_RATE
        };
        // SAFETY: see `writer`.
        match unsafe { writer(path, encoded) } {
            Ok((writer, stream)) => Ok(Self {
                writer: ManuallyDrop::new(writer),
                stream,
                rate: encoded,
                resampler: Resampler::new(rate, encoded),
                frames: 0,
                _thread: std::marker::PhantomData,
            }),
            Err(e) => {
                // SAFETY: pairs the MFStartup above.
                let _ = unsafe { MFShutdown() };
                Err(e)
            }
        }
    }

    /// Appends mono samples at the rate given to [`AacFile::create`].
    pub fn write(&mut self, samples: &[f32]) -> Result<(), String> {
        let pcm = to_i16(&self.resampler.push(samples));
        if pcm.is_empty() {
            return Ok(());
        }
        let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
        let len = u32::try_from(bytes.len()).map_err(|_| "too much audio at once".to_string())?;
        let time = |frames: u64| (frames as i64) * UNITS_PER_SECOND / i64::from(self.rate);
        let (from, to) = (self.frames, self.frames + pcm.len() as u64);
        // SAFETY: the buffer holds at least `len` bytes while locked and
        // is unlocked before the sample takes it.
        unsafe {
            let buffer = MFCreateMemoryBuffer(len).map_err(mf_error("no audio buffer"))?;
            let mut base = std::ptr::null_mut();
            let mut max = 0;
            buffer
                .Lock(&mut base, Some(&mut max), None)
                .map_err(mf_error("audio buffer"))?;
            if base.is_null() || max < len {
                let _ = buffer.Unlock();
                return Err("unusable audio buffer".into());
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), base, bytes.len());
            buffer
                .Unlock()
                .and_then(|()| buffer.SetCurrentLength(len))
                .map_err(mf_error("audio buffer"))?;
            let sample = MFCreateSample().map_err(mf_error("no audio sample"))?;
            sample
                .AddBuffer(&buffer)
                .and_then(|()| sample.SetSampleTime(time(from)))
                .and_then(|()| sample.SetSampleDuration(time(to) - time(from)))
                .map_err(mf_error("audio sample"))?;
            self.writer
                .WriteSample(self.stream, &sample)
                .map_err(mf_error("could not write the audio"))?;
        }
        self.frames = to;
        Ok(())
    }

    /// Whether any audio went in.
    pub fn is_empty(&self) -> bool {
        self.frames == 0
    }

    /// Finishes the file.
    pub fn finish(self) -> Result<(), String> {
        // SAFETY: called once, after the last sample.
        unsafe { self.writer.Finalize() }.map_err(mf_error("could not finish the audio"))
    }
}

impl Drop for AacFile {
    fn drop(&mut self) {
        // SAFETY: the writer is not used again; then this pairs the
        // MFStartup of `create`, on the same thread (it is not Send).
        unsafe {
            ManuallyDrop::drop(&mut self.writer);
            let _ = MFShutdown();
        }
    }
}

/// A Sink Writer for mono AAC at `rate` in an MPEG-4 file, taking 16-bit
/// PCM at the same rate.
///
/// # Safety
///
/// Media Foundation must be started on this thread.
unsafe fn writer(path: &Path, rate: u32) -> Result<(IMFSinkWriter, u32), String> {
    // SAFETY: the attribute values are the types Media Foundation documents
    // for their keys, the GUID pointers are to statics, and the writer is
    // set up in its documented order (output stream, input type,
    // BeginWriting).
    unsafe {
        let mut attributes = None;
        MFCreateAttributes(&mut attributes, 1).map_err(mf_error("writer settings"))?;
        let attributes = attributes.ok_or("no writer settings")?;
        attributes
            .SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4)
            .map_err(mf_error("writer settings"))?;
        let writer = MFCreateSinkWriterFromURL(&HSTRING::from(path), None, &attributes)
            .map_err(mf_error("could not create the audio file"))?;

        let output = MFCreateMediaType().map_err(mf_error("AAC settings"))?;
        output
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
            .and_then(|()| output.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC))
            .and_then(|()| output.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16))
            .and_then(|()| output.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, rate))
            .and_then(|()| output.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 1))
            .and_then(|()| output.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, BYTES_PER_SECOND))
            .map_err(mf_error("AAC settings"))?;
        let stream = writer
            .AddStream(&output)
            .map_err(mf_error("the writer refused the AAC settings"))?;

        let input = MFCreateMediaType().map_err(mf_error("audio format"))?;
        input
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
            .and_then(|()| input.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM))
            .and_then(|()| input.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16))
            .and_then(|()| input.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, rate))
            .and_then(|()| input.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 1))
            .and_then(|()| input.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 2))
            .and_then(|()| input.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, rate * 2))
            .map_err(mf_error("audio format"))?;
        writer
            .SetInputMediaType(stream, &input, None)
            .map_err(mf_error("no AAC encoder takes this audio"))?;
        writer
            .BeginWriting()
            .map_err(mf_error("could not start writing the audio"))?;
        Ok((writer, stream))
    }
}

/// Whether `name` loads from the system folder. It stays loaded, which
/// the delay-loaded calls then use.
fn system_library(name: &str) -> bool {
    use windows::Win32::System::LibraryLoader::{LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};
    // SAFETY: a NUL-terminated name; only System32 is searched.
    unsafe { LoadLibraryExW(&HSTRING::from(name), None, LOAD_LIBRARY_SEARCH_SYSTEM32) }.is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tone_is_written_as_an_m4a() {
        // SAFETY: this test thread joins the multithreaded apartment.
        let _ = unsafe {
            windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_MULTITHREADED,
            )
        };
        let dir = std::env::temp_dir().join(format!("gr-aac-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for rate in [48_000, 16_000] {
            let path = dir.join(format!("tone-{rate}.m4a"));
            let mut file = AacFile::create(&path, rate).unwrap();
            assert!(file.is_empty());
            let tone: Vec<f32> = (0..rate)
                .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 0.3)
                .collect();
            for chunk in tone.chunks(4_800) {
                file.write(chunk).unwrap();
            }
            assert!(!file.is_empty());
            file.finish().unwrap();
            let bytes = std::fs::read(&path).unwrap();
            // An MPEG-4 file: `ftyp` first.
            assert_eq!(&bytes[4..8], b"ftyp", "{rate}");
            assert!(bytes.len() > 4_000, "{rate}: {} bytes", bytes.len());
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
