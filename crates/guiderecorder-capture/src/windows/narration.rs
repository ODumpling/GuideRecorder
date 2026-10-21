//! Narration with WASAPI and SAPI 5 desktop dictation (see
//! [`crate::narration`]).
//!
//! - The default microphone is captured in shared mode on a thread of its
//!   own, in packets of about 10 ms the device signals. While listening,
//!   each packet is made mono ([`pcm`]), resampled to 16 kHz for the
//!   recogniser and, when asked, written to an AAC file ([`aac`]) at the
//!   microphone's own rate.
//! - Recognition is SAPI's in-process desktop recogniser with its
//!   dictation grammar ([`sapi`]): the one installed with the user's
//!   language's speech pack, running on this device. A user without one
//!   is refused at start. Windows.Media.SpeechRecognition is not used: its
//!   dictation sends the audio to Microsoft's online service.
//! - The recogniser reads the audio live from a queue that its reads wait
//!   on, and times words in seconds of audio fed so far ([`AudioClock`]).
//!   [`Narrator::stop`] ends the queue, and the recogniser finishes the
//!   last phrase.
//! - Each stretch of listening is marked on the clock with the time
//!   WASAPI gives its first packet: the performance counter, as events
//!   and frames are timed ([`event_tap::now`]).
//! - Dictation adds no punctuation of its own: only what is spoken
//!   ("comma", "full stop").

pub(crate) mod aac;
pub(crate) mod pcm;
pub(crate) mod sapi;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use guiderecorder_core::events::Timestamp;
use windows::Win32::Foundation::{CloseHandle, E_ACCESSDENIED, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::{
    eCapture, eConsole, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator,
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

use self::aac::AacFile;
use self::pcm::{to_i16, to_mono, MixFormat, Resampler};
use self::sapi::Recogniser;
use crate::narration::{AudioClock, Narration, NarrationConfig, NarrationError, SpokenWord};
use crate::permissions::{self, Permission};
use crate::windows::event_tap;

/// How long [`Narrator::stop`] waits for the last words.
const FINISH_WAIT: Duration = Duration::from_secs(20);
/// How long opening the microphone may take.
const START_WAIT: Duration = Duration::from_secs(10);
/// The shared-mode buffer asked for, in 100-nanosecond units (0.1 s).
const BUFFER_UNITS: i64 = 1_000_000;
/// How long the capture thread waits for a packet before looking for a
/// stop.
const PACKET_WAIT_MS: u32 = 200;
/// `WAVE_FORMAT_IEEE_FLOAT` and `WAVE_FORMAT_EXTENSIBLE`.
const FORMAT_FLOAT: u16 = 3;
const FORMAT_EXTENSIBLE: u16 = 0xfffe;

pub struct Narrator {
    shared: Arc<Shared>,
    /// The capture thread; gives the audio file written, if any.
    capture: Option<JoinHandle<Option<PathBuf>>>,
    recogniser: Option<Recogniser>,
}

struct Shared {
    state: Mutex<State>,
    /// Asks the capture thread to stop.
    quit: AtomicBool,
}

struct State {
    listening: bool,
    /// The next packet starts a stretch of listening: mark the clock.
    gap: bool,
    /// Samples fed to the recogniser so far, at [`sapi::RATE`].
    fed: u64,
    clock: AudioClock,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Narrator {
    pub fn start(config: NarrationConfig) -> Result<Self, NarrationError> {
        if !permissions::check(Permission::Microphone).is_granted() {
            return Err(NarrationError::NotAllowed(Permission::Microphone));
        }
        let token = sapi::for_user()?;
        let recogniser = Recogniser::start(&token)?;
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                listening: true,
                gap: true,
                fed: 0,
                clock: AudioClock::default(),
            }),
            quit: AtomicBool::new(false),
        });
        let (ready_tx, ready) = mpsc::channel();
        let capture = {
            let (shared, feed) = (shared.clone(), recogniser.feed().clone());
            std::thread::Builder::new()
                .name("guiderecorder-microphone".into())
                .spawn(move || capture(&shared, &feed, config.audio, ready_tx))
                .map_err(|e| NarrationError::Failed(e.to_string()))?
        };
        match ready.recv_timeout(START_WAIT) {
            Ok(Ok(())) => Ok(Self {
                shared,
                capture: Some(capture),
                recogniser: Some(recogniser),
            }),
            Ok(Err(e)) => {
                let _ = capture.join();
                Err(e)
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Left to stop by itself once the device answers.
                shared.quit.store(true, Ordering::Relaxed);
                Err(NarrationError::Failed(
                    "the microphone did not start in time".into(),
                ))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = capture.join();
                Err(NarrationError::Failed("the microphone stopped".into()))
            }
        }
    }

    pub fn set_listening(&self, listening: bool) {
        let mut state = self.shared.lock();
        if listening && !state.listening {
            state.gap = true;
        }
        state.listening = listening;
    }

    /// Stops the microphone; gives the audio file written, if any.
    fn halt(&mut self) -> Option<PathBuf> {
        self.shared.quit.store(true, Ordering::Relaxed);
        self.capture.take().and_then(|t| t.join().ok()).flatten()
    }

    pub fn stop(mut self) -> Result<Narration, NarrationError> {
        let audio = self.halt();
        let heard = match self.recogniser.take() {
            Some(recogniser) => recogniser.finish(FINISH_WAIT),
            None => Vec::new(),
        };
        let state = self.shared.lock();
        let mut words: Vec<SpokenWord> = heard
            .into_iter()
            .map(|(secs, duration, text)| state.clock.word(secs, duration, text))
            .collect();
        words.sort_by_key(|w| w.start);
        Ok(Narration { words, audio })
    }
}

