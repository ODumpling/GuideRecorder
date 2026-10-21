//! Narration (#13): the microphone, transcribed on the device while a
//! recording runs. macOS only (`macos::narration`: AVAudioEngine and the
//! Speech framework with `requiresOnDeviceRecognition`); every other
//! platform refuses to start ([`NarrationError::Unsupported`]). Windows'
//! dictation needs its online speech service, so it is not used. Audio
//! and text never leave the device.
//!
//! A [`Narrator`] runs beside a recording session, not inside it: the app
//! starts one when narration is on, and the microphone can be turned off
//! and on again during the recording. While it is not listening
//! ([`Narrator::set_listening`]: the toolbar's microphone is off, or the
//! recording is paused) nothing is heard, kept or transcribed.
//!
//! [`Narrator::stop`] returns the words heard, each timed on the event
//! clock ([`guiderecorder_core::events::Timestamp`]) so they line up with
//! the steps (see [`guiderecorder_core::narration`]). The recogniser times
//! words by the audio it was given, which leaves out the gaps while not
//! listening; an [`AudioClock`] maps that audio time back to the event
//! clock. With [`NarrationConfig::audio`] set, the audio heard is also
//! written there (AAC in an `.m4a`), gaps left out.

use std::path::PathBuf;

use guiderecorder_core::events::Timestamp;
pub use guiderecorder_core::narration::SpokenWord;

use crate::permissions::Permission;

/// How a [`Narrator`] starts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NarrationConfig {
    /// Also write the audio heard to this file (`.m4a`); none: keep no
    /// audio.
    pub audio: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NarrationError {
    #[error("narration is not available on this platform")]
    Unsupported,
    #[error("narration needs the {0:?} permission")]
    NotAllowed(Permission),
    #[error("no microphone was found")]
    NoMicrophone,
    #[error("speech recognition on this device is not available for {0}; download its dictation language in System Settings → Keyboard")]
    NotOnDevice(String),
    #[error("narration failed: {0}")]
    Failed(String),
}

/// What a narrator heard.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Narration {
    /// The words recognised, in time order.
    pub words: Vec<SpokenWord>,
    /// The audio file written, when [`NarrationConfig::audio`] asked for
    /// one and anything was heard.
    pub audio: Option<PathBuf>,
}

/// Whether narration can work on this platform (permissions aside).
pub fn is_supported() -> bool {
    cfg!(target_os = "macos")
}

/// Hears the microphone and transcribes it on the device. Starts
/// listening; stops when [`Narrator::stop`]ped or dropped (dropping
/// discards what was heard, and the audio file).
pub struct Narrator {
    #[cfg(target_os = "macos")]
    inner: crate::macos::narration::Narrator,
    #[cfg(not(target_os = "macos"))]
    never: std::convert::Infallible,
}

impl std::fmt::Debug for Narrator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Narrator").finish_non_exhaustive()
    }
}

impl Narrator {
    /// Starts listening. Refuses without the [`Permission::NARRATION`]
    /// permissions (never prompts), without a microphone, or when speech
    /// can't be recognised on this device for the user's language (it is
    /// never sent anywhere instead).
    pub fn start(config: NarrationConfig) -> Result<Self, NarrationError> {
        #[cfg(target_os = "macos")]
        {
            crate::macos::narration::Narrator::start(config).map(|inner| Self { inner })
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = config;
            Err(NarrationError::Unsupported)
        }
    }

    /// Whether the microphone is heard: off while the toolbar's microphone
    /// is off or the recording is paused.
    pub fn set_listening(&self, listening: bool) {
        #[cfg(target_os = "macos")]
        {
            self.inner.set_listening(listening)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = listening;
            match self.never {}
        }
    }

    /// Stops listening and returns what was heard. Waits a few seconds at
    /// most for the last words to be recognised.
    pub fn stop(self) -> Result<Narration, NarrationError> {
        #[cfg(target_os = "macos")]
        {
            self.inner.stop()
        }
        #[cfg(not(target_os = "macos"))]
        {
            match self.never {}
        }
    }
}

