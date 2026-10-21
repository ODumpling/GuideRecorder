//! Recording into an open guide (#12): **Re-capture** a step, or **Insert
//! recorded steps here**.
//!
//! `start_recording` takes a [`RecordInto`] naming the guide (open in the
//! editor, and writable) and what to do there. The session is the usual
//! one, with the usual toolbar, live feed, pause and cancel. At stop,
//! instead of a new guide, the steps' images are written into the guide's
//! bundle (`Recording::save_steps`), scanned for sensitive text as a new
//! recording's are, and the steps are handed to the webview in
//! [`RecordingSaved::recorded`](crate::recording::RecordingSaved::recorded).
//! The editor adds them as one undoable edit; a re-capture keeps the
//! step's text and the user's edits and offers to move or clear its
//! annotations. `guide.json` is never written here: the editor's copy is
//! the one that counts, and it saves. Images no edit ends up using are
//! swept when the guide closes, as for any other edit.
//!
//! A re-capture keeps the first step only, and stops by itself a moment
//! after the first capture ([`stop_delay`]), so one click, shortcut or
//! manual step is all it takes.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use guiderecorder_capture::backend::session::Recording;
use guiderecorder_capture::session::LiveSnapshot;
use guiderecorder_core::library::GuideHandle;
use guiderecorder_core::model::Step;
use guiderecorder_core::steps::LiveAction;
use guiderecorder_core::Ulid;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Runtime};

use crate::editor::{parse_id, Result, Session};
use crate::recording::{self, Control, Recorder, RecordingSaved, RecordingStatus};
use crate::settings::{self, Redaction, SettingsStore};

/// Where a recording goes when it is not a new guide. Serialized as
/// `{"mode": "replace", "guideId": …, "stepId": …}` (re-capture the step)
/// or `{"mode": "insert", "guideId": …, "afterId": …}` (insert the steps
/// after the item `afterId`; null or absent: at the end).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "camelCase")]
pub enum RecordInto {
    #[serde(rename_all = "camelCase")]
    Replace { guide_id: String, step_id: String },
    #[serde(rename_all = "camelCase")]
    Insert {
        guide_id: String,
        #[serde(default)]
        after_id: Option<String>,
    },
}

impl RecordInto {
    pub fn guide_id(&self) -> &str {
        match self {
            RecordInto::Replace { guide_id, .. } | RecordInto::Insert { guide_id, .. } => guide_id,
        }
    }

    /// How many steps are kept: a re-capture keeps the first.
    pub fn limit(&self) -> Option<usize> {
        match self {
            RecordInto::Replace { .. } => Some(1),
            RecordInto::Insert { .. } => None,
        }
    }
}

/// The steps a recording made for the guide it went into (see
/// [`RecordInto`]), for the editor to add. Without the item `type` tag,
/// as `add_image_step` returns steps.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordedSteps {
    /// New for each stop, so the webview handles each result once (the
    /// command result and the saved event both carry it).
    pub id: String,
    pub into: RecordInto,
    pub steps: Vec<Step>,
}

/// The bundle of the guide `into` names: `invalid` for an id that is not
/// one, `notOpen` unless the editor has it open, `readOnly` if it cannot be
/// written. The step to re-capture is not looked up: the editor's copy may
/// hold steps not saved yet, so the editor checks it when the steps come
/// back.
pub fn check(session: &Session, into: &RecordInto) -> Result<GuideHandle> {
    let guide_id = parse_id(into.guide_id())?;
    match into {
        RecordInto::Replace { step_id, .. } => {
            parse_id(step_id)?;
        }
        RecordInto::Insert { after_id, .. } => {
            after_id.as_deref().map(parse_id).transpose()?;
        }
    }
    session.writable_handle(guide_id)
}

/// How long after the first capture of a re-capture it stops by itself:
/// time for the step to finish (a typing run to fall idle) and for its
/// after frame (`recording.afterFrameDelayMs`), and a little more.
pub fn stop_delay(action: Option<&LiveAction>, settings: &settings::Recording) -> Duration {
    let after = if settings.after_frames {
        u64::from(settings.after_frame_delay_ms)
    } else {
        0
    };
    let finish = match action {
        // A run of keys ends after `typingIdleMs` without one.
        Some(LiveAction::Type) => 2 * u64::from(settings.typing_idle_ms),
        _ => 0,
    };
    Duration::from_millis(finish + after + 700)
}

