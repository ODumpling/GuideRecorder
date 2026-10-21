//! Session types every capture backend shares: the config a recording
//! starts with ([`SessionConfig`]), the hotkeys it handles ([`Hotkey`]),
//! the live feed it reports ([`LiveSnapshot`], [`LivePress`] and their
//! sinks) and the edits made to that feed before stop ([`CaptureEdits`]).
//!
//! The session itself is the backend's: [`backend::session`] has
//! `RecordingSession`, `Recording` and `SessionError`. Its docs say which
//! thread calls each sink and what the sections named here (`# After
//! frames`, `# Excluded apps`, ...) cover.
//!
//! [`backend::session`]: crate::backend::session

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use guiderecorder_core::events::{Key, Modifiers, Timestamp};
use guiderecorder_core::model::Point;
use guiderecorder_core::steps::{LiveAction, StepConfig};

use crate::frames::FrameStreamConfig;
use crate::keys;

#[cfg(doc)]
use crate::backend::session::{Recording, RecordingSession};
#[cfg(doc)]
use guiderecorder_core::{events::RawElement, model::StepImage};

/// Frames kept in the ring during a session. More than the stream default,
/// so a snapshot still finds the pre-click frame if the router falls behind.
pub const SESSION_RING_CAPACITY: usize = 6;
/// Default click crop in frame pixels (clamped to the frame).
pub const DEFAULT_CROP_SIZE: (u32, u32) = (800, 500);

/// How long after a step's action (its last key or release) its "after"
/// frame is taken by default ([`AfterFrames::settle_ms`]): long enough
/// for most UI to respond and redraw. See `# After frames`.
pub const AFTER_SETTLE_MS: u64 = 500;

/// A key plus exact modifiers (shift, control, option, command; function
/// and caps lock are ignored).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hotkey {
    pub code: u16,
    pub modifiers: Modifiers,
}

impl Hotkey {
    /// ⌃⌥S (Ctrl+Alt+S), the default manual-step hotkey.
    pub const MANUAL_STEP: Hotkey = Hotkey {
        code: keys::S,
        modifiers: Modifiers {
            shift: false,
            control: true,
            option: true,
            command: false,
            function: false,
            caps_lock: false,
        },
    };

    /// ⌘⌥Z, the default undo-last-capture hotkey; Ctrl+Alt+Z off macOS,
    /// where Windows keeps Win+Alt shortcuts for itself.
    pub const UNDO_CAPTURE: Hotkey = Hotkey {
        code: keys::Z,
        modifiers: Modifiers {
            shift: false,
            control: !cfg!(target_os = "macos"),
            option: true,
            command: cfg!(target_os = "macos"),
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
    /// every active display, one stream each; see the backend's `stream_targets`.
    pub frames: FrameStreamConfig,
    /// Bundle ids whose events are dropped and never snapshotted; while one
    /// of them is the frontmost app nothing is captured at all, and their
    /// windows are left out of every frame (added to
    /// [`FrameStreamConfig::excluded_bundle_ids`]; see `# Excluded apps`).
    /// An id also covers its `<id>.` helpers
    /// ([`bundle_id_matches`]). The backend's `default_excluded_bundle_ids` is
    /// the suggested list; the default here is empty.
    ///
    /// [`bundle_id_matches`]: crate::frames::bundle_id_matches
    pub excluded_bundle_ids: Vec<String>,
    /// Key press turned into a manual step (and not recorded as a key).
    pub manual_hotkey: Option<Hotkey>,
    /// Key presses the host handles itself (e.g. its global start/stop and
    /// pause hotkeys): dropped, repeats included, and never recorded or
    /// snapshotted. Empty by default.
    pub control_hotkeys: Vec<Hotkey>,
    pub steps: StepConfig,
    /// After frames (see `# After frames`); `None` takes none: the after
    /// thread only drains its messages, no `.after.webp` is written, every
    /// [`StepImage::after`] is `None` and the after metrics stay 0. The
    /// default takes them, [`AFTER_SETTLE_MS`] after each step.
    pub after_frames: Option<AfterFrames>,
    /// Click crop in frame pixels.
    pub crop_size: (u32, u32),
    /// Where the hidden session folder with encoded snapshots goes. `None`
    /// is the backend's `default_work_dir`, a local cache that is not synced. Never use
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
    /// Called with a short reason when every frame stream has stopped by
    /// itself (the window closed, the display was unplugged), and with
    /// `None` when a resume starts capturing again; see `# Lost capture`.
    /// Called on ScreenCaptureKit's queue or on the thread calling
    /// [`RecordingSession::resume`]. Keep it quick. `None` reports
    /// nothing; the state can also be polled with
    /// [`RecordingSession::capture_lost`].
    pub on_capture_lost: Option<CaptureLostSink>,
    /// Called on the element thread for each press kept, with its AX
    /// label once looked up (see `# Live feed`). Keep it quick. `None`
    /// reports nothing.
    pub on_press: Option<PressSink>,
    /// Key press that undoes the newest capture while recording: dropped
    /// (repeats included), never recorded or snapshotted, and reported
    /// through [`SessionConfig::on_undo`]. Default [`Hotkey::UNDO_CAPTURE`].
    pub undo_hotkey: Option<Hotkey>,
    /// Called on the router thread for each press of
    /// [`SessionConfig::undo_hotkey`] while recording (not while paused).
    /// Keep it quick.
    pub on_undo: Option<UndoSink>,
}

/// How after frames are taken ([`SessionConfig::after_frames`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AfterFrames {
    /// How long after a step's action (its last key or release) its after
    /// frame is taken. Default [`AFTER_SETTLE_MS`].
    pub settle_ms: u64,
}

impl Default for AfterFrames {
    fn default() -> Self {
        Self {
            settle_ms: AFTER_SETTLE_MS,
        }
    }
}

/// The callback behind [`SessionConfig::on_capture_lost`].
#[derive(Clone)]
pub struct CaptureLostSink(Arc<CaptureLostFn>);

type CaptureLostFn = dyn Fn(Option<&str>) + Send + Sync;

impl CaptureLostSink {
    pub fn new(f: impl Fn(Option<&str>) + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }

