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
//! [`StampedStatus`] payload (the status plus a `seq` that grows with each
//! change; commands return the same); a saved recording is announced as
//! [`SAVED_EVENT`] with a [`RecordingSaved`] payload (the same value the
//! `stop_recording` command returns).
//!
//! While an excluded app (settings: `recording.excludedApps`) is
//! frontmost the session captures nothing; the status says which one in
//! `excludedApp`, and a new [`STATUS_EVENT`] is sent each time that
//! changes (see the capture crate's `# Excluded apps`).
//!
//! The status carries the paused time (`pausedMs`, and `pausedAt` while
//! paused), so the UI's elapsed clock survives a webview reload.
//!
//! While recording, each screenshot the session encodes is announced as
//! [`SNAPSHOT_EVENT`] with a [`RecordingSnapshot`] payload, so the UI can
//! show what has been captured so far; `recording_snapshot` serves the
//! image. These are provisional: the steps, and which screenshots they
//! keep, are only decided at stop (see the capture crate's
//! `LiveSnapshot`).
//!
//! The library is checked at start (no open library, no recording) and
//! read again at stop: switching the library mid-recording saves into the
//! library open at stop time.
//!
//! Quitting the app drops any session still running ([`discard_on_quit`]),
//! as `cancel_recording` would; the frontend stops and saves first.
//!
//! The tray menu and the global hotkeys drive the same code as the
//! commands through [`control`]; every status sent also updates the tray
//! icon and opens or closes the recording toolbar window.
//!
//! `start_recording` takes an optional capture target (see
//! [`crate::picker`]). Without one the area comes from the settings, where
//! Window and Region fall back to every display (`captureAreaFallback`).
//! A start from the tray or ⌃⌥R with Window or Region stored does not
//! record: it shows the main window and sends [`PICK_AREA_EVENT`], and the
//! main window picks the area and starts with it.
//!
//! Not in this slice: after-frames per step (the capture crate has no
//! support yet).

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, TryLockError};

use chrono::{DateTime, Local, SecondsFormat, Utc};
use guidesya_capture::macos::frames::CaptureTarget;
use guidesya_capture::macos::session::{
    ExclusionSink, Hotkey, LiveSnapshot, RecordingSession, SessionConfig, SessionError,
    SnapshotSink,
};
use guidesya_core::events::Modifiers;
use guidesya_core::model::Point;
use serde::Serialize;
use tauri::ipc::Response;
use tauri::{AppHandle, Emitter, Manager, Runtime, State, WebviewWindow};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

use crate::blocking::blocking;
use crate::editor::{AppError, ErrorKind, Result, SessionCell};
use crate::picker::{self, CaptureTargetArg};
use crate::settings::{CaptureArea, Settings, SettingsStore};
use crate::settings_commands;

/// Emitted with a [`RecordingStatus`] whenever the status changes.
pub const STATUS_EVENT: &str = "recording://status";
/// Emitted with a [`RecordingSaved`] once a stopped recording is in the
/// library.
pub const SAVED_EVENT: &str = "recording://saved";
/// Emitted with a [`RecordingSnapshot`] for each screenshot the running
/// session encodes.
pub const SNAPSHOT_EVENT: &str = "recording://snapshot";
/// Emitted (no payload) when the tray or the hotkey asks to start while
/// the stored capture area needs picking: the main window picks it and
/// calls `start_recording` with the target.
pub const PICK_AREA_EVENT: &str = "recording://pick-area";

/// What the recorder is doing. Serialized as `{"state": "idle"}`,
/// `{"state": "recording", "startedAt": …, "title": …,
/// "captureAreaFallback": …, "pausedMs": …, "excludedApp": …}`,
/// `{"state": "paused", …}` with the fields of `recording` plus
/// `"pausedAt"`, or `{"state": "saving", "startedAt": …, "title": …}`.
///
/// Time spent recording so far is `now − startedAt − pausedMs`, less
/// `now − pausedAt` while paused.
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
        /// Started without a target while the stored capture area is
        /// Window or Region, so every display is being captured.
        capture_area_fallback: bool,
        /// Milliseconds spent in finished pauses.
        paused_ms: u64,
        /// Bundle id of the excluded app in front, while one is: nothing
        /// is captured meanwhile.
        excluded_app: Option<String>,
    },
    /// Between `pause_recording` and `resume_recording`; the fields are
    /// those of the `Recording` it paused, plus when the pause began.
    Paused {
        started_at: String,
        title: Option<String>,
        capture_area_fallback: bool,
        /// Milliseconds spent in earlier, finished pauses (not this one).
        paused_ms: u64,
        /// RFC 3339, UTC: when this pause began.
        paused_at: String,
        /// As for `Recording`; tracked while paused too.
        excluded_app: Option<String>,
    },
    Saving {
        started_at: String,
        title: Option<String>,
    },
}

/// A [`RecordingStatus`] as the [`Recorder`] stored it, serialized as the
/// status with a `seq` field added. `seq` grows with every change, so the
/// webview can drop a status that arrives after a newer one (events and
/// command results are sent after the lock is released, so they can
/// overtake each other).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct StampedStatus {
    pub seq: u64,
    #[serde(flatten)]
    pub status: RecordingStatus,
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

