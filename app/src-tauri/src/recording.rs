//! Tauri commands that record a guide: start a capture session, pause and
//! resume it, stop it and save the result into the open library, cancel
//! it, and report its status.
//!
//! The session lives in a managed [`Recorder`]. Every command runs on the
//! blocking pool: starting a session blocks for a few seconds while
//! ScreenCaptureKit starts, resuming restarts the stream the same way, and
//! stopping one joins the capture threads, runs step detection and writes
//! the bundle.
//!
//! Status changes are pushed to the webview as [`STATUS_EVENT`] with a
//! [`RecordingStatus`] payload; a saved recording is announced as
//! [`SAVED_EVENT`] with a [`RecordingSaved`] payload (the same value the
//! `stop_recording` command returns).
//!
//! The library is checked at start (no open library, no recording) and
//! read again at stop: switching the library mid-recording saves into the
//! library open at stop time.
//!
//! Quitting the app drops any session still running ([`discard_on_quit`]),
//! as `cancel_recording` would; the frontend stops and saves first.
//!
//! Not in this slice: after-frames per step (the capture crate has no
//! support yet) and the Window and Region capture areas (they need the
//! picker from #357 and fall back to the display).

use std::sync::{Arc, Mutex, MutexGuard, PoisonError, TryLockError};

use chrono::{Local, SecondsFormat, Utc};
use guidesya_capture::macos::frames::CaptureTarget;
use guidesya_capture::macos::session::{Hotkey, RecordingSession, SessionConfig, SessionError};
use guidesya_core::events::Modifiers;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

use crate::editor::{AppError, ErrorKind, Result, SessionCell};
use crate::settings::{CaptureArea, Settings, SettingsStore};
use crate::settings_commands;

/// Emitted with a [`RecordingStatus`] whenever the status changes.
pub const STATUS_EVENT: &str = "recording://status";
/// Emitted with a [`RecordingSaved`] once a stopped recording is in the
/// library.
pub const SAVED_EVENT: &str = "recording://saved";

/// What the recorder is doing. Serialized as `{"state": "idle"}`,
/// `{"state": "recording", "startedAt": …, "title": …,
/// "captureAreaFallback": …}`, `{"state": "paused", …}` with the same
/// fields as `recording`, or `{"state": "saving", "startedAt": …,
/// "title": …}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(
    tag = "state",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RecordingStatus {
    #[default]
    Idle,
    Recording {
        /// RFC 3339, UTC.
        started_at: String,
        /// The title given to `start_recording`, if any.
        title: Option<String>,
        /// The stored capture area is Window or Region, which have no
        /// picker yet (#357), so the whole display is being captured.
        capture_area_fallback: bool,
    },
    /// Between `pause_recording` and `resume_recording`; the fields are
    /// those of the `Recording` it paused.
    Paused {
        started_at: String,
        title: Option<String>,
        capture_area_fallback: bool,
    },
    Saving {
        started_at: String,
        title: Option<String>,
    },
}

/// A recording saved into the library (returned by `stop_recording` and
/// sent as [`SAVED_EVENT`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingSaved {
    pub guide_id: String,
    /// Steps in the new guide.
    pub steps: usize,
    /// Detected steps left out because no snapshot was close enough.
    pub skipped: usize,
}

/// The managed recording state: at most one session at a time.
///
/// `session` is held for the whole of a start, pause, resume, stop or
/// cancel, so those serialize; `status` is only ever held briefly, so `recording_status`
/// never waits on a start or stop in progress.
#[derive(Default)]
pub struct Recorder {
    session: Mutex<Option<RecordingSession>>,
    status: Mutex<RecordingStatus>,
}

impl Recorder {
    pub fn status(&self) -> RecordingStatus {
        lock(&self.status).clone()
    }

    fn set_status(&self, status: RecordingStatus) {
        *lock(&self.status) = status;
    }

