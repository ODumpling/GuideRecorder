//! Recording session (M0, todo #336): the event tap, the frame stream and
//! AX lookups wired together, ending in a `.guidesya` bundle.
//!
//! # Threads
//!
//! ```text
//!                                         ┌─▶ element thread (AX: element_at for clicks)
//! event tap ──TargetedEvent──▶ router ────┼─▶ focus thread   (AX: focused element per key)
//!                                         └─▶ encoder thread (snapshot frames → WebP)
//! ```
//!
//! - The **tap thread** (see `event_tap`) only translates events.
//! - The **router** does no AX work. It drops events aimed at our own pid
//!   or at an excluded app, turns the manual-step hotkey into
//!   [`CaptureEvent::ManualStep`], emits [`CaptureEvent::AppActivated`]
//!   when the target pid of a click, key or manual step changes, and takes
//!   a frame snapshot ([`FrameReader::frame_before`]) for every event that
//!   can start a step, as soon as the event arrives. New snapshot frames go
//!   to the encoder; the router keeps only their timestamps and geometry.
//! - The **element thread** fills `MouseDown.element` from
//!   [`ax::element_at`] and drops presses on excluded apps.
//! - The **focus thread** fills `KeyDown.secure` and emits
//!   [`CaptureEvent::FocusChanged`]. It is separate so a slow `element_at`
//!   never delays a key's focus lookup.
//! - The **encoder** writes each snapshot frame once, as lossless WebP, to
//!   a hidden session folder (`.guidesya-session-<ulid>` in
//!   [`SessionConfig::work_dir`], by default [`default_work_dir`], a local
//!   cache that is not synced) and drops the frame. Its queue holds at most
//!   [`ENCODER_QUEUE`] frames; a snapshot that finds it full is dropped and
//!   counted, so memory stays bounded however slow encoding is. Session
//!   folders a crash left behind are swept at the next start once older
//!   than [`STALE_SESSION_MS`]. With [`SessionConfig::on_snapshot`] set,
//!   it reports each frame it wrote as a [`LiveSnapshot`], from its own
//!   thread, so a UI can show the screenshots while recording. They are
//!   provisional: which steps the guide ends up with, and which frames
//!   are pruned, is only known at stop (see `# Snapshots`).
//!
//! No AX call runs on the main or tap thread (see `ax`'s `# Threading`).
//! [`RecordingSession::stop`] stops the tap, drains and joins all threads,
//! stops the frames, merges the events by timestamp and runs
//! [`detect_steps`]. [`Recording::save_guide`] writes the bundle;
//! dropping the [`Recording`] removes the session folder.
//!
//! # Pause
//!
//! [`RecordingSession::pause`] stops the tap (joining its thread, so every
//! event it saw is already queued for the router), notes the time and
//! stops the frame streams, so capture CPU and the frame rings' memory
//! drop to idle; the four threads stay up, idle.
//! [`RecordingSession::resume`] starts fresh frame streams (for the
//! displays active now, with empty rings, as at start), notes the time
//! and starts a new tap. The router compares event timestamps with the
//! pause and resume times the session shares with it (`PauseState`, same
//! clock as the events), so queue lag cannot reorder them: an event from
//! after the pause began is dropped, and the first event from after a
//! resume resets the key-run tracker, the current app (so `AppActivated`
//! is emitted again) and the exclusion filter, and tells the focus thread
//! (`FocusMsg::Resume`) to drop its cache and settle guard, report the
//! focus again and move the scrub boundary to the resume. A pre-pause
//! event still queued when `pause()` drops the frame streams may find no
//! frame and count in `missing_frames`; its step then borrows a neighbour's
//! snapshot or is skipped, as for any event without one. At stop, events
//! inside a pause are removed and [`detect_steps`] runs separately on each
//! stretch between pauses (`detect_steps_between_pauses`), so no typing run
//! or drag is joined across a pause. Stopping while paused works; that
//! pause then ends at stop.
//!
//! # Secure keys (fail closed)
//!
//! Every non-repeat key press reads the system-wide focused element
//! reference (one AX message). The full element (role, subrole, app) is
//! read only when that reference is not the cached one (`CFEqual`), and
//! the reference is read again afterwards: if focus moved in between, the
//! key is secure and nothing is cached. The cached verdict
//! ([`FocusCache`]) also expires on any click, Tab, Return, Esc, shortcut,
//! pid change or idle gap over [`StepConfig::typing_idle_ms`].
//!
//! A key is secure unless the lookup gave a definite non-secure element
//! ([`ax::must_mask_keys`]). It is also secure:
//!
//! - when the lookup ran more than [`STALE_LOOKUP_MS`] after the key
//!   ([`apply_staleness`]): by then the focus may already have moved on.
//!   This compares event timestamps with `event_tap::now()`, so it relies
//!   on both being on the same clock: the hardware timestamp check
//!   (`event_tap --raw-ts`, still open for a live run). If event `ts` were
//!   read too large, the lag would saturate to 0 and no key would ever
//!   count as stale;
//! - within [`StepConfig::focus_settle_ms`] after Tab, Return, Esc, a ⌘ or
//!   ⌃ shortcut or a click, until the focused element is seen to change
//!   ([`SettleGuard`]): the tap sees a key before the app has moved focus.
//!   Option-only chords type characters on many layouts and do not arm it
//!   ([`moves_focus`]). For a click, the focus from before is read fresh
//!   when the click is handled.
//!
//! When a key's full lookup finds a secure field, the plain keys stored
//! before it are scrubbed after the fact ([`retro_scrub`]): every key since
//! the last focus-moving key or click while keys stay within
//! `typing_idle_ms` of each other (so an app that moved focus late cannot
//! leave the start of the password in clear), or with no such boundary,
//! the keys within `focus_settle_ms`, the step detector's late-focus rule. Key repeats
//! reuse the previous key's verdict. The manual-step hotkey goes through
//! the same thread and is dropped when the focused app is excluded. The characters of a secure key are replaced here
//! ([`scrub_secure_key`]), so they never reach the events, the raw dump or
//! the step detector. A key whose focused element belongs to an excluded
//! app is dropped.
//!
//! # Excluded apps
//!
//! Events are checked against [`SessionConfig::excluded_bundle_ids`] one by
//! one (the target pid in the router, the clicked element in the element
//! thread, the focused element in the focus thread). On top of that,
//! nothing is captured while an excluded app is frontmost:
//!
//! - A `FrontmostObserver` notes each app activation
//!   (`NSWorkspaceDidActivateApplicationNotification`, on the main thread)
//!   at `event_tap::now()` in a [`FrontmostLog`] of excluded spans; start
//!   and resume also read the frontmost app directly.
//! - The router drops any event from inside a span, whatever app it is
//!   aimed at, through the [`ExclusionFilter`]: a press is never split, it
//!   goes or stays whole as its `MouseDown` does. So no key, click, manual
//!   step or step comes from inside a span, and the focus thread never
//!   sees its keys.
//! - The router takes no snapshot from a frame captured inside a span,
//!   less than [`EXCLUDED_FRAME_HEAD_MS`] before its start (the span starts
//!   when the main thread handles the notification, after the app came to
//!   the front and drew) or less than [`EXCLUDED_FRAME_TAIL_MS`] after its
//!   end. The encoder checks each frame again before writing it and
//!   reporting it through [`SessionConfig::on_snapshot`].
//! - Until the notice arrives, nothing is known: a frame of the excluded
//!   app taken in that time is written and **can flash up live** as a
//!   thumbnail. At stop the final log removes it (snapshot and file), and
//!   the events from inside a span that slipped through, by the same
//!   press rule ([`SessionMetrics::excluded_frames`],
//!   [`SessionMetrics::dropped_excluded`]). Events before the notice are
//!   kept: they get no head margin.
//!
//! [`RecordingSession::excluded_app`] and [`SessionConfig::on_excluded`]
//! expose the state, e.g. for a "paused: excluded app" toolbar.
//! Secure-field masking (below) does not depend on the exclusion list.
//!
//! # Snapshots
//!
//! A snapshot is taken for each `MouseDown`, `ManualStep`, shortcut, Return
//! or Esc, and for the first key of a typing burst as the router sees it
//! ([`KeyRunTracker`]). Snapshots of events dropped later (a click or key
//! found to be in an excluded app) are removed at stop, with their latency
//! samples and files. A step without a snapshot of its own uses the nearest
//! one within 2 × [`StepConfig::typing_idle_ms`] ([`match_snapshot`]), or
//! gets no image and counts as skipped.
//!
//! # Known gaps
//!
//! - No "after" frame yet.
//! - Exclusion only follows the frontmost app: a window of an excluded app
//!   left visible behind another app shows in that app's snapshots. (A
//!   ScreenCaptureKit content filter excluding those apps would cover it.)
//!   The click that brings another app to the front from an excluded one
//!   is dropped with it. Without a main run loop (a CLI that never runs
//!   it) only the reads at start and resume update the frontmost app.
//! - The focus after a click is read when the focus thread handles the
//!   click's `MouseUp`, which can be before the app has moved focus. This
//!   only affects labels: each key does its own lookup.
//! - With a window or region target, frames come from that one source;
//!   clicks elsewhere get no point and no crop. (A display target without
//!   an id captures every active display, one stream each, and each
//!   snapshot comes from the display under the event: see
//!   [`stream_targets`].)
//! - For an event without a point (a key, the manual-step hotkey), the
//!   display is the one the pointer was last seen on, which is normally
//!   where the user clicked to focus the window they type in. Typing into
//!   a window on another display after a keyboard-only switch (⌘Tab)
//!   snapshots the pointer's display.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use block2::RcBlock;
use guidesya_core::events::{
    AppRef, CaptureEvent, Key, Modifiers, RawElement, TimedEvent, Timestamp,
};
use guidesya_core::library::{
    self, GuideHandle, Library, LibraryError, PixelBuffer, PixelFormat, StepImageVariant,
};
use guidesya_core::model::{
    self, AppInfo, Guide, ImageShow, Item, Point, RecordingInfo, Rect, StepImage,
};
use guidesya_core::steps::{detect_steps, DraftStep, StepConfig};
use guidesya_core::Ulid;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2_app_kit::{
    NSRunningApplication, NSWorkspace, NSWorkspaceApplicationKey,
    NSWorkspaceDidActivateApplicationNotification,
};
use objc2_application_services::{AXError, AXUIElement};
use objc2_core_foundation::{CFRetained, CFString, CFType};
use objc2_foundation::{NSNotification, NSObjectProtocol};

use super::ax;
use super::event_tap::{self, EventTap, TargetedEvent};
use super::frames::{
    self, CaptureTarget, Frame, FrameError, FrameGeometry, FrameReader, FrameStream,
    FrameStreamConfig,
};

/// Frames kept in the ring during a session. More than the stream default,
/// so a snapshot still finds the pre-click frame if the router falls behind.
pub const SESSION_RING_CAPACITY: usize = 6;
/// Default click crop in frame pixels (clamped to the frame).
pub const DEFAULT_CROP_SIZE: (u32, u32) = (800, 500);
/// A focus lookup that runs later than this after its key no longer says
/// where the key went; the key is treated as secure.
pub const STALE_LOOKUP_MS: u64 = 75;
/// Prefix of the hidden per-session folder holding encoded snapshots.
pub const SESSION_DIR_PREFIX: &str = ".guidesya-session-";
/// Session folders older than this are swept when a session starts.
pub const STALE_SESSION_MS: u64 = 6 * 60 * 60 * 1000;
/// Frames waiting for the encoder at most; a snapshot that finds the queue
/// full is dropped (see [`SessionMetrics::snapshots_dropped_queue_full`]).
pub const ENCODER_QUEUE: usize = 4;
/// A frame captured this soon after an excluded app stopped being
/// frontmost may still show it (redraw and capture lag); it is never
/// snapshotted. See `# Excluded apps`.
pub const EXCLUDED_FRAME_TAIL_MS: u64 = 250;
/// A frame captured this soon before an excluded app's activation was
/// noted may already show it: the span starts when the main thread handles
/// the activation notification, after the app came to the front and drew.
/// Never snapshotted either. Same as the tail: a few frames of redraw plus
/// main-thread latency.
pub const EXCLUDED_FRAME_HEAD_MS: u64 = 250;
/// The suggested exclusion list (the app's settings default):
/// 1Password 8, 7 and 6, Bitwarden, Keychain Access, Passwords (macOS 15+)
/// and Messages. [`SessionConfig::default`] excludes nothing; the caller
/// passes the list it wants in [`SessionConfig::excluded_bundle_ids`].
pub const DEFAULT_EXCLUDED_BUNDLE_IDS: &[&str] = &[
    "com.1password.1password",
    "com.agilebits.onepassword7",
    "com.agilebits.onepassword-osx",
    "com.bitwarden.desktop",
    "com.apple.keychainaccess",
    "com.apple.Passwords",
    "com.apple.MobileSMS",
];

/// [`DEFAULT_EXCLUDED_BUNDLE_IDS`] as owned strings, ready for
/// [`SessionConfig::excluded_bundle_ids`].
pub fn default_excluded_bundle_ids() -> Vec<String> {
    DEFAULT_EXCLUDED_BUNDLE_IDS
        .iter()
        .map(|&id| id.to_owned())
        .collect()
}

const KEY_RETURN: u16 = 36;
const KEY_TAB: u16 = 48;
const KEY_ESCAPE: u16 = 53;
const KEY_KEYPAD_ENTER: u16 = 76;
/// `kVK_ANSI_S`.
const KEY_S: u16 = 1;

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("event tap: {0}")]
    Tap(#[from] event_tap::Error),
    #[error("frames: {0}")]
    Frames(#[from] FrameError),
    #[error("library: {0}")]
    Library(#[from] LibraryError),
    #[error("{path}: {source}")]
    Io { path: String, source: io::Error },
    #[error("could not spawn a session thread: {0}")]
    Spawn(io::Error),
    #[error("a session thread panicked")]
    ThreadPanicked,
}

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> SessionError + '_ {
    move |source| SessionError::Io {
        path: path.display().to_string(),
        source,
    }
}

/// A key plus exact modifiers (shift, control, option, command; function
/// and caps lock are ignored).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hotkey {
    pub code: u16,
    pub modifiers: Modifiers,
}

impl Hotkey {
    /// ⌃⌥S, the default manual-step hotkey.
    pub const MANUAL_STEP: Hotkey = Hotkey {
        code: KEY_S,
        modifiers: Modifiers {
            shift: false,
            control: true,
            option: true,
            command: false,
            function: false,
            caps_lock: false,
        },
    };

    pub fn matches(&self, key: &Key, modifiers: Modifiers) -> bool {
        let m = self.modifiers;
        key.code == self.code
            && modifiers.shift == m.shift
            && modifiers.control == m.control
            && modifiers.option == m.option
            && modifiers.command == m.command
    }
}

#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// The capture. A display target without an id (the default) records
    /// every active display, one stream each; see [`stream_targets`].
    pub frames: FrameStreamConfig,
    /// Bundle ids whose events are dropped and never snapshotted; while one
    /// of them is the frontmost app nothing is captured at all (see
    /// `# Excluded apps`). [`DEFAULT_EXCLUDED_BUNDLE_IDS`] is the suggested
    /// list; the default here is empty.
    pub excluded_bundle_ids: Vec<String>,
    /// Key press turned into a manual step (and not recorded as a key).
    pub manual_hotkey: Option<Hotkey>,
    pub steps: StepConfig,
    /// Click crop in frame pixels.
    pub crop_size: (u32, u32),
    /// Where the hidden session folder with encoded snapshots goes. `None`
    /// is [`default_work_dir`], a local cache that is not synced. Never use
    /// a library root: libraries live in synced folders.
    pub work_dir: Option<PathBuf>,
    /// Called on the encoder thread for each snapshot frame it writes, as
    /// soon as the file exists; `None` reports nothing. Keep it quick: the
    /// encoder waits for it.
    pub on_snapshot: Option<SnapshotSink>,
    /// Called when an excluded app becomes frontmost (with its bundle id)
    /// and when a non-excluded one does (`None`), e.g. to show "paused:
    /// excluded app". Called on the main thread, from the activation
    /// notification, or on the thread calling [`RecordingSession::start`]
    /// or [`RecordingSession::resume`] for the initial read; also while
    /// paused. Keep it quick. `None` reports nothing; the state can also
    /// be polled with [`RecordingSession::excluded_app`].
    pub on_excluded: Option<ExclusionSink>,
}

/// The callback behind [`SessionConfig::on_excluded`]: `Some(bundle id)`
/// when an excluded app becomes frontmost (or replaces another excluded
/// one), `None` when a non-excluded app does.
#[derive(Clone)]
pub struct ExclusionSink(Arc<ExclusionFn>);

type ExclusionFn = dyn Fn(Option<&str>) + Send + Sync;

impl ExclusionSink {
    pub fn new(f: impl Fn(Option<&str>) + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }

    fn report(&self, excluded: Option<&str>) {
        (self.0)(excluded);
    }
}

impl fmt::Debug for ExclusionSink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ExclusionSink")
    }
}

/// A snapshot frame the encoder wrote, reported live through
/// [`SessionConfig::on_snapshot`]. Provisional: the frame is removed at
/// stop if every event that took it turns out to be from an excluded app
/// or inside a pause, a frame can end up serving several steps or none,
/// and a step may borrow a neighbouring frame ([`match_snapshot`]).
#[derive(Debug, Clone, PartialEq)]
pub struct LiveSnapshot {
    /// The event that took the snapshot: the first one, when several share
    /// the frame.
    pub ts: Timestamp,
    pub frame: FrameKey,
    /// Lossless WebP in the session folder, named
    /// `<frame.file_stem()>.webp`.
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    /// The event's point in frame pixels, when it has one and it is on
    /// the frame (a click; `None` for a key or the manual-step hotkey).
    pub point: Option<Point>,
}

/// The callback behind [`SessionConfig::on_snapshot`].
#[derive(Clone)]
pub struct SnapshotSink(Arc<dyn Fn(&LiveSnapshot) + Send + Sync>);

impl SnapshotSink {
    pub fn new(f: impl Fn(&LiveSnapshot) + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }

    fn report(&self, snapshot: &LiveSnapshot) {
        (self.0)(snapshot);
    }
}

impl fmt::Debug for SnapshotSink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SnapshotSink")
    }
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            frames: FrameStreamConfig {
                ring_capacity: SESSION_RING_CAPACITY,
                ..FrameStreamConfig::default()
            },
            excluded_bundle_ids: Vec::new(),
            manual_hotkey: Some(Hotkey::MANUAL_STEP),
            steps: StepConfig::default(),
            crop_size: DEFAULT_CROP_SIZE,
            work_dir: None,
            on_snapshot: None,
            on_excluded: None,
        }
    }
}

/// `$HOME/Library/Caches/dev.guidesya.app/sessions`, or the same under the
/// system temp folder without `$HOME`.
pub fn default_work_dir() -> PathBuf {
    match std::env::var_os("HOME").filter(|h| !h.is_empty()) {
        Some(home) => PathBuf::from(home).join("Library/Caches/dev.guidesya.app/sessions"),
        None => std::env::temp_dir().join("dev.guidesya.app/sessions"),
    }
}

/// True for a `.guidesya-session-<ulid>` name whose ULID is more than
/// `max_age_ms` older than `now_ms` (Unix ms). Other names are never stale.
pub fn is_stale_session_dir(name: &str, now_ms: u64, max_age_ms: u64) -> bool {
    name.strip_prefix(SESSION_DIR_PREFIX)
        .and_then(|id| Ulid::from_string(id).ok())
        .is_some_and(|id| now_ms.saturating_sub(id.timestamp_ms()) > max_age_ms)
}