    #[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
    pub(crate) fn report(&self, reason: Option<&str>) {
        (self.0)(reason);
    }
}

impl fmt::Debug for CaptureLostSink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CaptureLostSink")
    }
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

    #[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
    pub(crate) fn report(&self, excluded: Option<&str>) {
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
/// and a step may borrow a neighbouring frame (the backend's `match_snapshot`).
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
    /// What the event was; `None` only for an event that never takes a
    /// snapshot.
    pub action: Option<LiveAction>,
    /// The name of the app the event was aimed at, when known.
    pub app: Option<String>,
}

/// The callback behind [`SessionConfig::on_snapshot`].
#[derive(Clone)]
pub struct SnapshotSink(Arc<dyn Fn(&LiveSnapshot) + Send + Sync>);

impl SnapshotSink {
    pub fn new(f: impl Fn(&LiveSnapshot) + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }

    #[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
    pub(crate) fn report(&self, snapshot: &LiveSnapshot) {
        (self.0)(snapshot);
    }
}

impl fmt::Debug for SnapshotSink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SnapshotSink")
    }
}

/// A press the element thread looked up (see [`SessionConfig::on_press`]).
#[derive(Debug, Clone, PartialEq)]
pub struct LivePress {
    /// The `MouseDown`'s ts: the [`LiveSnapshot::ts`] of its snapshot.
    pub ts: Timestamp,
    /// Where it landed, in global display points.
    pub point: Point,
    /// The pressed element's label ([`RawElement::best_label`]), if any.
    pub label: Option<String>,
}

/// The callback behind [`SessionConfig::on_press`].
#[derive(Clone)]
pub struct PressSink(Arc<dyn Fn(&LivePress) + Send + Sync>);

impl PressSink {
    pub fn new(f: impl Fn(&LivePress) + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }

    #[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
    pub(crate) fn report(&self, press: &LivePress) {
        (self.0)(press);
    }
}

impl fmt::Debug for PressSink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PressSink")
    }
}

/// The callback behind [`SessionConfig::on_undo`].
#[derive(Clone)]
pub struct UndoSink(Arc<dyn Fn() + Send + Sync>);

impl UndoSink {
    pub fn new(f: impl Fn() + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }

    #[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
    pub(crate) fn report(&self) {
        (self.0)();
    }
}

impl fmt::Debug for UndoSink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("UndoSink")
    }
}

/// Changes made to the live feed before stop, keyed by the
/// [`LiveSnapshot::ts`] of each capture (the key of
/// `Recording::snapshots`). Applied by [`Recording::save_guide`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CaptureEdits {
    /// Captures deleted: every step whose screenshot is one of them is left
    /// out (not counted as skipped).
    pub deleted: HashSet<Timestamp>,
    /// Titles given (guide Markdown): the first step whose screenshot is
    /// the capture takes it.
    pub titles: HashMap<Timestamp, String>,
}

/// What [`CaptureEdits`] do to a step whose screenshot is the capture
/// `ts`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureEdit<'a> {
    Keep,
    Drop,
    Retitle(&'a str),
}

impl CaptureEdits {
    pub fn is_empty(&self) -> bool {
        self.deleted.is_empty() && self.titles.is_empty()
    }

    /// The edit for a step whose screenshot is the capture `ts`; `titled`
    /// holds the captures whose title a step already took, so only the
    /// first step of each is retitled.
    pub fn edit(&self, ts: Timestamp, titled: &mut HashSet<Timestamp>) -> CaptureEdit<'_> {
        if self.deleted.contains(&ts) {
            return CaptureEdit::Drop;
        }
        match self.titles.get(&ts) {
            Some(title) if titled.insert(ts) => CaptureEdit::Retitle(title),
            _ => CaptureEdit::Keep,
        }
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
            control_hotkeys: Vec::new(),
            steps: StepConfig::default(),
            after_frames: Some(AfterFrames::default()),
            crop_size: DEFAULT_CROP_SIZE,
            work_dir: None,
            on_snapshot: None,
            on_excluded: None,
            on_capture_lost: None,
            on_press: None,
            undo_hotkey: Some(Hotkey::UNDO_CAPTURE),
            on_undo: None,
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_edits_drop_and_retitle_once() {
        let mut edits = CaptureEdits::default();
        let (a, b) = (Timestamp(1), Timestamp(2));
        edits.deleted.insert(a);
        edits.titles.insert(a, "Gone".into());
        edits.titles.insert(b, "Open **Settings**".into());
        let mut titled = HashSet::new();
        assert_eq!(edits.edit(a, &mut titled), CaptureEdit::Drop);
        assert_eq!(
            edits.edit(b, &mut titled),
            CaptureEdit::Retitle("Open **Settings**")
        );
        // A second step on the same capture keeps its own title.
        assert_eq!(edits.edit(b, &mut titled), CaptureEdit::Keep);
        assert_eq!(edits.edit(Timestamp(3), &mut titled), CaptureEdit::Keep);
        assert!(!edits.is_empty());
        assert!(CaptureEdits::default().is_empty());
    }
}