/// Called for each capture of the running session (on the encoder
/// thread): the first capture of a re-capture schedules its stop, after
/// [`stop_delay`], unless it was stopped or cancelled meanwhile. The stop
/// runs as a stop from the tray would ([`recording::control`]): it shows
/// the main window, whose editor takes the step.
pub fn on_snapshot<R: Runtime>(
    app: &AppHandle<R>,
    recorder: &Arc<Recorder>,
    snapshot: &LiveSnapshot,
) {
    let stamped = recorder.status();
    let Some(RecordInto::Replace { .. }) = &stamped.into else {
        return;
    };
    let RecordingStatus::Recording { started_at, .. } = &stamped.status else {
        return;
    };
    if recorder.stop_scheduled.swap(true, Ordering::SeqCst) {
        return;
    }
    let delay = app
        .try_state::<Arc<SettingsStore>>()
        .map(|store| stop_delay(snapshot.action.as_ref(), &store.get().recording))
        .unwrap_or_else(|| stop_delay(snapshot.action.as_ref(), &settings::Recording::default()));
    let app = app.clone();
    let started_at = started_at.clone();
    let recorder = Arc::downgrade(recorder);
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        let Some(recorder) = recorder.upgrade() else {
            return;
        };
        // Still the same re-capture (not stopped, cancelled or replaced).
        let same = match &recorder.status().status {
            RecordingStatus::Recording { started_at: s, .. }
            | RecordingStatus::Paused { started_at: s, .. } => *s == started_at,
            _ => false,
        };
        if same {
            recording::control(&app, Control::Stop);
        }
    });
}

/// Writes what `recording` captured into the guide `into` names and
/// returns the steps for the editor (see the module docs), with what
/// `heard` heard split across them. The guide must still be open and
/// writable.
pub fn save_into(
    recording: &Recording,
    session: &Session,
    into: RecordInto,
    heard: Option<recording::Heard>,
    redaction: &Redaction,
) -> Result<RecordingSaved> {
    let handle = check(session, &into)?;
    let saved = recording
        .save_steps(&handle, into.limit())
        .map_err(recording::session_error)?;
    let mut steps = saved.steps;
    let narrated_steps = heard.map_or(0, |heard| {
        crate::narration::finish_steps(heard, &mut steps, &saved.step_times)
    });
    let redacted = crate::redaction::redact_steps(&handle, &mut steps, redaction);
    Ok(RecordingSaved {
        guide_id: into.guide_id().to_owned(),
        steps: steps.len(),
        skipped: saved.skipped,
        suggested_blurs: redacted.suggested,
        applied_blurs: redacted.blurred,
        unscanned_steps: redacted.unscanned,
        narrated_steps,
        recorded: Some(RecordedSteps {
            id: Ulid::new().to_string(),
            into,
            steps,
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn record_into_deserializes_from_the_contract_shapes() {
        let into = |value| serde_json::from_value::<RecordInto>(value).unwrap();
        let replace = into(json!({"mode": "replace", "guideId": "G", "stepId": "S"}));
        assert_eq!(
            replace,
            RecordInto::Replace {
                guide_id: "G".into(),
                step_id: "S".into()
            }
        );
        assert_eq!(replace.limit(), Some(1));
        let insert = into(json!({"mode": "insert", "guideId": "G"}));
        assert_eq!(
            insert,
            RecordInto::Insert {
                guide_id: "G".into(),
                after_id: None
            }
        );
        assert_eq!(insert.limit(), None);
        assert_eq!(insert.guide_id(), "G");
        // And back, as the status and the saved result carry it.
        assert_eq!(
            serde_json::to_value(&replace).unwrap(),
            json!({"mode": "replace", "guideId": "G", "stepId": "S"})
        );
    }

    #[test]
    fn a_recapture_stops_once_its_step_is_done() {
        let mut settings = settings::Recording {
            after_frames: true,
            after_frame_delay_ms: 400,
            typing_idle_ms: 1500,
            ..settings::Recording::default()
        };
        let click = LiveAction::Click {
            verb: "Click".into(),
        };
        assert_eq!(
            stop_delay(Some(&click), &settings),
            Duration::from_millis(1100)
        );
        assert_eq!(
            stop_delay(Some(&LiveAction::Manual), &settings),
            Duration::from_millis(1100)
        );
        // Typing runs on until it falls idle.
        assert_eq!(
            stop_delay(Some(&LiveAction::Type), &settings),
            Duration::from_millis(4100)
        );
        settings.after_frames = false;
        assert_eq!(
            stop_delay(Some(&click), &settings),
            Duration::from_millis(700)
        );
    }

    #[test]
    fn only_an_open_writable_guide_is_recorded_into() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/guiderecorder-app-tests")
            .join(format!("recapture-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let session = Session::start(&dir, None).unwrap();
        let guide = guiderecorder_core::model::Guide::new("Into", "en");
        session.library().create(&guide).unwrap();
        let id = guide.id.to_string();
        let insert = |after: Option<&str>| RecordInto::Insert {
            guide_id: id.clone(),
            after_id: after.map(str::to_owned),
        };
        let kind = |into: &RecordInto| check(&session, into).unwrap_err().kind;
        use crate::editor::ErrorKind;
        assert_eq!(kind(&insert(None)), ErrorKind::NotOpen);
        session.open_guide(&id).unwrap();
        assert!(check(&session, &insert(None)).is_ok());
        assert_eq!(kind(&insert(Some("nope"))), ErrorKind::Invalid);
        let replace = RecordInto::Replace {
            guide_id: id.clone(),
            step_id: "nope".into(),
        };
        assert_eq!(kind(&replace), ErrorKind::Invalid);
        let replace = RecordInto::Replace {
            guide_id: id.clone(),
            step_id: Ulid::new().to_string(),
        };
        assert!(check(&session, &replace).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