/// Removes session folders left behind by a crash or kill (no `Drop` ran).
fn sweep_stale_sessions(work_dir: &Path) {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    let Ok(entries) = fs::read_dir(work_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
        if is_dir && is_stale_session_dir(&name.to_string_lossy(), now_ms, STALE_SESSION_MS) {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

/// A running recording. Stop it with [`RecordingSession::stop`]; dropping
/// it stops everything and discards the recording and its session folder.
pub struct RecordingSession {
    tap: Option<EventTap>,
    /// The router's input, kept so the router outlives the tap across a
    /// pause; dropped at stop, which ends the router.
    tap_tx: Option<Sender<TargetedEvent>>,
    /// One stream per captured display (or one for a window or region
    /// target); empty while paused and in headless tests.
    frames: Vec<FrameStream>,
    /// The frame stream config, kept to restart the streams on resume;
    /// `None` in a headless test session, which has no streams.
    frames_cfg: Option<FrameStreamConfig>,
    /// The streams' readers and the rects they show, shared with the
    /// router, which picks the one under each event's point.
    sources: SharedSources,
    router: Option<JoinHandle<RouterOut>>,
    element: Option<JoinHandle<ElementOut>>,
    focus: Option<JoinHandle<FocusOut>>,
    encoder: Option<JoinHandle<EncoderOut>>,
    session_dir: Option<PathBuf>,
    started: Instant,
    cpu_start: Option<CpuTime>,
    steps: StepConfig,
    crop_size: (u32, u32),
    /// The captured displays, for [`RecordingInfo`].
    displays: Vec<frames::DisplayInfo>,
    /// No tap is ever started: tests feed events through `tap_tx` and
    /// frames through the router's sources.
    #[cfg(test)]
    headless: bool,
    /// Shared with the router (see the module docs on pausing).
    pause: SharedPause,
    /// When the current pause began.
    paused_at: Option<Timestamp>,
    /// Pauses over, in order.
    pauses: Vec<PauseSpan>,
    /// The frontmost app's exclusion, shared with the router and encoder.
    front: Arc<Frontmost>,
    /// Feeds `front` from app activations; removed when dropped.
    observer: Option<FrontmostObserver>,
}

impl RecordingSession {
    /// Starts frames, then the threads, then the tap. Snapshots are encoded
    /// into a new hidden folder in [`SessionConfig::work_dir`], after
    /// folders older than [`STALE_SESSION_MS`] there are swept.
    ///
    /// Call it on the main thread (the tap loads the keyboard layout there;
    /// see `event_tap`). Blocks for up to a few seconds while
    /// ScreenCaptureKit starts.
    pub fn start(cfg: SessionConfig) -> Result<Self, SessionError> {
        let mut session = Self::new(&cfg);
        let work_dir = cfg.work_dir.clone().unwrap_or_else(default_work_dir);
        fs::create_dir_all(&work_dir).map_err(io_err(&work_dir))?;
        sweep_stale_sessions(&work_dir);
        let session_dir = work_dir.join(format!("{SESSION_DIR_PREFIX}{}", Ulid::new()));
        fs::create_dir_all(&session_dir).map_err(io_err(&session_dir))?;
        session.session_dir = Some(session_dir);
        // From here on, an error drops `session`, which joins whatever was
        // started and removes the session folder.
        session.frames_cfg = Some(cfg.frames.clone());
        session.start_frames()?;
        let sources = Arc::clone(&session.sources);
        session.spawn_threads(&cfg, sources, || AxFocus)?;
        // Before the tap, so the gate holds from the first event.
        session.observer = Some(FrontmostObserver::start(Arc::clone(&session.front)));
        session.read_frontmost();
        session.start_tap()?;
        Ok(session)
    }

    /// A session with nothing running and no session folder yet.
    fn new(cfg: &SessionConfig) -> Self {
        Self {
            tap: None,
            tap_tx: None,
            frames: Vec::new(),
            frames_cfg: None,
            sources: Arc::default(),
            router: None,
            element: None,
            focus: None,
            encoder: None,
            session_dir: None,
            started: Instant::now(),
            cpu_start: process_cpu_time(),
            steps: cfg.steps.clone(),
            crop_size: cfg.crop_size,
            displays: Vec::new(),
            #[cfg(test)]
            headless: false,
            pause: SharedPause::default(),
            paused_at: None,
            pauses: Vec::new(),
            front: Arc::new(Frontmost::new(cfg)),
            observer: None,
        }
    }

    /// Starts the frame streams for [`RecordingSession::frames_cfg`] (see
    /// [`stream_targets`]) and publishes their readers to the router. On
    /// error no stream is left running.
    fn start_frames(&mut self) -> Result<(), SessionError> {
        let Some(cfg) = self.frames_cfg.clone() else {
            return Ok(());
        };
        let mut streams = Vec::new();
        for target in stream_targets(cfg.target) {
            streams.push(FrameStream::start(FrameStreamConfig {
                target,
                ..cfg.clone()
            })?);
        }
        *lock(&self.sources) = streams.iter().map(FrameSource::of).collect();
        self.displays = streams
            .iter()
            .filter_map(|s| display_for(s.config().target))
            .collect();
        self.frames = streams;
        Ok(())
    }

    /// Stops every frame stream (releasing their rings) and removes them
    /// from the router's sources. Returns the first stop error; a failed
    /// stream is abandoned (see [`FrameStream::pause`]).
    fn stop_frames(&mut self) -> Result<(), SessionError> {
        lock(&self.sources).clear();
        let mut first = None;
        for stream in self.frames.drain(..) {
            if let Err(e) = stream.stop() {
                first.get_or_insert(e);
            }
        }
        first.map_or(Ok(()), |e| Err(e.into()))
    }

    /// Starts the encoder, element, focus and router threads. `make_focus`
    /// runs on the focus thread (AX references stay there).
    fn spawn_threads<S: FocusSource + 'static>(
        &mut self,
        cfg: &SessionConfig,
        sources: SharedSources,
        make_focus: impl FnOnce() -> S + Send + 'static,
    ) -> Result<(), SessionError> {
        let session_dir = self
            .session_dir
            .clone()
            .expect("the session folder is created before the threads start");
        let (tap_tx, tap_rx) = mpsc::channel::<TargetedEvent>();
        let (element_tx, element_rx) = mpsc::channel::<TimedEvent>();
        let (focus_tx, focus_rx) = mpsc::channel::<FocusMsg>();
        let (encode_tx, encode_rx) = mpsc::sync_channel::<EncodeJob>(ENCODER_QUEUE);
        let backlog = Arc::new(AtomicUsize::new(0));

        let mut encoder = Encoder::new(
            session_dir,
            Arc::clone(&backlog),
            cfg.on_snapshot.clone(),
            Arc::clone(&self.front),
        );
        self.encoder = Some(spawn("guidesya-session-encode", move || {
            encoder.run(encode_rx);
            encoder.finish()
        })?);
        let mut element = ElementWorker::new(cfg);
        self.element = Some(spawn("guidesya-session-ax-element", move || {
            element.run(element_rx);
            element.finish()
        })?);
        let focus_cfg = cfg.clone();
        self.focus = Some(spawn("guidesya-session-ax-focus", move || {
            // Built here: its cached AX reference stays on this thread.
            let mut focus = FocusWorker::new(&focus_cfg, make_focus());
            focus.run(focus_rx);
            focus.finish()
        })?);
        let mut router = Router::new(
            cfg,
            sources,
            Outputs {
                element: element_tx,
                focus: focus_tx,
                encode: encode_tx,
                backlog,
            },
            Arc::clone(&self.pause),
            Arc::clone(&self.front),
        );
        self.router = Some(spawn("guidesya-session-router", move || {
            router.run(tap_rx);
            router.finish()
        })?);
        self.tap_tx = Some(tap_tx);
        Ok(())
    }

    /// Records the app frontmost now (the initial read; activations after
    /// it come from the observer). Not in a headless test session, whose
    /// tests script the frontmost app.
    fn read_frontmost(&self) {
        #[cfg(test)]
        if self.headless {
            return;
        }
        self.front
            .activated(event_tap::now(), frontmost_bundle_id().as_deref());
    }

    /// Starts a tap feeding the router (none in a headless test session).
    fn start_tap(&mut self) -> Result<(), SessionError> {
        #[cfg(test)]
        if self.headless {
            return Ok(());
        }
        let tx = self
            .tap_tx
            .clone()
            .expect("the threads are spawned before the tap");
        self.tap = Some(EventTap::start_with_target_pid(tx)?);
        Ok(())
    }

    /// The frame streams (one per captured display), e.g. for a live
    /// preview. Empty while paused.
    pub fn frames(&self) -> &[FrameStream] {
        &self.frames
    }

    /// The displays being captured (none for a window target, and none
    /// while paused).
    pub fn displays(&self) -> &[frames::DisplayInfo] {
        &self.displays
    }

    /// The hidden folder the encoded snapshots are written to (the
    /// [`LiveSnapshot`] paths are in it); `None` only before a start.
    pub fn session_dir(&self) -> Option<&Path> {
        self.session_dir.as_deref()
    }

    /// True between [`RecordingSession::pause`] and
    /// [`RecordingSession::resume`].
    pub fn is_paused(&self) -> bool {
        self.paused_at.is_some()
    }

    /// The bundle id of the frontmost app when it is excluded, during which
    /// nothing is captured (see `# Excluded apps`); `None` otherwise.
    /// Tracked while paused too. [`SessionConfig::on_excluded`] reports
    /// the same changes as they happen.
    pub fn excluded_app(&self) -> Option<String> {
        self.front.current()
    }

    /// True while an excluded app is frontmost
    /// ([`RecordingSession::excluded_app`] is `Some`).
    pub fn is_excluded(&self) -> bool {
        self.excluded_app().is_some()
    }

    /// Pauses recording: stops the tap (joining its thread, so every event
    /// it saw is already on its way to the router), notes the time, from
    /// which on the router drops any event, and stops the frame streams so
    /// capture CPU and the frame rings' memory drop to idle. Pausing while
    /// paused is a no-op.
    ///
    /// The session is paused even if stopping a frame stream fails (that
    /// stream is then abandoned, see [`FrameStream::pause`]); the error is
    /// returned.
    pub fn pause(&mut self) -> Result<(), SessionError> {
        if self.paused_at.is_some() {
            return Ok(());
        }
        if let Some(mut tap) = self.tap.take() {
            tap.stop();
        }
        let at = event_tap::now();
        self.paused_at = Some(at);
        lock(&self.pause).paused_at = Some(at);
        self.stop_frames()
    }

    /// Resumes after [`RecordingSession::pause`]: starts fresh frame
    /// streams for the displays active now (so a display plugged in or
    /// removed during the pause is picked up), notes the time and starts a
    /// new tap. The first event after that time resets what would link it
    /// to the events from before the pause (see the module docs). Resuming
    /// while not paused is a no-op. On error the session stays paused.
    pub fn resume(&mut self) -> Result<(), SessionError> {
        let Some(paused_at) = self.paused_at else {
            return Ok(());
        };
        if let Err(e) = self.start_frames() {
            let _ = self.stop_frames();
            return Err(e);
        }
        // In case an activation was missed (no main run loop while paused).
        self.read_frontmost();
        let at = event_tap::now();
        let before = std::mem::replace(
            &mut *lock(&self.pause),
            PauseState {
                paused_at: None,
                resumed_at: Some(at),
            },
        );
        if let Err(e) = self.start_tap() {
            // The tap never started, so no event from after `at` exists.
            *lock(&self.pause) = before;
            let _ = self.stop_frames();
            return Err(e);
        }
        self.paused_at = None;
        self.pauses.push(PauseSpan {
            paused_at,
            resumed_at: Some(at),
        });
        Ok(())
    }

    /// Stops recording and runs step detection. Works while paused: that
    /// pause ends here.
    pub fn stop(mut self) -> Result<Recording, SessionError> {
        if let Some(paused_at) = self.paused_at.take() {
            self.pauses.push(PauseSpan {
                paused_at,
                resumed_at: None,
            });
        }
        let stopped_at = event_tap::now();
        self.halt_input();
        let router = join(self.router.take())?;
        let element = join(self.element.take())?;
        let focus = join(self.focus.take())?;
        let encoder = join(self.encoder.take())?;
        let mut frames_error = self.frames.iter().find_map(|f| f.reader().last_error());
        // The snapshots are already taken; a failed stop is only reported.
        if let Err(e) = self.stop_frames() {
            frames_error.get_or_insert_with(|| e.to_string());
        }
        let wall = self.started.elapsed();
        let (cpu_user, cpu_system) = match (self.cpu_start, process_cpu_time()) {
            (Some(a), Some(b)) => (
                b.user.saturating_sub(a.user),
                b.system.saturating_sub(a.system),
            ),
            _ => (Duration::ZERO, Duration::ZERO),
        };
        let session_dir = self
            .session_dir
            .take()
            .expect("session dir is set until stop");

        // No more activations count: the router and encoder have ended.
        self.observer = None;
        let front = self.front.log();
        let mut events = router.events;
        events.extend(element.events);
        events.extend(focus.events);
        // Stable: at equal ts, router events (AppActivated) come first, and
        // each thread's own order is kept (FocusChanged before its key).
        events.sort_by_key(|ev| ev.ts);
        // Events the router let through before it heard that an excluded
        // app had become frontmost (the activation is noted on the main
        // thread, a little late).
        let late_excluded = remove_excluded(&mut events, &front);

        let mut metas = router.snapshots;
        let mut latency = router.latency_ns;
        let dropped: Vec<Timestamp> = element
            .dropped_ts
            .iter()
            .chain(&focus.dropped_ts)
            .chain(&late_excluded)
            .copied()
            .collect();
        prune_snapshots(&mut metas, &mut latency, &dropped);
        // Frames taken while an excluded app was frontmost (or just before
        // its activation was noted, or just after), that the router took
        // before it knew.
        let before = metas.len();
        metas.retain(|ts, meta| {
            let keep = !front.frame_excluded(meta.frame.ts);
            if !keep {
                latency.remove(ts);
            }
            keep
        });
        let excluded_frames = router.excluded_frames + before - metas.len();
        // Nothing routed from inside a pause (there should be none).
        metas.retain(|ts, _| !in_pause(&self.pauses, *ts));
        latency.retain(|ts, _| !in_pause(&self.pauses, *ts));
        let mut snapshots = BTreeMap::new();
        let mut unencoded = 0;
        for (ts, meta) in metas {
            match encoder.written.get(&meta.frame) {
                Some(path) => {
                    snapshots.insert(
                        ts,
                        SnapshotRef {
                            frame_ts: meta.frame.ts,
                            source: meta.frame.source,
                            path: path.clone(),
                            geometry: meta.geometry,
                            width: meta.width,
                            height: meta.height,
                        },
                    );
                }
                None => {
                    latency.remove(&ts);
                    unencoded += 1;
                }
            }
        }
        // Frames no snapshot uses any more (e.g. of excluded apps).
        let used: HashSet<FrameKey> = snapshots.values().map(SnapshotRef::frame_key).collect();
        for (frame, path) in &encoder.written {
            if !used.contains(frame) {
                let _ = fs::remove_file(path);
            }
        }

        let dropped_paused = router.dropped_paused + remove_paused(&mut events, &self.pauses);
        let drafts = detect_steps_between_pauses(&events, &self.pauses, &self.steps);

        let mut ax = element.ax;
        ax.merge(&focus.ax);
        let metrics = SessionMetrics {
            wall,
            cpu_user,
            cpu_system,
            events: events.len(),
            dropped_own: router.dropped_own,
            dropped_excluded: router.dropped_excluded
                + element.dropped_ts.len()
                + focus.dropped_ts.len()
                + late_excluded.len(),
            excluded_frames,
            snapshots: snapshots.len(),
            missing_frames: router.missing_frames,
            unencoded_snapshots: unencoded,
            frame_latency_ns: latency.into_values().collect(),
            ax_lookups: ax.lookups,
            ax_errors: ax.errors,
            ax_total: ax.total,
            ax_max: ax.max,
            stale_masked: focus.stale_masked,
            encoded_frames: encoder.written.len(),
            encode_errors: encoder.errors,
            encode_total: encoder.total,
            encode_max: encoder.max,
            encoder_max_backlog: router.max_backlog,
            snapshots_dropped_queue_full: router.dropped_queue_full,
            settle_masked: focus.settle_masked,
            retro_scrubbed: focus.retro_scrubbed,
            probes_skipped: focus.probes_skipped,
            frames_error,
            dropped_paused,
            paused: self.pauses.iter().map(|p| p.duration(stopped_at)).sum(),
        };
        Ok(Recording {
            events,
            snapshots,
            drafts,
            metrics,
            displays: std::mem::take(&mut self.displays),
            crop_size: self.crop_size,
            fallback_ms: self.steps.typing_idle_ms.saturating_mul(2),
            pauses: std::mem::take(&mut self.pauses),
            session_dir,
        })
    }

    /// Stops the tap (joining its thread) and drops the router's input, so
    /// the router ends and drops the senders of the other threads.
    fn halt_input(&mut self) {
        if let Some(mut tap) = self.tap.take() {
            tap.stop();
        }
        self.tap_tx = None;
    }
}

impl Drop for RecordingSession {
    fn drop(&mut self) {
        self.halt_input();
        let _ = join(self.router.take());
        let _ = join(self.element.take());
        let _ = join(self.focus.take());
        let _ = join(self.encoder.take());
        let _ = self.stop_frames();
        if let Some(dir) = self.session_dir.take() {
            let _ = fs::remove_dir_all(dir);
        }
    }
}

fn spawn<T: Send + 'static>(
    name: &str,
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<JoinHandle<T>, SessionError> {
    thread::Builder::new()
        .name(name.into())
        .spawn(f)
        .map_err(SessionError::Spawn)
}

fn join<T>(handle: Option<JoinHandle<T>>) -> Result<T, SessionError> {
    handle
        .ok_or(SessionError::ThreadPanicked)?
        .join()
        .map_err(|_| SessionError::ThreadPanicked)
}

/// The display a target captures, for [`RecordingInfo`]; `None` for a
/// window. Read again when the frame streams restart after a pause.
fn display_for(target: CaptureTarget) -> Option<frames::DisplayInfo> {
    match target {
        CaptureTarget::Display { display_id } | CaptureTarget::Region { display_id, .. } => {
            display_id.map_or_else(frames::DisplayInfo::main, frames::DisplayInfo::for_id)
        }
        CaptureTarget::Window { .. } => None,
    }
}

/// The stream targets a session target stands for: a display target
/// without an id means every active display, one stream each, so a click
/// on any display finds its frame; anything else is one stream. With no
/// active display listed, the main display (the target as given).
pub fn stream_targets(target: CaptureTarget) -> Vec<CaptureTarget> {
    match target {
        CaptureTarget::Display { display_id: None } => {
            let displays: Vec<CaptureTarget> = frames::DisplayInfo::all()
                .into_iter()
                .map(|d| CaptureTarget::Display {
                    display_id: Some(d.id),
                })
                .collect();
            if displays.is_empty() {
                vec![target]
            } else {
                displays
            }
        }
        other => vec![other],
    }
}

/// A frame stream the router can snapshot from: its reader and the global
/// rect it shows, so a click picks the display it landed on.
#[derive(Debug, Clone)]
struct FrameSource {
    rect: Rect,
    reader: FrameReader,
}

impl FrameSource {
    fn of(stream: &FrameStream) -> Self {
        Self {
            rect: stream.geometry().map_or(
                Rect {
                    x: 0.0,
                    y: 0.0,
                    width: 0.0,
                    height: 0.0,
                },
                |g| g.rect_points(),
            ),
            reader: stream.reader(),
        }
    }
}

/// The sources, indexed as [`FrameKey::source`]; replaced whole when the
/// streams restart.
type SharedSources = Arc<Mutex<Vec<FrameSource>>>;

/// The source whose rect contains `point`; the only source if there is
/// one; the first source for a point on no source (its frame then has no
/// crop); `None` with no sources.
fn pick_source(sources: &[FrameSource], point: Option<Point>) -> Option<usize> {
    if sources.len() <= 1 {
        return (!sources.is_empty()).then_some(0);
    }
    point
        .and_then(|p| sources.iter().position(|s| rect_contains(s.rect, p)))
        .or(Some(0))
}

fn rect_contains(r: Rect, p: Point) -> bool {
    p.x >= r.x && p.y >= r.y && p.x < r.x + r.width && p.y < r.y + r.height
}

/// Identifies a snapshot frame: the source it came from and its
/// presentation time. Two streams can stamp frames with the same time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FrameKey {
    pub source: usize,
    pub ts: Timestamp,
}

impl FrameKey {
    /// File stem of the encoded frame: `<source>-<frame ts>`.
    pub fn file_stem(&self) -> String {
        format!("{}-{}", self.source, self.ts.0)
    }
}

/// An encoded snapshot frame.
#[derive(Debug, Clone, PartialEq)]
pub struct SnapshotRef {
    pub frame_ts: Timestamp,
    /// Index of the stream the frame came from (see
    /// [`RecordingSession::frames`]).
    pub source: usize,
    /// Lossless WebP in the session folder.
    pub path: PathBuf,
    pub geometry: FrameGeometry,
    pub width: u32,
    pub height: u32,
}

impl SnapshotRef {
    pub fn frame_key(&self) -> FrameKey {
        FrameKey {
            source: self.source,
            ts: self.frame_ts,
        }
    }
}

/// What a session recorded. Dropping it removes the session folder with
/// the encoded snapshots.
#[derive(Debug)]
pub struct Recording {
    /// Enriched events sorted by ts (secure keys already scrubbed).
    pub events: Vec<TimedEvent>,
    /// Snapshots keyed by the ts of the event that took them.
    pub snapshots: BTreeMap<Timestamp, SnapshotRef>,
    pub drafts: Vec<DraftStep>,
    pub metrics: SessionMetrics,
    /// The captured displays, for [`RecordingInfo`]; none for a window
    /// target.
    pub displays: Vec<frames::DisplayInfo>,
    pub crop_size: (u32, u32),
    /// How far a step may be from the snapshot it borrows.
    pub fallback_ms: u64,
    /// The pauses, in order; one still open at stop has no `resumed_at`.
    pub pauses: Vec<PauseSpan>,
    session_dir: PathBuf,
}

impl Drop for Recording {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.session_dir);
    }
}

/// The result of [`Recording::save_guide`].
#[derive(Debug, Clone)]
pub struct SavedGuide {
    pub handle: GuideHandle,
    pub guide: Guide,
    /// Draft steps left out because no snapshot was close enough.
    pub skipped: usize,
}

impl Recording {
    /// The hidden folder holding the encoded snapshots.
    pub fn session_dir(&self) -> &Path {
        &self.session_dir
    }

    /// Writes a new bundle titled `title`. Each draft step gets its snapshot
    /// ([`match_snapshot`]) as its own full image (`steps/<id>.webp`, a copy
    /// of the encoded snapshot), a crop around the point when it is on the
    /// frame (with [`StepImage::crop_rect`]), and its points in frame
    /// pixels. If any write fails, the bundle is removed.
    pub fn save_guide(&self, library: &Library, title: &str) -> Result<SavedGuide, SessionError> {
        let mut guide = Guide::new(title, "en");
        guide.recording = Some(self.recording_info());
        let handle = library.create(&guide)?;
        let filled = self.fill_steps(&handle, &mut guide).and_then(|skipped| {
            library.save_to(&handle, &guide)?;
            Ok(skipped)
        });
        match filled {
            Ok(skipped) => Ok(SavedGuide {
                handle,
                guide,
                skipped,
            }),
            Err(e) => {
                let _ = library.delete(&handle);
                Err(e)
            }
        }
    }

    fn fill_steps(&self, handle: &GuideHandle, guide: &mut Guide) -> Result<usize, SessionError> {
        let max_gap_ns = self.fallback_ms.saturating_mul(1_000_000);
        let mut skipped = 0;
        // The last decoded frame (RGBA), for crops.
        let mut decoded: Option<(FrameKey, u32, Vec<u8>)> = None;
        for draft in &self.drafts {
            let Some((_, snap)) = match_snapshot(&self.snapshots, draft.at, max_gap_ns) else {
                skipped += 1;
                continue;
            };
            let id = Ulid::new();
            // Each step gets its own `steps/<id>.webp`, even when steps share a
            // frame: a copy of the encoded snapshot (a clone on APFS).
            let full = library::step_image_path(id, StepImageVariant::Full);
            let dest = handle.resolve(&full)?;
            fs::copy(&snap.path, &dest).map_err(io_err(&dest))?;
            let (point, end_point) = step_points(&snap.geometry, draft);
            let (crop, crop_rect_px) = match point {
                Some(center) => {
                    if decoded.as_ref().map(|d| d.0) != Some(snap.frame_key()) {
                        let img = library::read_webp(&snap.path)?;
                        decoded = Some((snap.frame_key(), img.width(), img.into_raw()));
                    }
                    let (_, width, rgba) = decoded.as_ref().expect("decoded above");
                    let height = (rgba.len() / (*width as usize * 4).max(1)) as u32;
                    let rect = crop_rect(center, *width, height, self.crop_size);
                    let path = handle.write_step_image(
                        id,
                        StepImageVariant::Crop,
                        &crop_view(rgba, *width, rect),
                    )?;
                    (Some(path), Some(rect.to_rect()))
                }
                None => (None, None),
            };
            let image = StepImage {
                full,
                show: if crop.is_some() {
                    ImageShow::Crop
                } else {
                    ImageShow::Full
                },
                crop,
                crop_rect: crop_rect_px,
                after: None,
            };
            let mut step = draft.clone().into_step(id, image);
            step.point = point;
            step.end_point = end_point;
            guide.items.push(Item::Step(step));
        }
        Ok(skipped)
    }

