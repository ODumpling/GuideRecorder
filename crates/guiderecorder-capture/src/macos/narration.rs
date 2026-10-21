//! Narration with AVAudioEngine and the Speech framework (see
//! [`crate::narration`]).
//!
//! - A tap on the engine's input node gets the microphone in buffers of
//!   about 0.1 s on an audio thread. While listening, each buffer goes to
//!   the current recognition request and, when asked, to an AAC file.
//! - Recognition is `SFSpeechAudioBufferRecognitionRequest` with
//!   `requiresOnDeviceRecognition`: a locale whose model is not on the
//!   device is refused at start, never sent to Apple's servers.
//! - Only final results carry word timings, so a request is ended and a
//!   new one begun every [`REQUEST_SECS`] of audio; the ended one
//!   finishes in the background. A word spoken right at that moment may be
//!   lost or split. A request the recogniser ends early (it may, after a
//!   long silence) is replaced when audio next comes.
//! - Result handlers run on our own operation queue, not the main thread
//!   (which is busy, or absent in tests).
//! - Word times are seconds of audio fed so far ([`AudioClock`]): each
//!   request starts counting at the audio fed before it.

use std::path::PathBuf;
use std::ptr::NonNull;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol};
use objc2::{sel, AllocAnyThread};
use objc2_avf_audio::{
    AVAudioEngine, AVAudioFile, AVAudioPCMBuffer, AVAudioTime, AVEncoderBitRateKey, AVFormatIDKey,
    AVNumberOfChannelsKey, AVSampleRateKey,
};
use objc2_foundation::{
    NSDictionary, NSError, NSNumber, NSOperationQueue, NSRange, NSString, NSURL,
};
use objc2_speech::{
    SFSpeechAudioBufferRecognitionRequest, SFSpeechRecognitionResult, SFSpeechRecognitionTask,
    SFSpeechRecognitionTaskHint, SFSpeechRecognizer, SFTranscription,
};

use crate::macos::event_tap;
use crate::narration::{AudioClock, Narration, NarrationConfig, NarrationError, SpokenWord};
use crate::permissions::{self, Permission};

/// Audio given to one recognition request before it is ended and the
/// next begun.
const REQUEST_SECS: f64 = 60.0;
/// Frames asked of the tap at a time (it may give a different count).
const TAP_FRAMES: u32 = 4096;
/// How long [`Narrator::stop`] waits for the last results.
const FINISH_WAIT: Duration = Duration::from_secs(20);
/// Requests in a row that may end in an error with no words before
/// narration gives up recognising (audio is still written).
const MAX_FAILURES: u32 = 3;
/// `kAudioFormatMPEG4AAC`.
const AAC: u32 = 0x6161_6320;
/// AAC bit rate per channel.
const BIT_RATE: u32 = 64_000;

pub struct Narrator {
    engine: Retained<AVAudioEngine>,
    shared: Arc<Shared>,
    stopped: bool,
}

// SAFETY: AVAudioEngine may be started and stopped from any thread; the
// narrator only does that, from whichever thread owns it.
unsafe impl Send for Narrator {}

struct Shared {
    state: Mutex<State>,
    /// Signalled whenever a request finishes.
    finished: Condvar,
    recognizer: Retained<SFSpeechRecognizer>,
    /// Where result handlers run; kept alive here.
    _queue: Retained<NSOperationQueue>,
}

// SAFETY: SFSpeechRecognizer starts tasks from any thread (handlers run on
// `_queue`); every other Objective-C object is in `state`, so it is only
// touched under the mutex. Appending to a buffer request from the audio
// thread is what it is made for.
unsafe impl Send for Shared {}
// SAFETY: as above.
unsafe impl Sync for Shared {}

struct State {
    listening: bool,
    stopping: bool,
    /// Seconds of audio fed so far.
    fed: f64,
    /// The next buffer starts a stretch of listening: mark the clock.
    gap: bool,
    clock: AudioClock,
    /// The request being fed, if any.
    current: Option<Live>,
    /// Ended requests still recognising.
    pending: Vec<Live>,
    next_id: u64,
    /// Words from final results: (audio seconds, duration, text).
    words: Vec<(f64, f64, String)>,
    failures: u32,
    file: Option<Retained<AVAudioFile>>,
    audio: Option<PathBuf>,
    wrote_audio: bool,
}

/// One recognition request and its task.
struct Live {
    id: u64,
    /// Audio fed before it began: its word times count from here.
    base: f64,
    request: Retained<SFSpeechAudioBufferRecognitionRequest>,
    task: Retained<SFSpeechRecognitionTask>,
}

