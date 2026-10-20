//! The live capture feed: what the toolbar's panel shows about each
//! screenshot while recording, and the edits made there before stop.
//!
//! Each screenshot the session announces gets a provisional title from
//! what took it and the app it was aimed at ("Click in Safari"), refined
//! once the element thread has the pressed element's label ("Click
//! **Submit report** in Safari", sent as [`TITLE_EVENT`]). The label often
//! arrives first (encoding the frame takes longer than an AX lookup), so
//! labels wait here for their screenshot ([`PENDING_LABELS`] at most).
//!
//! A capture can be renamed ([`rename_capture`], also sent as
//! [`TITLE_EVENT`]) or deleted ([`delete_capture`], or the newest one with
//! the undo hotkey; sent as [`REMOVED_EVENT`]). Those edits are handed to
//! the recording at stop ([`LiveFeed::edits`]), where a deleted capture's
//! steps are left out and a renamed one's first step takes the title.
//! The steps themselves are still only worked out at stop.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use guiderecorder_capture::session::{CaptureEdits, LivePress, LiveSnapshot};
use guiderecorder_core::events::Timestamp;
use guiderecorder_core::steps::{plain_title, LiveAction};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime, State};

use crate::editor::{AppError, ErrorKind, Result};
use crate::recording::{self, Recorder, RecordingSnapshot};

/// Emitted with a [`TitleChange`] when a capture's title changes.
pub const TITLE_EVENT: &str = "recording://snapshot-title";
/// Emitted with a [`Removed`] when a capture is deleted.
pub const REMOVED_EVENT: &str = "recording://snapshot-removed";
/// Press labels kept while their screenshot is still being encoded.
const PENDING_LABELS: usize = 32;
/// Longest title a rename keeps, in characters.
pub const MAX_TITLE_CHARS: usize = 200;

/// A capture's new title (guide Markdown).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TitleChange {
    pub id: String,
    pub title: String,
}

/// A deleted capture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Removed {
    pub id: String,
}

#[derive(Debug, Clone)]
struct Capture {
    id: String,
    ts: Timestamp,
    action: Option<LiveAction>,
    app: Option<String>,
    label: Option<String>,
    title: String,
    /// Given by the user: a label found later does not replace it.
    renamed: bool,
    deleted: bool,
}

impl Capture {
    fn guessed(&self) -> String {
        match &self.action {
            Some(action) => action.title(self.label.as_deref(), self.app.as_deref()),
            None => "Step".to_owned(),
        }
    }
}

#[derive(Debug, Default)]
struct Feed {
    captures: Vec<Capture>,
    /// Press labels whose screenshot has not been announced yet, oldest
    /// first.
    labels: VecDeque<(Timestamp, Option<String>)>,
}

/// The running recording's feed (part of the [`Recorder`]).
#[derive(Debug, Default)]
pub struct LiveFeed(Mutex<Feed>);

impl LiveFeed {
    fn lock(&self) -> MutexGuard<'_, Feed> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Forgets everything: at start, stop and cancel.
    pub fn reset(&self) {
        *self.lock() = Feed::default();
    }

    /// Notes a screenshot the session announced and returns it as sent,
    /// titled; `elapsed_ms` is the recorded time when it was taken.
    pub fn snapshot(&self, snapshot: &LiveSnapshot, elapsed_ms: u64) -> RecordingSnapshot {
        let mut feed = self.lock();
        let label = feed
            .labels
            .iter()
            .position(|(ts, _)| *ts == snapshot.ts)
            .and_then(|i| feed.labels.remove(i))
            .and_then(|(_, label)| label);
        let mut capture = Capture {
            id: snapshot.frame.file_stem(),
            ts: snapshot.ts,
            action: snapshot.action.clone(),
            app: snapshot.app.clone(),
            label,
            title: String::new(),
            renamed: false,
            deleted: false,
        };
        capture.title = capture.guessed();
        let sent = RecordingSnapshot {
            id: capture.id.clone(),
            ts: snapshot.ts.0,
            width: snapshot.width,
            height: snapshot.height,
            point: snapshot.point,
            title: capture.title.clone(),
            elapsed_ms,
        };
        if !feed.captures.iter().any(|c| c.id == capture.id) {
            feed.captures.push(capture);
        }
        sent
    }

    /// Notes a press's label: the new title of its capture when that is
    /// known and the label changes it, else nothing (the label waits for
    /// the screenshot).
    pub fn press(&self, press: &LivePress) -> Option<TitleChange> {
        let mut feed = self.lock();
        let Some(capture) = feed.captures.iter_mut().find(|c| c.ts == press.ts) else {
            if press.label.is_some() {
                if feed.labels.len() == PENDING_LABELS {
                    feed.labels.pop_front();
                }
                feed.labels.push_back((press.ts, press.label.clone()));
            }
            return None;
        };
        if capture.renamed || capture.deleted || press.label.is_none() {
            return None;
        }
        capture.label = press.label.clone();
        let title = capture.guessed();
        if title == capture.title {
            return None;
        }
        capture.title = title.clone();
        Some(TitleChange {
            id: capture.id.clone(),
            title,
        })
    }