impl Drop for Narrator {
    /// Discards what was heard: stops recognising and removes the audio.
    fn drop(&mut self) {
        if let Some(path) = self.halt() {
            let _ = std::fs::remove_file(path);
        }
        // Dropping the recogniser cancels it.
        self.recogniser = None;
    }
}

/// The capture thread: opens the microphone, says whether that worked,
/// then hears it until asked to stop. Returns the audio file written, if
/// any audio went in.
fn capture(
    shared: &Shared,
    feed: &sapi::Feed,
    audio: Option<PathBuf>,
    ready: mpsc::Sender<Result<(), NarrationError>>,
) -> Option<PathBuf> {
    // SAFETY: a thread of our own joins the multithreaded apartment, left
    // at its end below.
    if let Err(e) = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok() {
        let _ = ready.send(Err(NarrationError::Failed(e.message())));
        return None;
    }
    let written = {
        // SAFETY: COM is up on this thread, which owns the microphone.
        match unsafe { Microphone::open() } {
            Ok(mic) => {
                let _ = ready.send(Ok(()));
                hear(shared, feed, &mic, audio)
            }
            Err(e) => {
                let _ = ready.send(Err(e));
                None
            }
        }
    };
    // SAFETY: pairs the CoInitializeEx above, after the microphone and the
    // audio file are released.
    unsafe { CoUninitialize() };
    written
}

/// Hears `mic` until asked to stop: feeds the recogniser and writes the
/// audio file while listening.
fn hear(
    shared: &Shared,
    feed: &sapi::Feed,
    mic: &Microphone,
    audio: Option<PathBuf>,
) -> Option<PathBuf> {
    let mut file = audio.as_ref().and_then(|path| {
        AacFile::create(path, mic.format.rate)
            .inspect_err(|e| eprintln!("guiderecorder: not keeping narration audio: {e}"))
            .ok()
    });
    let mut to_feed = Resampler::new(mic.format.rate, sapi::RATE);
    let rate = f64::from(mic.format.rate);
    while !shared.quit.load(Ordering::Relaxed) {
        // SAFETY: the microphone is live on this thread.
        let packets = unsafe {
            mic.next(&mut |mono, at| {
                let mut state = shared.lock();
                if !state.listening {
                    return;
                }
                if state.gap {
                    // Without a time from the device: it ends about now.
                    let at = at.unwrap_or_else(|| {
                        let secs = mono.len() as f64 / rate;
                        Timestamp(event_tap::now().0.saturating_sub((secs * 1e9) as u64))
                    });
                    let fed = state.fed as f64 / f64::from(sapi::RATE);
                    state.clock.mark(fed, at);
                    state.gap = false;
                }
                let pcm = to_i16(&to_feed.push(mono));
                state.fed += pcm.len() as u64;
                drop(state);
                feed.push(&pcm);
                if let Some(f) = &mut file {
                    if let Err(e) = f.write(mono) {
                        eprintln!("guiderecorder: narration audio stopped: {e}");
                        file = None;
                    }
                }
            })
        };
        if let Err(e) = packets {
            // Unplugged, most likely: nothing more is heard.
            eprintln!(
                "guiderecorder: narration: the microphone stopped: {}",
                e.message()
            );
            break;
        }
    }
    let path = audio?;
    let kept = match file {
        Some(file) if !file.is_empty() => file
            .finish()
            .inspect_err(|e| eprintln!("guiderecorder: narration audio: {e}"))
            .is_ok(),
        _ => false,
    };
    if kept {
        Some(path)
    } else {
        let _ = std::fs::remove_file(&path);
        None
    }
}