    /// Copies each distinct snapshot frame to
    /// `dir/frames/<source>-<frame ts>.webp`. Returns the number written.
    pub fn write_raw_frames(&self, dir: &Path) -> Result<usize, SessionError> {
        let frames_dir = dir.join("frames");
        fs::create_dir_all(&frames_dir).map_err(io_err(&frames_dir))?;
        let mut seen = HashSet::new();
        for snap in self.snapshots.values() {
            let key = snap.frame_key();
            if seen.insert(key) {
                let dest = frames_dir.join(format!("{}.webp", key.file_stem()));
                fs::copy(&snap.path, &dest).map_err(io_err(&dest))?;
            }
        }
        Ok(seen.len())
    }

    fn recording_info(&self) -> RecordingInfo {
        let mut apps: Vec<AppInfo> = Vec::new();
        for ev in &self.events {
            if let CaptureEvent::AppActivated { app } = &ev.event {
                let name = app
                    .name
                    .clone()
                    .or_else(|| app.bundle_id.clone())
                    .unwrap_or_else(|| format!("pid {}", app.pid));
                if !apps
                    .iter()
                    .any(|a| a.name == name && a.bundle_id == app.bundle_id)
                {
                    apps.push(AppInfo {
                        name,
                        bundle_id: app.bundle_id.clone(),
                        version: None,
                    });
                }
            }
        }
        let displays = self
            .displays
            .iter()
            .map(|d| {
                let (width, height) = d.pixel_size();
                model::DisplayInfo {
                    id: d.id,
                    name: None,
                    width,
                    height,
                    scale: d.scale,
                }
            })
            .collect();
        RecordingInfo {
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            os_version: None,
            apps,
            displays,
        }
    }
}

/// Counters and timings for one session.
#[derive(Debug, Clone, Default)]
pub struct SessionMetrics {
    pub wall: Duration,
    /// Process CPU time (getrusage) during the session, all threads,
    /// including whatever else the host process did.
    pub cpu_user: Duration,
    pub cpu_system: Duration,
    /// Events kept, including the ones the session added.
    pub events: usize,
    pub dropped_own: usize,
    /// Events of excluded apps (by target pid, clicked element or focused
    /// element), or from while an excluded app was frontmost.
    pub dropped_excluded: usize,
    /// Snapshot frames not taken or removed because they were captured
    /// while an excluded app was frontmost, less than
    /// [`EXCLUDED_FRAME_HEAD_MS`] before its activation was noted, or less
    /// than [`EXCLUDED_FRAME_TAIL_MS`] after it left.
    pub excluded_frames: usize,
    pub snapshots: usize,
    /// Step-triggering events for which the ring had no earlier frame.
    pub missing_frames: usize,
    /// Snapshots lost because their frame could not be encoded.
    pub unencoded_snapshots: usize,
    /// For each kept snapshot: event ts minus the chosen frame's ts.
    pub frame_latency_ns: Vec<u64>,
    pub ax_lookups: usize,
    pub ax_errors: usize,
    pub ax_total: Duration,
    pub ax_max: Duration,
    /// Keys masked only because their focus lookup ran too late.
    pub stale_masked: usize,
    pub encoded_frames: usize,
    pub encode_errors: usize,
    pub encode_total: Duration,
    pub encode_max: Duration,
    /// Most frames waiting for the encoder at once (at most
    /// [`ENCODER_QUEUE`]).
    pub encoder_max_backlog: usize,
    /// Snapshots dropped because the encoder queue was full.
    pub snapshots_dropped_queue_full: usize,
    /// Keys masked by the settle guard: typed within
    /// [`StepConfig::focus_settle_ms`] of a focus-moving key or click,
    /// before the focus was seen to move.
    pub settle_masked: usize,
    /// Plain keys scrubbed afterwards, when a secure field turned out to
    /// have taken focus just after they were typed.
    pub retro_scrubbed: usize,
    /// Focus reads after a click skipped because keys were waiting.
    pub probes_skipped: usize,
    /// Why ScreenCaptureKit stopped the stream, or why stopping it failed.
    pub frames_error: Option<String>,
    /// Events dropped for being from while paused (the tap is stopped
    /// first, so normally none), plus any found inside a pause at stop.
    pub dropped_paused: usize,
    /// Time spent paused, within `wall`.
    pub paused: Duration,
}

impl SessionMetrics {
    /// CPU time as a percentage of one core over the wall time.
    pub fn cpu_percent(&self) -> f64 {
        let wall = self.wall.as_secs_f64();
        if wall > 0.0 {
            (self.cpu_user + self.cpu_system).as_secs_f64() / wall * 100.0
        } else {
            0.0
        }
    }

    pub fn latency(&self) -> Option<LatencyStats> {
        LatencyStats::from_ns(&self.frame_latency_ns)
    }
}

/// Distribution of frame-to-event latencies, in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LatencyStats {
    pub count: usize,
    pub min_ms: f64,
    pub median_ms: f64,
    pub p95_ms: f64,
    pub max_ms: f64,
    pub mean_ms: f64,
}

impl LatencyStats {
    /// Nearest-rank percentiles; `None` for no samples.
    pub fn from_ns(samples: &[u64]) -> Option<Self> {
        if samples.is_empty() {
            return None;
        }
        let mut sorted = samples.to_vec();
        sorted.sort_unstable();
        let ms = |ns: u64| ns as f64 / 1e6;
        let rank = |p: f64| {
            let r = (p * sorted.len() as f64).ceil() as usize;
            sorted[r.clamp(1, sorted.len()) - 1]
        };
        let sum: u128 = sorted.iter().map(|&n| u128::from(n)).sum();
        Some(Self {
            count: sorted.len(),
            min_ms: ms(sorted[0]),
            median_ms: ms(rank(0.5)),
            p95_ms: ms(rank(0.95)),
            max_ms: ms(sorted[sorted.len() - 1]),
            mean_ms: sum as f64 / sorted.len() as f64 / 1e6,
        })
    }
}

// ---------------------------------------------------------------------------
// Router thread

/// Messages for the focus thread, in event order.
enum FocusMsg {
    Key {
        event: Box<TimedEvent>,
        pid: i32,
    },
    /// The manual-step hotkey, checked against the focused app's exclusion.
    Manual(Timestamp),
    /// A click at this ts: drop the cached verdict and arm the settle guard.
    Invalidate(Timestamp),
    /// Read the focus after a click's `MouseUp` at this ts.
    Probe(Timestamp),
    /// The session resumed at this ts: nothing from before the pause says
    /// where the next key goes.
    Resume(Timestamp),
}

struct Outputs {
    element: Sender<TimedEvent>,
    focus: Sender<FocusMsg>,
    /// Frames to encode.
    encode: SyncSender<EncodeJob>,
    backlog: Arc<AtomicUsize>,
}

/// A snapshot frame for the encoder, with the event that took it (for
/// [`LiveSnapshot`]).
struct EncodeJob {
    source: usize,
    frame: Frame,
    ts: Timestamp,
    /// The event's point in frame pixels, if on the frame.
    point: Option<Point>,
}

/// A snapshot before its frame is encoded.
struct SnapshotMeta {
    frame: FrameKey,
    geometry: FrameGeometry,
    width: u32,
    height: u32,
}

struct Router {
    own_pid: i32,
    excluded: Vec<String>,
    hotkey: Option<Hotkey>,
    filter: ExclusionFilter,
    runs: KeyRunTracker,
    apps: HashMap<i32, AppRef>,
    app_pid: Option<i32>,
    sources: SharedSources,
    /// Where the pointer was last seen (any mouse event, kept or dropped),
    /// to pick the display for events without a point (keys, manual steps).
    pointer: Option<Point>,
    out: Outputs,
    sent_frames: HashSet<FrameKey>,
    events: Vec<TimedEvent>,
    snapshots: BTreeMap<Timestamp, SnapshotMeta>,
    latency_ns: BTreeMap<Timestamp, u64>,
    missing_frames: usize,
    max_backlog: usize,
    dropped_queue_full: usize,
    dropped_own: usize,
    dropped_excluded: usize,
    excluded_frames: usize,
    pause: SharedPause,
    front: Arc<Frontmost>,
    /// The resume already applied, so it is applied once.
    resumed_at: Option<Timestamp>,
    dropped_paused: usize,
}

struct RouterOut {
    events: Vec<TimedEvent>,
    snapshots: BTreeMap<Timestamp, SnapshotMeta>,
    latency_ns: BTreeMap<Timestamp, u64>,
    missing_frames: usize,
    max_backlog: usize,
    dropped_queue_full: usize,
    dropped_own: usize,
    dropped_excluded: usize,
    excluded_frames: usize,
    dropped_paused: usize,
}

impl Router {
    fn new(
        cfg: &SessionConfig,
        sources: SharedSources,
        out: Outputs,
        pause: SharedPause,
        front: Arc<Frontmost>,
    ) -> Self {
        Self {
            own_pid: i32::try_from(std::process::id()).unwrap_or(-1),
            excluded: cfg.excluded_bundle_ids.clone(),
            hotkey: cfg.manual_hotkey,
            filter: ExclusionFilter::default(),
            runs: KeyRunTracker::new(cfg.steps.typing_idle_ms),
            apps: HashMap::new(),
            app_pid: None,
            sources,
            pointer: None,
            out,
            sent_frames: HashSet::new(),
            events: Vec::new(),
            snapshots: BTreeMap::new(),
            latency_ns: BTreeMap::new(),
            missing_frames: 0,
            max_backlog: 0,
            dropped_queue_full: 0,
            dropped_own: 0,
            dropped_excluded: 0,
            excluded_frames: 0,
            pause,
            front,
            resumed_at: None,
            dropped_paused: 0,
        }
    }

    fn run(&mut self, rx: Receiver<TargetedEvent>) {
        for targeted in rx {
            self.route(targeted);
        }
    }

    fn route(&mut self, targeted: TargetedEvent) {
        let TargetedEvent {
            target_pid,
            mut event,
        } = targeted;
        if !self.admit(event.ts) {
            self.dropped_paused += 1;
            return;
        }
        if let Some(point) = event_point(&event.event) {
            self.pointer = Some(point);
        }
        // Any click may move focus, including clicks we drop.
        if matches!(
            event.event,
            CaptureEvent::MouseDown { .. } | CaptureEvent::MouseUp { .. }
        ) {
            let _ = self.out.focus.send(FocusMsg::Invalidate(event.ts));
        }
        if target_pid == self.own_pid {
            self.dropped_own += 1;
            return;
        }
        let app = (target_pid > 0).then(|| {
            self.apps
                .entry(target_pid)
                .or_insert_with(|| app_for_pid(target_pid))
                .clone()
        });
        // Its own app, or whatever app is in front: a key or a click
        // aimed elsewhere (pid 0, a background window) while an excluded
        // app is frontmost is dropped too.
        let excluded = self.front.excluded_at(event.ts)
            || app
                .as_ref()
                .is_some_and(|app| is_excluded_app(app, &self.excluded));
        if !self.filter.keep(&event.event, excluded) {
            self.dropped_excluded += 1;
            return;
        }
        if let CaptureEvent::KeyDown {
            key,
            modifiers,
            is_repeat,
            ..
        } = &event.event
        {
            if self.hotkey.is_some_and(|h| h.matches(key, *modifiers)) {
                if *is_repeat {
                    return;
                }
                event.event = CaptureEvent::ManualStep;
            }
        }
        if matches!(
            event.event,
            CaptureEvent::MouseDown { .. }
                | CaptureEvent::KeyDown { .. }
                | CaptureEvent::ManualStep
        ) {
            self.note_app(event.ts, target_pid, app);
        }
        if self.runs.should_snapshot(&event, target_pid) {
            self.snapshot(event.ts, event_point(&event.event));
        }
        match event.event {
            CaptureEvent::MouseDown { .. } | CaptureEvent::MouseDragged { .. } => {
                let _ = self.out.element.send(event);
            }
            CaptureEvent::MouseUp { .. } => {
                let ts = event.ts;
                let _ = self.out.element.send(event);
                let _ = self.out.focus.send(FocusMsg::Probe(ts));
            }
            CaptureEvent::KeyDown { .. } => {
                let _ = self.out.focus.send(FocusMsg::Key {
                    event: Box::new(event),
                    pid: target_pid,
                });
            }
            CaptureEvent::ManualStep => {
                let _ = self.out.focus.send(FocusMsg::Manual(event.ts));
            }
            _ => self.events.push(event),
        }
    }

    /// False for an event from while paused. The first event from after a
    /// resume resets what links events across the pause: the next key
    /// starts a new run (with a snapshot), `AppActivated` is emitted again
    /// for the detector's new stretch, no press is half excluded, and the
    /// focus thread forgets what it knew.
    fn admit(&mut self, ts: Timestamp) -> bool {
        let PauseState {
            paused_at,
            resumed_at,
        } = *lock(&self.pause);
        if paused_at.is_some_and(|at| ts >= at) {
            return false;
        }
        if let Some(at) = resumed_at.filter(|at| ts >= *at && self.resumed_at != Some(*at)) {
            self.resumed_at = Some(at);
            self.runs.reset();
            self.app_pid = None;
            self.filter = ExclusionFilter::default();
            let _ = self.out.focus.send(FocusMsg::Resume(at));
        }
        true
    }

    /// Emits `AppActivated` when the target app of a click or key changes.
    fn note_app(&mut self, ts: Timestamp, pid: i32, app: Option<AppRef>) {
        if pid <= 0 || self.app_pid == Some(pid) {
            return;
        }
        self.app_pid = Some(pid);
        self.events.push(TimedEvent {
            ts,
            event: CaptureEvent::AppActivated {
                app: app.unwrap_or(AppRef {
                    pid,
                    ..AppRef::default()
                }),
            },
        });
    }

    /// Takes the pre-event frame from the source under `point`, or under
    /// the pointer as last seen for an event without one (a key), or
    /// under the mouse now if no mouse event has been seen yet.
    fn snapshot(&mut self, ts: Timestamp, point: Option<Point>) {
        let picked = {
            let sources = lock(&self.sources);
            let under = point
                .or(self.pointer)
                .or_else(|| sources.len().gt(&1).then(frames::mouse_location).flatten());
            pick_source(&sources, under).map(|i| (i, sources[i].reader.frame_before(ts)))
        };
        let Some((source, Some(frame))) = picked else {
            self.missing_frames += 1;
            return;
        };
        if self.front.frame_excluded(frame.ts) {
            self.excluded_frames += 1;
            return;
        }
        let meta = SnapshotMeta {
            frame: FrameKey {
                source,
                ts: frame.ts,
            },
            geometry: frame.geometry,
            width: frame.width,
            height: frame.height,
        };
        let latency = ts.0.saturating_sub(frame.ts.0);
        if !self.sent_frames.contains(&meta.frame) {
            let waiting = self.out.backlog.fetch_add(1, Ordering::SeqCst) + 1;
            // The event's own point (not the pointer fallback), on the frame.
            let point = point.and_then(|p| frame.geometry.point_to_pixel_checked(p));
            let job = EncodeJob {
                source,
                frame,
                ts,
                point,
            };
            // The encoder now owns the only extra reference to the frame.
            // Never wait for it: a full queue drops this snapshot, and the
            // step borrows a neighbour within the gap or is skipped.
            match self.out.encode.try_send(job) {
                Ok(()) => {
                    self.max_backlog = self.max_backlog.max(waiting);
                    self.sent_frames.insert(meta.frame);
                }
                Err(TrySendError::Full(_)) => {
                    self.out.backlog.fetch_sub(1, Ordering::SeqCst);
                    self.dropped_queue_full += 1;
                    return;
                }
                Err(TrySendError::Disconnected(_)) => {
                    self.out.backlog.fetch_sub(1, Ordering::SeqCst);
                    return;
                }
            }
        }
        self.latency_ns.insert(ts, latency);
        self.snapshots.entry(ts).or_insert(meta);
    }

    fn finish(self) -> RouterOut {
        // `self.out` is dropped here, which ends the other threads.
        RouterOut {
            events: self.events,
            snapshots: self.snapshots,
            latency_ns: self.latency_ns,
            missing_frames: self.missing_frames,
            max_backlog: self.max_backlog,
            dropped_queue_full: self.dropped_queue_full,
            dropped_own: self.dropped_own,
            dropped_excluded: self.dropped_excluded,
            excluded_frames: self.excluded_frames,
            dropped_paused: self.dropped_paused,
        }
    }
}

/// The pointer position of a mouse event.
fn event_point(event: &CaptureEvent) -> Option<Point> {
    match event {
        CaptureEvent::MouseDown { point, .. }
        | CaptureEvent::MouseUp { point, .. }
        | CaptureEvent::MouseDragged { point, .. }
        | CaptureEvent::Scroll { point, .. } => Some(*point),
        _ => None,
    }
}

/// Name and bundle id of a running app.
fn app_for_pid(pid: i32) -> AppRef {
    objc2::rc::autoreleasepool(|_| {
        let app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid);
        AppRef {
            pid,
            name: app
                .as_ref()
                .and_then(|a| a.localizedName())
                .map(|s| s.to_string()),
            bundle_id: app
                .as_ref()
                .and_then(|a| a.bundleIdentifier())
                .map(|s| s.to_string()),
        }
    })
}

// ---------------------------------------------------------------------------
// AX threads

#[derive(Debug, Default, Clone)]
struct AxStats {
    lookups: usize,
    errors: usize,
    total: Duration,
    max: Duration,
}

impl AxStats {
    fn time<T, E>(&mut self, f: impl FnOnce() -> Result<T, E>) -> Result<T, E> {
        let start = Instant::now();
        let result = f();
        let took = start.elapsed();
        self.lookups += 1;
        self.errors += usize::from(result.is_err());
        self.total += took;
        self.max = self.max.max(took);
        result
    }

    fn merge(&mut self, other: &AxStats) {
        self.lookups += other.lookups;
        self.errors += other.errors;
        self.total += other.total;
        self.max = self.max.max(other.max);
    }
}

/// Mouse events: fills `MouseDown.element` and drops presses on excluded
/// apps that the router could not see (target pid 0 or another pid).
struct ElementWorker {
    excluded: Vec<String>,
    filter: ExclusionFilter,
    events: Vec<TimedEvent>,
    dropped_ts: Vec<Timestamp>,
    ax: AxStats,
}

struct ElementOut {
    events: Vec<TimedEvent>,
    dropped_ts: Vec<Timestamp>,
    ax: AxStats,
}

impl ElementWorker {
    fn new(cfg: &SessionConfig) -> Self {
        Self {
            excluded: cfg.excluded_bundle_ids.clone(),
            filter: ExclusionFilter::default(),
            events: Vec::new(),
            dropped_ts: Vec::new(),
            ax: AxStats::default(),
        }
    }

    fn run(&mut self, rx: Receiver<TimedEvent>) {
        for event in rx {
            self.handle(event);
        }
    }

    fn handle(&mut self, mut event: TimedEvent) {
        let mut found = None;
        let mut excluded = false;
        if let CaptureEvent::MouseDown { point, .. } = &event.event {
            let point = *point;
            found = self.ax.time(|| ax::element_at(point)).ok().flatten();
            excluded = found
                .as_ref()
                .is_some_and(|el| is_excluded_app(&el.app, &self.excluded));
        }
        if !self.filter.keep(&event.event, excluded) {
            self.dropped_ts.push(event.ts);
            return;
        }
        if let CaptureEvent::MouseDown { element, .. } = &mut event.event {
            *element = found;
        }
        self.events.push(event);
    }

    fn finish(self) -> ElementOut {
        ElementOut {
            events: self.events,
            dropped_ts: self.dropped_ts,
            ax: self.ax,
        }
    }
}

/// What to do with a key press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyVerdict {
    /// A definite non-secure element: keep the characters.
    Plain,
    /// Secure, or unknown: scrub the characters.
    Secure,
    /// The focused element belongs to an excluded app: drop the key.
    Excluded,
}

/// The verdict for a full focused-element lookup.
pub fn key_verdict(focused: &ax::Result<Option<RawElement>>, excluded: &[String]) -> KeyVerdict {
    if let Ok(Some(el)) = focused {
        if is_excluded_app(&el.app, excluded) {
            return KeyVerdict::Excluded;
        }
    }
    if ax::must_mask_keys(focused) {
        KeyVerdict::Secure
    } else {
        KeyVerdict::Plain
    }
}

/// A plain verdict from a lookup that ran `lag_ns` after its key becomes
/// secure once the lag exceeds `max_lag_ns`: the focus may have moved since.
pub fn apply_staleness(verdict: KeyVerdict, lag_ns: u64, max_lag_ns: u64) -> KeyVerdict {
    if verdict == KeyVerdict::Plain && lag_ns > max_lag_ns {
        KeyVerdict::Secure
    } else {
        verdict
    }
}

/// A focused-element reference compared with `CFEqual`.
#[derive(Clone)]
struct AxRef(CFRetained<AXUIElement>);

impl PartialEq for AxRef {
    fn eq(&self, other: &Self) -> bool {
        let a: &CFType = &self.0;
        let b: &CFType = &other.0;
        a == b
    }
}

/// The system-wide focused element reference (one AX message), or `None`
/// when there is none or the lookup failed.
fn focused_ref() -> Option<AxRef> {
    // SAFETY: no arguments; returns a +1 element wrapped by the binding.
    let system = unsafe { AXUIElement::new_system_wide() };
    // SAFETY: valid element. A failure only means the default timeout stays.
    let _ = unsafe { system.set_messaging_timeout(ax::MESSAGING_TIMEOUT_SECS) };
    let name = CFString::from_static_str("AXFocusedUIElement");
    let mut out: *const CFType = std::ptr::null();
    // SAFETY: valid element and out-pointer; on success `out` is +1.
    let err = unsafe { system.copy_attribute_value(&name, NonNull::from(&mut out)) };
    let value = NonNull::new(out.cast_mut())
        // SAFETY: a non-null `out` holds a +1 reference we now own (also
        // released this way on failure, which leaves it null in practice).
        .map(|p| unsafe { CFRetained::from_raw(p) })?;
    if err != AXError::Success {
        return None;
    }
    value.downcast::<AXUIElement>().ok().map(AxRef)
}

/// Where the focus thread reads the focus: AX in a session, a script in
/// tests.
trait FocusSource {
    type Ref: PartialEq + Clone;
    /// The focused element's identity (one cheap read), `None` if unknown.
    fn focused_ref(&mut self) -> Option<Self::Ref>;
    fn focused_element(&mut self) -> ax::Result<Option<RawElement>>;
    /// The clock event timestamps are on.
    fn now(&self) -> Timestamp;
}

struct AxFocus;

impl FocusSource for AxFocus {
    type Ref = AxRef;