    /// Renames the capture `id` to the plain text `text`; returns its new
    /// title (guide Markdown). `invalid` for an empty title, `notFound` for
    /// a capture not in the feed or deleted.
    pub fn rename(&self, id: &str, text: &str) -> Result<TitleChange> {
        let text: String = text.trim().chars().take(MAX_TITLE_CHARS).collect();
        if text.trim().is_empty() {
            return Err(AppError::new(ErrorKind::Invalid, "a step needs a title"));
        }
        let mut feed = self.lock();
        let capture = feed
            .captures
            .iter_mut()
            .find(|c| c.id == id && !c.deleted)
            .ok_or_else(not_found)?;
        capture.title = plain_title(&text);
        capture.renamed = true;
        Ok(TitleChange {
            id: capture.id.clone(),
            title: capture.title.clone(),
        })
    }

    /// Deletes the capture `id`; false if it is not in the feed or already
    /// deleted.
    pub fn delete(&self, id: &str) -> bool {
        let mut feed = self.lock();
        match feed.captures.iter_mut().find(|c| c.id == id && !c.deleted) {
            Some(capture) => {
                capture.deleted = true;
                true
            }
            None => false,
        }
    }

    /// Deletes the newest capture not yet deleted; returns its id.
    pub fn undo(&self) -> Option<String> {
        let mut feed = self.lock();
        let capture = feed.captures.iter_mut().rev().find(|c| !c.deleted)?;
        capture.deleted = true;
        Some(capture.id.clone())
    }

    /// The deletions and renames, for the recording at stop.
    pub fn edits(&self) -> CaptureEdits {
        let feed = self.lock();
        let mut edits = CaptureEdits::default();
        for capture in &feed.captures {
            if capture.deleted {
                edits.deleted.insert(capture.ts);
            } else if capture.renamed {
                edits.titles.insert(capture.ts, capture.title.clone());
            }
        }
        edits
    }
}

fn not_found() -> AppError {
    AppError::new(ErrorKind::NotFound, "no such capture in this recording")
}

/// Sends `change` as [`TITLE_EVENT`].
pub fn emit_title<R: Runtime>(app: &AppHandle<R>, change: &TitleChange) {
    if let Err(e) = app.emit(TITLE_EVENT, change) {
        eprintln!("guiderecorder: cannot emit {TITLE_EVENT}: {e}");
    }
}

/// Sends the capture `id` as [`REMOVED_EVENT`].
pub fn emit_removed<R: Runtime>(app: &AppHandle<R>, id: String) {
    if let Err(e) = app.emit(REMOVED_EVENT, Removed { id }) {
        eprintln!("guiderecorder: cannot emit {REMOVED_EVENT}: {e}");
    }
}

/// Deletes a capture of the running recording (paused is fine): none of
/// the steps that use its screenshot end up in the guide. `invalid` when
/// nothing is recording, `notFound` for a capture not in the feed or
/// already deleted. Sent as [`REMOVED_EVENT`].
#[tauri::command]
pub fn delete_capture<R: Runtime>(
    app: AppHandle<R>,
    recorder: State<'_, Arc<Recorder>>,
    id: String,
) -> Result<()> {
    recording::ensure_active(&recorder)?;
    if !recorder.feed.delete(&id) {
        return Err(not_found());
    }
    emit_removed(&app, id);
    Ok(())
}

/// Renames a capture of the running recording: the first step that uses
/// its screenshot takes `title` (plain text, trimmed, at most
/// [`MAX_TITLE_CHARS`]). Returns the title as stored (guide Markdown), also
/// sent as [`TITLE_EVENT`]. `invalid` when nothing is recording or the
/// title is empty, `notFound` for a capture not in the feed.
#[tauri::command]
pub fn rename_capture<R: Runtime>(
    app: AppHandle<R>,
    recorder: State<'_, Arc<Recorder>>,
    id: String,
    title: String,
) -> Result<String> {
    recording::ensure_active(&recorder)?;
    let change = recorder.feed.rename(&id, &title)?;
    emit_title(&app, &change);
    Ok(change.title)
}

#[cfg(test)]
mod tests {
    use super::*;
    use guiderecorder_capture::session::FrameKey;
    use guiderecorder_core::model::Point;
    use std::path::PathBuf;

    fn snap(frame_ts: u64, ts: u64, action: LiveAction) -> LiveSnapshot {
        LiveSnapshot {
            ts: Timestamp(ts),
            frame: FrameKey {
                source: 0,
                ts: Timestamp(frame_ts),
            },
            path: PathBuf::from("/tmp/x.webp"),
            width: 16,
            height: 10,
            point: Some(Point { x: 8.0, y: 5.0 }),
            action: Some(action),
            app: Some("Expensify".into()),
        }
    }