    /// The session slot, held across a whole start, pause, resume, stop or
    /// cancel.
    fn slot(&self) -> MutexGuard<'_, Option<RecordingSession>> {
        lock(&self.session)
    }

    /// Drops the session, if any, as `cancel_recording` does (the session
    /// folder is removed) and sets the status to idle without an event.
    /// Never waits: if a start, pause, resume, stop or cancel holds the
    /// slot, it does nothing and returns false (a start in progress may be
    /// waiting on the main thread, which is where `quit_app` runs). Returns
    /// true if a session was dropped.
    fn discard(&self) -> bool {
        let mut slot = match self.session.try_lock() {
            Ok(slot) => slot,
            Err(TryLockError::Poisoned(e)) => e.into_inner(),
            Err(TryLockError::WouldBlock) => {
                eprintln!("guidesya: a recording command is running at quit, not waiting for it");
                return false;
            }
        };
        let Some(session) = slot.take() else {
            return false;
        };
        // Dropping the session stops everything and removes its folder.
        drop(session);
        self.set_status(RecordingStatus::Idle);
        true
    }
}

/// The quit backstop: `AppHandle::exit` skips destructors, so a session
/// left running would keep its folder in the cache dir. Drops it first
/// (see [`Recorder::discard`]); the frontend normally stops and saves
/// before quitting, so this only catches what it missed.
pub fn discard_on_quit<R: Runtime>(app: &AppHandle<R>) {
    if let Some(recorder) = app.try_state::<Arc<Recorder>>() {
        if recorder.discard() {
            eprintln!("guidesya: discarded the recording still running at quit");
        }
    }
}

/// Unlike the `.lock().unwrap()` elsewhere in the app, poison is ignored
/// here: a panic inside a start or stop must not disable recording until
/// the app restarts. Both values are valid after any panic (the slot holds
/// a session or nothing; the status is a plain value).
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn emit_status<R: Runtime>(app: &AppHandle<R>, recorder: &Recorder, status: RecordingStatus) {
    recorder.set_status(status.clone());
    if let Err(e) = app.emit(STATUS_EVENT, &status) {
        eprintln!("guidesya: cannot emit {STATUS_EVENT}: {e}");
    }
}

fn session_error(e: SessionError) -> AppError {
    match e {
        SessionError::Library(e) => e.into(),
        e => AppError::new(ErrorKind::Io, e.to_string()),
    }
}

fn not_recording() -> AppError {
    AppError::new(ErrorKind::Invalid, "no recording is running")
}

/// `status` with its `recording`/`paused` state set by `paused`, keeping
/// its fields; `idle` and `saving` are returned unchanged.
fn with_paused(status: RecordingStatus, paused: bool) -> RecordingStatus {
    match status {
        RecordingStatus::Recording {
            started_at,
            title,
            capture_area_fallback,
        }
        | RecordingStatus::Paused {
            started_at,
            title,
            capture_area_fallback,
        } => {
            if paused {
                RecordingStatus::Paused {
                    started_at,
                    title,
                    capture_area_fallback,
                }
            } else {
                RecordingStatus::Recording {
                    started_at,
                    title,
                    capture_area_fallback,
                }
            }
        }
        other => other,
    }
}

/// The running session in `slot`; `invalid` when there is none (idle, or
/// a stop is saving, which takes the session out of the slot).
fn running(slot: &mut Option<RecordingSession>) -> Result<&mut RecordingSession> {
    slot.as_mut().ok_or_else(not_recording)
}

fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// `Recording 2026-10-15 14:03`, the title of an untitled recording.
fn default_title() -> String {
    format!("Recording {}", Local::now().format("%Y-%m-%d %H:%M"))
}