    fn focused_ref(&mut self) -> Option<AxRef> {
        focused_ref()
    }

    fn focused_element(&mut self) -> ax::Result<Option<RawElement>> {
        ax::focused_element()
    }

    fn now(&self) -> Timestamp {
        event_tap::now()
    }
}

/// True for keys that can move focus: Tab, Return, Enter, Esc and ⌘ or ⌃
/// shortcuts. Option-only chords (with or without ⇧) are not: on many
/// layouts they type characters (⌥L is "@" on German), as the step
/// detector also assumes.
pub fn moves_focus(code: u16, modifiers: Modifiers) -> bool {
    modifiers.command
        || modifiers.control
        || matches!(code, KEY_TAB | KEY_RETURN | KEY_KEYPAD_ENTER | KEY_ESCAPE)
}

/// Fail-closed window after a focus-moving key or a click.
///
/// The tap sees an event before the target app does, so the first keys
/// after Tab or a click can be looked up while the old field is still
/// focused. For `settle_ms` after [`SettleGuard::arm`], a key is held
/// secure until the focused element is seen to differ from the one focused
/// before (`before`; unknown holds for the whole window).
#[derive(Debug, Clone)]
pub struct SettleGuard<K> {
    settle_ms: u64,
    armed: Option<(Timestamp, Option<K>)>,
}

impl<K: PartialEq> SettleGuard<K> {
    pub fn new(settle_ms: u64) -> Self {
        Self {
            settle_ms,
            armed: None,
        }
    }

    pub fn arm(&mut self, ts: Timestamp, before: Option<K>) {
        self.armed = Some((ts, before));
    }

    /// True if a key at `ts`, with `current` focused (`None`: unknown),
    /// must be treated as secure.
    pub fn holds(&mut self, ts: Timestamp, current: Option<&K>) -> bool {
        let Some((at, before)) = &self.armed else {
            return false;
        };
        if ts < *at {
            return false;
        }
        if ts.millis_since(*at) > self.settle_ms {
            self.armed = None;
            return false;
        }
        match (before, current) {
            (Some(before), Some(current)) if before != current => {
                self.armed = None;
                false
            }
            _ => true,
        }
    }
}

/// Masks plain `KeyDown`s typed before a secure field was found focused at
/// `secure_at`, walking back from it. Returns the number scrubbed.
///
/// - With `since` (the last focus-moving key or click): every key after
///   it, as long as consecutive keys are at most `idle_ms` apart. The
///   boundary explains the run, so a stalled app that moved focus late
///   cannot leave earlier characters of the run in clear.
/// - Without: keys at most `settle_ms` before `secure_at`, the step
///   detector's late-secure-focus rule.
pub fn retro_scrub(
    events: &mut [TimedEvent],
    since: Option<Timestamp>,
    secure_at: Timestamp,
    settle_ms: u64,
    idle_ms: u64,
) -> usize {
    let settle_ns = settle_ms.saturating_mul(1_000_000);
    let idle_ns = idle_ms.saturating_mul(1_000_000);
    let mut newer = secure_at;
    let mut scrubbed = 0;
    for ev in events.iter_mut().rev() {
        if ev.ts > secure_at {
            continue;
        }
        let past = match since {
            Some(since) => ev.ts <= since,
            None => secure_at.0 - ev.ts.0 > settle_ns,
        };
        if past {
            break;
        }
        if let CaptureEvent::KeyDown { key, secure, .. } = &mut ev.event {
            if since.is_some() && newer.0 - ev.ts.0 > idle_ns {
                break;
            }
            newer = ev.ts;
            if !*secure {
                *secure = true;
                scrub_secure_key(key);
                scrubbed += 1;
            }
        }
    }
    scrubbed
}

/// One key's lookup.
struct Lookup<R> {
    verdict: KeyVerdict,
    /// How long after the key the focus was sampled.
    lag_ns: u64,
    /// The verdict came from a full read, not the cache.
    miss: bool,
    current: Option<R>,
}

/// Keys and manual steps: fills `KeyDown.secure`, scrubs secure keys, drops
/// keys and manual steps focused on excluded apps, and emits
/// `FocusChanged`.
struct FocusWorker<S: FocusSource> {
    source: S,
    excluded: Vec<String>,
    settle_ms: u64,
    idle_ms: u64,
    cache: FocusCache<S::Ref>,
    guard: SettleGuard<S::Ref>,
    /// The focus seen at the last key lookup.
    last_ref: Option<S::Ref>,
    /// The last focus-moving key or click.
    boundary: Option<Timestamp>,
    last_verdict: Option<KeyVerdict>,
    /// Last focused element reported; outer `None` until the first lookup.
    focus: Option<Option<RawElement>>,
    events: Vec<TimedEvent>,
    dropped_ts: Vec<Timestamp>,
    stale_masked: usize,
    settle_masked: usize,
    retro_scrubbed: usize,
    probes_skipped: usize,
    ax: AxStats,
}

struct FocusOut {
    events: Vec<TimedEvent>,
    dropped_ts: Vec<Timestamp>,
    stale_masked: usize,
    settle_masked: usize,
    retro_scrubbed: usize,
    probes_skipped: usize,
    ax: AxStats,
}

impl<S: FocusSource> FocusWorker<S> {
    fn new(cfg: &SessionConfig, source: S) -> Self {
        Self {
            source,
            excluded: cfg.excluded_bundle_ids.clone(),
            settle_ms: cfg.steps.focus_settle_ms,
            idle_ms: cfg.steps.typing_idle_ms,
            cache: FocusCache::new(cfg.steps.typing_idle_ms),
            guard: SettleGuard::new(cfg.steps.focus_settle_ms),
            last_ref: None,
            boundary: None,
            last_verdict: None,
            focus: None,
            events: Vec::new(),
            dropped_ts: Vec::new(),
            stale_masked: 0,
            settle_masked: 0,
            retro_scrubbed: 0,
            probes_skipped: 0,
            ax: AxStats::default(),
        }
    }

    fn run(&mut self, rx: Receiver<FocusMsg>) {
        let mut pending = VecDeque::new();
        loop {
            let msg = match pending.pop_front() {
                Some(msg) => msg,
                None => match rx.recv() {
                    Ok(msg) => msg,
                    Err(_) => break,
                },
            };
            if matches!(msg, FocusMsg::Probe(_)) {
                pending.extend(rx.try_iter());
            }
            let keys_waiting = pending.iter().any(|m| matches!(m, FocusMsg::Key { .. }));
            self.handle(msg, keys_waiting);
        }
    }

    fn handle(&mut self, msg: FocusMsg, keys_waiting: bool) {
        match msg {
            FocusMsg::Key { event, pid } => self.key(*event, pid),
            FocusMsg::Manual(ts) => self.manual(ts),
            FocusMsg::Invalidate(ts) => {
                self.cache.invalidate();
                // A fresh read: `last_ref` is only as new as the last key.
                // This runs before the app has handled the click, so it is
                // the old focus; if it is already the new one, the guard
                // holds for the whole window, which is safe.
                let source = &mut self.source;
                let before = self.ax.time(|| source.focused_ref().ok_or(())).ok();
                self.arm(ts, before);
            }
            FocusMsg::Probe(ts) => {
                // Only for labels; never delay waiting keys with it.
                if keys_waiting {
                    self.probes_skipped += 1;
                    return;
                }
                let source = &mut self.source;
                let focused = self.ax.time(|| source.focused_element());
                self.note_focus(ts, &focused);
            }
            FocusMsg::Resume(ts) => {
                // The next key gets a full lookup, the focus is reported
                // again for the detector's new stretch, and no scrub
                // reaches back across the pause.
                self.cache.invalidate();
                self.guard = SettleGuard::new(self.settle_ms);
                self.last_ref = None;
                self.last_verdict = None;
                self.focus = None;
                self.boundary = Some(ts);
            }
        }
    }

    /// Arms the settle guard at `ts` with the focus from before the move.
    fn arm(&mut self, ts: Timestamp, before: Option<S::Ref>) {
        self.guard.arm(ts, before);
        self.boundary = Some(ts);
    }

    /// Keeps the manual step unless the focused app is excluded.
    fn manual(&mut self, ts: Timestamp) {
        let source = &mut self.source;
        let focused = self.ax.time(|| source.focused_element());
        if key_verdict(&focused, &self.excluded) == KeyVerdict::Excluded {
            self.dropped_ts.push(ts);
            return;
        }
        self.events.push(TimedEvent {
            ts,
            event: CaptureEvent::ManualStep,
        });
    }

    fn key(&mut self, mut event: TimedEvent, pid: i32) {
        let ts = event.ts;
        let CaptureEvent::KeyDown {
            key,
            modifiers,
            is_repeat,
            secure,
        } = &mut event.event
        else {
            return;
        };
        let lookup = if *is_repeat {
            // Same key held: same verdict, unknown → secure.
            Lookup {
                verdict: self.last_verdict.unwrap_or(KeyVerdict::Secure),
                lag_ns: self.source.now().0.saturating_sub(ts.0),
                miss: false,
                current: self.last_ref.clone(),
            }
        } else {
            self.lookup(ts, pid)
        };
        let mut verdict = lookup.verdict;
        if verdict == KeyVerdict::Plain && self.guard.holds(ts, lookup.current.as_ref()) {
            verdict = KeyVerdict::Secure;
            self.settle_masked += 1;
        }
        let staled = apply_staleness(verdict, lookup.lag_ns, STALE_LOOKUP_MS * 1_000_000);
        if staled != verdict {
            self.stale_masked += 1;
            verdict = staled;
        }
        if lookup.miss && lookup.verdict == KeyVerdict::Secure {
            // A secure field just took focus: keys typed just before may
            // have gone to it already.
            self.retro_scrubbed += retro_scrub(
                &mut self.events,
                self.boundary,
                ts,
                self.settle_ms,
                self.idle_ms,
            );
        }
        self.last_verdict = Some(verdict);
        self.cache.after_key(key.code, *modifiers);
        if moves_focus(key.code, *modifiers) {
            // This key's own lookup saw the focus from before the move.
            self.arm(ts, self.last_ref.clone());
        }
        match verdict {
            KeyVerdict::Excluded => {
                self.dropped_ts.push(ts);
                return;
            }
            KeyVerdict::Secure => {
                *secure = true;
                scrub_secure_key(key);
            }
            KeyVerdict::Plain => *secure = false,
        }
        self.events.push(event);
    }

    /// The verdict for a key at `ts` to `pid`.
    fn lookup(&mut self, ts: Timestamp, pid: i32) -> Lookup<S::Ref> {
        let source = &mut self.source;
        let focused = self.ax.time(|| source.focused_ref().ok_or(()));
        let lag_ns = self.source.now().0.saturating_sub(ts.0);
        let Ok(focused) = focused else {
            // Nothing focused, or no answer: fail closed, cache nothing.
            return Lookup {
                verdict: KeyVerdict::Secure,
                lag_ns,
                miss: true,
                current: None,
            };
        };
        self.last_ref = Some(focused.clone());
        if let Some(verdict) = self.cache.get(&focused, pid, ts) {
            return Lookup {
                verdict,
                lag_ns,
                miss: false,
                current: Some(focused),
            };
        }
        let source = &mut self.source;
        let full = self.ax.time(|| source.focused_element());
        // The full read re-reads the focus; trust it only if focus did not
        // move in between.
        let source = &mut self.source;
        let again = self.ax.time(|| source.focused_ref().ok_or(())).ok();
        let confirmed = again.as_ref() == Some(&focused);
        self.note_focus(ts, &full);
        let verdict = if confirmed {
            let verdict = key_verdict(&full, &self.excluded);
            self.cache.store(focused.clone(), pid, ts, verdict);
            verdict
        } else {
            KeyVerdict::Secure
        };
        Lookup {
            verdict,
            lag_ns,
            miss: true,
            current: Some(focused),
        }
    }

    /// Emits `FocusChanged` when a definite lookup differs from the last
    /// one. Elements of excluded apps are never reported.
    fn note_focus(&mut self, ts: Timestamp, focused: &ax::Result<Option<RawElement>>) {
        let Ok(element) = focused else {
            return;
        };
        if element
            .as_ref()
            .is_some_and(|el| is_excluded_app(&el.app, &self.excluded))
        {
            return;
        }
        if self.focus.as_ref() == Some(element) {
            return;
        }
        self.focus = Some(element.clone());
        self.events.push(TimedEvent {
            ts,
            event: CaptureEvent::FocusChanged {
                element: element.clone(),
            },
        });
    }

    fn finish(self) -> FocusOut {
        FocusOut {
            events: self.events,
            dropped_ts: self.dropped_ts,
            stale_masked: self.stale_masked,
            settle_masked: self.settle_masked,
            retro_scrubbed: self.retro_scrubbed,
            probes_skipped: self.probes_skipped,
            ax: self.ax,
        }
    }
}

// ---------------------------------------------------------------------------
// Encoder thread

struct Encoder {
    dir: PathBuf,
    backlog: Arc<AtomicUsize>,
    sink: Option<SnapshotSink>,
    front: Arc<Frontmost>,
    written: HashMap<FrameKey, PathBuf>,
    errors: usize,
    total: Duration,
    max: Duration,
}

struct EncoderOut {
    written: HashMap<FrameKey, PathBuf>,
    errors: usize,
    total: Duration,
    max: Duration,
}

impl Encoder {
    fn new(
        dir: PathBuf,
        backlog: Arc<AtomicUsize>,
        sink: Option<SnapshotSink>,
        front: Arc<Frontmost>,
    ) -> Self {
        Self {
            dir,
            backlog,
            sink,
            front,
            written: HashMap::new(),
            errors: 0,
            total: Duration::ZERO,
            max: Duration::ZERO,
        }
    }

    fn run(&mut self, rx: Receiver<EncodeJob>) {
        for job in rx {
            self.encode(job);
            self.backlog.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// Writes the frame (once per [`FrameKey`]), drops its pixels and
    /// reports it to the sink. Checks the frame against the excluded apps
    /// again first: the router may have taken it just before it heard of
    /// an excluded app's activation. Such a frame is neither written nor
    /// reported (and its snapshot is removed at stop).
    fn encode(&mut self, job: EncodeJob) {
        let EncodeJob {
            source,
            frame,
            ts,
            point,
        } = job;
        let key = FrameKey {
            source,
            ts: frame.ts,
        };
        if self.written.contains_key(&key) || self.front.frame_excluded(frame.ts) {
            return;
        }
        let start = Instant::now();
        let path = self.dir.join(format!("{}.webp", key.file_stem()));
        let pixels = PixelBuffer::bgra(frame.width, frame.height, &frame.pixels);
        let written = library::write_webp(&path, &pixels).is_ok();
        let took = start.elapsed();
        self.total += took;
        self.max = self.max.max(took);
        let (width, height) = (frame.width, frame.height);
        drop(frame);
        if !written {
            self.errors += 1;
            return;
        }
        self.written.insert(key, path.clone());
        if let Some(sink) = &self.sink {
            sink.report(&LiveSnapshot {
                ts,
                frame: key,
                path,
                width,
                height,
                point,
            });
        }
    }

    fn finish(self) -> EncoderOut {
        EncoderOut {
            written: self.written,
            errors: self.errors,
            total: self.total,
            max: self.max,
        }
    }
}

// ---------------------------------------------------------------------------
// Frontmost app

/// A stretch of event time during which an excluded app was frontmost.
/// `end` is `None` while it still is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExcludedSpan {
    pub bundle_id: String,
    pub start: Timestamp,
    pub end: Option<Timestamp>,
}

/// The excluded apps that were frontmost, in order, built from app
/// activations (see `# Excluded apps`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FrontmostLog {
    spans: Vec<ExcludedSpan>,
}

impl FrontmostLog {
    /// Notes that `bundle_id` became frontmost at `ts` (activations come
    /// in order). Returns the change, if any: `Some(Some(id))` when an
    /// excluded app `id` is now in front (also replacing another excluded
    /// one), `Some(None)` when an excluded app left the front. An app
    /// without a bundle id is not excluded.
    pub fn activated(
        &mut self,
        ts: Timestamp,
        bundle_id: Option<&str>,
        excluded: &[String],
    ) -> Option<Option<String>> {
        let now = bundle_id.filter(|id| excluded.iter().any(|e| e == id));
        if self.current() == now {
            return None;
        }
        if let Some(open) = self.spans.last_mut().filter(|s| s.end.is_none()) {
            open.end = Some(ts);
        }
        if let Some(id) = now {
            self.spans.push(ExcludedSpan {
                bundle_id: id.to_owned(),
                start: ts,
                end: None,
            });
        }
        Some(now.map(str::to_owned))
    }

    /// The excluded app in front now, if any.
    pub fn current(&self) -> Option<&str> {
        self.spans
            .last()
            .filter(|s| s.end.is_none())
            .map(|s| s.bundle_id.as_str())
    }

    pub fn spans(&self) -> &[ExcludedSpan] {
        &self.spans
    }

    /// True if an excluded app was frontmost at `ts` (as noted: events
    /// get no head margin).
    pub fn excluded_at(&self, ts: Timestamp) -> bool {
        self.within(ts, 0, 0)
    }

    /// True if a frame captured at `ts` may show an excluded app: one was
    /// frontmost then, was noted as frontmost less than
    /// [`EXCLUDED_FRAME_HEAD_MS`] after, or stopped being so less than
    /// [`EXCLUDED_FRAME_TAIL_MS`] before.
    pub fn frame_excluded(&self, ts: Timestamp) -> bool {
        const MS: u64 = 1_000_000;
        self.within(ts, EXCLUDED_FRAME_HEAD_MS * MS, EXCLUDED_FRAME_TAIL_MS * MS)
    }

    /// True if `ts` falls in a span widened by `head_ns` before its start
    /// and `tail_ns` after its end.
    fn within(&self, ts: Timestamp, head_ns: u64, tail_ns: u64) -> bool {
        self.spans.iter().rev().any(|s| {
            ts.0 >= s.start.0.saturating_sub(head_ns)
                && s.end.is_none_or(|end| ts.0 < end.0.saturating_add(tail_ns))
        })
    }
}

/// Removes the events (sorted by ts) from while an excluded app was
/// frontmost, by the router's rule ([`ExclusionFilter`]): a press goes or
/// stays whole, as its `MouseDown` does. Returns their timestamps.
fn remove_excluded(events: &mut Vec<TimedEvent>, log: &FrontmostLog) -> Vec<Timestamp> {
    let mut filter = ExclusionFilter::default();
    let mut removed = Vec::new();
    events.retain(|ev| {
        let keep = filter.keep(&ev.event, log.excluded_at(ev.ts));
        if !keep {
            removed.push(ev.ts);
        }
        keep
    });
    removed
}

/// The frontmost app's exclusion, shared by the session (which notes
/// activations), the router and the encoder (which gate on it).
struct Frontmost {
    excluded: Vec<String>,
    sink: Option<ExclusionSink>,
    log: Mutex<FrontmostLog>,
}

impl Frontmost {
    fn new(cfg: &SessionConfig) -> Self {
        Self {
            excluded: cfg.excluded_bundle_ids.clone(),
            sink: cfg.on_excluded.clone(),
            log: Mutex::default(),
        }
    }

    /// Notes an activation and reports a change to the sink (outside the
    /// lock).
    fn activated(&self, ts: Timestamp, bundle_id: Option<&str>) {
        let change = lock(&self.log).activated(ts, bundle_id, &self.excluded);
        if let (Some(change), Some(sink)) = (change, &self.sink) {
            sink.report(change.as_deref());
        }
    }

    fn current(&self) -> Option<String> {
        lock(&self.log).current().map(str::to_owned)
    }

    fn excluded_at(&self, ts: Timestamp) -> bool {
        lock(&self.log).excluded_at(ts)
    }

    fn frame_excluded(&self, ts: Timestamp) -> bool {
        lock(&self.log).frame_excluded(ts)
    }

    fn log(&self) -> FrontmostLog {
        lock(&self.log).clone()
    }
}

/// The bundle id of the frontmost app now.
fn frontmost_bundle_id() -> Option<String> {
    objc2::rc::autoreleasepool(|_| {
        NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .and_then(|app| app.bundleIdentifier())
            .map(|id| id.to_string())
    })
}

/// Notes every app activation (`NSWorkspaceDidActivateApplicationNotification`)
/// in a [`Frontmost`] until dropped. The notification is posted, and so
/// handled, on the main thread: it needs the main run loop running, as in
/// the app.
struct FrontmostObserver {
    token: Retained<ProtocolObject<dyn NSObjectProtocol>>,
}

// SAFETY: the token is only passed to `removeObserver:`, and
// NSNotificationCenter is thread-safe.
unsafe impl Send for FrontmostObserver {}

impl FrontmostObserver {
    fn start(front: Arc<Frontmost>) -> Self {
        let block = RcBlock::new(move |note: NonNull<NSNotification>| {
            let ts = event_tap::now();
            // SAFETY: the center passes a valid notification for the call.
            let note = unsafe { note.as_ref() };
            let bundle_id = objc2::rc::autoreleasepool(|_| {
                note.userInfo()
                    // SAFETY: an NSString key, as the dictionary expects.
                    .and_then(|info| unsafe { info.objectForKey(NSWorkspaceApplicationKey) })
                    .and_then(|app| app.downcast::<NSRunningApplication>().ok())
                    .and_then(|app| app.bundleIdentifier())
                    .map(|id| id.to_string())
            });
            front.activated(ts, bundle_id.as_deref());
        });
        let center = NSWorkspace::sharedWorkspace().notificationCenter();
        // SAFETY: no object filter and no queue (handled on the posting
        // thread); the block only touches `Send + Sync` state.
        let token = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(NSWorkspaceDidActivateApplicationNotification),
                None,
                None,
                &block,
            )
        };
        Self { token }
    }
}

impl Drop for FrontmostObserver {
    fn drop(&mut self) {
        let center = NSWorkspace::sharedWorkspace().notificationCenter();
        let token: &AnyObject = self.token.as_ref();
        // SAFETY: the token this center returned.
        unsafe { center.removeObserver(token) };
    }
}

// ---------------------------------------------------------------------------
// Pauses

/// One pause of a session, in event time. `resumed_at` is `None` for a
/// pause still open when the session stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PauseSpan {
    pub paused_at: Timestamp,
    pub resumed_at: Option<Timestamp>,
}

impl PauseSpan {
    /// True if `ts` falls inside the pause: from `paused_at` up to, not
    /// including, `resumed_at` (or forever, for an open pause).
    pub fn contains(&self, ts: Timestamp) -> bool {
        ts >= self.paused_at && self.resumed_at.is_none_or(|resumed| ts < resumed)
    }