    fn click() -> LiveAction {
        LiveAction::Click {
            verb: "Click".into(),
        }
    }

    fn press(ts: u64, label: Option<&str>) -> LivePress {
        LivePress {
            ts: Timestamp(ts),
            point: Point { x: 1.0, y: 1.0 },
            label: label.map(str::to_owned),
        }
    }

    #[test]
    fn titles_are_guessed_then_refined_by_the_label() {
        let feed = LiveFeed::default();
        let sent = feed.snapshot(&snap(5, 10, click()), 58_000);
        assert_eq!(sent.id, "0-5");
        assert_eq!(sent.title, "Click in Expensify");
        assert_eq!(sent.elapsed_ms, 58_000);
        assert_eq!(
            feed.press(&press(10, Some("Submit report"))),
            Some(TitleChange {
                id: "0-5".into(),
                title: "Click **Submit report** in Expensify".into()
            })
        );
        // The same label again, or none: no change.
        assert_eq!(feed.press(&press(10, Some("Submit report"))), None);
        assert_eq!(feed.press(&press(10, None)), None);
    }

    #[test]
    fn a_label_found_first_waits_for_its_screenshot() {
        let feed = LiveFeed::default();
        assert_eq!(feed.press(&press(10, Some("Save"))), None);
        assert_eq!(feed.press(&press(11, None)), None);
        let sent = feed.snapshot(&snap(5, 10, click()), 0);
        assert_eq!(sent.title, "Click **Save** in Expensify");
        assert!(feed.lock().labels.is_empty());
        // Only so many wait.
        for ts in 100..100 + PENDING_LABELS as u64 + 5 {
            feed.press(&press(ts, Some("x")));
        }
        assert_eq!(feed.lock().labels.len(), PENDING_LABELS);
    }

    #[test]
    fn renames_stick_and_are_escaped() {
        let feed = LiveFeed::default();
        feed.snapshot(&snap(5, 10, click()), 0);
        let change = feed.rename("0-5", "  Open *Settings*  ").unwrap();
        assert_eq!(change.title, "Open \\*Settings\\*");
        // A label found later does not replace a rename.
        assert_eq!(feed.press(&press(10, Some("Save"))), None);
        assert_eq!(
            feed.rename("0-5", "   ").unwrap_err().kind,
            ErrorKind::Invalid
        );
        assert_eq!(
            feed.rename("0-9", "x").unwrap_err().kind,
            ErrorKind::NotFound
        );
        let long = "a".repeat(MAX_TITLE_CHARS + 50);
        assert_eq!(
            feed.rename("0-5", &long).unwrap().title.len(),
            MAX_TITLE_CHARS
        );
    }

    #[test]
    fn delete_and_undo_feed_the_edits() {
        let feed = LiveFeed::default();
        feed.snapshot(&snap(5, 10, click()), 0);
        feed.snapshot(&snap(15, 20, LiveAction::Type), 0);
        feed.snapshot(&snap(25, 30, LiveAction::Manual), 0);
        feed.rename("0-5", "First").unwrap();
        assert!(feed.delete("0-15"));
        assert!(!feed.delete("0-15"));
        // Undo takes the newest capture still there.
        assert_eq!(feed.undo().as_deref(), Some("0-25"));
        assert_eq!(feed.undo().as_deref(), Some("0-5"));
        assert_eq!(feed.undo(), None);
        assert_eq!(
            feed.rename("0-5", "x").unwrap_err().kind,
            ErrorKind::NotFound
        );
        let edits = feed.edits();
        assert_eq!(
            edits.deleted,
            [Timestamp(10), Timestamp(20), Timestamp(30)].into()
        );
        assert!(edits.titles.is_empty());
        feed.reset();
        assert!(feed.edits().is_empty());
    }

    #[test]
    fn renamed_captures_are_retitled_at_stop() {
        let feed = LiveFeed::default();
        feed.snapshot(&snap(5, 10, click()), 0);
        feed.snapshot(&snap(5, 10, click()), 0);
        feed.rename("0-5", "Pick a **date**").unwrap();
        let edits = feed.edits();
        assert_eq!(
            edits.titles.get(&Timestamp(10)).map(String::as_str),
            Some("Pick a \\*\\*date\\*\\*")
        );
        // The same frame announced twice is one capture.
        assert_eq!(feed.lock().captures.len(), 1);
    }

    #[test]
    fn events_are_camel_case() {
        assert_eq!(
            serde_json::to_value(TitleChange {
                id: "0-5".into(),
                title: "Click".into()
            })
            .unwrap(),
            serde_json::json!({"id": "0-5", "title": "Click"})
        );
        assert_eq!(
            serde_json::to_value(Removed { id: "0-5".into() }).unwrap(),
            serde_json::json!({"id": "0-5"})
        );
    }
}