/// A screenshot the running session took (sent as [`SNAPSHOT_EVENT`]).
/// Provisional: see the module docs.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingSnapshot {
    /// Names the image for `recording_snapshot`; unique within the
    /// recording.
    pub id: String,
    /// When the event that took it happened: host monotonic nanoseconds,
    /// for ordering only.
    pub ts: u64,
    /// Image size in pixels.
    pub width: u32,
    pub height: u32,
    /// Where the click landed, in image pixels; none for a key.
    pub point: Option<Point>,
}

impl RecordingSnapshot {
    fn of(snapshot: &LiveSnapshot) -> Self {
        Self {
            id: snapshot.frame.file_stem(),
            ts: snapshot.ts.0,
            width: snapshot.width,
            height: snapshot.height,
            point: snapshot.point,
        }
    }
}

/// True for an id `recording_snapshot` accepts: a file stem as
/// `FrameKey::file_stem` makes them, so never a path.
fn is_snapshot_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_digit() || b == b'-')
}

/// The managed recording state: at most one session at a time.
///
/// `session` is held for the whole of a start, pause, resume, stop or
/// cancel, so those serialize; `status` is only ever held briefly, so `recording_status`
/// never waits on a start or stop in progress.
#[derive(Default)]
pub struct Recorder {
    session: Mutex<Option<RecordingSession>>,
    status: Mutex<StampedStatus>,
    /// A [`control`] is running; another one meanwhile is ignored (a
    /// double press must not start a second recording or show an error).
    controlling: AtomicBool,
}

impl Recorder {
    /// The current status with the `seq` it was stored with.
    pub fn status(&self) -> StampedStatus {
        lock(&self.status).clone()
    }

    /// Stores `status` under the next `seq` and returns it stamped.
    fn set_status(&self, status: RecordingStatus) -> StampedStatus {
        let mut stored = lock(&self.status);
        stored.seq += 1;
        stored.status = status;
        stored.clone()
    }

