//! Narrating while recording (#13): the toolbar's microphone.
//!
//! With `recording.narration.enabled` on, a recording starts a capture
//! crate `Narrator` beside its session: the microphone is heard and
//! transcribed on the device. The toolbar turns it on and off mid-recording
//! ([`set_narration`], stored as the setting, so the next recording starts
//! the same way); pausing the recording stops hearing it too. Its state is
//! sent as [`NARRATION_EVENT`] whenever it changes.
//!
//! At stop the narrator runs out while the guide is saved, and its words
//! are split across the steps (`guiderecorder_core::narration`): each
//! step gets what was said during it as its `narration`, and as its
//! description when it has none. The AI passes then see it. With
//! `recording.narration.keepAudio` on, the audio is kept in the bundle
//! (`narration.m4a`); otherwise none is written.
//!
//! Turning the microphone on never prompts: without the Microphone and
//! Speech Recognition permissions it stays off and says why. Onboarding
//! and Settings → Permissions ask for them.

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;

use guiderecorder_capture::narration::{
    self as capture, Narration, NarrationConfig, NarrationError, Narrator,
};
use guiderecorder_capture::permissions::Permission;
use guiderecorder_core::library::{GuideHandle, Library, NARRATION_AUDIO_FILE};
use guiderecorder_core::narration;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime, State};

use crate::blocking::blocking;
use crate::editor::Result;
use crate::recording::Recorder;
use crate::settings::SettingsStore;

/// Emitted with a [`NarrationState`] whenever it changes.
pub const NARRATION_EVENT: &str = "recording://narration";

/// The toolbar's microphone.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NarrationState {
    /// Narration works on this platform (macOS).
    pub supported: bool,
    /// The microphone is on: what is said is transcribed while recording
    /// (not while paused). Outside a recording, the same as `enabled`.
    pub on: bool,
    /// `recording.narration.enabled` as stored: whether the next
    /// recording starts with the microphone on.
    pub enabled: bool,
    /// Why the microphone could not be turned on, until it next is.
    pub error: Option<String>,
}

/// The running recording's narrator, kept by the [`Recorder`].
#[derive(Debug, Default)]
pub struct NarrationSlot(Mutex<Slot>);

#[derive(Debug, Default)]
struct Slot {
    /// A recording is running (or paused).
    active: bool,
    on: bool,
    narrator: Option<Narrator>,
    error: Option<String>,
}

impl NarrationSlot {
    fn lock(&self) -> MutexGuard<'_, Slot> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The state, with `enabled` the stored setting.
    fn state(&self, enabled: bool) -> NarrationState {
        let slot = self.lock();
        NarrationState {
            supported: capture::is_supported(),
            on: if slot.active { slot.on } else { enabled },
            enabled,
            error: slot.error.clone(),
        }
    }

    /// The recording ended: drops the narrator, if any, discarding what it
    /// heard.
    pub fn discard(&self) {
        *self.lock() = Slot::default();
    }

    /// The recording stopped: the narrator, if any, runs out on its own
    /// thread while the guide is saved (see [`finish`]).
    pub fn take(&self) -> Option<JoinHandle<std::result::Result<Narration, NarrationError>>> {
        let narrator = std::mem::take(&mut *self.lock()).narrator?;
        Some(std::thread::spawn(move || narrator.stop()))
    }

    /// Hears the microphone only while it is on and the recording is not
    /// paused.
    pub fn set_paused(&self, paused: bool) {
        let slot = self.lock();
        if let Some(narrator) = &slot.narrator {
            narrator.set_listening(slot.on && !paused);
        }
    }

    /// Turns the microphone on or off for the recording whose session
    /// folder is `session_dir`. Turning it on starts a narrator the first
    /// time; failing to, it stays off with the reason in `error`.
    fn turn(&self, on: bool, session_dir: &Path, paused: bool, keep_audio: bool) {
        let mut slot = self.lock();
        slot.active = true;
        slot.error = None;
        slot.on = on;
        if on && slot.narrator.is_none() {
            let config = NarrationConfig {
                audio: keep_audio.then(|| session_dir.join(NARRATION_AUDIO_FILE)),
            };
            match Narrator::start(config) {
                Ok(narrator) => slot.narrator = Some(narrator),
                Err(e) => {
                    eprintln!("guiderecorder: narration: {e}");
                    slot.on = false;
                    slot.error = Some(message(&e));
                }
            }
        }
        if let Some(narrator) = &slot.narrator {
            narrator.set_listening(slot.on && !paused);
        }
    }
}