/// The default microphone, open in shared mode, signalling each packet.
struct Microphone {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    /// Signalled when a packet is ready.
    event: HANDLE,
    format: MixFormat,
}

impl Microphone {
    /// Opens and starts the default capture device.
    ///
    /// # Safety
    ///
    /// COM must be up on this thread; the microphone stays on it.
    unsafe fn open() -> Result<Self, NarrationError> {
        let failed = |e: windows::core::Error| {
            if e.code() == E_ACCESSDENIED {
                NarrationError::NotAllowed(Permission::Microphone)
            } else {
                NarrationError::Failed(format!("the microphone could not start: {}", e.message()))
            }
        };
        // SAFETY (whole body): WASAPI's documented set-up order (device,
        // client, format, Initialize, event, capture service, Start); the
        // mix format is read before it is freed, and the event is closed
        // on every path that does not keep it.
        unsafe {
            let devices: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(failed)?;
            let device = devices
                .GetDefaultAudioEndpoint(eCapture, eConsole)
                .map_err(|_| NarrationError::NoMicrophone)?;
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None).map_err(failed)?;
            let mix = client.GetMixFormat().map_err(failed)?;
            let format = mix_format(mix);
            let init = client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                BUFFER_UNITS,
                0,
                mix,
                None,
            );
            CoTaskMemFree(Some(mix as *const _));
            init.map_err(failed)?;
            let Some(format) = format.filter(MixFormat::is_readable) else {
                return Err(NarrationError::Failed(
                    "the microphone's audio format is not one narration reads".into(),
                ));
            };
            let event = CreateEventW(None, false, false, None).map_err(failed)?;
            let mic = Self {
                capture: match client
                    .SetEventHandle(event)
                    .and_then(|()| client.GetService::<IAudioCaptureClient>())
                {
                    Ok(capture) => capture,
                    Err(e) => {
                        let _ = CloseHandle(event);
                        return Err(failed(e));
                    }
                },
                client,
                event,
                format,
            };
            // Dropping `mic` on failure closes the event.
            mic.client.Start().map_err(failed)?;
            Ok(mic)
        }
    }

    /// Waits a little for packets and gives each to `heard` as mono
    /// samples with the event time of its first, when the device gave it.
    ///
    /// # Safety
    ///
    /// On the thread that opened it.
    unsafe fn next(
        &self,
        heard: &mut dyn FnMut(&[f32], Option<Timestamp>),
    ) -> windows::core::Result<()> {
        // SAFETY (whole body): the event and clients are live; each buffer
        // got is read for the frames it holds and released once.
        unsafe {
            if WaitForSingleObject(self.event, PACKET_WAIT_MS) != WAIT_OBJECT_0 {
                return Ok(());
            }
            loop {
                if self.capture.GetNextPacketSize()? == 0 {
                    return Ok(());
                }
                let mut data = std::ptr::null_mut();
                let mut frames = 0;
                let mut flags = 0;
                let mut qpc = 0u64;
                self.capture
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, Some(&mut qpc))?;
                let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
                let timed = flags & AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR.0 as u32 == 0 && qpc > 0;
                let mono = if silent || data.is_null() {
                    vec![0.0; frames as usize]
                } else {
                    let len = frames as usize * self.format.frame_bytes();
                    to_mono(std::slice::from_raw_parts(data, len), &self.format)
                };
                self.capture.ReleaseBuffer(frames)?;
                if !mono.is_empty() {
                    // The counter in 100-nanosecond units.
                    heard(&mono, timed.then(|| Timestamp(qpc.saturating_mul(100))));
                }
            }
        }
    }
}

impl Drop for Microphone {
    fn drop(&mut self) {
        // SAFETY: stopping our own stream and closing our own event.
        unsafe {
            let _ = self.client.Stop();
            let _ = CloseHandle(self.event);
        }
    }
}