    /// Replaces the status with `f(current)` in one step (so a concurrent
    /// update is never lost) and returns it, with whether it changed; only
    /// a change takes the next `seq`.
    fn update(&self, f: impl FnOnce(&RecordingStatus) -> RecordingStatus) -> (StampedStatus, bool) {
        let mut stored = lock(&self.status);
        let next = f(&stored.status);
        let changed = next != stored.status;
        if changed {
            stored.seq += 1;
            stored.status = next;
        }
        (stored.clone(), changed)
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

/// Stores `status` and sends it; returns it stamped.
fn emit_status<R: Runtime>(
    app: &AppHandle<R>,
    recorder: &Recorder,
    status: RecordingStatus,
) -> StampedStatus {
    let stamped = recorder.set_status(status);
    emit(app, &stamped);
    stamped
}

/// Sends `status` as [`STATUS_EVENT`] (the caller has stored it), and
/// brings the tray icon and the toolbar window in line with the stored
/// status (read again, so a status sent late never wins).
fn emit<R: Runtime>(app: &AppHandle<R>, status: &StampedStatus) {
    if let Err(e) = app.emit(STATUS_EVENT, status) {
        eprintln!("guidesya: cannot emit {STATUS_EVENT}: {e}");
    }
    crate::tray::refresh(app);
    crate::toolbar::sync(app);
}

fn session_error(e: SessionError) -> AppError {
    match e {
        SessionError::Library(e) => e.into(),
        e => AppError::new(ErrorKind::Io, e.to_string()),
    }
}

const NOT_RECORDING: &str = "no recording is running";
const ALREADY_RUNNING: &str = "a recording is already running";

fn not_recording() -> AppError {
    AppError::new(ErrorKind::Invalid, NOT_RECORDING)
}

/// True for the refusal of an action that lost a race to another one (a
/// start finding a recording already running, a pause or stop finding none
/// left): the state it was resolved from is gone, so it is a no-op.
fn lost_race(e: &AppError) -> bool {
    e.kind == ErrorKind::Invalid && (e.message == ALREADY_RUNNING || e.message == NOT_RECORDING)
}

/// `status` with its `recording`/`paused` state set by `paused` at
/// `now`, keeping its fields: pausing records `pausedAt`, resuming adds
/// the pause to `pausedMs`. Already in that state, `idle` and `saving`
/// are returned unchanged.
fn with_paused(status: RecordingStatus, paused: bool, now: DateTime<Utc>) -> RecordingStatus {
    match status {
        RecordingStatus::Recording {
            started_at,
            title,
            capture_area_fallback,
            paused_ms,
            excluded_app,
        } if paused => RecordingStatus::Paused {
            started_at,
            title,
            capture_area_fallback,
            paused_ms,
            paused_at: rfc3339(now),
            excluded_app,
        },
        RecordingStatus::Paused {
            started_at,
            title,
            capture_area_fallback,
            paused_ms,
            paused_at,
            excluded_app,
        } if !paused => RecordingStatus::Recording {
            started_at,
            title,
            capture_area_fallback,
            paused_ms: paused_ms.saturating_add(ms_since(&paused_at, now)),
            excluded_app,
        },
        other => other,
    }
}

/// `status` with its `excludedApp` set to `app`; `idle` and `saving` are
/// returned unchanged.
fn with_excluded(mut status: RecordingStatus, app: Option<&str>) -> RecordingStatus {
    if let RecordingStatus::Recording { excluded_app, .. }
    | RecordingStatus::Paused { excluded_app, .. } = &mut status
    {
        *excluded_app = app.map(str::to_owned);
    }
    status
}

/// Whole milliseconds from the RFC 3339 time `at` to `now`; 0 when `at`
/// is later or unreadable.
fn ms_since(at: &str, now: DateTime<Utc>) -> u64 {
    DateTime::parse_from_rfc3339(at)
        .map(|at| (now - at.with_timezone(&Utc)).num_milliseconds())
        .map_or(0, |ms| u64::try_from(ms).unwrap_or(0))
}

/// The running session in `slot`; `invalid` when there is none (idle, or
/// a stop is saving, which takes the session out of the slot).
fn running(slot: &mut Option<RecordingSession>) -> Result<&mut RecordingSession> {
    slot.as_mut().ok_or_else(not_recording)
}

fn now_rfc3339() -> String {
    rfc3339(Utc::now())
}

fn rfc3339(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Millis, true)
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

/// The session config for the stored settings capturing `target`, and
/// whether the capture area fell back to the displays (no target given
/// while the stored area is Window or Region).
fn session_config(settings: &Settings, target: Option<CaptureTarget>) -> (SessionConfig, bool) {
    let recording = &settings.recording;
    let fallback = target.is_none() && !matches!(recording.capture_area, CaptureArea::Display);
    let mut cfg = SessionConfig::default();
    // By default every active display, one stream each (see
    // `session::stream_targets`).
    cfg.frames.target = target.unwrap_or(CaptureTarget::Display { display_id: None });
    cfg.frames.exclude_own_windows = true;
    cfg.excluded_bundle_ids = recording.excluded_apps.clone();
    cfg.steps.typing_idle_ms = u64::from(recording.typing_idle_ms);
    cfg.manual_hotkey = manual_hotkey(&recording.hotkeys.manual_step);
    // Handled by the app (start/stop, pause): never steps.
    cfg.control_hotkeys = crate::hotkeys::capture_hotkeys();
    cfg.work_dir = None;
    (cfg, fallback)
}

/// Starts recording the display. Refuses with `invalid` when a recording
/// is already running, with the library's startup error when there is no
/// open library (so a recording can never be lost to a missing library at
/// stop), and with `invalid` when a permission is missing (Screen
/// Recording, Accessibility, Input Monitoring; checked live, never
/// prompts). The session is configured from the stored settings and
/// captures `target` when given (`invalid` for a window no longer open or
/// a region under 40×40 points; see [`crate::picker::capture_target`]),
/// else the stored area (see the module docs). Returns the new status,
/// which is also sent as [`STATUS_EVENT`].
#[tauri::command]
pub async fn start_recording<R: Runtime>(
    app: AppHandle<R>,
    recorder: State<'_, Arc<Recorder>>,
    store: State<'_, Arc<SettingsStore>>,
    cell: State<'_, Arc<SessionCell>>,
    title: Option<String>,
    target: Option<CaptureTargetArg>,
) -> Result<StampedStatus> {
    let recorder = Arc::clone(&recorder);
    let store = Arc::clone(&store);
    let cell = Arc::clone(&cell);
    blocking(move || start(&app, &recorder, &store, &cell, title, target)).await
}

/// The body of `start_recording` (blocking), also run from [`control`].
fn start<R: Runtime>(
    app: &AppHandle<R>,
    recorder: &Arc<Recorder>,
    store: &SettingsStore,
    cell: &SessionCell,
    title: Option<String>,
    target: Option<CaptureTargetArg>,
) -> Result<StampedStatus> {
    let title = title.map(|t| t.trim().to_owned()).filter(|t| !t.is_empty());
    let mut slot = recorder.slot();
    if slot.is_some() || recorder.status().status != RecordingStatus::Idle {
        return Err(AppError::new(ErrorKind::Invalid, ALREADY_RUNNING));
    }
    // Waits for the library still starting; refuses if it did not open.
    cell.get()?;
    settings_commands::ensure_can_record()?;
    let settings = store.get();
    let target = target
        .map(|t| picker::resolve_target(t, &settings.recording.excluded_apps))
        .transpose()?;
    let (mut cfg, capture_area_fallback) = session_config(&settings, target);
    let emitter = app.clone();
    cfg.on_snapshot = Some(SnapshotSink::new(move |snapshot| {
        if let Err(e) = emitter.emit(SNAPSHOT_EVENT, RecordingSnapshot::of(snapshot)) {
            eprintln!("guidesya: cannot emit {SNAPSHOT_EVENT}: {e}");
        }
    }));
    cfg.on_excluded = Some(exclusion_sink(app, recorder));
    let session = RecordingSession::start(cfg).map_err(session_error)?;
    let status = emit_status(
        app,
        recorder,
        RecordingStatus::Recording {
            started_at: now_rfc3339(),
            title,
            capture_area_fallback,
            paused_ms: 0,
            excluded_app: None,
        },
    );
    // The sink's initial read came while the status was still idle, so
    // it was ignored; from here on the sink applies its changes. Read
    // the session's state now, inside the status lock, so a change the
    // sink reports meanwhile is never overwritten by an older one.
    let status = sync_excluded(app, recorder, &session).unwrap_or(status);
    *slot = Some(session);
    Ok(status)
}

/// Sets the status's `excludedApp` to the session's, read inside the
/// status lock; sends and returns the new status if that changed it.
fn sync_excluded<R: Runtime>(
    app: &AppHandle<R>,
    recorder: &Recorder,
    session: &RecordingSession,
) -> Option<StampedStatus> {
    let (status, changed) =
        recorder.update(|s| with_excluded(s.clone(), session.excluded_app().as_deref()));
    changed.then(|| {
        emit(app, &status);
        status
    })
}

/// Puts each change of excluded app in front into the status and sends
/// it. Runs on the main thread (or a command's thread for the initial
/// read), so it only takes the status lock, never the session slot.
/// Holds the recorder weakly: the session it belongs to lives inside it.
fn exclusion_sink<R: Runtime>(app: &AppHandle<R>, recorder: &Arc<Recorder>) -> ExclusionSink {
    let app = app.clone();
    let recorder = Arc::downgrade(recorder);
    ExclusionSink::new(move |excluded: Option<&str>| {
        let Some(recorder) = recorder.upgrade() else {
            return;
        };
        let (status, changed) = recorder.update(|s| with_excluded(s.clone(), excluded));
        if changed {
            emit(&app, &status);
        }
    })
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
) -> Result<StampedStatus> {
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
) -> Result<StampedStatus> {
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
) -> Result<StampedStatus> {
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
    let now_paused = session.is_paused();
    // The excluded app is read inside the status lock, as in `sync_excluded`.
    let (status, changed) = recorder.update(|s| {
        with_excluded(
            with_paused(s.clone(), now_paused, Utc::now()),
            session.excluded_app().as_deref(),
        )
    });
    if changed {
        emit(app, &status);
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
    window: WebviewWindow<R>,
    recorder: State<'_, Arc<Recorder>>,
    cell: State<'_, Arc<SessionCell>>,
) -> Result<RecordingSaved> {
    let recorder = Arc::clone(&recorder);
    let cell = Arc::clone(&cell);
    // From the toolbar: the main window's editor opens the saved guide, so
    // show it, as a stop from the tray or a hotkey does.
    if shows_main_on_stop(window.label()) {
        crate::tray::show_main(&app);
    }
    blocking(move || stop(&app, &recorder, cell)).await
}

/// True when a stop from the window `label` shows the main window first.
fn shows_main_on_stop(label: &str) -> bool {
    label == crate::toolbar::LABEL
}

/// The body of `stop_recording` (blocking), also run from [`control`].
fn stop<R: Runtime>(
    app: &AppHandle<R>,
    recorder: &Recorder,
    cell: Arc<SessionCell>,
) -> Result<RecordingSaved> {
    let mut slot = recorder.slot();
    let session = slot.take().ok_or_else(not_recording)?;
    let (started_at, title) = match recorder.status().status {
        RecordingStatus::Recording {
            started_at, title, ..
        }
        | RecordingStatus::Paused {
            started_at, title, ..
        } => (started_at, title),
        _ => (now_rfc3339(), None),
    };
    emit_status(
        app,
        recorder,
        RecordingStatus::Saving {
            started_at,
            title: title.clone(),
        },
    );
    let saved = save(session, cell, title.unwrap_or_else(default_title));
    emit_status(app, recorder, RecordingStatus::Idle);
    let saved = saved?;
    if let Err(e) = app.emit(SAVED_EVENT, &saved) {
        eprintln!("guidesya: cannot emit {SAVED_EVENT}: {e}");
    }
    Ok(saved)
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

/// The WebP bytes of a screenshot the running session announced as
/// [`SNAPSHOT_EVENT`], by its `id`. `invalid` for an id that is not one
/// or when nothing is recording (paused is fine), `notFound` when the file
/// is gone (the session pruned it at stop). The session slot is held only
/// to find the session folder, not while reading.
#[tauri::command]
pub async fn recording_snapshot(
    recorder: State<'_, Arc<Recorder>>,
    id: String,
) -> Result<Response> {
    if !is_snapshot_id(&id) {
        return Err(AppError::new(ErrorKind::Invalid, "not a snapshot id"));
    }
    let recorder = Arc::clone(&recorder);
    blocking(move || {
        let dir = {
            let slot = recorder.slot();
            slot.as_ref()
                .and_then(|session| session.session_dir().map(Path::to_path_buf))
                .ok_or_else(not_recording)?
        };
        let path = dir.join(format!("{id}.webp"));
        let bytes = std::fs::read(&path).map_err(|e| {
            let kind = if e.kind() == std::io::ErrorKind::NotFound {
                ErrorKind::NotFound
            } else {
                ErrorKind::Io
            };
            AppError::new(kind, format!("{}: {e}", path.display()))
        })?;
        Ok(Response::new(bytes))
    })
    .await
}

/// The current [`RecordingStatus`], stamped; never waits on a start or
/// stop in progress.
#[tauri::command]
pub fn recording_status(recorder: State<'_, Arc<Recorder>>) -> StampedStatus {
    recorder.status()
}

/// Adds a manual step from the screen as it is now, as the manual-step
/// hotkey does (see `RecordingSession::manual_step`; dropped while an
/// excluded app is in front). `invalid` ("no recording is running") unless
/// a recording is running: idle, paused and saving all refuse.
#[tauri::command]
pub async fn add_manual_step(recorder: State<'_, Arc<Recorder>>) -> Result<()> {
    let recorder = Arc::clone(&recorder);
    blocking(move || manual_step(&recorder)).await
}

/// The body of `add_manual_step`. Holds the slot, so it waits for a
/// start, pause or resume in progress.
fn manual_step(recorder: &Recorder) -> Result<()> {
    let slot = recorder.slot();
    match slot.as_ref() {
        Some(session) if !session.is_paused() && session.manual_step() => Ok(()),
        _ => Err(not_recording()),
    }
}

/// A recording action from outside the webview: the tray menu or a global
/// hotkey.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    Start,
    Pause,
    Resume,
    Stop,
    /// Start when idle, stop when recording or paused.
    ToggleRecording,
    /// Pause when recording, resume when paused.
    TogglePause,
}

impl Control {
    /// What `self` does in `status`: a toggle becomes the action it stands
    /// for; `None` when nothing applies (e.g. a stale menu item, or any
    /// action while saving), so nothing runs and no error is shown.
    pub fn resolve(self, status: &RecordingStatus) -> Option<Control> {
        use RecordingStatus as S;
        match (self, status) {
            (Control::Start | Control::ToggleRecording, S::Idle) => Some(Control::Start),
            (Control::Pause | Control::TogglePause, S::Recording { .. }) => Some(Control::Pause),
            (Control::Resume | Control::TogglePause, S::Paused { .. }) => Some(Control::Resume),
            (Control::Stop | Control::ToggleRecording, S::Recording { .. } | S::Paused { .. }) => {
                Some(Control::Stop)
            }
            _ => None,
        }
    }
}

/// What a start from the tray or a hotkey does for the stored capture
/// area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartFrom {
    /// Record now, from the stored settings.
    Record,
    /// Show the main window and send [`PICK_AREA_EVENT`] instead: the area
    /// is picked there.
    PickArea,
}

impl StartFrom {
    pub fn of(area: CaptureArea) -> Self {
        match area {
            CaptureArea::Display => Self::Record,
            CaptureArea::Window | CaptureArea::Region => Self::PickArea,
        }
    }
}

/// Runs `control` on the blocking pool through the same code as the
/// commands, so statuses (and `recording://saved`) are sent as usual. A
/// start uses the stored settings and no title; with Window or Region
/// stored it records nothing and asks the main window to pick the area
/// (see [`StartFrom`]). A stop shows the main
/// window, whose editor opens the saved guide. Errors are shown in a
/// dialog. Ignored while another control is still running.
pub fn control<R: Runtime>(app: &AppHandle<R>, control: Control) {
    let recorder = Arc::clone(app.state::<Arc<Recorder>>().inner());
    if recorder.controlling.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let result = run_control(&app, &recorder, control);
        recorder.controlling.store(false, Ordering::SeqCst);
        if let Err(e) = result {
            eprintln!("guidesya: {control:?} failed: {}", e.message);
            app.dialog()
                .message(e.message)
                .title("GuidesYa")
                .kind(MessageDialogKind::Error)
                .show(|_| {});
        }
    });
}