/// What the toolbar says when the microphone can't be turned on.
fn message(e: &NarrationError) -> String {
    match e {
        NarrationError::NotAllowed(Permission::Microphone) => {
            "Allow GuideRecorder to use the microphone in Settings → Permissions.".into()
        }
        NarrationError::NotAllowed(_) => {
            "Allow GuideRecorder to use speech recognition in Settings → Permissions.".into()
        }
        NarrationError::Unsupported => "Narration is not available on this computer.".into(),
        e => {
            let text = e.to_string();
            let mut chars = text.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect::<String>() + ".",
                None => text,
            }
        }
    }
}

fn emit<R: Runtime>(app: &AppHandle<R>, state: &NarrationState) {
    if let Err(e) = app.emit(NARRATION_EVENT, state) {
        eprintln!("guiderecorder: cannot emit {NARRATION_EVENT}: {e}");
    }
}

/// At the start of a recording (session folder `session_dir`): turns the
/// microphone on when the settings say so, and sends the state.
pub fn on_start<R: Runtime>(
    app: &AppHandle<R>,
    recorder: &Recorder,
    store: &SettingsStore,
    session_dir: Option<&Path>,
) {
    let settings = store.get().recording.narration;
    match session_dir {
        Some(dir) => recorder
            .narration
            .turn(settings.enabled, dir, false, settings.keep_audio),
        None => recorder.narration.discard(),
    }
    emit(app, &recorder.narration.state(settings.enabled));
}

/// The microphone's state.
#[tauri::command]
pub fn narration_state(
    recorder: State<'_, Arc<Recorder>>,
    store: State<'_, Arc<SettingsStore>>,
) -> NarrationState {
    recorder
        .narration
        .state(store.get().recording.narration.enabled)
}

/// Turns the toolbar's microphone on or off: stored as
/// `recording.narration.enabled` for the next recording, and applied to
/// the running one, if any. Never prompts for a permission; one missing
/// leaves it off, with the reason in `error`. Returns the new state, also
/// sent as [`NARRATION_EVENT`].
#[tauri::command]
pub async fn set_narration<R: Runtime>(
    app: AppHandle<R>,
    recorder: State<'_, Arc<Recorder>>,
    store: State<'_, Arc<SettingsStore>>,
    enabled: bool,
) -> Result<NarrationState> {
    let (recorder, store) = (Arc::clone(&recorder), Arc::clone(&store));
    blocking(move || {
        let settings = store.modify(|s| s.recording.narration.enabled = enabled)?;
        let keep_audio = settings.recording.narration.keep_audio;
        recorder.with_session(|session| {
            if let Some(session) = session {
                if let Some(dir) = session.session_dir() {
                    let paused = session.is_paused();
                    recorder.narration.turn(enabled, dir, paused, keep_audio);
                }
            }
        });
        let state = recorder.narration.state(enabled);
        emit(&app, &state);
        Ok(state)
    })
    .await
}

/// After a narrated recording is saved: splits what `heard` heard across
/// the guide's steps (`step_times`, from the save) and saves the guide
/// again, and moves the audio into the bundle. Returns how many steps got
/// some narration. Failures are logged: the recording is saved either
/// way.
pub fn finish(
    heard: JoinHandle<std::result::Result<Narration, NarrationError>>,
    library: &Library,
    handle: &GuideHandle,
    guide: &mut guiderecorder_core::model::Guide,
    step_times: &[(
        guiderecorder_core::Ulid,
        guiderecorder_core::events::Timestamp,
    )],
) -> usize {
    let narration = match heard.join() {
        Ok(Ok(narration)) => narration,
        Ok(Err(e)) => {
            eprintln!("guiderecorder: narration: {e}");
            return 0;
        }
        Err(_) => {
            eprintln!("guiderecorder: narration: the narrator panicked");
            return 0;
        }
    };
    if let Some(audio) = &narration.audio {
        if let Err(e) = keep_audio(audio, handle) {
            eprintln!("guiderecorder: narration: cannot keep the audio: {e}");
        }
    }
    let mut narrated = guide.clone();
    let count = narration::apply(&mut narrated, step_times, &narration.words);
    if count == 0 {
        return 0;
    }
    match library.save_to(handle, &narrated) {
        Ok(()) => {
            *guide = narrated;
            count
        }
        Err(e) => {
            eprintln!(
                "guiderecorder: narration: cannot save {}: {e}",
                handle.path.display()
            );
            0
        }
    }
}