fn lock(m: &Mutex<State>) -> MutexGuard<'_, State> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn failed(e: &NSError) -> NarrationError {
    NarrationError::Failed(e.localizedDescription().to_string())
}

impl Narrator {
    pub fn start(config: NarrationConfig) -> Result<Self, NarrationError> {
        for permission in Permission::NARRATION {
            if !permissions::check(permission).is_granted() {
                return Err(NarrationError::NotAllowed(permission));
            }
        }
        // SAFETY: `init` uses the user's language (or dictation's), and
        // returns nil when neither can be recognised.
        let recognizer = unsafe { SFSpeechRecognizer::init(SFSpeechRecognizer::alloc()) }
            .ok_or_else(|| NarrationError::NotOnDevice("your language".into()))?;
        // SAFETY: plain property reads.
        let (on_device, available) = unsafe {
            (
                recognizer.supportsOnDeviceRecognition(),
                recognizer.isAvailable(),
            )
        };
        if !on_device {
            // SAFETY: a plain property read.
            let locale = unsafe { recognizer.locale() };
            return Err(NarrationError::NotOnDevice(
                locale.localeIdentifier().to_string(),
            ));
        }
        if !available {
            return Err(NarrationError::Failed(
                "speech recognition is not available right now".into(),
            ));
        }
        let queue = NSOperationQueue::new();
        queue.setMaxConcurrentOperationCount(1);
        // SAFETY: a serial queue of our own; the handlers lock what they
        // touch.
        unsafe { recognizer.setQueue(&queue) };

        // SAFETY: a fresh engine; its input node is the default input.
        let engine = unsafe { AVAudioEngine::new() };
        let input = unsafe { engine.inputNode() };
        // SAFETY: bus 0 is the input node's only output bus.
        let format = unsafe { input.outputFormatForBus(0) };
        // SAFETY: plain property reads.
        let (rate, channels) = unsafe { (format.sampleRate(), format.channelCount()) };
        if rate <= 0.0 || channels == 0 {
            return Err(NarrationError::NoMicrophone);
        }
        let file = match &config.audio {
            Some(path) => match audio_file(path, rate, channels, &format) {
                Ok(file) => Some(file),
                Err(e) => {
                    eprintln!("guiderecorder: not keeping narration audio: {e}");
                    None
                }
            },
            None => None,
        };
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                listening: true,
                stopping: false,
                fed: 0.0,
                gap: true,
                clock: AudioClock::default(),
                current: None,
                pending: Vec::new(),
                next_id: 0,
                words: Vec::new(),
                failures: 0,
                audio: file.as_ref().and(config.audio),
                file,
                wrote_audio: false,
            }),
            finished: Condvar::new(),
            recognizer,
            _queue: queue,
        });

        let heard = Arc::downgrade(&shared);
        let tap = RcBlock::new(
            move |buffer: NonNull<AVAudioPCMBuffer>, when: NonNull<AVAudioTime>| {
                if let Some(shared) = heard.upgrade() {
                    // SAFETY: both are valid for the call.
                    shared.heard(unsafe { buffer.as_ref() }, unsafe { when.as_ref() });
                }
            },
        );
        // SAFETY: the block takes the buffer and time the tap passes and is
        // copied by the node; `format` is the bus's own.
        unsafe {
            input.installTapOnBus_bufferSize_format_block(
                0,
                TAP_FRAMES,
                Some(&format),
                RcBlock::as_ptr(&tap),
            );
            engine.prepare();
        }
        // SAFETY: the graph is input → tap; nothing plays.
        if let Err(e) = unsafe { engine.startAndReturnError() } {
            // SAFETY: as above.
            unsafe { input.removeTapOnBus(0) };
            return Err(failed(&e));
        }
        Ok(Self {
            engine,
            shared,
            stopped: false,
        })
    }

    pub fn set_listening(&self, listening: bool) {
        let mut state = lock(&self.shared.state);
        if listening && !state.listening {
            state.gap = true;
        }
        state.listening = listening;
    }

    /// Stops the engine and the tap; ends the request being fed.
    fn halt(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        // SAFETY: stopping a running engine and removing our tap.
        unsafe {
            self.engine.stop();
            self.engine.inputNode().removeTapOnBus(0);
        }
        let mut state = lock(&self.shared.state);
        state.stopping = true;
        if let Some(live) = state.current.take() {
            // SAFETY: no more audio comes; the result follows.
            unsafe { live.request.endAudio() };
            state.pending.push(live);
        }
        // Releasing the file finishes it.
        state.file = None;
    }

    pub fn stop(mut self) -> Result<Narration, NarrationError> {
        self.halt();
        let deadline = Instant::now() + FINISH_WAIT;
        let mut state = lock(&self.shared.state);
        while !state.pending.is_empty() {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            state = self
                .shared
                .finished
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        for live in state.pending.drain(..) {
            // SAFETY: giving up on a task that did not finish in time.
            unsafe { live.task.cancel() };
        }
        let mut words: Vec<SpokenWord> = state
            .words
            .iter()
            .map(|(secs, duration, text)| state.clock.word(*secs, *duration, text.clone()))
            .collect();
        words.sort_by_key(|w| w.start);
        let audio = match state.audio.take() {
            Some(path) if state.wrote_audio => Some(path),
            Some(path) => {
                let _ = std::fs::remove_file(path);
                None
            }
            None => None,
        };
        Ok(Narration { words, audio })
    }
}