/// Maps time in the audio heard (seconds, gaps while not listening left
/// out) to the event clock: each stretch of listening is [`mark`]ed when it
/// starts with the audio time and event time of its first sound.
///
/// [`mark`]: AudioClock::mark
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AudioClock {
    /// (audio seconds, event time), in audio order.
    marks: Vec<(f64, Timestamp)>,
}

impl AudioClock {
    /// Notes that the audio at `secs` was heard at `at`. Marks must come in
    /// audio order; one at the same audio time as the last replaces it.
    pub fn mark(&mut self, secs: f64, at: Timestamp) {
        if let Some(last) = self.marks.last_mut() {
            if last.0 >= secs {
                *last = (last.0, at);
                return;
            }
        }
        self.marks.push((secs, at));
    }

    pub fn is_empty(&self) -> bool {
        self.marks.is_empty()
    }

    /// The event time of audio time `secs`: counted from the last mark at
    /// or before it (the first, for times before any).
    pub fn at(&self, secs: f64) -> Timestamp {
        let i = self.marks.partition_point(|(s, _)| *s <= secs);
        let Some(&(base, at)) = self.marks.get(i.saturating_sub(1)) else {
            return Timestamp(0);
        };
        let offset_ns = ((secs - base) * 1e9).round();
        if offset_ns >= 0.0 {
            Timestamp(at.0.saturating_add(offset_ns as u64))
        } else {
            Timestamp(at.0.saturating_sub((-offset_ns) as u64))
        }
    }

    /// A word the recogniser timed at `start` seconds for `duration`, on
    /// the event clock.
    pub fn word(&self, start: f64, duration: f64, text: String) -> SpokenWord {
        SpokenWord {
            start: self.at(start),
            end: self.at(start + duration.max(0.0)),
            text,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: u64 = 1_000_000_000;

    #[test]
    fn audio_time_maps_through_each_stretch_of_listening() {
        let mut clock = AudioClock::default();
        assert_eq!(clock.at(1.0), Timestamp(0));
        // Listening from 100 s; muted at audio 4 s, back at 130 s.
        clock.mark(0.0, Timestamp(100 * S));
        clock.mark(4.0, Timestamp(130 * S));
        assert_eq!(clock.at(0.0), Timestamp(100 * S));
        assert_eq!(clock.at(2.5), Timestamp(102 * S + S / 2));
        assert_eq!(clock.at(4.0), Timestamp(130 * S));
        assert_eq!(clock.at(6.0), Timestamp(132 * S));
        // Before the first mark counts back from it.
        let mut late = AudioClock::default();
        late.mark(1.0, Timestamp(100 * S));
        assert_eq!(late.at(0.5), Timestamp(100 * S - S / 2));
    }

    #[test]
    fn a_mark_at_the_same_audio_time_replaces_the_last() {
        let mut clock = AudioClock::default();
        clock.mark(0.0, Timestamp(10 * S));
        // Muted and unmuted before any audio came: the later start counts.
        clock.mark(0.0, Timestamp(20 * S));
        assert_eq!(clock.at(1.0), Timestamp(21 * S));
        assert!(!clock.is_empty());
    }

    #[test]
    fn words_are_timed_on_the_event_clock() {
        let mut clock = AudioClock::default();
        clock.mark(0.0, Timestamp(50 * S));
        let word = clock.word(1.5, 0.5, "save".into());
        assert_eq!(word.start, Timestamp(51 * S + S / 2));
        assert_eq!(word.end, Timestamp(52 * S));
        assert_eq!(word.text, "save");
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn narration_is_refused_off_macos() {
        assert!(!is_supported());
        assert_eq!(
            Narrator::start(NarrationConfig::default()).unwrap_err(),
            NarrationError::Unsupported
        );
    }
}