/// [`finish`] for steps recorded into an existing guide (see
/// [`crate::recapture`]): what was heard goes into `steps`, which the
/// editor adds to the guide. The guide's audio, if any, is the first
/// recording's, so this one's is not kept.
pub fn finish_steps(
    heard: JoinHandle<std::result::Result<Narration, NarrationError>>,
    steps: &mut Vec<guiderecorder_core::model::Step>,
    step_times: &[(
        guiderecorder_core::Ulid,
        guiderecorder_core::events::Timestamp,
    )],
) -> usize {
    let narration = match heard.join() {
        Ok(Ok(narration)) => narration,
        Ok(Err(e)) => {
            eprintln!("guiderecorder: narration: {e}");
            return 0;
        }
        Err(_) => {
            eprintln!("guiderecorder: narration: the narrator panicked");
            return 0;
        }
    };
    if let Some(audio) = &narration.audio {
        let _ = std::fs::remove_file(audio);
    }
    apply_to_steps(steps, step_times, &narration.words)
}

/// Core's `narration::apply` on bare steps, through a scratch guide.
fn apply_to_steps(
    steps: &mut Vec<guiderecorder_core::model::Step>,
    step_times: &[(
        guiderecorder_core::Ulid,
        guiderecorder_core::events::Timestamp,
    )],
    words: &[guiderecorder_core::narration::SpokenWord],
) -> usize {
    use guiderecorder_core::model::{Guide, Item};
    let mut scratch = Guide::new("", "en");
    scratch.items = steps.drain(..).map(Item::Step).collect();
    let count = narration::apply(&mut scratch, step_times, words);
    steps.extend(scratch.items.into_iter().filter_map(|item| match item {
        Item::Step(step) => Some(step),
        _ => None,
    }));
    count
}