    /// How long the pause lasted; an open one lasts until `end`.
    pub fn duration(&self, end: Timestamp) -> Duration {
        let to = self.resumed_at.unwrap_or(end);
        Duration::from_nanos(to.0.saturating_sub(self.paused_at.0))
    }
}

/// True if `ts` falls inside any of `pauses`.
fn in_pause(pauses: &[PauseSpan], ts: Timestamp) -> bool {
    pauses.iter().any(|p| p.contains(ts))
}

/// Removes the events inside any of `pauses`; returns how many.
fn remove_paused(events: &mut Vec<TimedEvent>, pauses: &[PauseSpan]) -> usize {
    let before = events.len();
    events.retain(|ev| !in_pause(pauses, ev.ts));
    before - events.len()
}

/// Runs [`detect_steps`] separately on each stretch of `events` (sorted by
/// ts, the events inside pauses already removed) between `pauses` (in
/// order), so no typing run, drag or late-focus rule reaches across a
/// pause. Returns the drafts in order.
fn detect_steps_between_pauses(
    events: &[TimedEvent],
    pauses: &[PauseSpan],
    cfg: &StepConfig,
) -> Vec<DraftStep> {
    let mut drafts = Vec::new();
    let mut rest = events;
    for pause in pauses {
        let cut = rest.partition_point(|ev| ev.ts < pause.paused_at);
        let (before, after) = rest.split_at(cut);
        drafts.extend(detect_steps(before, cfg));
        rest = after;
    }
    drafts.extend(detect_steps(rest, cfg));
    drafts
}

/// Pause times the session shares with the router, which compares them
/// with event timestamps (same clock, see `event_tap::now`), so queue lag
/// cannot reorder a pause and the events around it: an event at or after
/// `paused_at` is dropped; the first event at or after `resumed_at` resets
/// the state that would link it to the events before the pause.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct PauseState {
    paused_at: Option<Timestamp>,
    resumed_at: Option<Timestamp>,
}

type SharedPause = Arc<Mutex<PauseState>>;

/// Locks state shared between the session and the router; a panic while
/// holding it cannot leave it unusable.
fn lock<T>(shared: &Mutex<T>) -> MutexGuard<'_, T> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---------------------------------------------------------------------------
// Pure helpers

/// True if `app`'s bundle id is in `excluded`.
pub fn is_excluded_app(app: &AppRef, excluded: &[String]) -> bool {
    app.bundle_id
        .as_deref()
        .is_some_and(|id| excluded.iter().any(|e| e == id))
}

/// Drops events of excluded apps. A press is never split: its drag and
/// release go or stay as its `MouseDown` did, whatever app or exclusion
/// span they end in (a press that started in an excluded app is dropped
/// even if the pointer leaves it; one that started outside is kept even
/// if it ends over, or after the activation of, an excluded app). Drags
/// and releases carry only points and take no snapshot. A release or drag
/// without a press seen (after a resume) goes by its own `excluded`.
#[derive(Debug, Default, Clone)]
pub struct ExclusionFilter {
    /// Whether the current press's `MouseDown` was kept; `None` outside a
    /// press.
    press_kept: Option<bool>,
}

impl ExclusionFilter {
    /// `excluded`: the event's app is known to be excluded.
    pub fn keep(&mut self, event: &CaptureEvent, excluded: bool) -> bool {
        match event {
            CaptureEvent::MouseDown { .. } => {
                self.press_kept = Some(!excluded);
                !excluded
            }
            CaptureEvent::MouseDragged { .. } => self.press_kept.unwrap_or(!excluded),
            CaptureEvent::MouseUp { .. } => self.press_kept.take().unwrap_or(!excluded),
            _ => !excluded,
        }
    }
}

/// Decides which events get a frame snapshot: clicks, manual steps,
/// shortcuts, Return and Esc, and the first key of a typing burst (a new
/// app, or more than `idle_ms` since the last key, or after any of the
/// former).
#[derive(Debug, Clone)]
pub struct KeyRunTracker {
    idle_ms: u64,
    last_key: Option<(Timestamp, i32)>,
}

impl KeyRunTracker {
    pub fn new(idle_ms: u64) -> Self {
        Self {
            idle_ms,
            last_key: None,
        }
    }

    /// Forgets the current run: the next key starts a new one.
    pub fn reset(&mut self) {
        self.last_key = None;
    }

    pub fn should_snapshot(&mut self, ev: &TimedEvent, pid: i32) -> bool {
        match &ev.event {
            CaptureEvent::MouseDown { .. } => {
                self.last_key = None;
                true
            }
            CaptureEvent::ManualStep => true,
            CaptureEvent::KeyDown {
                key,
                modifiers,
                is_repeat,
                ..
            } => {
                if *is_repeat {
                    return false;
                }
                if modifiers.is_shortcut()
                    || matches!(key.code, KEY_RETURN | KEY_KEYPAD_ENTER | KEY_ESCAPE)
                {
                    self.last_key = None;
                    return true;
                }
                if key.code == KEY_TAB {
                    self.last_key = None;
                    return false;
                }
                let new_run = match self.last_key {
                    Some((last, last_pid)) => {
                        last_pid != pid || ev.ts.millis_since(last) > self.idle_ms
                    }
                    None => true,
                };
                self.last_key = Some((ev.ts, pid));
                new_run
            }
            _ => false,
        }
    }
}

/// The key verdict for the focused element `K`.
///
/// A hit needs the same element (`K: PartialEq`, `CFEqual` for AX), the
/// same pid, a ts no earlier than the last key and no gap over `idle_ms`.
/// [`FocusCache::invalidate`] (clicks) and [`FocusCache::after_key`] (Tab,
/// Return, Esc, shortcuts) drop it.
#[derive(Debug, Clone)]
pub struct FocusCache<K> {
    idle_ms: u64,
    entry: Option<(K, i32, Timestamp, KeyVerdict)>,
}

impl<K: PartialEq> FocusCache<K> {
    pub fn new(idle_ms: u64) -> Self {
        Self {
            idle_ms,
            entry: None,
        }
    }

    pub fn invalidate(&mut self) {
        self.entry = None;
    }

    /// The cached verdict for a key at `ts` to `pid` with `element`
    /// focused. A hit extends the burst to `ts`; a miss drops the entry.
    pub fn get(&mut self, element: &K, pid: i32, ts: Timestamp) -> Option<KeyVerdict> {
        let (cached, cached_pid, last, verdict) = self.entry.as_mut()?;
        if cached != element
            || *cached_pid != pid
            || ts < *last
            || ts.millis_since(*last) > self.idle_ms
        {
            self.entry = None;
            return None;
        }
        *last = ts;
        Some(*verdict)
    }

    pub fn store(&mut self, element: K, pid: i32, ts: Timestamp, verdict: KeyVerdict) {
        self.entry = Some((element, pid, ts, verdict));
    }

    /// Call after each key: keys that can move focus drop the entry.
    pub fn after_key(&mut self, code: u16, modifiers: Modifiers) {
        if moves_focus(code, modifiers) {
            self.invalidate();
        }
    }
}

/// Removes the characters of a key typed into a secure field. A key that
/// typed text keeps one `•` so the detector still counts it as typing;
/// non-printing keys (arrows, function keys) keep none.
pub fn scrub_secure_key(key: &mut Key) {
    let typed = key.characters.as_deref().is_some_and(|s| {
        s.chars()
            .any(|c| !c.is_control() && !('\u{F700}'..='\u{F8FF}').contains(&c))
    });
    key.characters = typed.then(|| "•".to_owned());
    key.characters_ignoring_modifiers = None;
}

/// Removes the snapshots (and latency samples) of events that were dropped
/// after the router took them.
pub fn prune_snapshots<T>(
    snapshots: &mut BTreeMap<Timestamp, T>,
    latency_ns: &mut BTreeMap<Timestamp, u64>,
    dropped: &[Timestamp],
) {
    for ts in dropped {
        snapshots.remove(ts);
        latency_ns.remove(ts);
    }
}

/// The snapshot for a step at `at`: taken exactly then, else the newest
/// one before, else the oldest one after, but only within `max_gap_ns`.
pub fn match_snapshot<T>(
    snapshots: &BTreeMap<Timestamp, T>,
    at: Timestamp,
    max_gap_ns: u64,
) -> Option<(Timestamp, &T)> {
    let before = snapshots
        .range(..=at)
        .next_back()
        .filter(|(ts, _)| at.0 - ts.0 <= max_gap_ns);
    before
        .or_else(|| {
            snapshots
                .range(at..)
                .next()
                .filter(|(ts, _)| ts.0 - at.0 <= max_gap_ns)
        })
        .map(|(ts, v)| (*ts, v))
}

/// A draft step's point and end point in frame pixels; `None` if off the
/// frame (e.g. on another display).
pub fn step_points(geometry: &FrameGeometry, draft: &DraftStep) -> (Option<Point>, Option<Point>) {
    let map = |p: Option<Point>| p.and_then(|p| geometry.point_to_pixel_checked(p));
    (map(draft.point), map(draft.end_point))
}

/// A pixel rectangle within a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CropRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl CropRect {
    /// As a model [`Rect`] in full-screenshot pixels, for
    /// [`StepImage::crop_rect`].
    pub fn to_rect(self) -> Rect {
        Rect {
            x: f64::from(self.x),
            y: f64::from(self.y),
            width: f64::from(self.width),
            height: f64::from(self.height),
        }
    }
}

/// A `size` crop centred on `center` (frame pixels), shifted to stay inside
/// a `frame_w`×`frame_h` frame, and shrunk if the frame is smaller.
pub fn crop_rect(center: Point, frame_w: u32, frame_h: u32, size: (u32, u32)) -> CropRect {
    let width = size.0.min(frame_w);
    let height = size.1.min(frame_h);
    let axis = |c: f64, len: u32, frame: u32| {
        let start = (c - f64::from(len) / 2.0).round();
        // NaN casts to 0.
        (start.max(0.0) as u32).min(frame - len)
    };
    CropRect {
        x: axis(center.x, width, frame_w),
        y: axis(center.y, height, frame_h),
        width,
        height,
    }
}

/// A view of `rect` inside a packed RGBA image `width` pixels wide.
fn crop_view(rgba: &[u8], width: u32, rect: CropRect) -> PixelBuffer<'_> {
    let stride = width as usize * 4;
    let start = rect.y as usize * stride + rect.x as usize * 4;
    PixelBuffer {
        width: rect.width,
        height: rect.height,
        bytes_per_row: stride,
        format: PixelFormat::Rgba,
        data: &rgba[start..],
    }
}

// ---------------------------------------------------------------------------
// CPU time

#[derive(Debug, Clone, Copy)]
struct CpuTime {
    user: Duration,
    system: Duration,
}

#[repr(C)]
#[derive(Default)]
struct Timeval {
    tv_sec: i64,
    /// `suseconds_t` is `i32` on Darwin.
    tv_usec: i32,
}

/// Darwin `struct rusage`: two timevals and 14 longs.
#[repr(C)]
#[derive(Default)]
struct Rusage {
    ru_utime: Timeval,
    ru_stime: Timeval,
    rest: [i64; 14],
}

const _: () = assert!(std::mem::size_of::<Rusage>() == 144);

const RUSAGE_SELF: i32 = 0;

extern "C" {
    fn getrusage(who: i32, usage: *mut Rusage) -> i32;
}