/// Virtual key codes (`kVK_*`) of every key the settings accept as a
/// shortcut key (see `settings::is_hotkey`): the ANSI letters and digits,
/// `F1` to `F20`, and the named keys.
const KEY_CODES: &[(&str, u16)] = &[
    ("A", 0),
    ("S", 1),
    ("D", 2),
    ("F", 3),
    ("H", 4),
    ("G", 5),
    ("Z", 6),
    ("X", 7),
    ("C", 8),
    ("V", 9),
    ("B", 11),
    ("Q", 12),
    ("W", 13),
    ("E", 14),
    ("R", 15),
    ("Y", 16),
    ("T", 17),
    ("1", 18),
    ("2", 19),
    ("3", 20),
    ("4", 21),
    ("6", 22),
    ("5", 23),
    ("9", 25),
    ("7", 26),
    ("8", 28),
    ("0", 29),
    ("O", 31),
    ("U", 32),
    ("I", 34),
    ("P", 35),
    ("L", 37),
    ("J", 38),
    ("K", 40),
    ("N", 45),
    ("M", 46),
    ("Enter", 36),
    ("Tab", 48),
    ("Space", 49),
    ("Backspace", 51),
    ("Escape", 53),
    ("F17", 64),
    ("F18", 79),
    ("F19", 80),
    ("F20", 90),
    ("F5", 96),
    ("F6", 97),
    ("F7", 98),
    ("F3", 99),
    ("F8", 100),
    ("F9", 101),
    ("F11", 103),
    ("F13", 105),
    ("F16", 106),
    ("F14", 107),
    ("F10", 109),
    ("F12", 111),
    ("F15", 113),
    ("Delete", 117),
    ("F4", 118),
    ("F2", 120),
    ("F1", 122),
];

/// Parses a shortcut as the settings store it (`CmdOrCtrl+Shift+M`: Tauri
/// modifier names joined by `+`, then one key) into a capture [`Hotkey`].
///
/// `CmdOrCtrl`, `CommandOrControl`, `Cmd`, `Command` and `Super` are
/// Command on macOS; `Ctrl` and `Control` are Control; `Alt` and `Option`
/// are Option. The key is any key the settings accept (a letter in either
/// case, a digit, `F1` to `F20`, `Space`, `Enter`, `Tab`, `Escape`,
/// `Backspace` or `Delete`) and at least one modifier is required (a bare
/// key would swallow every press of it). Anything else is `None`.
pub fn parse_hotkey(value: &str) -> Option<Hotkey> {
    let mut parts = value.split('+').map(str::trim);
    let key = parts.next_back()?;
    let mut modifiers = Modifiers::default();
    let mut count = 0;
    for part in parts {
        match part {
            "CmdOrCtrl" | "CommandOrControl" | "Cmd" | "Command" | "Super" => {
                modifiers.command = true
            }
            "Ctrl" | "Control" => modifiers.control = true,
            "Alt" | "Option" => modifiers.option = true,
            "Shift" => modifiers.shift = true,
            _ => return None,
        }
        count += 1;
    }
    if count == 0 {
        return None;
    }
    let key = key.to_ascii_uppercase();
    let (_, code) = KEY_CODES
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(&key))?;
    Some(Hotkey {
        code: *code,
        modifiers,
    })
}

/// The manual-step hotkey from the settings: `None` when the setting is
/// empty (no hotkey), the default [`Hotkey::MANUAL_STEP`] when the stored
/// value cannot be mapped.
fn manual_hotkey(value: &str) -> Option<Hotkey> {
    if value.is_empty() {
        return None;
    }
    match parse_hotkey(value) {
        Some(hotkey) => Some(hotkey),
        None => {
            eprintln!("guidesya: manual-step hotkey {value:?} is not supported, using the default");
            Some(Hotkey::MANUAL_STEP)
        }
    }
}

/// The session config for the stored settings, and whether the capture
/// area fell back to the display.
fn session_config(settings: &Settings) -> (SessionConfig, bool) {
    let recording = &settings.recording;
    let fallback = !matches!(recording.capture_area, CaptureArea::Display);
    let mut cfg = SessionConfig::default();
    cfg.frames.target = CaptureTarget::Display { display_id: None };
    cfg.frames.exclude_own_windows = true;
    cfg.excluded_bundle_ids = recording.excluded_apps.clone();
    cfg.steps.typing_idle_ms = u64::from(recording.typing_idle_ms);
    cfg.manual_hotkey = manual_hotkey(&recording.hotkeys.manual_step);
    cfg.work_dir = None;
    (cfg, fallback)
}