impl Drop for Narrator {
    /// Discards what was heard: cancels recognition and removes the audio.
    fn drop(&mut self) {
        if self.stopped {
            return;
        }
        self.halt();
        let mut state = lock(&self.shared.state);
        for live in state.pending.drain(..) {
            // SAFETY: cancelling our own task.
            unsafe { live.task.cancel() };
        }
        if let Some(path) = state.audio.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

impl Shared {
    /// A buffer from the tap, on the audio thread.
    fn heard(self: &Arc<Self>, buffer: &AVAudioPCMBuffer, when: &AVAudioTime) {
        let mut state = lock(&self.state);
        if !state.listening || state.stopping {
            return;
        }
        // SAFETY: plain property reads on a valid buffer and time.
        let (frames, rate, host) = unsafe {
            (
                buffer.frameLength(),
                buffer.format().sampleRate(),
                when.isHostTimeValid().then(|| when.hostTime()),
            )
        };
        if frames == 0 || rate <= 0.0 {
            return;
        }
        let secs = f64::from(frames) / rate;
        if state.gap {
            let at = match host {
                Some(ticks) => event_tap::mach_ticks_to_timestamp(ticks),
                // The buffer ends about now.
                None => {
                    let now = event_tap::now();
                    guiderecorder_core::events::Timestamp(now.0.saturating_sub((secs * 1e9) as u64))
                }
            };
            let fed = state.fed;
            state.clock.mark(fed, at);
            state.gap = false;
        }
        if state
            .current
            .as_ref()
            .is_some_and(|live| state.fed - live.base >= REQUEST_SECS)
        {
            let live = state.current.take().expect("checked above");
            // SAFETY: ends the request; its final result follows.
            unsafe { live.request.endAudio() };
            state.pending.push(live);
        }
        if state.current.is_none() && state.failures < MAX_FAILURES {
            state.current = self.begin(&mut state);
        }
        if let Some(live) = &state.current {
            // SAFETY: a PCM buffer of the input format, as the request takes.
            unsafe { live.request.appendAudioPCMBuffer(buffer) };
        }
        if let Some(file) = &state.file {
            // SAFETY: the buffer is in the file's processing format.
            match unsafe { file.writeFromBuffer_error(buffer) } {
                Ok(()) => state.wrote_audio = true,
                Err(e) => {
                    eprintln!(
                        "guiderecorder: narration audio stopped: {}",
                        e.localizedDescription()
                    );
                    state.file = None;
                }
            }
        }
        state.fed += secs;
    }

    /// Begins a recognition request counting from the audio fed so far.
    fn begin(self: &Arc<Self>, state: &mut State) -> Option<Live> {
        // SAFETY: a fresh request, set up before any audio is appended.
        let request = unsafe {
            let request = SFSpeechAudioBufferRecognitionRequest::new();
            request.setRequiresOnDeviceRecognition(true);
            request.setShouldReportPartialResults(false);
            request.setTaskHint(SFSpeechRecognitionTaskHint::Dictation);
            // macOS 13 and later.
            if request.respondsToSelector(sel!(setAddsPunctuation:)) {
                request.setAddsPunctuation(true);
            }
            request
        };
        let id = state.next_id;
        state.next_id += 1;
        let shared: Weak<Shared> = Arc::downgrade(self);
        let handler = RcBlock::new(
            move |result: *mut SFSpeechRecognitionResult, error: *mut NSError| {
                if let Some(shared) = shared.upgrade() {
                    // SAFETY: each is null or valid for the call.
                    let (result, error) = unsafe { (result.as_ref(), error.as_ref()) };
                    shared.recognised(id, result, error);
                }
            },
        );
        // SAFETY: the handler matches the result handler's signature and
        // is copied by the recogniser; it runs on our queue.
        let task = unsafe {
            self.recognizer
                .recognitionTaskWithRequest_resultHandler(&request, &handler)
        };
        Some(Live {
            id,
            base: state.fed,
            request,
            task,
        })
    }

    /// A result or an error for request `id`, on the queue.
    fn recognised(
        &self,
        id: u64,
        result: Option<&SFSpeechRecognitionResult>,
        error: Option<&NSError>,
    ) {
        let mut state = lock(&self.state);
        let base = match (&state.current, state.pending.iter().find(|l| l.id == id)) {
            (Some(live), _) if live.id == id => live.base,
            (_, Some(live)) => live.base,
            _ => return,
        };
        // SAFETY: a plain property read on a valid result.
        let is_final = result.is_some_and(|r| unsafe { r.isFinal() });
        let mut heard = 0;
        if let Some(result) = result.filter(|_| is_final) {
            // SAFETY: as above.
            let transcription = unsafe { result.bestTranscription() };
            for (secs, duration, text) in words(&transcription) {
                state.words.push((base + secs, duration, text));
                heard += 1;
            }
        }
        if !is_final && error.is_none() {
            return;
        }
        if let Some(e) = error {
            if heard == 0 {
                state.failures += 1;
            }
            // "No speech detected" ends a silent request; not worth more.
            eprintln!(
                "guiderecorder: narration request ended: {}",
                e.localizedDescription()
            );
        } else {
            state.failures = 0;
        }
        // The recogniser may end the request being fed (after a long
        // silence): the next buffer begins another.
        if state.current.as_ref().is_some_and(|l| l.id == id) {
            state.current = None;
        }
        state.pending.retain(|l| l.id != id);
        self.finished.notify_all();
    }
}

/// The words of a final transcription: (seconds from the request's start,
/// duration, text with the punctuation and spacing that follows it in the
/// formatted string).
fn words(transcription: &SFTranscription) -> Vec<(f64, f64, String)> {
    // SAFETY: plain property reads on a final transcription.
    let (text, segments) = unsafe { (transcription.formattedString(), transcription.segments()) };
    let ranges: Vec<NSRange> = segments
        .iter()
        // SAFETY: as above.
        .map(|s| unsafe { s.substringRange() })
        .collect();
    let len = text.length();
    segments
        .iter()
        .enumerate()
        .filter_map(|(i, segment)| {
            let start = ranges[i].location;
            let end = ranges.get(i + 1).map_or(len, |r| r.location).max(start);
            let word = if start + ranges[i].length <= len && end <= len {
                text.substringWithRange(NSRange::new(start, end - start))
                    .to_string()
            } else {
                // SAFETY: as above.
                unsafe { segment.substring() }.to_string()
            };
            let word = word.trim().to_owned();
            // SAFETY: as above.
            let (secs, duration) = unsafe { (segment.timestamp(), segment.duration()) };
            (!word.is_empty()).then_some((secs, duration, word))
        })
        .collect()
}

/// An AAC file at `path` taking buffers in `format` (the tap's).
fn audio_file(
    path: &std::path::Path,
    rate: f64,
    channels: u32,
    format: &objc2_avf_audio::AVAudioFormat,
) -> Result<Retained<AVAudioFile>, String> {
    let url = NSURL::from_file_path(path).ok_or("not a file path")?;
    // SAFETY: static NSString keys from AVFAudio.
    let keys: Vec<&NSString> = unsafe {
        [
            AVFormatIDKey,
            AVSampleRateKey,
            AVNumberOfChannelsKey,
            AVEncoderBitRateKey,
        ]
    }
    .into_iter()
    .collect::<Option<_>>()
    .ok_or("AVFAudio keys are missing")?;
    let values = [
        NSNumber::new_u32(AAC),
        NSNumber::new_f64(rate),
        NSNumber::new_u32(channels),
        NSNumber::new_u32(BIT_RATE * channels.min(2)),
    ];
    let values: Vec<&AnyObject> = values.iter().map(|v| v.as_ref()).collect();
    let settings = NSDictionary::from_slices(&keys, &values);
    // SAFETY: AAC settings with NSNumber values; the processing format is
    // the tap's, so its buffers can be written as they come.
    unsafe {
        AVAudioFile::initForWriting_settings_commonFormat_interleaved_error(
            AVAudioFile::alloc(),
            &url,
            &settings,
            format.commonFormat(),
            format.isInterleaved(),
        )
    }
    .map_err(|e| e.localizedDescription().to_string())
}