/// Runs `control` for the current status. An action that lost a race to
/// another (see [`lost_race`]) is a no-op, not an error.
fn run_control<R: Runtime>(
    app: &AppHandle<R>,
    recorder: &Arc<Recorder>,
    control: Control,
) -> Result<()> {
    let Some(action) = control.resolve(&recorder.status().status) else {
        return Ok(());
    };
    match run_action(app, recorder, action) {
        Err(e) if lost_race(&e) => Ok(()),
        result => result,
    }
}

fn run_action<R: Runtime>(
    app: &AppHandle<R>,
    recorder: &Arc<Recorder>,
    action: Control,
) -> Result<()> {
    let cell = Arc::clone(app.state::<Arc<SessionCell>>().inner());
    match action {
        Control::Start => {
            let store = Arc::clone(app.state::<Arc<SettingsStore>>().inner());
            match StartFrom::of(store.get().recording.capture_area) {
                StartFrom::Record => start(app, recorder, &store, &cell, None, None).map(drop),
                StartFrom::PickArea => {
                    // A pick already under way: bring its overlay forward.
                    if let Some(overlay) = app.get_webview_window(picker::LABEL) {
                        let _ = overlay.set_focus();
                        return Ok(());
                    }
                    crate::tray::show_main(app);
                    if let Err(e) = app.emit(PICK_AREA_EVENT, ()) {
                        eprintln!("guidesya: cannot emit {PICK_AREA_EVENT}: {e}");
                    }
                    Ok(())
                }
            }
        }
        Control::Pause => set_paused(app, recorder, true).map(drop),
        Control::Resume => set_paused(app, recorder, false).map(drop),
        _ => {
            // Shown first, so the save is seen in progress.
            crate::tray::show_main(app);
            stop(app, recorder, cell).map(drop)
        }
    }
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
        let (cfg, fallback) = session_config(&settings, None);
        assert!(!fallback);
        assert_eq!(
            cfg.frames.target,
            CaptureTarget::Display { display_id: None }
        );
        assert!(cfg.frames.exclude_own_windows);
        assert_eq!(cfg.excluded_bundle_ids, ["com.apple.keychainaccess"]);
        assert_eq!(cfg.steps.typing_idle_ms, 700);
        assert_eq!(cfg.manual_hotkey, Some(Hotkey::MANUAL_STEP));
        assert_eq!(cfg.control_hotkeys, crate::hotkeys::capture_hotkeys());
        assert_eq!(cfg.control_hotkeys.len(), 2);
        assert_eq!(cfg.work_dir, None);

        settings.recording.capture_area = CaptureArea::Window;
        let (cfg, fallback) = session_config(&settings, None);
        assert!(fallback);
        assert_eq!(
            cfg.frames.target,
            CaptureTarget::Display { display_id: None }
        );
    }

    #[test]
    fn a_given_target_is_used_and_never_a_fallback() {
        let mut settings = Settings::default();
        let window = CaptureTarget::Window { window_id: 42 };
        let region = CaptureTarget::Region {
            display_id: Some(1),
            rect: guidesya_core::model::Rect {
                x: 10.0,
                y: 20.0,
                width: 300.0,
                height: 200.0,
            },
        };
        let display = CaptureTarget::Display { display_id: None };
        for area in [
            CaptureArea::Display,
            CaptureArea::Window,
            CaptureArea::Region,
        ] {
            settings.recording.capture_area = area;
            for target in [window, region, display] {
                let (cfg, fallback) = session_config(&settings, Some(target));
                assert!(!fallback, "{area:?} {target:?}");
                assert_eq!(cfg.frames.target, target);
                assert!(cfg.frames.exclude_own_windows);
            }
        }
        settings.recording.capture_area = CaptureArea::Region;
        assert!(session_config(&settings, None).1);
    }

    #[test]
    fn tray_and_hotkey_starts_pick_the_area_for_window_and_region() {
        assert_eq!(StartFrom::of(CaptureArea::Display), StartFrom::Record);
        assert_eq!(StartFrom::of(CaptureArea::Window), StartFrom::PickArea);
        assert_eq!(StartFrom::of(CaptureArea::Region), StartFrom::PickArea);
        // Only a start resolves to it: a toggle while recording stops.
        let recording = RecordingStatus::Recording {
            started_at: "2026-10-15T01:00:00.000Z".into(),
            title: None,
            capture_area_fallback: false,
            paused_ms: 0,
            excluded_app: None,
        };
        assert_eq!(
            Control::ToggleRecording.resolve(&recording),
            Some(Control::Stop)
        );
        assert_eq!(
            Control::ToggleRecording.resolve(&RecordingStatus::Idle),
            Some(Control::Start)
        );
        assert_eq!(PICK_AREA_EVENT, "recording://pick-area");
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
                paused_ms: 0,
                excluded_app: Some("com.1password.1password".into()),
            })
            .unwrap(),
            json!({
                "state": "recording",
                "startedAt": "2026-10-15T01:02:03.000Z",
                "title": null,
                "captureAreaFallback": true,
                "pausedMs": 0,
                "excludedApp": "com.1password.1password",
            })
        );
        assert_eq!(
            serde_json::to_value(RecordingStatus::Paused {
                started_at: "2026-10-15T01:02:03.000Z".into(),
                title: Some("Rotate a key".into()),
                capture_area_fallback: false,
                paused_ms: 1500,
                paused_at: "2026-10-15T01:05:00.000Z".into(),
                excluded_app: None,
            })
            .unwrap(),
            json!({
                "state": "paused",
                "startedAt": "2026-10-15T01:02:03.000Z",
                "title": "Rotate a key",
                "captureAreaFallback": false,
                "pausedMs": 1500,
                "pausedAt": "2026-10-15T01:05:00.000Z",
                "excludedApp": null,
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
        assert_eq!(
            serde_json::to_value(RecordingSnapshot {
                id: "0-5000000".into(),
                ts: 7_000_000,
                width: 16,
                height: 10,
                point: Some(Point { x: 8.0, y: 5.0 }),
            })
            .unwrap(),
            json!({
                "id": "0-5000000",
                "ts": 7_000_000,
                "width": 16,
                "height": 10,
                "point": {"x": 8.0, "y": 5.0},
            })
        );
    }

    #[test]
    fn snapshot_ids_are_file_stems_never_paths() {
        assert!(is_snapshot_id("0-5000000"));
        assert!(is_snapshot_id("12-0"));
        for bad in ["", "..", "../x", "0-5.webp", "a-1", "0/1", &"1".repeat(65)] {
            assert!(!is_snapshot_id(bad), "{bad:?}");
        }
    }

    #[test]
    fn recording_snapshot_needs_a_session() {
        let recorder = Recorder::default();
        let slot = recorder.slot();
        let err = slot
            .as_ref()
            .and_then(|session| session.session_dir().map(Path::to_path_buf))
            .ok_or_else(not_recording)
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Invalid);
    }

    #[test]
    fn recorder_starts_idle_and_refuses_to_stop() {
        let recorder = Recorder::default();
        assert_eq!(recorder.status().status, RecordingStatus::Idle);
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

    fn at(rfc3339: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(rfc3339)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn pausing_and_resuming_keep_the_status_fields() {
        let recording = RecordingStatus::Recording {
            started_at: "2026-10-15T01:02:03.000Z".into(),
            title: Some("Rotate a key".into()),
            capture_area_fallback: true,
            paused_ms: 0,
            excluded_app: Some("com.apple.keychainaccess".into()),
        };
        let paused = RecordingStatus::Paused {
            started_at: "2026-10-15T01:02:03.000Z".into(),
            title: Some("Rotate a key".into()),
            capture_area_fallback: true,
            paused_ms: 0,
            paused_at: "2026-10-15T01:03:00.000Z".into(),
            excluded_app: Some("com.apple.keychainaccess".into()),
        };
        let t = at("2026-10-15T01:03:00.000Z");
        assert_eq!(with_paused(recording.clone(), true, t), paused);
        // Already in that state: unchanged, `pausedAt` included.
        let later = at("2026-10-15T01:09:00.000Z");
        assert_eq!(with_paused(paused.clone(), true, later), paused);
        assert_eq!(with_paused(recording.clone(), false, later), recording);
        assert_eq!(
            with_paused(RecordingStatus::Idle, true, t),
            RecordingStatus::Idle
        );
        let saving = RecordingStatus::Saving {
            started_at: "2026-10-15T01:02:03.000Z".into(),
            title: None,
        };
        assert_eq!(with_paused(saving.clone(), true, t), saving);
    }

    #[test]
    fn paused_time_adds_up_across_pauses() {
        let mut status = RecordingStatus::Recording {
            started_at: "2026-10-15T01:00:00.000Z".into(),
            title: None,
            capture_area_fallback: false,
            paused_ms: 0,
            excluded_app: None,
        };
        status = with_paused(status, true, at("2026-10-15T01:01:00.000Z"));
        status = with_paused(status, false, at("2026-10-15T01:01:02.500Z"));
        assert!(
            matches!(
                status,
                RecordingStatus::Recording {
                    paused_ms: 2500,
                    ..
                }
            ),
            "{status:?}"
        );
        status = with_paused(status, true, at("2026-10-15T01:05:00.000Z"));
        match &status {
            RecordingStatus::Paused {
                paused_ms,
                paused_at,
                ..
            } => {
                // The pause in progress is not in `pausedMs` yet.
                assert_eq!(*paused_ms, 2500);
                assert_eq!(paused_at, "2026-10-15T01:05:00.000Z");
            }
            other => panic!("{other:?}"),
        }
        status = with_paused(status, false, at("2026-10-15T01:05:01.000Z"));
        assert!(
            matches!(
                status,
                RecordingStatus::Recording {
                    paused_ms: 3500,
                    ..
                }
            ),
            "{status:?}"
        );
    }

    #[test]
    fn ms_since_never_goes_negative() {
        let now = at("2026-10-15T01:00:01.250Z");
        assert_eq!(ms_since("2026-10-15T01:00:00.000Z", now), 1250);
        assert_eq!(ms_since("2026-10-15T03:00:00.000+02:00", now), 1250);
        assert_eq!(ms_since("2026-10-15T01:00:02.000Z", now), 0);
        assert_eq!(ms_since("not a time", now), 0);
    }

    #[test]
    fn excluded_app_is_set_on_recording_and_paused_only() {
        let recording = RecordingStatus::Recording {
            started_at: "2026-10-15T01:00:00.000Z".into(),
            title: None,
            capture_area_fallback: false,
            paused_ms: 0,
            excluded_app: None,
        };
        let excluded = with_excluded(recording.clone(), Some("com.1password.1password"));
        assert!(matches!(
            &excluded,
            RecordingStatus::Recording { excluded_app: Some(id), .. } if id == "com.1password.1password"
        ));
        assert_eq!(with_excluded(excluded.clone(), None), recording);
        let paused = with_paused(excluded, true, at("2026-10-15T01:01:00.000Z"));
        assert!(matches!(
            with_excluded(paused, Some("com.apple.MobileSMS")),
            RecordingStatus::Paused { excluded_app: Some(id), .. } if id == "com.apple.MobileSMS"
        ));
        assert_eq!(
            with_excluded(RecordingStatus::Idle, Some("com.apple.MobileSMS")),
            RecordingStatus::Idle
        );
    }

    #[test]
    fn update_reports_whether_the_status_changed() {
        let recorder = Recorder::default();
        let (status, changed) =
            recorder.update(|s| with_excluded(s.clone(), Some("com.apple.MobileSMS")));
        assert_eq!(
            (status.status, status.seq, changed),
            (RecordingStatus::Idle, 0, false)
        );
        recorder.set_status(RecordingStatus::Recording {
            started_at: "2026-10-15T01:00:00.000Z".into(),
            title: None,
            capture_area_fallback: false,
            paused_ms: 0,
            excluded_app: None,
        });
        let (_, changed) =
            recorder.update(|s| with_excluded(s.clone(), Some("com.apple.MobileSMS")));
        assert!(changed);
        let (status, changed) =
            recorder.update(|s| with_excluded(s.clone(), Some("com.apple.MobileSMS")));
        assert!(!changed);
        assert_eq!(recorder.status(), status);
    }

    #[test]
    fn seq_strictly_increases_with_each_change() {
        let recorder = Recorder::default();
        assert_eq!(recorder.status().seq, 0);
        let recording = RecordingStatus::Recording {
            started_at: "2026-10-15T01:00:00.000Z".into(),
            title: None,
            capture_area_fallback: false,
            paused_ms: 0,
            excluded_app: None,
        };
        let mut seqs = vec![recorder.set_status(recording.clone()).seq];
        // Storing the same value again is still a new status.
        seqs.push(recorder.set_status(recording).seq);
        let (s, changed) =
            recorder.update(|s| with_excluded(s.clone(), Some("com.1password.1password")));
        assert!(changed);
        seqs.push(s.seq);
        let (s, changed) = recorder.update(|s| with_paused(s.clone(), true, Utc::now()));
        assert!(changed);
        seqs.push(s.seq);
        // No change: no new seq.
        let (s, changed) = recorder.update(|s| s.clone());
        assert!(!changed);
        assert_eq!(s.seq, *seqs.last().unwrap());
        seqs.push(recorder.set_status(RecordingStatus::Idle).seq);
        assert!(seqs.windows(2).all(|w| w[0] < w[1]), "{seqs:?}");
        assert_eq!(recorder.status().seq, *seqs.last().unwrap());
        // `discard` stores idle under a new seq too.
        recorder.set_status(RecordingStatus::Idle);
        assert!(recorder.status().seq > *seqs.last().unwrap());
    }

    #[test]
    fn stamped_status_puts_seq_beside_the_state() {
        assert_eq!(
            serde_json::to_value(StampedStatus {
                seq: 7,
                status: RecordingStatus::Idle,
            })
            .unwrap(),
            json!({"seq": 7, "state": "idle"})
        );
        assert_eq!(
            serde_json::to_value(StampedStatus {
                seq: 8,
                status: RecordingStatus::Paused {
                    started_at: "2026-10-15T01:02:03.000Z".into(),
                    title: None,
                    capture_area_fallback: false,
                    paused_ms: 0,
                    paused_at: "2026-10-15T01:05:00.000Z".into(),
                    excluded_app: Some("com.apple.MobileSMS".into()),
                },
            })
            .unwrap(),
            json!({
                "seq": 8,
                "state": "paused",
                "startedAt": "2026-10-15T01:02:03.000Z",
                "title": null,
                "captureAreaFallback": false,
                "pausedMs": 0,
                "pausedAt": "2026-10-15T01:05:00.000Z",
                "excludedApp": "com.apple.MobileSMS",
            })
        );
    }

    #[test]
    fn discard_never_waits_and_is_a_no_op_when_idle() {
        let recorder = Recorder::default();
        assert!(!recorder.discard());
        assert_eq!(recorder.status().status, RecordingStatus::Idle);
        // A command holding the slot (e.g. a start waiting on the main
        // thread) must not block the quit.
        let slot = recorder.slot();
        recorder.set_status(RecordingStatus::Saving {
            started_at: "2026-10-15T01:02:03.000Z".into(),
            title: None,
        });
        assert!(!recorder.discard());
        assert!(matches!(
            recorder.status().status,
            RecordingStatus::Saving { .. }
        ));
        drop(slot);
    }

    #[test]
    fn add_manual_step_refuses_without_a_running_recording() {
        let recorder = Recorder::default();
        let err = manual_step(&recorder).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Invalid);
        assert_eq!(err.message, "no recording is running");
        // Saving: the session is out of the slot.
        recorder.set_status(RecordingStatus::Saving {
            started_at: "2026-10-15T01:02:03.000Z".into(),
            title: None,
        });
        assert_eq!(
            manual_step(&recorder).unwrap_err().message,
            "no recording is running"
        );
    }

    #[test]
    fn only_raced_refusals_are_ignored() {
        assert!(lost_race(&not_recording()));
        assert!(lost_race(&AppError::new(
            ErrorKind::Invalid,
            ALREADY_RUNNING
        )));
        // A missing permission or library is still shown.
        assert!(!lost_race(&AppError::new(
            ErrorKind::Invalid,
            "Screen Recording permission is missing"
        )));
        assert!(!lost_race(&AppError::new(ErrorKind::Io, NOT_RECORDING)));
    }

    #[test]
    fn stop_from_the_toolbar_shows_the_main_window() {
        assert!(shows_main_on_stop(crate::toolbar::LABEL));
        assert!(!shows_main_on_stop("main"));
    }

    #[test]
    fn controls_resolve_per_status() {
        let recording = RecordingStatus::Recording {
            started_at: "2026-10-15T01:00:00.000Z".into(),
            title: None,
            capture_area_fallback: false,
            paused_ms: 0,
            excluded_app: None,
        };
        let paused = with_paused(recording.clone(), true, Utc::now());
        let saving = RecordingStatus::Saving {
            started_at: "2026-10-15T01:00:00.000Z".into(),
            title: None,
        };
        use Control::*;
        let all = [Start, Pause, Resume, Stop, ToggleRecording, TogglePause];
        let table = |status: &RecordingStatus| -> Vec<Option<Control>> {
            all.iter().map(|c| c.resolve(status)).collect()
        };
        assert_eq!(
            table(&RecordingStatus::Idle),
            [Some(Start), None, None, None, Some(Start), None]
        );
        assert_eq!(
            table(&recording),
            [None, Some(Pause), None, Some(Stop), Some(Stop), Some(Pause)]
        );
        assert_eq!(
            table(&paused),
            [
                None,
                None,
                Some(Resume),
                Some(Stop),
                Some(Stop),
                Some(Resume)
            ]
        );
        assert_eq!(table(&saving), [None; 6]);
    }

    #[test]
    fn default_title_names_the_recording() {
        let title = default_title();
        assert!(title.starts_with("Recording 20"), "{title}");
        assert_eq!(title.len(), "Recording 2026-10-15 14:03".len(), "{title}");
    }
}