/// Moves the audio at `from` into the bundle as [`NARRATION_AUDIO_FILE`]
/// (copied when it is on another volume).
fn keep_audio(from: &Path, handle: &GuideHandle) -> std::result::Result<(), String> {
    let to = handle
        .resolve(NARRATION_AUDIO_FILE)
        .map_err(|e| e.to_string())?;
    if std::fs::rename(from, &to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, &to).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(from);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_state_is_camel_case() {
        let state = NarrationState {
            supported: true,
            on: false,
            enabled: true,
            error: Some("No microphone was found.".into()),
        };
        assert_eq!(
            serde_json::to_value(&state).unwrap(),
            serde_json::json!({ "supported": true, "on": false, "enabled": true, "error": "No microphone was found." })
        );
    }

    #[test]
    fn steps_recorded_into_a_guide_get_their_narration_in_order() {
        use guiderecorder_core::events::Timestamp;
        use guiderecorder_core::model::Step;
        let step = |n: u32| -> Step {
            serde_json::from_value(serde_json::json!({
                "id": guiderecorder_core::Ulid::new().to_string(),
                "kind": "click",
                "title": format!("Step {n}"),
                "image": { "full": "" },
            }))
            .unwrap()
        };
        let mut steps = vec![step(1), step(2)];
        let ids: Vec<_> = steps.iter().map(|s| s.id).collect();
        let s = |secs: u64| Timestamp(secs * 1_000_000_000);
        let times = [(ids[0], s(1)), (ids[1], s(10))];
        let word = |at: u64, text: &str| narration::SpokenWord {
            start: s(at),
            end: s(at),
            text: text.into(),
        };
        let words = [word(2, "open settings."), word(11, "now save.")];
        assert_eq!(apply_to_steps(&mut steps, &times, &words), 2);
        assert_eq!(steps.iter().map(|s| s.id).collect::<Vec<_>>(), ids);
        assert!(steps[0].narration.contains("open settings"));
        assert!(steps[1].narration.contains("now save"));
    }

    #[test]
    fn outside_a_recording_the_microphone_is_the_setting() {
        let slot = NarrationSlot::default();
        assert!(!slot.state(false).on);
        let state = slot.state(true);
        assert!(state.on && state.enabled);
        assert_eq!(state.error, None);
        assert_eq!(state.supported, capture::is_supported());
        assert!(slot.take().is_none());
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn where_narration_is_unsupported_the_microphone_stays_off_and_says_why() {
        let slot = NarrationSlot::default();
        let dir = std::env::temp_dir();
        slot.turn(true, &dir, false, true);
        // On in the settings, off in this recording.
        let state = slot.state(true);
        assert!(!state.on);
        assert!(state.enabled);
        assert!(!state.supported);
        assert_eq!(
            state.error.as_deref(),
            Some("Narration is not available on this computer.")
        );
        // Off clears the reason.
        slot.turn(false, &dir, false, true);
        assert_eq!(slot.state(false).error, None);
        // Once the recording ends, the setting counts again.
        slot.turn(true, &dir, false, true);
        assert!(slot.take().is_none());
        assert!(slot.state(true).on);
        assert_eq!(slot.state(true).error, None);
        slot.discard();
    }

    #[test]
    fn errors_read_as_sentences() {
        assert_eq!(
            message(&NarrationError::NotAllowed(Permission::SpeechRecognition)),
            "Allow GuideRecorder to use speech recognition in Settings → Permissions."
        );
        assert_eq!(
            message(&NarrationError::NoMicrophone),
            "No microphone was found."
        );
    }

    fn library() -> (Library, std::path::PathBuf) {
        let root =
            std::env::temp_dir().join(format!("gr-narration-{}", guiderecorder_core::Ulid::new()));
        std::fs::create_dir_all(&root).unwrap();
        (Library::open(&root).unwrap(), root)
    }

    #[test]
    fn finish_narrates_the_saved_guide_and_keeps_the_audio() {
        use guiderecorder_core::events::Timestamp;
        use guiderecorder_core::model::{Guide, Item, StepImage, StepKind};
        use guiderecorder_core::narration::SpokenWord;
        use guiderecorder_core::steps::DraftStep;

        let (library, root) = library();
        let mut guide = Guide::new("Narrated", "en");
        let id = guiderecorder_core::Ulid::new();
        let step = DraftStep {
            kind: StepKind::Click,
            at: Timestamp(0),
            point: None,
            end_point: None,
            element: None,
            app: None,
            keystrokes: None,
            url: None,
            title: "Click **Save**".into(),
        }
        .into_step(
            id,
            StepImage {
                full: format!("steps/{id}.webp"),
                crop: None,
                crop_rect: None,
                after: None,
                show: Default::default(),
            },
        );
        guide.items.push(Item::Step(step));
        let handle = library.create(&guide).unwrap();
        let audio = root.join("heard.m4a");
        std::fs::write(&audio, b"aac").unwrap();
        let heard = std::thread::spawn(move || {
            Ok(Narration {
                words: vec![SpokenWord {
                    start: Timestamp(9_000_000_000),
                    end: Timestamp(9_500_000_000),
                    text: "save the draft".into(),
                }],
                audio: Some(audio),
            })
        });
        let count = finish(
            heard,
            &library,
            &handle,
            &mut guide,
            &[(id, Timestamp(10_000_000_000))],
        );
        assert_eq!(count, 1);
        let saved = library.read(&handle.path).unwrap().guide;
        let step = saved.steps().next().unwrap();
        assert_eq!(step.narration, "save the draft");
        assert_eq!(step.description, "Save the draft.");
        assert_eq!(saved, guide);
        assert_eq!(
            std::fs::read(handle.path.join(NARRATION_AUDIO_FILE)).unwrap(),
            b"aac"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_failed_narrator_leaves_the_guide_alone() {
        use guiderecorder_core::model::Guide;
        let (library, root) = library();
        let mut guide = Guide::new("Quiet", "en");
        let handle = library.create(&guide).unwrap();
        let heard = std::thread::spawn(|| Err(NarrationError::NoMicrophone));
        assert_eq!(finish(heard, &library, &handle, &mut guide, &[]), 0);
        assert!(!handle.path.join(NARRATION_AUDIO_FILE).exists());
        let _ = std::fs::remove_dir_all(root);
    }
}