async fn blocking<T, F>(f: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| AppError::new(ErrorKind::Io, format!("task failed: {e}")))?
}

/// Starts recording the display. Refuses with `invalid` when a recording
/// is already running, with the library's startup error when there is no
/// open library (so a recording can never be lost to a missing library at
/// stop), and with `invalid` when a permission is missing (Screen
/// Recording, Accessibility, Input Monitoring; checked live, never
/// prompts). The session is configured from the stored settings; see the
/// module docs for what is not supported yet. Returns the new status,
/// which is also sent as [`STATUS_EVENT`].
#[tauri::command]
pub async fn start_recording<R: Runtime>(
    app: AppHandle<R>,
    recorder: State<'_, Arc<Recorder>>,
    store: State<'_, Arc<SettingsStore>>,
    cell: State<'_, Arc<SessionCell>>,
    title: Option<String>,
) -> Result<RecordingStatus> {
    let recorder = Arc::clone(&recorder);
    let store = Arc::clone(&store);
    let cell = Arc::clone(&cell);
    let title = title.map(|t| t.trim().to_owned()).filter(|t| !t.is_empty());
    blocking(move || {
        let mut slot = recorder.slot();
        if slot.is_some() || recorder.status() != RecordingStatus::Idle {
            return Err(AppError::new(
                ErrorKind::Invalid,
                "a recording is already running",
            ));
        }
        // Waits for the library still starting; refuses if it did not open.
        cell.get()?;
        settings_commands::ensure_can_record()?;
        let (cfg, capture_area_fallback) = session_config(&store.get());
        let session = RecordingSession::start(cfg).map_err(session_error)?;
        *slot = Some(session);
        let status = RecordingStatus::Recording {
            started_at: now_rfc3339(),
            title,
            capture_area_fallback,
        };
        emit_status(&app, &recorder, status.clone());
        Ok(status)
    })
    .await
}

/// Pauses the recording: input and screen capture stop until
/// `resume_recording`, and nothing in between ends up in the guide.
/// `invalid` when nothing is recording; already paused returns the current
/// status unchanged. If stopping the screen stream fails the session is
/// paused anyway: the status becomes `paused` and the error is returned.
/// Returns the new status, also sent as [`STATUS_EVENT`].
#[tauri::command]
pub async fn pause_recording<R: Runtime>(
    app: AppHandle<R>,
    recorder: State<'_, Arc<Recorder>>,
) -> Result<RecordingStatus> {
    let recorder = Arc::clone(&recorder);
    blocking(move || set_paused(&app, &recorder, true)).await
}

/// Resumes a paused recording (restarting screen capture takes a moment,
/// as at start). `invalid` when nothing is recording; not paused returns
/// the current status unchanged. On error the session stays paused, the
/// status stays `paused` and the error is returned. Returns the new
/// status, also sent as [`STATUS_EVENT`].
#[tauri::command]
pub async fn resume_recording<R: Runtime>(
    app: AppHandle<R>,
    recorder: State<'_, Arc<Recorder>>,
) -> Result<RecordingStatus> {
    let recorder = Arc::clone(&recorder);
    blocking(move || set_paused(&app, &recorder, false)).await
}

/// Pauses (`paused`) or resumes the session, holding the slot throughout.
/// The status follows `RecordingSession::is_paused` after the call, so it
/// is right whether or not the call failed.
fn set_paused<R: Runtime>(
    app: &AppHandle<R>,
    recorder: &Recorder,
    paused: bool,
) -> Result<RecordingStatus> {
    let mut slot = recorder.slot();
    let session = running(&mut slot)?;
    if session.is_paused() == paused {
        return Ok(recorder.status());
    }
    let result = if paused {
        session.pause()
    } else {
        session.resume()
    };
    let current = recorder.status();
    let status = with_paused(current.clone(), session.is_paused());
    if status != current {
        emit_status(app, recorder, status.clone());
    }
    result.map_err(session_error)?;
    Ok(status)
}

