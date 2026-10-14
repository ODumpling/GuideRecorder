//! Raw capture events: the contract between `guidesya-capture` (producer)
//! and step detection in `steps` (consumer). See design scratchpad #440.
//!
//! Coordinates are global display points (logical, not pixels) with the
//! origin at the top-left of the main display, as reported by CGEvent.
//! Timestamps are host monotonic nanoseconds (mach absolute time converted
//! to ns), the same clock ScreenCaptureKit frame timestamps use, so events
//! and frames can be matched directly.

use serde::{Deserialize, Serialize};

use crate::model::{ElementInfo, Point, Rect};

/// Host monotonic time in nanoseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(pub u64);

impl Timestamp {
    /// Milliseconds elapsed from `earlier` to `self` (0 if `earlier` is later).
    pub fn millis_since(self, earlier: Timestamp) -> u64 {
        self.0.saturating_sub(earlier.0) / 1_000_000
    }
}

/// One event with its time of occurrence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimedEvent {
    pub ts: Timestamp,
    pub event: CaptureEvent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum CaptureEvent {
    MouseDown {
        point: Point,
        button: MouseButton,
        /// 1 for a single click, 2 for the second press of a double click, etc.
        click_count: u32,
        modifiers: Modifiers,
        /// AX element under the pointer. The event tap leaves this `None`;
        /// the recording session fills it from an AX lookup at the click.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        element: Option<RawElement>,
    },
    MouseUp {
        point: Point,
        button: MouseButton,
        click_count: u32,
        modifiers: Modifiers,
    },
    /// Pointer moved with a button held.
    MouseDragged {
        point: Point,
        button: MouseButton,
    },
    Scroll {
        point: Point,
        delta_x: f64,
        delta_y: f64,
    },
    KeyDown {
        key: Key,
        modifiers: Modifiers,
        is_repeat: bool,
        /// True when the focused element is a secure text field. The event
        /// tap cannot know this; the recording session sets it from the AX
        /// focused element. Text of secure keys must never be persisted.
        #[serde(default)]
        secure: bool,
    },
    FlagsChanged {
        modifiers: Modifiers,
    },
    /// The AX focused element changed (or focus was lost).
    FocusChanged {
        element: Option<RawElement>,
    },
    /// The frontmost application changed.
    AppActivated {
        app: AppRef,
    },
    /// User pressed the manual-step hotkey.
    ManualStep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MouseButton {
    Left,
    Right,
    Other,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Modifiers {
    pub shift: bool,
    pub control: bool,
    pub option: bool,
    pub command: bool,
    pub function: bool,
    pub caps_lock: bool,
}

impl Modifiers {
    /// True if any modifier that turns a key press into a shortcut is held
    /// (command, control or option; shift alone does not).
    pub fn is_shortcut(&self) -> bool {
        self.command || self.control || self.option
    }
}

/// A key press. `characters` is what the key produced with current
/// modifiers (None for non-printing keys); `code` is the virtual key code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Key {
    pub code: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub characters: Option<String>,
    /// Characters ignoring modifiers (e.g. "s" for ⌘S), used for shortcut labels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub characters_ignoring_modifiers: Option<String>,
}

/// An application, identified for exclusion and labelling.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppRef {
    pub pid: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
}

/// Accessibility element as read from AX, before it is reduced to
/// [`ElementInfo`] for storage.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawElement {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subrole: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Label from a linked title element or placeholder, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_title: Option<String>,
    #[serde(default)]
    pub app: AppRef,
    /// Element frame in global display points.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<Rect>,
    /// Secure text field (AXSecureTextField subrole or equivalent).
    #[serde(default)]
    pub is_secure: bool,
}

impl RawElement {
    /// Best human label: explicit label, then title, then description.
    pub fn best_label(&self) -> Option<&str> {
        [&self.label, &self.title, &self.description]
            .into_iter()
            .flatten()
            .map(String::as_str)
            .find(|s| !s.trim().is_empty())
    }

    pub fn to_element_info(&self) -> ElementInfo {
        ElementInfo {
            role: self.role.clone(),
            label: self.best_label().map(str::to_owned),
            app: self.app.name.clone(),
            bundle_id: self.app.bundle_id.clone(),
            window: self.window_title.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_json_shape() {
        let ev = TimedEvent {
            ts: Timestamp(42),
            event: CaptureEvent::MouseDown {
                point: Point { x: 1.0, y: 2.0 },
                button: MouseButton::Left,
                click_count: 1,
                modifiers: Modifiers::default(),
                element: None,
            },
        };
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["ts"], 42);
        assert_eq!(json["event"]["type"], "mouseDown");
        assert_eq!(json["event"]["clickCount"], 1);
        let back: TimedEvent = serde_json::from_value(json).unwrap();
        assert_eq!(back, ev);
    }

    #[test]
    fn best_label_skips_blank() {
        let el = RawElement {
            label: Some("  ".into()),
            title: Some("Save".into()),
            ..Default::default()
        };
        assert_eq!(el.best_label(), Some("Save"));
        assert_eq!(el.to_element_info().label.as_deref(), Some("Save"));
    }
}