fn process_cpu_time() -> Option<CpuTime> {
    let mut usage = Rusage::default();
    // SAFETY: `usage` is a valid, correctly sized out-pointer.
    if unsafe { getrusage(RUSAGE_SELF, &mut usage) } != 0 {
        return None;
    }
    let dur = |t: &Timeval| {
        Duration::from_secs(t.tv_sec.max(0) as u64) + Duration::from_micros(t.tv_usec.max(0) as u64)
    };
    Some(CpuTime {
        user: dur(&usage.ru_utime),
        system: dur(&usage.ru_stime),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use guidesya_core::events::MouseButton;
    use guidesya_core::model::StepKind;

    use super::*;

    const MS: u64 = 1_000_000;

    fn mods() -> Modifiers {
        Modifiers::default()
    }

    fn cmd() -> Modifiers {
        Modifiers {
            command: true,
            ..Modifiers::default()
        }
    }

    fn key(code: u16, chars: Option<&str>) -> Key {
        Key {
            code,
            characters: chars.map(str::to_owned),
            characters_ignoring_modifiers: chars.map(str::to_owned),
        }
    }

    fn key_down(ts_ms: u64, code: u16, chars: &str, modifiers: Modifiers) -> TimedEvent {
        TimedEvent {
            ts: Timestamp(ts_ms * MS),
            event: CaptureEvent::KeyDown {
                key: key(code, Some(chars)),
                modifiers,
                is_repeat: false,
                secure: false,
            },
        }
    }

    fn mouse_down(ts_ms: u64) -> TimedEvent {
        TimedEvent {
            ts: Timestamp(ts_ms * MS),
            event: CaptureEvent::MouseDown {
                point: Point { x: 10.0, y: 10.0 },
                button: MouseButton::Left,
                click_count: 1,
                modifiers: mods(),
                element: None,
            },
        }
    }

    fn mouse(kind: &str) -> CaptureEvent {
        let point = Point { x: 0.0, y: 0.0 };
        match kind {
            "down" => CaptureEvent::MouseDown {
                point,
                button: MouseButton::Left,
                click_count: 1,
                modifiers: mods(),
                element: None,
            },
            "drag" => CaptureEvent::MouseDragged {
                point,
                button: MouseButton::Left,
            },
            _ => CaptureEvent::MouseUp {
                point,
                button: MouseButton::Left,
                click_count: 1,
                modifiers: mods(),
            },
        }
    }

    /// BGRA frame whose pixel (x, y) is B=x, G=y, R=0, A=255.
    fn frame(ts: u64, width: u32, height: u32, geometry: FrameGeometry) -> Frame {
        let mut pixels = vec![0u8; width as usize * height as usize * 4];
        for (i, px) in pixels.chunks_exact_mut(4).enumerate() {
            let (x, y) = (i as u32 % width, i as u32 / width);
            px.copy_from_slice(&[x as u8, y as u8, 0, 255]);
        }
        Frame {
            ts: Timestamp(ts),
            width,
            height,
            geometry,
            pixels: Arc::new(pixels),
        }
    }

    fn geometry(w_pt: f64, h_pt: f64, w_px: u32, h_px: u32) -> FrameGeometry {
        FrameGeometry::from_rect(
            Rect {
                x: 0.0,
                y: 0.0,
                width: w_pt,
                height: h_pt,
            },
            w_px,
            h_px,
        )
    }

    #[test]
    fn hotkey_matches_exact_modifiers() {
        let hk = Hotkey::MANUAL_STEP;
        let ctrl_opt = Modifiers {
            control: true,
            option: true,
            ..mods()
        };
        assert!(hk.matches(&key(KEY_S, Some("s")), ctrl_opt));
        let with_caps = Modifiers {
            caps_lock: true,
            ..ctrl_opt
        };
        assert!(hk.matches(&key(KEY_S, None), with_caps));
        let with_cmd = Modifiers {
            command: true,
            ..ctrl_opt
        };
        assert!(!hk.matches(&key(KEY_S, None), with_cmd));
        assert!(!hk.matches(&key(KEY_S, None), cmd()));
        assert!(!hk.matches(&key(2, None), ctrl_opt));
    }

    #[test]
    fn exclusion_filter_drops_whole_press() {
        let mut f = ExclusionFilter::default();
        assert!(!f.keep(&mouse("down"), true));
        // The pointer left the excluded app mid-drag: still dropped.
        assert!(!f.keep(&mouse("drag"), false));
        assert!(!f.keep(&mouse("up"), false));
        // The next press in another app is kept.
        assert!(f.keep(&mouse("down"), false));
        assert!(f.keep(&mouse("drag"), false));
        assert!(f.keep(&mouse("up"), false));
        assert!(!f.keep(&CaptureEvent::ManualStep, true));
        assert!(f.keep(&CaptureEvent::ManualStep, false));
        // A press kept at its start is kept whole, even if it ends over an
        // excluded app or after one was activated.
        assert!(f.keep(&mouse("down"), false));
        assert!(f.keep(&mouse("drag"), true));
        assert!(f.keep(&mouse("up"), true));
        // A release without a press seen goes by itself.
        assert!(!f.keep(&mouse("up"), true));
        assert!(f.keep(&mouse("up"), false));
    }

    #[test]
    fn stop_pass_never_splits_a_click() {
        let excluded = vec![ONE_PASSWORD.to_owned()];
        let mut log = FrontmostLog::default();
        log.activated(Timestamp(10 * MS), Some(ONE_PASSWORD), &excluded);
        log.activated(Timestamp(20 * MS), Some("com.apple.Safari"), &excluded);
        let at = |ms: u64, kind: &str| TimedEvent {
            ts: Timestamp(ms * MS),
            event: mouse(kind),
        };
        let mut events = vec![
            // Pressed before the span, released inside: kept whole.
            at(5, "down"),
            at(12, "up"),
            // Pressed inside, released after: removed whole.
            at(15, "down"),
            key_down(16, 0, "x", mods()),
            at(17, "drag"),
            at(25, "up"),
            key_down(26, 0, "a", mods()),
        ];
        let removed = remove_excluded(&mut events, &log);
        let ms = |ts: &[Timestamp]| ts.iter().map(|t| t.0 / MS).collect::<Vec<_>>();
        assert_eq!(ms(&removed), [15, 16, 17, 25]);
        let kept: Vec<Timestamp> = events.iter().map(|ev| ev.ts).collect();
        assert_eq!(ms(&kept), [5, 12, 26]);
    }

    #[test]
    fn excluded_app_by_bundle_id() {
        let list = vec!["com.1password.1password".to_owned()];
        let app = |id: Option<&str>| AppRef {
            pid: 1,
            name: None,
            bundle_id: id.map(str::to_owned),
        };
        assert!(is_excluded_app(
            &app(Some("com.1password.1password")),
            &list
        ));
        assert!(!is_excluded_app(&app(Some("com.apple.finder")), &list));
        assert!(!is_excluded_app(&app(None), &list));
    }

    #[test]
    fn key_runs_snapshot_first_key_only() {
        let mut t = KeyRunTracker::new(1500);
        assert!(t.should_snapshot(&key_down(0, 0, "a", mods()), 7));
        assert!(!t.should_snapshot(&key_down(100, 11, "b", mods()), 7));
        // Idle gap.
        assert!(t.should_snapshot(&key_down(2000, 0, "a", mods()), 7));
        // Other app.
        assert!(t.should_snapshot(&key_down(2100, 0, "a", mods()), 8));
        // Tab ends the run without a snapshot; the next key starts one.
        assert!(!t.should_snapshot(&key_down(2200, KEY_TAB, "\t", mods()), 8));
        assert!(t.should_snapshot(&key_down(2300, 0, "a", mods()), 8));
        // Shortcut and Return always, and they end the run.
        assert!(t.should_snapshot(&key_down(2400, KEY_S, "s", cmd()), 8));
        assert!(t.should_snapshot(&key_down(2500, 0, "a", mods()), 8));
        assert!(t.should_snapshot(&key_down(2600, KEY_RETURN, "\r", mods()), 8));
        assert!(t.should_snapshot(&key_down(2650, 0, "a", mods()), 8));
        assert!(!t.should_snapshot(&key_down(2660, 0, "a", mods()), 8));
        // Clicks always, and they end the run.
        assert!(t.should_snapshot(&mouse_down(2700), 8));
        assert!(t.should_snapshot(&key_down(2800, 0, "a", mods()), 8));
        let repeat = TimedEvent {
            ts: Timestamp(5000 * MS),
            event: CaptureEvent::KeyDown {
                key: key(0, Some("a")),
                modifiers: mods(),
                is_repeat: true,
                secure: false,
            },
        };
        assert!(!t.should_snapshot(&repeat, 8));
        let manual = TimedEvent {
            ts: Timestamp(0),
            event: CaptureEvent::ManualStep,
        };
        assert!(t.should_snapshot(&manual, 8));
    }

    #[test]
    fn focus_cache_is_per_element() {
        use KeyVerdict::*;
        let ts = |ms: u64| Timestamp(ms * MS);
        let mut c: FocusCache<u32> = FocusCache::new(1500);
        assert_eq!(c.get(&1, 7, ts(0)), None);
        c.store(1, 7, ts(0), Plain);
        assert_eq!(c.get(&1, 7, ts(100)), Some(Plain));
        // Each hit extends the burst.
        assert_eq!(c.get(&1, 7, ts(1500)), Some(Plain));
        assert_eq!(c.get(&1, 7, ts(2900)), Some(Plain));
        // Focus moved to another element mid-burst (e.g. the app moved it):
        // miss, and the old entry is gone.
        assert_eq!(c.get(&2, 7, ts(3000)), None);
        assert_eq!(c.get(&1, 7, ts(3010)), None);
        // Idle gap.
        c.store(1, 7, ts(4000), Plain);
        assert_eq!(c.get(&1, 7, ts(5600)), None);
        // Other pid.
        c.store(1, 7, ts(6000), Secure);
        assert_eq!(c.get(&1, 8, ts(6100)), None);
        // Clicks invalidate.
        c.store(1, 7, ts(7000), Plain);
        c.invalidate();
        assert_eq!(c.get(&1, 7, ts(7100)), None);
        // Keys that can move focus.
        for (code, m) in [
            (KEY_TAB, mods()),
            (KEY_RETURN, mods()),
            (KEY_KEYPAD_ENTER, mods()),
            (KEY_ESCAPE, mods()),
            (KEY_S, cmd()),
        ] {
            c.store(1, 7, ts(8000), Plain);
            c.after_key(code, m);
            assert_eq!(c.get(&1, 7, ts(8100)), None, "key {code}");
        }
        c.store(1, 7, ts(9000), Excluded);
        c.after_key(0, mods());
        assert_eq!(c.get(&1, 7, ts(9100)), Some(Excluded));
        // Out-of-order ts does not reuse the verdict.
        assert_eq!(c.get(&1, 7, ts(8000)), None);
    }

    #[test]
    fn key_verdict_fails_closed_and_excludes() {
        let excluded = vec!["com.1password.1password".to_owned()];
        let el = |secure: bool, bundle: &str| RawElement {
            is_secure: secure,
            app: AppRef {
                pid: 3,
                name: None,
                bundle_id: Some(bundle.to_owned()),
            },
            ..RawElement::default()
        };
        assert_eq!(
            key_verdict(&Ok(Some(el(false, "com.apple.TextEdit"))), &excluded),
            KeyVerdict::Plain
        );
        assert_eq!(
            key_verdict(&Ok(Some(el(true, "com.apple.Safari"))), &excluded),
            KeyVerdict::Secure
        );
        assert_eq!(key_verdict(&Ok(None), &excluded), KeyVerdict::Secure);
        assert_eq!(
            key_verdict(&Err(ax::AxError::Timeout), &excluded),
            KeyVerdict::Secure
        );
        // Excluded wins over plain and secure.
        for secure in [false, true] {
            assert_eq!(
                key_verdict(&Ok(Some(el(secure, "com.1password.1password"))), &excluded),
                KeyVerdict::Excluded
            );
        }
    }

    #[test]
    fn stale_lookups_mask() {
        let max = STALE_LOOKUP_MS * MS;
        assert_eq!(
            apply_staleness(KeyVerdict::Plain, 10 * MS, max),
            KeyVerdict::Plain
        );
        assert_eq!(
            apply_staleness(KeyVerdict::Plain, max, max),
            KeyVerdict::Plain
        );
        assert_eq!(
            apply_staleness(KeyVerdict::Plain, max + 1, max),
            KeyVerdict::Secure
        );
        assert_eq!(
            apply_staleness(KeyVerdict::Secure, 0, max),
            KeyVerdict::Secure
        );
        assert_eq!(
            apply_staleness(KeyVerdict::Excluded, 10 * max, max),
            KeyVerdict::Excluded
        );
    }

    #[test]
    fn scrub_keeps_only_a_dot() {
        let mut k = key(0, Some("p"));
        scrub_secure_key(&mut k);
        assert_eq!(k.characters.as_deref(), Some("•"));
        assert_eq!(k.characters_ignoring_modifiers, None);
        // Arrow keys (AppKit function-key range) and control characters
        // type nothing.
        let mut arrow = key(123, Some("\u{F702}"));
        scrub_secure_key(&mut arrow);
        assert_eq!(arrow.characters, None);
        let mut ret = key(KEY_RETURN, Some("\r"));
        scrub_secure_key(&mut ret);
        assert_eq!(ret.characters, None);
        let mut none = key(0, None);
        scrub_secure_key(&mut none);
        assert_eq!(none.characters, None);
    }

    #[test]
    fn scrubbed_secure_typing_still_detected_and_masked() {
        let mut events = Vec::new();
        for (i, c) in "hunter2".chars().enumerate() {
            let mut k = key(0, Some(&c.to_string()));
            scrub_secure_key(&mut k);
            events.push(TimedEvent {
                ts: Timestamp(i as u64 * 100 * MS),
                event: CaptureEvent::KeyDown {
                    key: k,
                    modifiers: mods(),
                    is_repeat: false,
                    secure: true,
                },
            });
        }
        let json = format!("{events:?}");
        assert!(!json.contains("hunter"));
        let steps = detect_steps(&events, &StepConfig::default());
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].kind, StepKind::Type);
        assert_eq!(
            steps[0].keystrokes.as_deref(),
            Some(guidesya_core::steps::SECURE_MASK)
        );
    }

    #[test]
    fn snapshot_matching_within_gap() {
        let gap = 3000 * MS;
        let at = |ms: u64| Timestamp(ms * MS);
        let mut snaps = BTreeMap::new();
        assert_eq!(
            match_snapshot(&snaps, at(5), gap),
            None::<(Timestamp, &u32)>
        );
        snaps.insert(at(10), 1u32);
        snaps.insert(at(20), 2);
        snaps.insert(at(30), 3);
        assert_eq!(match_snapshot(&snaps, at(20), gap), Some((at(20), &2)));
        assert_eq!(match_snapshot(&snaps, at(25), gap), Some((at(20), &2)));
        // Before every snapshot: the oldest one after.
        assert_eq!(match_snapshot(&snaps, at(1), gap), Some((at(10), &1)));
        // Within the gap after the last one.
        assert_eq!(match_snapshot(&snaps, at(3030), gap), Some((at(30), &3)));
        // Too far from any snapshot: none.
        assert_eq!(match_snapshot(&snaps, at(3031), gap), None);
        let mut late = BTreeMap::new();
        late.insert(at(5000), 9u32);
        assert_eq!(match_snapshot(&late, at(1000), gap), None);
        assert_eq!(match_snapshot(&late, at(2000), gap), Some((at(5000), &9)));
    }

    #[test]
    fn prune_removes_dropped_snapshots_and_latency() {
        let mut snaps: BTreeMap<Timestamp, &str> = [
            (Timestamp(1), "a"),
            (Timestamp(2), "b"),
            (Timestamp(3), "c"),
        ]
        .into();
        let mut latency: BTreeMap<Timestamp, u64> =
            [(Timestamp(1), 10), (Timestamp(2), 20), (Timestamp(3), 30)].into();
        prune_snapshots(&mut snaps, &mut latency, &[Timestamp(2), Timestamp(9)]);
        assert_eq!(snaps.keys().map(|t| t.0).collect::<Vec<_>>(), vec![1, 3]);
        assert_eq!(latency.values().copied().collect::<Vec<_>>(), vec![10, 30]);
    }

    #[test]
    fn crop_is_centred_and_clamped() {
        let size = (800, 500);
        let c = crop_rect(
            Point {
                x: 1000.0,
                y: 600.0,
            },
            2880,
            1800,
            size,
        );
        assert_eq!(
            c,
            CropRect {
                x: 600,
                y: 350,
                width: 800,
                height: 500
            }
        );
        // Near the top-left corner.
        let c = crop_rect(Point { x: 5.0, y: 5.0 }, 2880, 1800, size);
        assert_eq!((c.x, c.y), (0, 0));
        // Near the bottom-right corner.
        let c = crop_rect(
            Point {
                x: 2879.0,
                y: 1799.0,
            },
            2880,
            1800,
            size,
        );
        assert_eq!((c.x, c.y), (2080, 1300));
        // Frame smaller than the crop.
        let c = crop_rect(Point { x: 50.0, y: 50.0 }, 640, 400, size);
        assert_eq!(
            c,
            CropRect {
                x: 0,
                y: 0,
                width: 640,
                height: 400
            }
        );
        let c = crop_rect(
            Point {
                x: f64::NAN,
                y: 50.0,
            },
            2880,
            1800,
            size,
        );
        assert_eq!(c.x, 0);
        assert_eq!(
            CropRect {
                x: 1,
                y: 2,
                width: 3,
                height: 4
            }
            .to_rect(),
            Rect {
                x: 1.0,
                y: 2.0,
                width: 3.0,
                height: 4.0
            }
        );
    }

    #[test]
    fn crop_view_reads_the_right_pixels() {
        let (w, h) = (100u32, 60u32);
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for (i, px) in rgba.chunks_exact_mut(4).enumerate() {
            let (x, y) = (i as u32 % w, i as u32 / w);
            px.copy_from_slice(&[x as u8, y as u8, 7, 255]);
        }
        // Bottom-right corner: the view must pass the length check.
        let rect = crop_rect(Point { x: 99.0, y: 59.0 }, w, h, (20, 10));
        let out = crop_view(&rgba, w, rect).to_rgba().unwrap();
        assert_eq!(out.len(), 20 * 10 * 4);
        assert_eq!(&out[..4], &[rect.x as u8, rect.y as u8, 7, 255]);
        assert_eq!(&out[out.len() - 4..], &[99, 59, 7, 255]);
    }

    #[test]
    fn points_map_to_frame_pixels() {
        // Retina secondary display left of the main one.
        let geometry = FrameGeometry::from_rect(
            Rect {
                x: -1440.0,
                y: 0.0,
                width: 1440.0,
                height: 900.0,
            },
            2880,
            1800,
        );
        let draft = DraftStep {
            kind: StepKind::Drag,
            at: Timestamp(0),
            point: Some(Point {
                x: -1430.0,
                y: 20.0,
            }),
            end_point: Some(Point { x: 10.0, y: 20.0 }),
            element: None,
            app: None,
            keystrokes: None,
            title: "Drag".into(),
        };
        let (p, end) = step_points(&geometry, &draft);
        assert_eq!(p, Some(Point { x: 20.0, y: 40.0 }));
        // On the main display, not in this frame.
        assert_eq!(end, None);
    }

    #[test]
    fn latency_stats() {
        assert_eq!(LatencyStats::from_ns(&[]), None);
        let samples: Vec<u64> = (1..=100).map(|i| i * MS).collect();
        let s = LatencyStats::from_ns(&samples).unwrap();
        assert_eq!(s.count, 100);
        assert_eq!(s.min_ms, 1.0);
        assert_eq!(s.median_ms, 50.0);
        assert_eq!(s.p95_ms, 95.0);
        assert_eq!(s.max_ms, 100.0);
        assert!((s.mean_ms - 50.5).abs() < 1e-9);
        let one = LatencyStats::from_ns(&[3 * MS]).unwrap();
        assert_eq!((one.median_ms, one.p95_ms), (3.0, 3.0));
    }

    #[test]
    fn cpu_time_is_readable() {
        let a = process_cpu_time().expect("getrusage");
        let mut x = 0u64;
        for i in 0..2_000_000u64 {
            x = x.wrapping_add(i * i);
        }
        std::hint::black_box(x);
        let b = process_cpu_time().expect("getrusage");
        assert!(b.user + b.system >= a.user + a.system);
        assert!(b.user < Duration::from_secs(3600));
    }

    /// Scratch folder under `target/`, removed on drop.
    struct ScratchDir(PathBuf);

    impl ScratchDir {
        fn new() -> Self {
            let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/session-tests")
                .join(Ulid::new().to_string());
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
            // Also the parent, once the last test is done with it.
            let _ = fs::remove_dir(self.0.parent().unwrap());
        }
    }

    #[test]
    fn encoder_writes_each_frame_once() {
        let scratch = ScratchDir::new();
        let backlog = Arc::new(AtomicUsize::new(0));
        let seen: Arc<Mutex<Vec<LiveSnapshot>>> = Arc::default();
        let sink = Arc::clone(&seen);
        let sink = SnapshotSink::new(move |snap| lock(&sink).push(snap.clone()));
        let mut enc = Encoder::new(
            scratch.0.clone(),
            Arc::clone(&backlog),
            Some(sink),
            no_exclusions(),
        );
        let (tx, rx) = mpsc::channel();
        let f = frame(5 * MS, 16, 10, geometry(8.0, 5.0, 16, 10));
        for i in 0..2 {
            backlog.fetch_add(1, Ordering::SeqCst);
            tx.send(EncodeJob {
                source: 0,
                frame: f.clone(),
                ts: Timestamp((10 + i) * MS),
                point: Some(Point { x: 8.0, y: 5.0 }),
            })
            .unwrap();
        }
        drop(tx);
        enc.run(rx);
        let out = enc.finish();
        assert_eq!(out.written.len(), 1);
        // Reported once, for the first event that took the frame.
        let seen = lock(&seen);
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].ts, Timestamp(10 * MS));
        assert_eq!(seen[0].point, Some(Point { x: 8.0, y: 5.0 }));
        assert_eq!((seen[0].width, seen[0].height), (16, 10));
        assert_eq!(seen[0].path, scratch.0.join(format!("0-{}.webp", 5 * MS)));
        assert!(seen[0].path.exists());
        assert_eq!(out.errors, 0);
        assert_eq!(backlog.load(Ordering::SeqCst), 0);
        let key = FrameKey {
            source: 0,
            ts: Timestamp(5 * MS),
        };
        let img = library::read_webp(&out.written[&key]).unwrap();
        assert_eq!(img.dimensions(), (16, 10));
        // BGRA (x, y, 0, 255) is stored as RGBA (0, y, x, 255).
        assert_eq!(img.get_pixel(3, 2).0, [0, 2, 3, 255]);
        // The encoder kept no reference to the frame.
        assert_eq!(Arc::strong_count(&f.pixels), 1);
    }

    /// A recording whose snapshots are encoded into `scratch/session`.
    fn recording(
        scratch: &ScratchDir,
        events: Vec<TimedEvent>,
        frames: &[(u64, &Frame)],
    ) -> Recording {
        let session_dir = scratch.0.join("session");
        fs::create_dir_all(&session_dir).unwrap();
        let mut snapshots = BTreeMap::new();
        for (event_ms, f) in frames {
            let path = session_dir.join(format!("{}.webp", f.ts.0));
            if !path.exists() {
                library::write_webp(&path, &PixelBuffer::bgra(f.width, f.height, &f.pixels))
                    .unwrap();
            }
            snapshots.insert(
                Timestamp(event_ms * MS),
                SnapshotRef {
                    frame_ts: f.ts,
                    source: 0,
                    path,
                    geometry: f.geometry,
                    width: f.width,
                    height: f.height,
                },
            );
        }
        let drafts = detect_steps(&events, &StepConfig::default());
        Recording {
            events,
            snapshots,
            drafts,
            metrics: SessionMetrics::default(),
            displays: Vec::new(),
            crop_size: (40, 20),
            fallback_ms: 3000,
            pauses: Vec::new(),
            session_dir,
        }
    }

    fn click(ts_ms: u64, x: f64, y: f64) -> [TimedEvent; 2] {
        let point = Point { x, y };
        [
            TimedEvent {
                ts: Timestamp(ts_ms * MS),
                event: CaptureEvent::MouseDown {
                    point,
                    button: MouseButton::Left,
                    click_count: 1,
                    modifiers: mods(),
                    element: Some(RawElement {
                        role: Some("AXButton".into()),
                        title: Some("Save".into()),
                        ..RawElement::default()
                    }),
                },
            },
            TimedEvent {
                ts: Timestamp((ts_ms + 50) * MS),
                event: CaptureEvent::MouseUp {
                    point,
                    button: MouseButton::Left,
                    click_count: 1,
                    modifiers: mods(),
                },
            },
        ]
    }

    #[test]
    fn save_guide_writes_bundle_with_images() {
        let scratch = ScratchDir::new();
        let f1 = frame(5 * MS, 160, 100, geometry(80.0, 50.0, 160, 100));
        let mut events = click(10, 40.0, 25.0).to_vec();
        events.push(key_down(500, KEY_S, "s", cmd()));
        let rec = recording(&scratch, events, &[(10, &f1), (500, &f1)]);
        assert_eq!(rec.drafts.len(), 2);

        let lib = Library::open(scratch.0.join("lib")).unwrap();
        let saved = rec.save_guide(&lib, "Session test").unwrap();
        assert_eq!(saved.skipped, 0);
        let loaded = lib.load(&saved.handle.path).unwrap();
        let steps: Vec<_> = loaded.guide.steps().cloned().collect();
        assert_eq!(steps.len(), 2);

        let click = &steps[0];
        assert_eq!(click.kind, StepKind::Click);
        assert_eq!(click.point, Some(Point { x: 80.0, y: 50.0 }));
        assert_eq!(click.image.show, ImageShow::Crop);
        let full = saved.handle.read_image(&click.image.full).unwrap();
        assert_eq!(full.dimensions(), (160, 100));
        let crop = saved
            .handle
            .read_image(click.image.crop.as_deref().unwrap())
            .unwrap();
        assert_eq!(crop.dimensions(), (40, 20));
        // Crop top-left is pixel (60, 40): RGBA (0, y, x, 255).
        assert_eq!(crop.get_pixel(0, 0).0, [0, 40, 60, 255]);
        // cropRect in guide.json is the exact cut, in full-screenshot pixels.
        let json: serde_json::Value =
            serde_json::from_slice(&fs::read(saved.handle.guide_json()).unwrap()).unwrap();
        let cut = &json["items"][0]["image"]["cropRect"];
        assert_eq!(cut["x"], 60.0);
        assert_eq!(cut["y"], 40.0);
        assert_eq!(cut["width"].as_f64(), Some(f64::from(crop.width())));
        assert_eq!(cut["height"].as_f64(), Some(f64::from(crop.height())));
        assert_eq!(
            click.image.crop_rect,
            Some(Rect {
                x: 60.0,
                y: 40.0,
                width: 40.0,
                height: 20.0
            })
        );

        let shortcut = &steps[1];
        assert_eq!(shortcut.kind, StepKind::Shortcut);
        assert_eq!(shortcut.point, None);
        assert_eq!(shortcut.image.crop, None);
        assert_eq!(shortcut.image.crop_rect, None);
        assert_eq!(shortcut.image.show, ImageShow::Full);
        // Both steps show the same frame, but each has its own file.
        assert_eq!(
            click.image.full,
            library::step_image_path(click.id, StepImageVariant::Full)
        );
        assert_eq!(
            shortcut.image.full,
            library::step_image_path(shortcut.id, StepImageVariant::Full)
        );
        let read = |rel: &str| fs::read(saved.handle.resolve(rel).unwrap()).unwrap();
        assert_eq!(read(&click.image.full), read(&shortcut.image.full));

        let raw = scratch.0.join("raw");
        assert_eq!(rec.write_raw_frames(&raw).unwrap(), 1);
        assert!(raw.join(format!("frames/0-{}.webp", 5 * MS)).exists());

        // Dropping the recording removes its session folder.
        let session_dir = rec.session_dir().to_path_buf();
        drop(rec);
        assert!(!session_dir.exists());
    }

    #[test]
    fn save_guide_skips_steps_without_a_close_snapshot() {
        let scratch = ScratchDir::new();
        let f1 = frame(5 * MS, 16, 10, geometry(8.0, 5.0, 16, 10));
        let mut events = click(10, 4.0, 2.5).to_vec();
        // 10 s later, with no snapshot of its own (e.g. a missing frame).
        events.extend(click(10_000, 4.0, 2.5));
        let rec = recording(&scratch, events, &[(10, &f1)]);
        assert_eq!(rec.drafts.len(), 2);
        let lib = Library::open(scratch.0.join("lib")).unwrap();
        let saved = rec.save_guide(&lib, "Gap").unwrap();
        assert_eq!(saved.skipped, 1);
        assert_eq!(saved.guide.steps().count(), 1);
    }

    #[test]
    fn failed_save_removes_the_bundle() {
        let scratch = ScratchDir::new();
        let f1 = frame(5 * MS, 16, 10, geometry(8.0, 5.0, 16, 10));
        let rec = recording(&scratch, click(10, 4.0, 2.5).to_vec(), &[(10, &f1)]);
        // The encoded snapshot is gone, so copying the full image fails.
        fs::remove_file(&rec.snapshots.values().next().unwrap().path).unwrap();
        let lib = Library::open(scratch.0.join("lib")).unwrap();
        assert!(rec.save_guide(&lib, "Broken").is_err());
        assert!(lib.list().unwrap().is_empty());
    }

    /// A scripted focus: `current` is what AX would report right now.
    struct FakeFocus {
        current: Option<(u32, RawElement)>,
        clock: Timestamp,
    }

    impl FocusSource for FakeFocus {
        type Ref = u32;

        fn focused_ref(&mut self) -> Option<u32> {
            self.current.as_ref().map(|(id, _)| *id)
        }

        fn focused_element(&mut self) -> ax::Result<Option<RawElement>> {
            Ok(self.current.as_ref().map(|(_, el)| el.clone()))
        }

        fn now(&self) -> Timestamp {
            self.clock
        }
    }

    fn field(title: &str, secure: bool, bundle: &str) -> RawElement {
        RawElement {
            role: Some("AXTextField".into()),
            subrole: secure.then(|| "AXSecureTextField".into()),
            title: Some(title.into()),
            is_secure: secure,
            app: AppRef {
                pid: 5,
                name: Some("Safari".into()),
                bundle_id: Some(bundle.into()),
            },
            ..RawElement::default()
        }
    }

    fn worker(focus: Option<(u32, RawElement)>) -> FocusWorker<FakeFocus> {
        let cfg = SessionConfig {
            excluded_bundle_ids: vec!["com.1password.1password".into()],
            ..SessionConfig::default()
        };
        FocusWorker::new(
            &cfg,
            FakeFocus {
                current: focus,
                clock: Timestamp(0),
            },
        )
    }

    /// Feeds a key typed at `ms`, looked up 1 ms later.
    fn type_key(w: &mut FocusWorker<FakeFocus>, ms: u64, code: u16, chars: &str) {
        w.source.clock = Timestamp((ms + 1) * MS);
        w.handle(
            FocusMsg::Key {
                event: Box::new(key_down(ms, code, chars, mods())),
                pid: 5,
            },
            false,
        );
    }

    fn typed(events: &[TimedEvent]) -> Vec<(u64, Option<String>, bool)> {
        events
            .iter()
            .filter_map(|ev| match &ev.event {
                CaptureEvent::KeyDown { key, secure, .. } => {
                    Some((ev.ts.0 / MS, key.characters.clone(), *secure))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn username_tab_password_never_stores_password_characters() {
        let user = field("Username", false, "com.apple.Safari");
        let pass = field("Password", true, "com.apple.Safari");
        let mut w = worker(Some((1, user)));
        for (i, c) in ["b", "o", "b"].iter().enumerate() {
            type_key(&mut w, i as u64 * 100, 0, c);
        }
        type_key(&mut w, 300, KEY_TAB, "\t");
        // The app has not moved focus yet: the first password key is
        // looked up while the username field is still focused.
        type_key(&mut w, 310, 0, "x");
        w.source.current = Some((2, pass));
        type_key(&mut w, 330, 0, "y");
        type_key(&mut w, 340, 0, "z");
        let out = w.finish();
        assert_eq!(out.settle_masked, 1);

        let keys = typed(&out.events);
        assert_eq!(
            keys[..3],
            [
                (0, Some("b".into()), false),
                (100, Some("o".into()), false),
                (200, Some("b".into()), false)
            ]
        );
        for (ms, chars, secure) in &keys[4..] {
            assert!(secure, "key at {ms} ms");
            assert_eq!(chars.as_deref(), Some("•"));
        }
        // Nothing of the password reaches the raw dump (events.jsonl).
        let json = serde_json::to_string(&out.events).unwrap();
        for c in ["\"x\"", "\"y\"", "\"z\""] {
            assert!(!json.contains(c), "{c} in {json}");
        }
        // The guide: the username stays, the password is masked.
        let steps = detect_steps(&out.events, &StepConfig::default());
        let shown: Vec<_> = steps
            .iter()
            .filter_map(|s| s.keystrokes.as_deref())
            .collect();
        assert_eq!(shown, ["bob", guidesya_core::steps::SECURE_MASK]);
    }

    #[test]
    fn late_secure_focus_scrubs_keys_already_stored() {
        // No Tab or click: the page moved focus to the password field by
        // itself while the user kept typing.
        let user = field("Username", false, "com.apple.Safari");
        let pass = field("Password", true, "com.apple.Safari");
        let mut w = worker(Some((1, user)));
        type_key(&mut w, 0, 0, "a");
        type_key(&mut w, 1000, 0, "p");
        w.source.current = Some((2, pass));
        type_key(&mut w, 1100, 0, "q");
        let out = w.finish();
        assert_eq!(out.retro_scrubbed, 1);
        let keys = typed(&out.events);
        // Outside focus_settle_ms: kept. Within it: scrubbed.
        assert_eq!(keys[0], (0, Some("a".into()), false));
        assert_eq!(keys[1], (1000, Some("•".into()), true));
        assert_eq!(keys[2], (1100, Some("•".into()), true));
    }

    #[test]
    fn click_guard_masks_until_focus_moves() {
        let a = field("Search", false, "com.apple.Safari");
        let b = field("Name", false, "com.apple.Safari");
        let mut w = worker(Some((1, a)));
        type_key(&mut w, 0, 0, "s");
        // Click into another field at 1000; the app moves focus a bit later.
        w.handle(FocusMsg::Invalidate(Timestamp(1000 * MS)), false);
        type_key(&mut w, 1050, 0, "n");
        w.source.current = Some((2, b));
        type_key(&mut w, 1080, 0, "m");
        let keys = typed(&w.finish().events);
        assert_eq!(keys[1], (1050, Some("•".into()), true));
        assert_eq!(keys[2], (1080, Some("m".into()), false));
    }

    #[test]
    fn manual_step_in_excluded_app_is_dropped() {
        let vault = field("Search", false, "com.1password.1password");
        let mut w = worker(Some((1, vault)));
        w.handle(FocusMsg::Manual(Timestamp(5 * MS)), false);
        w.source.current = Some((2, field("Doc", false, "com.apple.TextEdit")));
        w.handle(FocusMsg::Manual(Timestamp(9 * MS)), false);
        let out = w.finish();
        assert_eq!(out.dropped_ts, vec![Timestamp(5 * MS)]);
        assert_eq!(out.events.len(), 1);
        assert_eq!(out.events[0].event, CaptureEvent::ManualStep);
        assert_eq!(out.events[0].ts, Timestamp(9 * MS));
    }

    #[test]
    fn probe_is_skipped_when_keys_wait() {
        let mut w = worker(Some((1, field("Doc", false, "com.apple.TextEdit"))));
        let (tx, rx) = mpsc::channel();
        tx.send(FocusMsg::Probe(Timestamp(MS))).unwrap();
        tx.send(FocusMsg::Key {
            event: Box::new(key_down(2, 0, "a", mods())),
            pid: 5,
        })
        .unwrap();
        tx.send(FocusMsg::Probe(Timestamp(3 * MS))).unwrap();
        drop(tx);
        w.source.clock = Timestamp(3 * MS);
        w.run(rx);
        let out = w.finish();
        assert_eq!(out.probes_skipped, 1);
        // The key was still handled, and the last probe ran.
        assert_eq!(typed(&out.events).len(), 1);
    }

    #[test]
    fn settle_guard_rules() {
        let at = |ms: u64| Timestamp(ms * MS);
        let mut g: SettleGuard<u32> = SettleGuard::new(300);
        assert!(!g.holds(at(0), Some(&1)));
        g.arm(at(100), Some(1));
        assert!(g.holds(at(150), Some(&1)));
        assert!(g.holds(at(160), None));
        // Focus seen to move: released for good.
        assert!(!g.holds(at(170), Some(&2)));
        assert!(!g.holds(at(180), Some(&1)));
        // Window over.
        g.arm(at(1000), Some(1));
        assert!(!g.holds(at(1301), Some(&1)));
        // Unknown "before": holds for the whole window.
        g.arm(at(2000), None);
        assert!(g.holds(at(2100), Some(&5)));
        assert!(g.holds(at(2300), Some(&6)));
        assert!(!g.holds(at(2301), Some(&6)));
        // Keys from before the arming are not held.
        g.arm(at(3000), Some(1));
        assert!(!g.holds(at(2990), Some(&1)));
    }

    #[test]
    fn retro_scrub_respects_boundary_and_window() {
        let mut events = vec![
            key_down(0, 0, "a", mods()),
            key_down(500, 0, "b", mods()),
            key_down(600, KEY_TAB, "\t", mods()),
            key_down(700, 0, "c", mods()),
            key_down(800, 0, "d", mods()),
        ];
        // Tab at 600 is the boundary: only "c" and "d" qualify.
        assert_eq!(
            retro_scrub(
                &mut events,
                Some(Timestamp(600 * MS)),
                Timestamp(850 * MS),
                300,
                1500
            ),
            2
        );
        let keys = typed(&events);
        assert_eq!(keys[1], (500, Some("b".into()), false));
        assert_eq!(keys[3], (700, Some("•".into()), true));
        assert_eq!(keys[4], (800, Some("•".into()), true));
        // No boundary: limited by the window only.
        let mut events = vec![key_down(0, 0, "a", mods()), key_down(500, 0, "b", mods())];
        assert_eq!(
            retro_scrub(&mut events, None, Timestamp(700 * MS), 300, 1500),
            1
        );
        assert!(!typed(&events)[0].2);
        // With a boundary: back to it, while keys stay within idle_ms of each
        // other, however long ago the boundary was.
        let mut events = vec![
            key_down(0, 0, "a", mods()),
            key_down(100, KEY_TAB, "\t", mods()),
            key_down(400, 0, "b", mods()),
            key_down(1500, 0, "c", mods()),
            key_down(2600, 0, "d", mods()),
        ];
        assert_eq!(
            retro_scrub(
                &mut events,
                Some(Timestamp(100 * MS)),
                Timestamp(2700 * MS),
                300,
                1500
            ),
            3
        );
        // A gap over idle_ms between keys ends the run.
        let mut events = vec![
            key_down(100, KEY_TAB, "\t", mods()),
            key_down(200, 0, "b", mods()),
            key_down(5000, 0, "c", mods()),
        ];
        assert_eq!(
            retro_scrub(
                &mut events,
                Some(Timestamp(100 * MS)),
                Timestamp(5100 * MS),
                300,
                1500
            ),
            1
        );
        assert_eq!(typed(&events)[1], (200, Some("b".into()), false));
    }

    #[test]
    fn moves_focus_keys() {
        for code in [KEY_TAB, KEY_RETURN, KEY_KEYPAD_ENTER, KEY_ESCAPE] {
            assert!(moves_focus(code, mods()));
        }
        assert!(moves_focus(0, cmd()));
        let ctrl = Modifiers {
            control: true,
            ..mods()
        };
        assert!(moves_focus(0, ctrl));
        assert!(!moves_focus(0, mods()));
        // ⌥L types "@" on German layouts; ⌥⇧ chords type too.
        let opt = Modifiers {
            option: true,
            ..mods()
        };
        assert!(!moves_focus(37, opt));
        assert!(!moves_focus(37, Modifiers { shift: true, ..opt }));
        assert!(moves_focus(
            37,
            Modifiers {
                command: true,
                ..opt
            }
        ));
    }

    #[test]
    fn option_typing_stays_plain() {
        // "jürgen@firma.de" on a German layout: "@" is ⌥L.
        let mut w = worker(Some((1, field("E-Mail", false, "com.apple.Safari"))));
        let opt = Modifiers {
            option: true,
            ..mods()
        };
        for (i, c) in "jürgen@firma.de".chars().enumerate() {
            let ms = i as u64 * 80;
            let (modifiers, ignoring) = if c == '@' {
                (opt, "l".to_owned())
            } else {
                (mods(), c.to_string())
            };
            w.source.clock = Timestamp((ms + 1) * MS);
            w.handle(
                FocusMsg::Key {
                    event: Box::new(TimedEvent {
                        ts: Timestamp(ms * MS),
                        event: CaptureEvent::KeyDown {
                            key: Key {
                                code: if c == '@' { 37 } else { 0 },
                                characters: Some(c.to_string()),
                                characters_ignoring_modifiers: Some(ignoring),
                            },
                            modifiers,
                            is_repeat: false,
                            secure: false,
                        },
                    }),
                    pid: 5,
                },
                false,
            );
        }
        let out = w.finish();
        assert_eq!(out.settle_masked, 0);
        assert!(typed(&out.events).iter().all(|(_, _, secure)| !secure));
        let steps = detect_steps(&out.events, &StepConfig::default());
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].keystrokes.as_deref(), Some("jürgen@firma.de"));
    }

    #[test]
    fn stalled_app_after_tab_scrubs_the_whole_password() {
        let user = field("Username", false, "com.apple.Safari");
        let pass = field("Password", true, "com.apple.Safari");
        let mut w = worker(Some((1, user)));
        type_key(&mut w, 0, 0, "b");
        type_key(&mut w, 300, KEY_TAB, "\t");
        // The app stalls for 500 ms: these keys are looked up while the
        // username field is still focused, past the 300 ms guard window.
        type_key(&mut w, 400, 0, "x");
        type_key(&mut w, 650, 0, "y");
        type_key(&mut w, 800, 0, "z");
        w.source.current = Some((2, pass));
        type_key(&mut w, 1000, 0, "w");
        let out = w.finish();
        let keys = typed(&out.events);
        assert_eq!(keys[0], (0, Some("b".into()), false));
        for (ms, chars, secure) in &keys[2..] {
            assert!(secure, "key at {ms} ms");
            assert_eq!(chars.as_deref(), Some("•"));
        }
        let json = serde_json::to_string(&out.events).unwrap();
        for c in ["\"x\"", "\"y\"", "\"z\"", "\"w\""] {
            assert!(!json.contains(c), "{c} in {json}");
        }
    }

    #[test]
    fn click_guard_reads_fresh_focus() {
        // Typed in A, then clicked C without typing (last_ref is still A),
        // then clicked password field P and typed before the app moved focus.
        let mut w = worker(Some((1, field("A", false, "com.apple.Safari"))));
        type_key(&mut w, 0, 0, "a");
        w.source.current = Some((3, field("C", false, "com.apple.Safari")));
        w.handle(FocusMsg::Invalidate(Timestamp(2000 * MS)), false);
        w.handle(FocusMsg::Invalidate(Timestamp(4000 * MS)), false);
        // The app has not handled the click on P yet: focus is still C.
        type_key(&mut w, 4050, 0, "p");
        let out = w.finish();
        assert_eq!(out.settle_masked, 1);
        assert_eq!(typed(&out.events)[1], (4050, Some("•".into()), true));
    }

    #[test]
    fn stale_session_dirs() {
        let id = Ulid::from_parts(1_000_000, 7);
        let name = format!("{SESSION_DIR_PREFIX}{id}");
        let hour = 60 * 60 * 1000;
        assert!(!is_stale_session_dir(
            &name,
            1_000_000 + 5 * hour,
            STALE_SESSION_MS
        ));
        assert!(is_stale_session_dir(
            &name,
            1_000_000 + 7 * hour,
            STALE_SESSION_MS
        ));
        assert!(!is_stale_session_dir(
            "Recipes.guidesya",
            u64::MAX,
            STALE_SESSION_MS
        ));
        assert!(!is_stale_session_dir(
            &format!("{SESSION_DIR_PREFIX}nope"),
            u64::MAX,
            STALE_SESSION_MS
        ));
        assert!(default_work_dir().ends_with("dev.guidesya.app/sessions"));
    }

    #[test]
    fn stale_sessions_are_swept() {
        let scratch = ScratchDir::new();
        let old = scratch
            .0
            .join(format!("{SESSION_DIR_PREFIX}{}", Ulid::from_parts(1, 1)));
        let fresh = scratch
            .0
            .join(format!("{SESSION_DIR_PREFIX}{}", Ulid::new()));
        let other = scratch.0.join("keep");
        for dir in [&old, &fresh, &other] {
            fs::create_dir_all(dir).unwrap();
        }
        sweep_stale_sessions(&scratch.0);
        assert!(!old.exists());
        assert!(fresh.exists());
        assert!(other.exists());
    }

    fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    /// Router sources, each a reader and the global rect it shows.
    fn sources(list: Vec<(FrameReader, Rect)>) -> SharedSources {
        Arc::new(Mutex::new(
            list.into_iter()
                .map(|(reader, rect)| FrameSource { rect, reader })
                .collect(),
        ))
    }

    /// One 4×4-point source at the origin.
    fn single_source(reader: FrameReader) -> SharedSources {
        sources(vec![(reader, rect(0.0, 0.0, 4.0, 4.0))])
    }

    #[test]
    fn full_encoder_queue_drops_snapshots() {
        let reader = FrameReader::detached(4);
        reader.push(frame(5 * MS, 4, 4, geometry(4.0, 4.0, 4, 4)));
        let (element, _er) = mpsc::channel();
        let (focus, _fr) = mpsc::channel();
        let (encode, encode_rx) = mpsc::sync_channel(1);
        let mut router = Router::new(
            &SessionConfig::default(),
            single_source(reader.clone()),
            Outputs {
                element,
                focus,
                encode,
                backlog: Arc::new(AtomicUsize::new(0)),
            },
            SharedPause::default(),
            no_exclusions(),
        );
        router.snapshot(Timestamp(10 * MS), None);
        // A new frame, but the queue (size 1) is still full.
        reader.push(frame(15 * MS, 4, 4, geometry(4.0, 4.0, 4, 4)));
        router.snapshot(Timestamp(20 * MS), None);
        let out = router.finish();
        assert_eq!(out.dropped_queue_full, 1);
        assert_eq!(
            out.snapshots.keys().copied().collect::<Vec<_>>(),
            [Timestamp(10 * MS)]
        );
        assert_eq!(out.latency_ns.len(), 1);
        assert_eq!(encode_rx.try_iter().count(), 1);
    }

    /// Nothing excluded, no app activation noted.
    fn no_exclusions() -> Arc<Frontmost> {
        Arc::new(Frontmost::new(&SessionConfig::default()))
    }

    fn targeted(event: TimedEvent) -> TargetedEvent {
        TargetedEvent {
            target_pid: 5,
            event,
        }
    }

    #[test]
    fn picks_the_source_under_the_point() {
        let a = FrameReader::detached(2);
        let b = FrameReader::detached(2);
        let list = vec![
            (a.clone(), rect(0.0, 0.0, 4.0, 4.0)),
            (b.clone(), rect(4.0, 0.0, 4.0, 4.0)),
        ];
        let sources = sources(list);
        let sources = lock(&sources);
        assert_eq!(
            pick_source(&sources, Some(Point { x: 1.0, y: 1.0 })),
            Some(0)
        );
        assert_eq!(
            pick_source(&sources, Some(Point { x: 6.0, y: 1.0 })),
            Some(1)
        );
        // Edges: the right edge is exclusive.
        assert_eq!(
            pick_source(&sources, Some(Point { x: 4.0, y: 0.0 })),
            Some(1)
        );
        // On no source, or unknown: the first one.
        assert_eq!(
            pick_source(&sources, Some(Point { x: 9.0, y: 9.0 })),
            Some(0)
        );
        assert_eq!(pick_source(&sources, None), Some(0));
        // A single source is used whatever the point.
        assert_eq!(
            pick_source(&sources[1..], Some(Point { x: 1.0, y: 1.0 })),
            Some(0)
        );
        assert_eq!(pick_source(&[], Some(Point { x: 1.0, y: 1.0 })), None);
    }

    /// With a stream per display, a click snapshots the display it is on,
    /// and a key the display the pointer was last seen on.
    #[test]
    fn snapshots_come_from_the_display_under_the_event() {
        let left = FrameReader::detached(4);
        let right = FrameReader::detached(4);
        let left_geometry = geometry(4.0, 4.0, 4, 4);
        let right_geometry = FrameGeometry::from_rect(rect(4.0, 0.0, 4.0, 4.0), 4, 4);
        left.push(frame(5 * MS, 4, 4, left_geometry));
        right.push(frame(6 * MS, 4, 4, right_geometry));
        let (element, _er) = mpsc::channel();
        let (focus, _fr) = mpsc::channel();
        let (encode, encode_rx) = mpsc::sync_channel(ENCODER_QUEUE);
        let mut router = Router::new(
            &SessionConfig::default(),
            sources(vec![
                (left, rect(0.0, 0.0, 4.0, 4.0)),
                (right, rect(4.0, 0.0, 4.0, 4.0)),
            ]),
            Outputs {
                element,
                focus,
                encode,
                backlog: Arc::new(AtomicUsize::new(0)),
            },
            SharedPause::default(),
            no_exclusions(),
        );
        // A click on the right display.
        let [down, up] = click(10, 6.0, 1.0);
        router.route(targeted(down));
        router.route(targeted(up));
        // A key (no point): the pointer is still on the right display.
        router.route(targeted(key_down(20, 0, "a", mods())));
        // A click on the left display.
        let [down, up] = click(30, 1.0, 1.0);
        router.route(targeted(down));
        router.route(targeted(up));
        let out = router.finish();
        let by_event: Vec<(Timestamp, usize, Timestamp, f64)> = out
            .snapshots
            .iter()
            .map(|(ts, m)| (*ts, m.frame.source, m.frame.ts, m.geometry.origin.x))
            .collect();
        assert_eq!(
            by_event,
            [
                (Timestamp(10 * MS), 1, Timestamp(6 * MS), 4.0),
                (Timestamp(20 * MS), 1, Timestamp(6 * MS), 4.0),
                (Timestamp(30 * MS), 0, Timestamp(5 * MS), 0.0),
            ]
        );
        // Each frame went to the encoder once, tagged with its source, the
        // event that took it and that event's point in the frame's pixels
        // (none for the key).
        let encoded: Vec<(usize, Timestamp, Timestamp, Option<Point>)> = encode_rx
            .try_iter()
            .map(|j| (j.source, j.frame.ts, j.ts, j.point))
            .collect();
        assert_eq!(
            encoded,
            [
                (
                    1,
                    Timestamp(6 * MS),
                    Timestamp(10 * MS),
                    Some(Point { x: 2.0, y: 1.0 })
                ),
                (
                    0,
                    Timestamp(5 * MS),
                    Timestamp(30 * MS),
                    Some(Point { x: 1.0, y: 1.0 })
                ),
            ]
        );
        assert_eq!(out.missing_frames, 0);
    }

    #[test]
    fn paused_router_drops_events_and_takes_no_snapshots() {
        let reader = FrameReader::detached(4);
        reader.push(frame(5 * MS, 4, 4, geometry(4.0, 4.0, 4, 4)));
        let pause = SharedPause::default();
        lock(&pause).paused_at = Some(Timestamp(50 * MS));
        let (element, element_rx) = mpsc::channel();
        let (focus, focus_rx) = mpsc::channel();
        let (encode, encode_rx) = mpsc::sync_channel(ENCODER_QUEUE);
        let mut router = Router::new(
            &SessionConfig::default(),
            single_source(reader.clone()),
            Outputs {
                element,
                focus,
                encode,
                backlog: Arc::new(AtomicUsize::new(0)),
            },
            Arc::clone(&pause),
            no_exclusions(),
        );
        // From after the pause began: dropped, and no AX work asked for.
        router.route(targeted(key_down(60, 0, "a", mods())));
        router.route(targeted(mouse_down(70)));
        assert!(element_rx.try_recv().is_err());
        assert!(focus_rx.try_recv().is_err());
        assert!(encode_rx.try_recv().is_err());
        // From before the pause (queued before the tap stopped): kept.
        router.route(targeted(key_down(40, 0, "a", mods())));
        assert!(matches!(
            focus_rx.try_recv(),
            Ok(FocusMsg::Key { pid: 5, .. })
        ));
        assert_eq!(
            encode_rx.try_recv().map(|j| j.frame.ts),
            Ok(Timestamp(5 * MS))
        );
        // Resumed: the first event after the resume starts a new run even
        // though the last key was less than `typing_idle_ms` ago, and the
        // focus thread is told first.
        *lock(&pause) = PauseState {
            paused_at: None,
            resumed_at: Some(Timestamp(100 * MS)),
        };
        reader.push(frame(90 * MS, 4, 4, geometry(4.0, 4.0, 4, 4)));
        router.route(targeted(key_down(110, 11, "b", mods())));
        assert!(matches!(
            focus_rx.try_recv(),
            Ok(FocusMsg::Resume(at)) if at == Timestamp(100 * MS)
        ));
        assert!(matches!(focus_rx.try_recv(), Ok(FocusMsg::Key { .. })));
        // The resume is applied once.
        router.route(targeted(key_down(120, 11, "c", mods())));
        assert!(matches!(focus_rx.try_recv(), Ok(FocusMsg::Key { .. })));
        assert!(focus_rx.try_recv().is_err());
        let out = router.finish();
        assert_eq!(out.dropped_paused, 2);
        assert_eq!(
            out.snapshots.keys().copied().collect::<Vec<_>>(),
            [Timestamp(40 * MS), Timestamp(110 * MS)]
        );
        // `AppActivated` again after the resume, for the detector's new
        // stretch.
        let apps: Vec<u64> = out
            .events
            .iter()
            .filter(|ev| matches!(ev.event, CaptureEvent::AppActivated { .. }))
            .map(|ev| ev.ts.0 / MS)
            .collect();
        assert_eq!(apps, [40, 110]);
    }

    #[test]
    fn resume_drops_focus_state_and_bounds_the_scrub() {
        let mut w = worker(Some((1, field("Search", false, "com.apple.Safari"))));
        type_key(&mut w, 0, 0, "a");
        let full = w.ax.lookups;
        type_key(&mut w, 100, 11, "b");
        assert_eq!(w.ax.lookups, full + 1, "cache hit");
        // A click just before the pause armed the settle guard (one read).
        w.handle(FocusMsg::Invalidate(Timestamp(150 * MS)), false);
        w.handle(FocusMsg::Resume(Timestamp(200 * MS)), false);
        // After the resume: a full lookup (three reads), the focus reported
        // again, and the guard no longer holds.
        type_key(&mut w, 300, 8, "c");
        assert_eq!(w.ax.lookups, full + 5);
        let focus_changes: Vec<u64> = w
            .events
            .iter()
            .filter(|ev| matches!(ev.event, CaptureEvent::FocusChanged { .. }))
            .map(|ev| ev.ts.0 / MS)
            .collect();
        assert_eq!(focus_changes, [0, 300]);
        // A secure field found next scrubs back only to the resume: "b"
        // (within `focus_settle_ms`, but before the pause) stays plain.
        w.source.current = Some((2, field("Password", true, "com.apple.Safari")));
        type_key(&mut w, 350, 0, "p");
        assert_eq!(
            typed(&w.events),
            vec![
                (0, Some("a".to_owned()), false),
                (100, Some("b".to_owned()), false),
                (300, Some("•".to_owned()), true),
                (350, Some("•".to_owned()), true),
            ]
        );
    }

    #[test]
    fn key_run_reset_starts_a_new_run() {
        let mut t = KeyRunTracker::new(1500);
        assert!(t.should_snapshot(&key_down(0, 0, "a", mods()), 7));
        assert!(!t.should_snapshot(&key_down(100, 11, "b", mods()), 7));
        t.reset();
        assert!(t.should_snapshot(&key_down(200, 8, "c", mods()), 7));
    }

    #[test]
    fn steps_are_detected_per_stretch_between_pauses() {
        let cfg = StepConfig::default();
        let mut events = vec![
            key_down(0, 0, "a", mods()),
            key_down(100, 11, "b", mods()),
            key_down(170, 7, "x", mods()),
            key_down(300, 8, "c", mods()),
            key_down(400, 2, "d", mods()),
        ];
        let pause = PauseSpan {
            paused_at: Timestamp(150 * MS),
            resumed_at: Some(Timestamp(200 * MS)),
        };
        assert!(pause.contains(Timestamp(150 * MS)));
        assert!(!pause.contains(Timestamp(200 * MS)));
        assert_eq!(
            pause.duration(Timestamp(1000 * MS)),
            Duration::from_millis(50)
        );
        assert_eq!(remove_paused(&mut events, &[pause]), 1);
        assert_eq!(events.len(), 4);
        // Without the pause the four keys are one typing step.
        assert_eq!(detect_steps(&events, &cfg).len(), 1);
        let typed = |drafts: &[DraftStep]| -> Vec<(u64, String)> {
            drafts
                .iter()
                .map(|d| (d.at.0 / MS, d.keystrokes.clone().unwrap_or_default()))
                .collect()
        };
        let drafts = detect_steps_between_pauses(&events, &[pause], &cfg);
        assert_eq!(
            typed(&drafts),
            [(0, "ab".to_owned()), (300, "cd".to_owned())]
        );
        // A pause still open at stop removes everything after it.
        let open = PauseSpan {
            paused_at: Timestamp(350 * MS),
            resumed_at: None,
        };
        assert!(open.contains(Timestamp(u64::MAX)));
        assert_eq!(
            open.duration(Timestamp(1000 * MS)),
            Duration::from_millis(650)
        );
        assert_eq!(remove_paused(&mut events, &[open]), 1);
        let drafts = detect_steps_between_pauses(&events, &[pause, open], &cfg);
        assert_eq!(
            typed(&drafts),
            [(0, "ab".to_owned()), (300, "c".to_owned())]
        );
    }

    /// A session with no tap and no frame stream: events are fed through
    /// the returned sender (as the tap would), frames through `reader`.
    fn headless_session(
        scratch: &ScratchDir,
        reader: FrameReader,
    ) -> (RecordingSession, Sender<TargetedEvent>) {
        headless_session_with(scratch, reader, SessionConfig::default())
    }

    fn headless_session_with(
        scratch: &ScratchDir,
        reader: FrameReader,
        cfg: SessionConfig,
    ) -> (RecordingSession, Sender<TargetedEvent>) {
        let session_dir = scratch.0.join(format!("{SESSION_DIR_PREFIX}headless"));
        fs::create_dir_all(&session_dir).unwrap();
        let mut session = RecordingSession::new(&cfg);
        session.headless = true;
        session.session_dir = Some(session_dir);
        let source = FrameSource {
            rect: Rect {
                x: 0.0,
                y: 0.0,
                width: 4.0,
                height: 4.0,
            },
            reader,
        };
        let sources: SharedSources = Arc::new(Mutex::new(vec![source]));
        let focus = field("Search", false, "com.apple.Safari");
        session
            .spawn_threads(&cfg, sources, move || FakeFocus {
                current: Some((1, focus)),
                clock: Timestamp(0),
            })
            .unwrap();
        let tx = session.tap_tx.clone().unwrap();
        (session, tx)
    }

    fn key_at(ts: Timestamp, chars: &str) -> TargetedEvent {
        targeted(TimedEvent {
            ts,
            event: CaptureEvent::KeyDown {
                key: key(0, Some(chars)),
                modifiers: mods(),
                is_repeat: false,
                secure: false,
            },
        })
    }

    fn typed_text(rec: &Recording) -> String {
        typed(&rec.events)
            .into_iter()
            .filter_map(|(_, chars, _)| chars)
            .collect()
    }

    fn count(rec: &Recording, f: impl Fn(&CaptureEvent) -> bool) -> usize {
        rec.events.iter().filter(|ev| f(&ev.event)).count()
    }

    #[test]
    fn pause_halts_input_and_resume_starts_a_fresh_typing_run() {
        let scratch = ScratchDir::new();
        let reader = FrameReader::detached(4);
        let (mut session, tx) = headless_session(&scratch, reader.clone());
        assert!(!session.is_paused());
        // Not paused: a no-op.
        session.resume().unwrap();
        let t0 = event_tap::now();
        reader.push(frame(t0.0, 4, 4, geometry(4.0, 4.0, 4, 4)));
        let a = Timestamp(t0.0 + 1);
        tx.send(key_at(a, "a")).unwrap();
        tx.send(key_at(Timestamp(t0.0 + 2), "b")).unwrap();
        session.pause().unwrap();
        // Twice: a no-op.
        session.pause().unwrap();
        assert!(session.is_paused());
        // Typed while paused (as if the tap were still running): dropped.
        tx.send(key_at(event_tap::now(), "x")).unwrap();
        session.resume().unwrap();
        session.resume().unwrap();
        assert!(!session.is_paused());
        // The restarted stream's first frame.
        let t1 = event_tap::now();
        reader.push(frame(t1.0, 4, 4, geometry(4.0, 4.0, 4, 4)));
        let c = Timestamp(t1.0 + 1);
        tx.send(key_at(c, "c")).unwrap();
        tx.send(key_at(Timestamp(t1.0 + 2), "d")).unwrap();
        // As the tap does when it stops: let the router see the end.
        drop(tx);
        let rec = session.stop().unwrap();
        assert_eq!(rec.pauses.len(), 1);
        assert!(rec.pauses[0].resumed_at.is_some());
        assert!(rec.metrics.paused > Duration::ZERO);
        assert_eq!(typed_text(&rec), "abcd");
        assert_eq!(rec.metrics.dropped_paused, 1);
        // Two typing steps, though the keys are microseconds apart.
        let steps: Vec<(StepKind, Timestamp, String)> = rec
            .drafts
            .iter()
            .map(|d| (d.kind, d.at, d.keystrokes.clone().unwrap_or_default()))
            .collect();
        assert_eq!(
            steps,
            [
                (StepKind::Type, a, "ab".to_owned()),
                (StepKind::Type, c, "cd".to_owned()),
            ]
        );
        // A snapshot for the first key of each run, from the frame before.
        assert_eq!(rec.snapshots.keys().copied().collect::<Vec<_>>(), [a, c]);
        assert_eq!(rec.snapshots[&c].frame_ts, t1);
        assert_eq!(rec.metrics.snapshots, 2);
        // The app and the focus are reported again after the resume.
        assert_eq!(
            count(&rec, |ev| matches!(ev, CaptureEvent::AppActivated { .. })),
            2
        );
        assert_eq!(
            count(&rec, |ev| matches!(ev, CaptureEvent::FocusChanged { .. })),
            2
        );
        // Strictly ordered.
        assert!(rec.events.windows(2).all(|w| w[0].ts <= w[1].ts));
    }

    #[test]
    fn reports_each_encoded_frame_live() {
        let scratch = ScratchDir::new();
        let reader = FrameReader::detached(4);
        let seen: Arc<Mutex<Vec<LiveSnapshot>>> = Arc::default();
        let sink = Arc::clone(&seen);
        let cfg = SessionConfig {
            on_snapshot: Some(SnapshotSink::new(move |snap| {
                lock(&sink).push(snap.clone())
            })),
            ..SessionConfig::default()
        };
        let (session, tx) = headless_session_with(&scratch, reader.clone(), cfg);
        assert!(session.session_dir().unwrap().starts_with(&scratch.0));
        let t0 = event_tap::now();
        reader.push(frame(t0.0, 4, 4, geometry(4.0, 4.0, 4, 4)));
        // Two keys of one typing run: one snapshot, one frame, one report.
        tx.send(key_at(Timestamp(t0.0 + 1), "a")).unwrap();
        tx.send(key_at(Timestamp(t0.0 + 2), "b")).unwrap();
        // Later, a second frame and a shortcut: a second snapshot.
        let t1 = Timestamp(t0.0 + 10 * MS);
        reader.push(frame(t1.0, 4, 4, geometry(4.0, 4.0, 4, 4)));
        let shortcut = TimedEvent {
            ts: Timestamp(t1.0 + 1),
            event: CaptureEvent::KeyDown {
                key: key(KEY_S, Some("s")),
                modifiers: Modifiers {
                    command: true,
                    ..mods()
                },
                is_repeat: false,
                secure: false,
            },
        };
        tx.send(targeted(shortcut)).unwrap();
        drop(tx);
        let rec = session.stop().unwrap();
        assert_eq!(rec.snapshots.len(), 2);

        let seen = lock(&seen);
        assert_eq!(seen.len(), 2, "{seen:?}");
        let first = &seen[0];
        assert_eq!(first.ts, Timestamp(t0.0 + 1));
        assert_eq!(first.frame, FrameKey { source: 0, ts: t0 });
        assert_eq!((first.width, first.height), (4, 4));
        assert_eq!(first.point, None);
        assert_eq!(
            first.path,
            rec.session_dir()
                .join(format!("{}.webp", first.frame.file_stem()))
        );
        assert!(first.path.exists());
        assert_eq!(seen[1].ts, Timestamp(t1.0 + 1));
        assert_eq!(seen[1].frame.ts, t1);
    }

    #[test]
    fn stop_while_paused_produces_a_recording() {
        let scratch = ScratchDir::new();
        let reader = FrameReader::detached(4);
        let (mut session, tx) = headless_session(&scratch, reader.clone());
        let t0 = event_tap::now();
        reader.push(frame(t0.0, 4, 4, geometry(4.0, 4.0, 4, 4)));
        tx.send(key_at(Timestamp(t0.0 + 1), "a")).unwrap();
        session.pause().unwrap();
        tx.send(key_at(event_tap::now(), "x")).unwrap();
        drop(tx);
        let rec = session.stop().unwrap();
        assert_eq!(rec.pauses.len(), 1);
        assert_eq!(rec.pauses[0].resumed_at, None);
        assert!(rec.metrics.paused > Duration::ZERO);
        assert_eq!(typed_text(&rec), "a");
        assert_eq!(rec.metrics.dropped_paused, 1);
        assert_eq!(rec.drafts.len(), 1);
        assert_eq!(rec.snapshots.len(), 1);
        assert!(rec.session_dir().starts_with(&scratch.0));
    }

    const ONE_PASSWORD: &str = "com.1password.1password";

    #[test]
    fn default_exclusions_cover_password_managers_and_messages() {
        let ids = default_excluded_bundle_ids();
        for id in [
            ONE_PASSWORD,
            "com.agilebits.onepassword7",
            "com.bitwarden.desktop",
            "com.apple.keychainaccess",
            "com.apple.MobileSMS",
        ] {
            assert!(ids.iter().any(|e| e == id), "{id}");
        }
        // The session itself excludes nothing unless told.
        assert!(SessionConfig::default().excluded_bundle_ids.is_empty());
    }

    #[test]
    fn frontmost_log_tracks_excluded_spans() {
        let excluded = default_excluded_bundle_ids();
        let mut log = FrontmostLog::default();
        let at = |ms: u64| Timestamp(ms * MS);
        assert_eq!(
            log.activated(at(0), Some("com.apple.Safari"), &excluded),
            None
        );
        assert_eq!(log.activated(at(5), None, &excluded), None);
        assert_eq!(
            log.activated(at(10), Some(ONE_PASSWORD), &excluded),
            Some(Some(ONE_PASSWORD.to_owned()))
        );
        // Again (a resume's initial read): no change.
        assert_eq!(log.activated(at(15), Some(ONE_PASSWORD), &excluded), None);
        assert_eq!(log.current(), Some(ONE_PASSWORD));
        // Straight to another excluded app: a new span.
        assert_eq!(
            log.activated(at(20), Some("com.apple.MobileSMS"), &excluded),
            Some(Some("com.apple.MobileSMS".to_owned()))
        );
        assert_eq!(
            log.activated(at(30), Some("com.apple.Safari"), &excluded),
            Some(None)
        );
        assert_eq!(log.current(), None);
        assert_eq!(log.spans().len(), 2);
        assert_eq!(log.spans()[0].end, Some(at(20)));
        assert!(!log.excluded_at(at(9)));
        assert!(log.excluded_at(at(10)));
        assert!(log.excluded_at(at(29)));
        assert!(!log.excluded_at(at(30)));
        // Frames: a margin on both sides. Just before the activation was
        // noted, and just after the app left, the screen may show it.
        let mut log = FrontmostLog::default();
        let (enter, leave) = (1000, 2000);
        log.activated(at(enter), Some(ONE_PASSWORD), &excluded);
        log.activated(at(leave), Some("com.apple.Safari"), &excluded);
        assert!(!log.frame_excluded(at(enter - EXCLUDED_FRAME_HEAD_MS - 1)));
        assert!(log.frame_excluded(at(enter - EXCLUDED_FRAME_HEAD_MS)));
        assert!(!log.excluded_at(at(enter - 1)), "events get no head margin");
        assert!(log.frame_excluded(at(leave)));
        assert!(log.frame_excluded(at(leave + EXCLUDED_FRAME_TAIL_MS - 1)));
        assert!(!log.frame_excluded(at(leave + EXCLUDED_FRAME_TAIL_MS)));
        assert!(!log.excluded_at(at(leave)), "events get no tail margin");
        // An open span excludes everything after its start.
        log.activated(at(1000), Some("com.apple.keychainaccess"), &excluded);
        assert!(log.excluded_at(Timestamp(u64::MAX)));
        // Not in the list: never excluded.
        let mut log = FrontmostLog::default();
        assert_eq!(log.activated(at(0), Some(ONE_PASSWORD), &[]), None);
        assert!(!log.excluded_at(at(1)));
    }

    fn event_at(ts: Timestamp, event: CaptureEvent) -> TargetedEvent {
        targeted(TimedEvent { ts, event })
    }

    fn shortcut_at(ts: Timestamp) -> TargetedEvent {
        event_at(
            ts,
            CaptureEvent::KeyDown {
                key: key(KEY_S, Some("s")),
                modifiers: cmd(),
                is_repeat: false,
                secure: false,
            },
        )
    }

    fn webp_files(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".webp"))
            .collect();
        names.sort();
        names
    }

    type Seen<T> = Arc<Mutex<Vec<T>>>;

    /// A config excluding the default list, recording live snapshots and
    /// exclusion changes.
    fn excluding_cfg() -> (SessionConfig, Seen<LiveSnapshot>, Seen<Option<String>>) {
        let snaps: Seen<LiveSnapshot> = Arc::default();
        let changes: Seen<Option<String>> = Arc::default();
        let (s, c) = (Arc::clone(&snaps), Arc::clone(&changes));
        let cfg = SessionConfig {
            excluded_bundle_ids: default_excluded_bundle_ids(),
            on_snapshot: Some(SnapshotSink::new(move |snap| lock(&s).push(snap.clone()))),
            on_excluded: Some(ExclusionSink::new(move |id| {
                lock(&c).push(id.map(str::to_owned))
            })),
            ..SessionConfig::default()
        };
        (cfg, snaps, changes)
    }

    #[test]
    fn excluded_frontmost_app_captures_nothing_live() {
        let scratch = ScratchDir::new();
        let reader = FrameReader::detached(8);
        let (cfg, snaps, changes) = excluding_cfg();
        let (session, tx) = headless_session_with(&scratch, reader.clone(), cfg);
        let push = |ts: Timestamp| reader.push(frame(ts.0, 4, 4, geometry(4.0, 4.0, 4, 4)));
        let t0 = event_tap::now();
        push(t0);
        let a = Timestamp(t0.0 + 1);
        tx.send(key_at(a, "a")).unwrap();

        // 1Password comes to the front (later than the head margin, so the
        // frame for "a" stays).
        let enter = Timestamp(t0.0 + (EXCLUDED_FRAME_HEAD_MS + 50) * MS);
        session.front.activated(enter, Some(ONE_PASSWORD));
        assert!(session.is_excluded());
        assert_eq!(session.excluded_app().as_deref(), Some(ONE_PASSWORD));
        push(Timestamp(enter.0 + MS));
        // Whatever these are aimed at (pid 5 is not 1Password): a key, a
        // click and the manual-step hotkey are all dropped.
        tx.send(key_at(Timestamp(enter.0 + 2 * MS), "x")).unwrap();
        tx.send(event_at(Timestamp(enter.0 + 3 * MS), mouse("down")))
            .unwrap();
        tx.send(event_at(Timestamp(enter.0 + 4 * MS), mouse("up")))
            .unwrap();
        tx.send(event_at(
            Timestamp(enter.0 + 5 * MS),
            CaptureEvent::KeyDown {
                key: key(KEY_S, Some("ß")),
                modifiers: Hotkey::MANUAL_STEP.modifiers,
                is_repeat: false,
                secure: false,
            },
        ))
        .unwrap();

        // Back to Safari.
        let leave = Timestamp(enter.0 + 100 * MS);
        session.front.activated(leave, Some("com.apple.Safari"));
        assert!(!session.is_excluded());
        // Within the tail the screen may still show 1Password: the shortcut
        // is kept but takes no snapshot.
        push(Timestamp(leave.0 + MS));
        let s1 = Timestamp(leave.0 + 2 * MS);
        tx.send(shortcut_at(s1)).unwrap();
        // After the tail: snapshotted again.
        let later = Timestamp(leave.0 + (EXCLUDED_FRAME_TAIL_MS + 10) * MS);
        push(later);
        let s2 = Timestamp(later.0 + 1);
        tx.send(shortcut_at(s2)).unwrap();
        drop(tx);
        let rec = session.stop().unwrap();

        assert_eq!(
            *lock(&changes),
            [Some(ONE_PASSWORD.to_owned()), None],
            "entered and left"
        );
        // No "x". The shortcuts are masked: the dropped click still armed
        // the settle guard, and the scripted focus never moves.
        assert_eq!(typed_text(&rec), "a••");
        assert_eq!(
            count(&rec, |ev| matches!(
                ev,
                CaptureEvent::MouseDown { .. }
                    | CaptureEvent::MouseUp { .. }
                    | CaptureEvent::ManualStep
            )),
            0
        );
        assert_eq!(rec.metrics.dropped_excluded, 4);
        assert_eq!(rec.metrics.excluded_frames, 1);
        assert!(rec.drafts.iter().all(|d| {
            d.kind != StepKind::Click
                && d.kind != StepKind::Manual
                && !d.keystrokes.as_deref().unwrap_or_default().contains('x')
        }));
        // Only the frames from before and well after were snapshotted,
        // written and reported.
        assert_eq!(rec.snapshots.keys().copied().collect::<Vec<_>>(), [a, s2]);
        let reported: Vec<Timestamp> = lock(&snaps).iter().map(|s| s.frame.ts).collect();
        assert_eq!(reported, [t0, later]);
        assert_eq!(
            webp_files(rec.session_dir()),
            [
                format!("{}.webp", FrameKey { source: 0, ts: t0 }.file_stem()),
                format!(
                    "{}.webp",
                    FrameKey {
                        source: 0,
                        ts: later
                    }
                    .file_stem()
                ),
            ]
        );
    }

    #[test]
    fn late_activation_notice_still_removes_what_slipped_through() {
        let scratch = ScratchDir::new();
        let reader = FrameReader::detached(8);
        let (cfg, snaps, _) = excluding_cfg();
        let (session, tx) = headless_session_with(&scratch, reader.clone(), cfg);
        let push = |ts: Timestamp| reader.push(frame(ts.0, 4, 4, geometry(4.0, 4.0, 4, 4)));
        let t0 = event_tap::now();
        push(t0);
        let a = Timestamp(t0.0 + 1);
        tx.send(key_at(a, "a")).unwrap();
        // 1Password has come to the front and drawn (frame t1), and a
        // shortcut is pressed, but its activation is not noted yet.
        let t1 = Timestamp(t0.0 + (EXCLUDED_FRAME_HEAD_MS + 150) * MS);
        push(t1);
        let y = Timestamp(t1.0 + 1);
        tx.send(shortcut_at(y)).unwrap();
        // The router handles the shortcut first: its frame is written and
        // even reported live (the flash the docs admit to).
        let deadline = Instant::now() + Duration::from_secs(5);
        while lock(&snaps).len() < 2 {
            assert!(Instant::now() < deadline, "the snapshot was never reported");
            thread::sleep(Duration::from_millis(1));
        }
        // Then the notice: noted after the frame and the event, within the
        // head margin, as the observer would.
        let noted = Timestamp(t1.0 + 5 * MS);
        session.front.activated(noted, Some(ONE_PASSWORD));
        // From now on, dropped live.
        tx.send(key_at(Timestamp(noted.0 + MS), "z")).unwrap();
        drop(tx);
        let rec = session.stop().unwrap();
        // The shortcut came before the notice and stays (events get no head
        // margin), but its frame may show 1Password: its snapshot and file
        // are gone.
        assert_eq!(typed_text(&rec), "as");
        assert_eq!(rec.metrics.dropped_excluded, 1);
        assert_eq!(rec.metrics.excluded_frames, 1);
        assert_eq!(rec.snapshots.keys().copied().collect::<Vec<_>>(), [a]);
        assert_eq!(
            webp_files(rec.session_dir()),
            [format!(
                "{}.webp",
                FrameKey { source: 0, ts: t0 }.file_stem()
            )]
        );
    }

    #[test]
    fn secure_key_is_masked_without_any_exclusions() {
        let password = field("Password", true, "com.apple.Safari");
        assert_eq!(
            key_verdict(&Ok(Some(password.clone())), &[]),
            KeyVerdict::Secure
        );
        let mut w = FocusWorker::new(
            &SessionConfig::default(),
            FakeFocus {
                current: Some((1, password)),
                clock: Timestamp(0),
            },
        );
        type_key(&mut w, 0, 0, "p");
        type_key(&mut w, 50, 1, "w");
        assert_eq!(
            typed(&w.events),
            vec![
                (0, Some("•".to_owned()), true),
                (50, Some("•".to_owned()), true),
            ]
        );
    }

    #[test]
    #[ignore = "needs Screen Recording, Input Monitoring and Accessibility (TCC)"]
    fn live_session_starts_and_stops() {
        let scratch = ScratchDir::new();
        let cfg = SessionConfig {
            work_dir: Some(scratch.0.clone()),
            ..SessionConfig::default()
        };
        let session = RecordingSession::start(cfg).expect("start");
        thread::sleep(Duration::from_millis(500));
        let recording = session.stop().expect("stop");
        assert!(recording.metrics.wall >= Duration::from_millis(500));
        assert!(recording.session_dir().starts_with(&scratch.0));
    }
}