/// Stops the recording (paused or not), runs step detection and saves the
/// guide into the open library (titled as given to `start_recording`, else
/// `Recording <date time>`). The status is `saving` meanwhile and `idle`
/// after, whether or not the save succeeded; a failed save discards the
/// recording. Returns the saved guide, also sent as [`SAVED_EVENT`].
#[tauri::command]
pub async fn stop_recording<R: Runtime>(
    app: AppHandle<R>,
    recorder: State<'_, Arc<Recorder>>,
    cell: State<'_, Arc<SessionCell>>,
) -> Result<RecordingSaved> {
    let recorder = Arc::clone(&recorder);
    let cell = Arc::clone(&cell);
    blocking(move || {
        let mut slot = recorder.slot();
        let session = slot.take().ok_or_else(not_recording)?;
        let (started_at, title) = match recorder.status() {
            RecordingStatus::Recording {
                started_at, title, ..
            }
            | RecordingStatus::Paused {
                started_at, title, ..
            } => (started_at, title),
            _ => (now_rfc3339(), None),
        };
        emit_status(
            &app,
            &recorder,
            RecordingStatus::Saving {
                started_at,
                title: title.clone(),
            },
        );
        let saved = save(session, cell, title.unwrap_or_else(default_title));
        emit_status(&app, &recorder, RecordingStatus::Idle);
        let saved = saved?;
        if let Err(e) = app.emit(SAVED_EVENT, &saved) {
            eprintln!("guidesya: cannot emit {SAVED_EVENT}: {e}");
        }
        Ok(saved)
    })
    .await
}

/// Stops `session`, writes its guide into the library and indexes it.
fn save(
    session: RecordingSession,
    cell: Arc<SessionCell>,
    title: String,
) -> Result<RecordingSaved> {
    // Dropping the `Recording` on any error removes the session folder.
    let recording = session.stop().map_err(session_error)?;
    let library = cell.get()?;
    let saved = recording
        .save_guide(library.library(), &title)
        .map_err(session_error)?;
    library.index_bundle(&saved.handle);
    Ok(RecordingSaved {
        guide_id: saved.guide.id.to_string(),
        steps: saved.guide.steps().count(),
        skipped: saved.skipped,
    })
}

/// Stops the recording (paused or not) and discards it (the session
/// folder is removed). `invalid` when nothing is recording.
#[tauri::command]
pub async fn cancel_recording<R: Runtime>(
    app: AppHandle<R>,
    recorder: State<'_, Arc<Recorder>>,
) -> Result<()> {
    let recorder = Arc::clone(&recorder);
    blocking(move || {
        let mut slot = recorder.slot();
        let session = slot.take().ok_or_else(not_recording)?;
        // Dropping the session stops everything and removes its folder.
        drop(session);
        emit_status(&app, &recorder, RecordingStatus::Idle);
        Ok(())
    })
    .await
}