/// The layout of a WASAPI mix format, when it is PCM or float.
///
/// # Safety
///
/// `format` points to a valid `WAVEFORMATEX`, extended when its tag says
/// so.
unsafe fn mix_format(format: *const WAVEFORMATEX) -> Option<MixFormat> {
    // SAFETY: as the caller promises; the structs are packed, so read
    // unaligned.
    let base = unsafe { std::ptr::read_unaligned(format) };
    let (tag, bits) = match base.wFormatTag {
        FORMAT_EXTENSIBLE if base.cbSize >= 22 => {
            // SAFETY: as above.
            let ext = unsafe { std::ptr::read_unaligned(format.cast::<WAVEFORMATEXTENSIBLE>()) };
            // The sub-format GUID's first field is the plain tag.
            let sub = ext.SubFormat;
            (u16::try_from(sub.data1).ok()?, base.wBitsPerSample)
        }
        tag => (tag, base.wBitsPerSample),
    };
    let float = match tag {
        FORMAT_FLOAT => true,
        t if u32::from(t) == windows::Win32::Media::Audio::WAVE_FORMAT_PCM => false,
        _ => return None,
    };
    Some(MixFormat {
        rate: base.nSamplesPerSec,
        channels: base.nChannels,
        // The container size: 24 valid bits may sit in 32.
        bytes: (base.nBlockAlign / base.nChannels.max(1)).max(bits / 8),
        float,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mix_formats_are_read_plain_and_extended() {
        let plain = WAVEFORMATEX {
            wFormatTag: FORMAT_FLOAT,
            nChannels: 2,
            nSamplesPerSec: 48_000,
            nAvgBytesPerSec: 384_000,
            nBlockAlign: 8,
            wBitsPerSample: 32,
            cbSize: 0,
        };
        // SAFETY: a valid format.
        let read = unsafe { mix_format(&plain) }.unwrap();
        assert_eq!(
            read,
            MixFormat {
                rate: 48_000,
                channels: 2,
                bytes: 4,
                float: true
            }
        );
        let mut ext = WAVEFORMATEXTENSIBLE {
            Format: WAVEFORMATEX {
                wFormatTag: FORMAT_EXTENSIBLE,
                nBlockAlign: 4,
                wBitsPerSample: 32,
                cbSize: 22,
                ..plain
            },
            // KSDATAFORMAT_SUBTYPE_PCM.
            SubFormat: windows::core::GUID::from_u128(0x00000001_0000_0010_8000_00aa00389b71),
            ..Default::default()
        };
        ext.Format.nChannels = 1;
        // SAFETY: a valid extended format.
        let read = unsafe { mix_format(std::ptr::addr_of!(ext).cast()) }.unwrap();
        assert_eq!((read.bytes, read.float, read.channels), (4, false, 1));
        // Anything else is not read.
        let other = WAVEFORMATEX {
            wFormatTag: 0x55,
            ..plain
        };
        // SAFETY: a valid format.
        assert_eq!(unsafe { mix_format(&other) }, None);
    }

    #[test]
    fn device_times_are_on_the_event_clock() {
        // WASAPI gives the performance counter in 100 ns units: the same
        // clock as `event_tap::now`, which counts it in nanoseconds.
        let mut ticks = 0i64;
        let mut frequency = 0i64;
        // SAFETY: valid out-pointers.
        unsafe {
            windows::Win32::System::Performance::QueryPerformanceFrequency(&mut frequency).unwrap();
            windows::Win32::System::Performance::QueryPerformanceCounter(&mut ticks).unwrap();
        }
        let units = (ticks as u128 * 10_000_000 / frequency as u128) as u64;
        let now = event_tap::now();
        let device = Timestamp(units.saturating_mul(100));
        assert!(device <= now);
        assert!(now.0 - device.0 < 50_000_000, "{device:?} vs {now:?}");
    }

    /// Hears the real microphone for a few seconds, muted for one: say
    /// something. Needs a microphone and the permission.
    #[test]
    #[ignore = "uses the microphone"]
    fn the_microphone_is_heard_and_timed() {
        let dir = std::env::temp_dir().join(format!("gr-mic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let audio = dir.join("narration.m4a");
        let before = event_tap::now();
        let narrator = Narrator::start(NarrationConfig {
            audio: Some(audio.clone()),
        })
        .unwrap();
        std::thread::sleep(Duration::from_secs(3));
        narrator.set_listening(false);
        std::thread::sleep(Duration::from_secs(1));
        narrator.set_listening(true);
        std::thread::sleep(Duration::from_secs(3));
        let stopping = event_tap::now();
        let narration = narrator.stop().unwrap();
        eprintln!("heard: {:?}", narration.words);
        for word in &narration.words {
            assert!(word.start >= before && word.end <= stopping, "{word:?}");
        }
        assert_eq!(narration.audio.as_deref(), Some(audio.as_path()));
        assert!(std::fs::metadata(&audio).unwrap().len() > 1_000);
        let _ = std::fs::remove_dir_all(dir);
    }
}