/// The current [`RecordingStatus`]; never waits on a start or stop in
/// progress.
#[tauri::command]
pub fn recording_status(recorder: State<'_, Arc<Recorder>>) -> RecordingStatus {
    recorder.status()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn mods(shift: bool, control: bool, option: bool, command: bool) -> Modifiers {
        Modifiers {
            shift,
            control,
            option,
            command,
            function: false,
            caps_lock: false,
        }
    }

    #[test]
    fn parses_settings_style_hotkeys() {
        assert_eq!(
            parse_hotkey("CmdOrCtrl+Shift+M"),
            Some(Hotkey {
                code: 46,
                modifiers: mods(true, false, false, true)
            })
        );
        assert_eq!(parse_hotkey("Ctrl+Alt+S"), Some(Hotkey::MANUAL_STEP));
        assert_eq!(parse_hotkey("Control+Option+s"), Some(Hotkey::MANUAL_STEP));
        assert_eq!(
            parse_hotkey("Command+1"),
            Some(Hotkey {
                code: 18,
                modifiers: mods(false, false, false, true)
            })
        );
        assert_eq!(
            parse_hotkey("Super+Shift+0"),
            Some(Hotkey {
                code: 29,
                modifiers: mods(true, false, false, true)
            })
        );
        assert_eq!(
            parse_hotkey("CommandOrControl+Ctrl+Alt+Shift+Z"),
            Some(Hotkey {
                code: 6,
                modifiers: mods(true, true, true, true)
            })
        );
    }

    #[test]
    fn parses_function_and_named_keys() {
        for (value, code) in [
            ("CmdOrCtrl+F5", 96),
            ("Cmd+f1", 122),
            ("Ctrl+F20", 90),
            ("Cmd+Space", 49),
            ("Cmd+Enter", 36),
            ("Cmd+Tab", 48),
            ("Cmd+Escape", 53),
            ("Cmd+Backspace", 51),
            ("Cmd+Delete", 117),
        ] {
            assert_eq!(parse_hotkey(value).map(|h| h.code), Some(code), "{value}");
        }
    }

    /// Every key `settings::is_hotkey` accepts maps to a distinct code, so
    /// no stored shortcut falls back to the default.
    #[test]
    fn every_key_the_settings_accept_has_a_code() {
        let mut keys: Vec<String> = ('A'..='Z').chain('0'..='9').map(String::from).collect();
        keys.extend((1..=20).map(|n| format!("F{n}")));
        keys.extend(
            ["Space", "Enter", "Tab", "Escape", "Backspace", "Delete"]
                .iter()
                .map(|k| k.to_string()),
        );
        assert_eq!(keys.len(), KEY_CODES.len());
        for key in &keys {
            let value = format!("CmdOrCtrl+{key}");
            assert!(crate::settings::is_hotkey(&value), "{value}");
            let hotkey = parse_hotkey(&value).unwrap_or_else(|| panic!("{value}"));
            assert_eq!(
                KEY_CODES
                    .iter()
                    .filter(|(_, code)| *code == hotkey.code)
                    .count(),
                1,
                "{key} shares a code"
            );
        }
    }

    #[test]
    fn rejects_unsupported_hotkeys() {
        for bad in [
            "",
            "M",
            "Shift",
            "Cmd+",
            "Cmd+F21",
            "Cmd+F0",
            "Cmd+Home",
            "Cmd+Esc",
            "Meta+M",
            "Cmd+Shift+MM",
            "Cmd+Shift+é",
            "Cmd++M",
        ] {
            assert_eq!(parse_hotkey(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn manual_hotkey_falls_back() {
        assert_eq!(manual_hotkey(""), None);
        assert_eq!(manual_hotkey("Cmd+Home"), Some(Hotkey::MANUAL_STEP));
        assert_eq!(manual_hotkey("CmdOrCtrl+Shift+M").map(|h| h.code), Some(46));
    }

    #[test]
    fn session_config_from_settings() {
        let mut settings = Settings::default();
        settings.recording.excluded_apps = vec!["com.apple.keychainaccess".into()];
        settings.recording.typing_idle_ms = 700;
        settings.recording.hotkeys.manual_step = "Ctrl+Alt+S".into();
        let (cfg, fallback) = session_config(&settings);
        assert!(!fallback);
        assert_eq!(
            cfg.frames.target,
            CaptureTarget::Display { display_id: None }
        );
        assert!(cfg.frames.exclude_own_windows);
        assert_eq!(cfg.excluded_bundle_ids, ["com.apple.keychainaccess"]);
        assert_eq!(cfg.steps.typing_idle_ms, 700);
        assert_eq!(cfg.manual_hotkey, Some(Hotkey::MANUAL_STEP));
        assert_eq!(cfg.work_dir, None);

        settings.recording.capture_area = CaptureArea::Window;
        let (cfg, fallback) = session_config(&settings);
        assert!(fallback);
        assert_eq!(
            cfg.frames.target,
            CaptureTarget::Display { display_id: None }
        );
    }

    #[test]
    fn status_and_saved_payloads_are_camel_case() {
        assert_eq!(
            serde_json::to_value(RecordingStatus::Idle).unwrap(),
            json!({"state": "idle"})
        );
        assert_eq!(
            serde_json::to_value(RecordingStatus::Recording {
                started_at: "2026-10-15T01:02:03.000Z".into(),
                title: None,
                capture_area_fallback: true,
            })
            .unwrap(),
            json!({
                "state": "recording",
                "startedAt": "2026-10-15T01:02:03.000Z",
                "title": null,
                "captureAreaFallback": true,
            })
        );
        assert_eq!(
            serde_json::to_value(RecordingStatus::Paused {
                started_at: "2026-10-15T01:02:03.000Z".into(),
                title: Some("Rotate a key".into()),
                capture_area_fallback: false,
            })
            .unwrap(),
            json!({
                "state": "paused",
                "startedAt": "2026-10-15T01:02:03.000Z",
                "title": "Rotate a key",
                "captureAreaFallback": false,
            })
        );
        assert_eq!(
            serde_json::to_value(RecordingStatus::Saving {
                started_at: "2026-10-15T01:02:03.000Z".into(),
                title: Some("Rotate a key".into()),
            })
            .unwrap(),
            json!({
                "state": "saving",
                "startedAt": "2026-10-15T01:02:03.000Z",
                "title": "Rotate a key",
            })
        );
        assert_eq!(
            serde_json::to_value(RecordingSaved {
                guide_id: "01J".into(),
                steps: 3,
                skipped: 1,
            })
            .unwrap(),
            json!({"guideId": "01J", "steps": 3, "skipped": 1})
        );
    }

    #[test]
    fn recorder_starts_idle_and_refuses_to_stop() {
        let recorder = Recorder::default();
        assert_eq!(recorder.status(), RecordingStatus::Idle);
        let mut slot = recorder.slot();
        match slot.take().ok_or_else(not_recording) {
            Ok(_) => panic!("a session out of nowhere"),
            Err(err) => assert_eq!(err.kind, ErrorKind::Invalid),
        }
    }

    #[test]
    fn pause_and_resume_refuse_when_idle() {
        let recorder = Recorder::default();
        let mut slot = recorder.slot();
        match running(&mut slot) {
            Ok(_) => panic!("a session out of nowhere"),
            Err(err) => {
                assert_eq!(err.kind, ErrorKind::Invalid);
                assert_eq!(err.message, "no recording is running");
            }
        }
    }

    #[test]
    fn pausing_and_resuming_keep_the_status_fields() {
        let recording = RecordingStatus::Recording {
            started_at: "2026-10-15T01:02:03.000Z".into(),
            title: Some("Rotate a key".into()),
            capture_area_fallback: true,
        };
        let paused = RecordingStatus::Paused {
            started_at: "2026-10-15T01:02:03.000Z".into(),
            title: Some("Rotate a key".into()),
            capture_area_fallback: true,
        };
        assert_eq!(with_paused(recording.clone(), true), paused);
        assert_eq!(with_paused(paused.clone(), false), recording);
        assert_eq!(with_paused(paused.clone(), true), paused);
        assert_eq!(with_paused(recording.clone(), false), recording);
        assert_eq!(
            with_paused(RecordingStatus::Idle, true),
            RecordingStatus::Idle
        );
        let saving = RecordingStatus::Saving {
            started_at: "2026-10-15T01:02:03.000Z".into(),
            title: None,
        };
        assert_eq!(with_paused(saving.clone(), true), saving);
    }

    #[test]
    fn discard_never_waits_and_is_a_no_op_when_idle() {
        let recorder = Recorder::default();
        assert!(!recorder.discard());
        assert_eq!(recorder.status(), RecordingStatus::Idle);
        // A command holding the slot (e.g. a start waiting on the main
        // thread) must not block the quit.
        let slot = recorder.slot();
        recorder.set_status(RecordingStatus::Saving {
            started_at: "2026-10-15T01:02:03.000Z".into(),
            title: None,
        });
        assert!(!recorder.discard());
        assert!(matches!(recorder.status(), RecordingStatus::Saving { .. }));
        drop(slot);
    }

    #[test]
    fn default_title_names_the_recording() {
        let title = default_title();
        assert!(title.starts_with("Recording 20"), "{title}");
        assert_eq!(title.len(), "Recording 2026-10-15 14:03".len(), "{title}");
    }
}
