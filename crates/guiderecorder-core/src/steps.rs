//! Step detection: raw capture events to draft steps. See design scratchpad
//! #440 ("Step detection").
//!
//! [`detect_steps`] is a pure function over a recorded event trace, which it
//! first stable-sorts by timestamp:
//!
//! - a click, double click or right click is one step: later presses of a
//!   multi-click merge into the step of the first press. A triple click keeps
//!   kind [`StepKind::DoubleClick`] and is titled "Triple-click";
//! - typing is grouped per focused field and flushed on focus change, app
//!   change, Return, Tab, or idle longer than [`StepConfig::typing_idle_ms`]
//!   (focus moving to no element does not end a run);
//!   Return or Esc pressed with nothing typed is a step of its own;
//! - a shortcut (command, control or option held) is one step, except that
//!   option (with or without shift) producing a different printable character
//!   counts as typing, e.g. ⌥L typing "@" on German layouts;
//! - a press that moves beyond [`StepConfig::drag_threshold`] before release
//!   is one drag step with a start and an end point;
//! - scrolling produces no step;
//! - [`CaptureEvent::ManualStep`] produces a manual step (the recording
//!   session drops the hotkey's own key press, so it is not filtered here).
//!
//! A click or drag is labelled from the element on its `MouseDown` when that
//! element has a label; otherwise from a focus change caused by the press,
//! or the previously focused element, whose frame holds the point.
//!
//! Text typed into secure fields is never kept: it becomes [`SECURE_MASK`].
//! Any option-only chord in a secure field counts as typing, not as a
//! shortcut. When a secure field gains focus within
//! [`StepConfig::focus_settle_ms`] of the last key of a typing run (open,
//! or just ended by Return or a focus change and still the newest step), the
//! focus notification is taken to be late and the whole run is masked and
//! moved to that field. A run ended by Tab, a click or an app switch is never
//! masked this way: those explain the focus change, so a username typed
//! before tabbing or clicking into the password field stays plain. This
//! over-masks on purpose: a form that moves focus to a password field by
//! itself right after typing masks the text typed before it too. Draft steps keep geometry in global display points; map it to
//! screenshot pixels with [`DraftStep::map_points`] before
//! [`DraftStep::into_step`].
//!
//! Limitations (v1): typed text is rebuilt from key presses only. Cursor
//! movement is ignored (typing `a`, ←, `b` records "ab"), and Backspace only
//! removes text typed in the same run. Input methods (Japanese, Chinese and
//! so on) record the raw keystrokes rather than the composed text, and the
//! Return that commits a composition reads as "press Return". Dead keys
//! without option (e.g. US-International `'`) produce no characters, so the
//! accent is lost. Option dead keys (⌥E then `e` for "é") may split a run:
//! the dead key reads as a "Press **⌥E**" shortcut if it reports no
//! characters, or as a separate "´" if it reports the accent.

use serde::{Deserialize, Serialize};
use ulid::Ulid;

use crate::events::{
    AppRef, CaptureEvent, Key, Modifiers, MouseButton, RawElement, TimedEvent, Timestamp,
};
use crate::keyboard::{is_printable_char, KeyPlatform, NamedKey};
use crate::model::{AiState, Point, Rect, Step, StepImage, StepKind};

/// Replaces anything typed into a secure field. Fixed length, so the
/// length of the secret is not revealed either.
pub const SECURE_MASK: &str = "••••••••";

/// Tuning for [`detect_steps`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StepConfig {
    /// A pause longer than this between keys ends a typing step.
    pub typing_idle_ms: u64,
    /// Distance in points a pressed pointer must move to count as a drag.
    pub drag_threshold: f64,
    /// How long after a mouse-up a focus change still counts as the result
    /// of the click, for labelling the clicked element.
    pub focus_settle_ms: u64,
    /// Whose key codes the trace holds and how its shortcuts are written:
    /// by default the platform this runs on, which recorded the trace.
    #[serde(skip)]
    pub keys: KeyPlatform,
}

impl Default for StepConfig {
    fn default() -> Self {
        Self {
            typing_idle_ms: 1500,
            drag_threshold: 5.0,
            focus_settle_ms: 300,
            keys: KeyPlatform::CURRENT,
        }
    }
}

/// A step detected from events, before it has an id or a screenshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftStep {
    pub kind: StepKind,
    /// When the step's screenshot should be taken: the first press of a
    /// click or drag, the first key of a typing run, the shortcut key-down.
    pub at: Timestamp,
    /// Interaction point (global display points). For typing, the centre of
    /// the field when its frame is known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub point: Option<Point>,
    /// End of a drag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_point: Option<Point>,
    /// Element acted on, if it could be identified.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element: Option<RawElement>,
    /// Frontmost application at the time of the step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<AppRef>,
    /// Typed text (masked for secure fields) or a shortcut label like `⌘S`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keystrokes: Option<String>,
    /// Template title in the guide Markdown subset.
    pub title: String,
}

impl DraftStep {
    /// Converts `point` and `end_point` with `f`, e.g. from global display
    /// points to screenshot pixels.
    pub fn map_points(mut self, f: impl Fn(Point) -> Point) -> Self {
        self.point = self.point.map(&f);
        self.end_point = self.end_point.map(&f);
        self
    }

    /// Builds the stored step. Points are copied as they are, so map them to
    /// screenshot pixels first with [`DraftStep::map_points`].
    pub fn into_step(self, id: Ulid, image: StepImage) -> Step {
        let element = match (&self.element, &self.app) {
            (Some(el), app) => {
                let mut info = el.to_element_info();
                if let Some(app) = app {
                    info.app = info.app.or_else(|| app.name.clone());
                    info.bundle_id = info.bundle_id.or_else(|| app.bundle_id.clone());
                }
                Some(info)
            }
            (None, Some(app)) if app.name.is_some() || app.bundle_id.is_some() => {
                Some(crate::model::ElementInfo {
                    app: app.name.clone(),
                    bundle_id: app.bundle_id.clone(),
                    ..Default::default()
                })
            }
            (None, _) => None,
        };
        Step {
            id,
            title: self.title,
            description: String::new(),
            kind: self.kind,
            point: self.point,
            end_point: self.end_point,
            element,
            keystrokes: self.keystrokes,
            image,
            annotations: Vec::new(),
            narration: String::new(),
            ai_state: AiState::default(),
        }
    }

    fn app_name(&self) -> Option<&str> {
        self.element
            .as_ref()
            .and_then(|el| el.app.name.as_deref())
            .or_else(|| self.app.as_ref().and_then(|app| app.name.as_deref()))
            .map(str::trim)
            .filter(|name| !name.is_empty())
    }

    fn label(&self) -> Option<&str> {
        self.element.as_ref().and_then(RawElement::best_label)
    }
}

/// Detects steps in an event trace. Events are stable-sorted by timestamp
/// first, since the session merges event-tap and AX events from different
/// threads.
pub fn detect_steps(events: &[TimedEvent], cfg: &StepConfig) -> Vec<DraftStep> {
    let mut sorted = events.to_vec();
    sorted.sort_by_key(|ev| ev.ts);
    let events = sorted.as_slice();
    let mut detector = Detector {
        events,
        cfg,
        steps: Vec::new(),
        focus: None,
        app: None,
        typing: None,
        press: None,
        last_typed: None,
    };
    for (index, timed) in events.iter().enumerate() {
        detector.handle(index, timed);
    }
    if let Some(press) = detector.press.take() {
        let at = press.last_point;
        detector.finish_press(press, at, events.len(), None);
    }
    detector.flush_typing();
    detector.steps
}

struct Detector<'a> {
    events: &'a [TimedEvent],
    cfg: &'a StepConfig,
    steps: Vec<DraftStep>,
    focus: Option<RawElement>,
    app: Option<AppRef>,
    typing: Option<Typing>,
    press: Option<Press>,
    /// The newest step if it is a plain (unmasked) Type step ended by Return
    /// or a focus change, with the time of its last key, for masking it when
    /// a secure focus arrives late.
    last_typed: Option<LastTyped>,
}

struct LastTyped {
    index: usize,
    last: Timestamp,
    submitted: bool,
}

struct Typing {
    at: Timestamp,
    last: Timestamp,
    element: Option<RawElement>,
    text: String,
    secure: bool,
    /// Ended by Return, to be mentioned in the title.
    submitted: bool,
}

struct Press {
    index: usize,
    at: Timestamp,
    point: Point,
    last_point: Point,
    button: MouseButton,
    click_count: u32,
    modifiers: Modifiers,
    dragged: bool,
    /// Element under the pointer from the AX lookup at the press.
    element: Option<RawElement>,
    focus_before: Option<RawElement>,
}

impl Detector<'_> {
    fn handle(&mut self, index: usize, timed: &TimedEvent) {
        let ts = timed.ts;
        match &timed.event {
            CaptureEvent::MouseDown {
                point,
                button,
                click_count,
                modifiers,
                element,
            } => {
                self.end_run();
                if let Some(press) = self.press.take() {
                    let at = press.last_point;
                    self.finish_press(press, at, index, None);
                }
                self.press = Some(Press {
                    index,
                    at: ts,
                    point: *point,
                    last_point: *point,
                    button: *button,
                    click_count: *click_count,
                    modifiers: *modifiers,
                    dragged: false,
                    element: element.clone(),
                    focus_before: self.focus.clone(),
                });
            }
            CaptureEvent::MouseDragged { point, .. } => {
                if let Some(press) = &mut self.press {
                    press.last_point = *point;
                    if distance(press.point, *point) > self.cfg.drag_threshold {
                        press.dragged = true;
                    }
                }
            }
            CaptureEvent::MouseUp { point, .. } => {
                if let Some(mut press) = self.press.take() {
                    if distance(press.point, *point) > self.cfg.drag_threshold {
                        press.dragged = true;
                    }
                    self.finish_press(press, *point, index, Some(ts));
                }
            }
            CaptureEvent::Scroll { .. } | CaptureEvent::FlagsChanged { .. } => {}
            CaptureEvent::KeyDown {
                key,
                modifiers,
                is_repeat,
                secure,
            } => self.key_down(ts, key, *modifiers, *is_repeat, *secure),
            CaptureEvent::FocusChanged { element } => {
                // A secure field focused just after keys were typed: the focus
                // notification was late and the keys went into that field.
                // Over-masking is safe; leaking a password is not.
                if let (Some(typing), Some(el)) = (&mut self.typing, element) {
                    if el.is_secure && ts.millis_since(typing.last) <= self.cfg.focus_settle_ms {
                        typing.secure = true;
                        typing.element = Some(el.clone());
                    }
                }
                // The same when the run already ended (Return, or a focus loss
                // just before), e.g. password-manager auto-type.
                if let Some(el) = element.as_ref().filter(|el| el.is_secure) {
                    self.mask_last_typed(ts, el);
                }
                // Losing focus does not end the run: AX often reports no focus
                // briefly between two fields, and splitting a run there could
                // leave the start of a password unmasked. The next focus or
                // the idle rule decides.
                if element.is_some() && self.typing.as_ref().is_some_and(|t| t.element != *element)
                {
                    self.flush_typing();
                }
                self.focus = element.clone();
            }
            CaptureEvent::AppActivated { app } => {
                if self.app.as_ref() != Some(app) {
                    self.end_run();
                }
                self.app = Some(app.clone());
            }
            CaptureEvent::ManualStep => {
                self.flush_typing();
                let mut step = self.draft(StepKind::Manual, ts);
                step.title = "Manual step".to_owned();
                self.steps.push(step);
            }
        }
    }

    fn draft(&self, kind: StepKind, at: Timestamp) -> DraftStep {
        DraftStep {
            kind,
            at,
            point: None,
            end_point: None,
            element: None,
            app: self.app.clone(),
            keystrokes: None,
            title: String::new(),
        }
    }

    fn key_down(
        &mut self,
        ts: Timestamp,
        key: &Key,
        modifiers: Modifiers,
        is_repeat: bool,
        secure: bool,
    ) {
        let secure = secure || self.focus.as_ref().is_some_and(|el| el.is_secure);
        if let Some(typing) = &self.typing {
            if ts.millis_since(typing.last) > self.cfg.typing_idle_ms {
                self.flush_typing();
            }
        }

        let text = key.characters.as_deref().map(printable).unwrap_or_default();
        // Option (with or without shift) types characters on many layouts,
        // e.g. ⌥L is "@" on German keyboards. In a secure field it always
        // counts as typing, so the base key is never shown.
        let option_only = modifiers.option && !modifiers.command && !modifiers.control;
        let option_typing = option_only
            && (secure
                || (!text.is_empty()
                    && key.characters_ignoring_modifiers.as_deref() != Some(text.as_str())));
        if modifiers.is_shortcut() && !option_typing {
            if is_repeat {
                return;
            }
            self.flush_typing();
            let label = self.cfg.keys.shortcut_label(key, modifiers);
            let mut step = self.draft(StepKind::Shortcut, ts);
            step.title = format!("Press {}", bold(&label));
            step.keystrokes = Some(label);
            self.steps.push(step);
            return;
        }

        let keys = self.cfg.keys;
        match keys.named(key.code) {
            Some(NamedKey::Return) => {
                if is_repeat {
                    return;
                }
                if let Some(typing) = &mut self.typing {
                    typing.submitted = true;
                    typing.last = ts;
                    self.flush_typing();
                } else {
                    let mut step = self.draft(StepKind::Shortcut, ts);
                    step.element = self.focus.clone();
                    step.title = format!("Press {}", bold(keys.return_name()));
                    step.keystrokes = Some(keys.key_name(NamedKey::Return));
                    self.steps.push(step);
                }
                return;
            }
            Some(NamedKey::Escape) => {
                if is_repeat {
                    return;
                }
                self.flush_typing();
                let mut step = self.draft(StepKind::Shortcut, ts);
                step.title = format!("Press {}", bold("Esc"));
                step.keystrokes = Some(keys.key_name(NamedKey::Escape));
                self.steps.push(step);
                return;
            }
            Some(NamedKey::Tab) => {
                self.end_run();
                return;
            }
            Some(NamedKey::Backspace) => {
                if let Some(typing) = &mut self.typing {
                    typing.text.pop();
                    typing.last = ts;
                }
                return;
            }
            _ => {}
        }

        if text.is_empty() {
            return;
        }
        if self.typing.is_none() {
            // A new run means the previous one was not followed by the late
            // secure focus that would mask it.
            self.last_typed = None;
        }
        let typing = self.typing.get_or_insert_with(|| Typing {
            at: ts,
            last: ts,
            element: self.focus.clone(),
            text: String::new(),
            secure: false,
            submitted: false,
        });
        typing.last = ts;
        typing.secure |= secure;
        if typing.secure {
            // Keep only a count so the buffer is never non-empty by accident;
            // the characters themselves are dropped here.
            typing.text.extend(text.chars().map(|_| '•'));
        } else {
            typing.text.push_str(&text);
        }
    }

    fn flush_typing(&mut self) {
        let Some(typing) = self.typing.take() else {
            return;
        };
        let mut step = self.draft(StepKind::Type, typing.at);
        step.point = typing.element.as_ref().and_then(|el| el.frame).map(center);
        step.element = typing.element;
        let has_text = !typing.text.is_empty();
        let shown = if typing.secure {
            SECURE_MASK.to_owned()
        } else {
            typing.text
        };
        if !has_text {
            if !typing.submitted {
                return;
            }
            // Everything typed was deleted again: only the Return remains.
            step.at = typing.last;
            step.kind = StepKind::Shortcut;
            step.title = format!("Press {}", bold(self.cfg.keys.return_name()));
            step.keystrokes = Some(self.cfg.keys.key_name(NamedKey::Return));
            self.steps.push(step);
            return;
        }
        step.title = type_title(&step, &shown, typing.submitted, self.cfg.keys);
        step.keystrokes = Some(shown);
        self.steps.push(step);
        if !typing.secure {
            self.last_typed = Some(LastTyped {
                index: self.steps.len() - 1,
                last: typing.last,
                submitted: typing.submitted,
            });
        }
    }

    /// Flushes the typing run for a reason that explains a following focus
    /// change (Tab, a click, an app switch), so the run is not masked by a
    /// secure focus that arrives soon after.
    fn end_run(&mut self) {
        self.flush_typing();
        self.last_typed = None;
    }

    /// Masks the newest step if it is a plain Type step whose last key came
    /// within [`StepConfig::focus_settle_ms`] before secure field `el` was
    /// focused, and moves it to that field.
    fn mask_last_typed(&mut self, ts: Timestamp, el: &RawElement) {
        let Some(last) = self.last_typed.take() else {
            return;
        };
        if last.index + 1 != self.steps.len()
            || ts.millis_since(last.last) > self.cfg.focus_settle_ms
        {
            return;
        }
        let step = &mut self.steps[last.index];
        step.element = Some(el.clone());
        step.point = el.frame.map(center);
        step.title = type_title(step, SECURE_MASK, last.submitted, self.cfg.keys);
        step.keystrokes = Some(SECURE_MASK.to_owned());
    }

    /// Emits the step for a finished press. `end` is where the pointer was
    /// released; `next` is the index of the first event after the press.
    fn finish_press(&mut self, press: Press, end: Point, next: usize, up: Option<Timestamp>) {
        let element = self.element_for_press(&press, next, up);
        if press.dragged {
            let mut step = self.draft(StepKind::Drag, press.at);
            step.point = Some(press.point);
            step.end_point = Some(end);
            step.element = element;
            step.title = match (step.label(), step.app_name()) {
                (Some(label), Some(app)) => format!("Drag {} in {}", bold(label), escape(app)),
                (Some(label), None) => format!("Drag {}", bold(label)),
                (None, Some(app)) => format!("Drag in {}", escape(app)),
                (None, None) => "Drag".to_owned(),
            };
            self.steps.push(step);
            return;
        }

        let keys = self.cfg.keys;
        let (kind, verb, modifiers) =
            click_verb(press.button, press.click_count, press.modifiers, keys);
        let right = kind == StepKind::RightClick;

        // Later presses of a multi-click merge into the step of the first.
        if press.click_count >= 2 {
            if let Some(last) = self.steps.last_mut() {
                let same_button = if right {
                    last.kind == StepKind::RightClick
                } else {
                    matches!(last.kind, StepKind::Click | StepKind::DoubleClick)
                };
                let near = last
                    .point
                    .is_some_and(|p| distance(p, press.point) <= self.cfg.drag_threshold * 2.0);
                if same_button && near {
                    last.kind = kind;
                    if last.element.is_none() {
                        last.element = element;
                    }
                    last.title = click_title(last, verb, modifiers, keys);
                    return;
                }
            }
        }

        let mut step = self.draft(kind, press.at);
        step.point = Some(press.point);
        step.element = element;
        step.title = click_title(&step, verb, modifiers, keys);
        self.steps.push(step);
    }

    /// The element under a press: the AX element from the press itself when it
    /// has a label. Otherwise a focus change during the press, or soon after
    /// release, whose frame holds the point; else the element focused before
    /// the press if its frame holds the point; else the unlabelled pressed
    /// element. A focus change during the press without a frame is trusted as
    /// the clicked element.
    fn element_for_press(
        &self,
        press: &Press,
        next: usize,
        up: Option<Timestamp>,
    ) -> Option<RawElement> {
        if let Some(el) = press
            .element
            .as_ref()
            .filter(|el| el.best_label().is_some())
        {
            return Some(el.clone());
        }
        let hits = |el: &RawElement| el.frame.is_some_and(|frame| contains(frame, press.point));
        let during = &self.events[press.index + 1..next];
        let after = self.events[next.min(self.events.len())..]
            .iter()
            .skip(usize::from(up.is_some()))
            .take_while(|ev| {
                let settled =
                    up.is_some_and(|up| ev.ts.millis_since(up) <= self.cfg.focus_settle_ms);
                settled
                    && !matches!(
                        ev.event,
                        CaptureEvent::MouseDown { .. } | CaptureEvent::KeyDown { .. }
                    )
            });
        let focus_changes = |evs: &mut dyn Iterator<Item = &TimedEvent>| -> Vec<RawElement> {
            evs.filter_map(|ev| match &ev.event {
                CaptureEvent::FocusChanged { element: Some(el) } => Some(el.clone()),
                _ => None,
            })
            .collect()
        };
        let during = focus_changes(&mut during.iter());
        let after = focus_changes(&mut after.into_iter());

        during
            .iter()
            .rev()
            .find(|el| el.frame.is_none() || hits(el))
            .or_else(|| after.iter().find(|el| hits(el)))
            .or_else(|| press.focus_before.as_ref().filter(|el| hits(el)))
            .or(press.element.as_ref())
            .cloned()
    }
}

/// A press's step kind, its verb ("Click", "Right-click", "Double-click",
/// "Triple-click") and the modifiers to show with it. A triple click keeps
/// kind DoubleClick (the model has no TripleClick) and is told apart only
/// by its title. On a Mac, control turns a left click into a right click,
/// and is then not shown twice.
fn click_verb(
    button: MouseButton,
    click_count: u32,
    modifiers: Modifiers,
    keys: KeyPlatform,
) -> (StepKind, &'static str, Modifiers) {
    let right = button == MouseButton::Right
        || (button == MouseButton::Left
            && modifiers.control
            && keys.control_click_is_right_click());
    let (kind, verb) = if right {
        (StepKind::RightClick, "Right-click")
    } else if click_count >= 3 {
        (StepKind::DoubleClick, "Triple-click")
    } else if click_count == 2 {
        (StepKind::DoubleClick, "Double-click")
    } else {
        (StepKind::Click, "Click")
    };
    let modifiers = if right {
        Modifiers {
            control: button == MouseButton::Right && modifiers.control,
            ..modifiers
        }
    } else {
        modifiers
    };
    (kind, verb, modifiers)
}

/// What took a screenshot while recording, reduced to what a provisional
/// title needs (see [`LiveAction::title`]). Holds no typed text: a key is
/// read before the session knows whether its field is secure, so only a
/// ⌘ or ⌃ (Win or Ctrl) shortcut, Return and Esc keep their key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveAction {
    /// A press: its verb ("Click", "Double-click", …) with the held
    /// modifiers, e.g. "⌘-click" or "Ctrl+click".
    Click {
        verb: String,
    },
    /// A shortcut, Return or Esc, by its label ("⌘S", "Return", "Ctrl+S",
    /// "Enter").
    Press {
        label: String,
    },
    /// Typing (the first key of a run).
    Type,
    Manual,
}

impl LiveAction {
    /// The action of an event that can take a snapshot (a press, a key, a
    /// manual step); `None` for any other. Keys are read as this
    /// platform's ([`KeyPlatform::CURRENT`]).
    pub fn of(event: &CaptureEvent) -> Option<Self> {
        Self::of_on(event, KeyPlatform::CURRENT)
    }

    /// [`LiveAction::of`], with keys read as `keys`'.
    pub fn of_on(event: &CaptureEvent, keys: KeyPlatform) -> Option<Self> {
        Some(match event {
            CaptureEvent::MouseDown {
                button,
                click_count,
                modifiers,
                ..
            } => {
                let (_, verb, modifiers) = click_verb(*button, *click_count, *modifiers, keys);
                Self::Click {
                    verb: with_click_prefix(verb, modifiers, keys),
                }
            }
            CaptureEvent::KeyDown { key, modifiers, .. } => {
                if modifiers.command || modifiers.control {
                    Self::Press {
                        label: keys.shortcut_label(key, *modifiers),
                    }
                } else if modifiers.option {
                    // Option types characters on many layouts.
                    Self::Type
                } else {
                    match keys.named(key.code) {
                        Some(NamedKey::Return) => Self::Press {
                            label: keys.return_name().to_owned(),
                        },
                        Some(NamedKey::Escape) => Self::Press {
                            label: "Esc".to_owned(),
                        },
                        _ => Self::Type,
                    }
                }
            }
            CaptureEvent::ManualStep => Self::Manual,
            _ => return None,
        })
    }

    /// A provisional title in the guide Markdown subset, worded as
    /// [`detect_steps`] words the step: "Click **Submit** in Safari",
    /// "Press **⌘S**", "Type in Notes", "Manual step". `label` is the
    /// element's ([`RawElement::best_label`]), `app` the app's name.
    pub fn title(&self, label: Option<&str>, app: Option<&str>) -> String {
        let label = label.map(str::trim).filter(|l| !l.is_empty());
        let app = app.map(str::trim).filter(|a| !a.is_empty());
        match self {
            Self::Click { verb } => match (label, app) {
                (Some(label), Some(app)) => format!("{verb} {} in {}", bold(label), escape(app)),
                (Some(label), None) => format!("{verb} {}", bold(label)),
                (None, Some(app)) => format!("{verb} in {}", escape(app)),
                (None, None) => verb.clone(),
            },
            Self::Press { label } => format!("Press {}", bold(label)),
            Self::Type => match (label, app) {
                (Some(label), _) => format!("Type into {}", bold(label)),
                (None, Some(app)) => format!("Type in {}", escape(app)),
                (None, None) => "Type".to_owned(),
            },
            Self::Manual => "Manual step".to_owned(),
        }
    }
}

/// `text` as a title in the guide Markdown subset that reads as typed:
/// Markdown characters are escaped and whitespace is collapsed.
pub fn plain_title(text: &str) -> String {
    escape(text)
}

fn type_title(step: &DraftStep, shown: &str, submitted: bool, keys: KeyPlatform) -> String {
    let mut title = format!("Type {}", code_span(shown));
    match (step.label(), step.app_name()) {
        (Some(label), _) => title.push_str(&format!(" into {}", bold(label))),
        (None, Some(app)) => title.push_str(&format!(" in {}", escape(app))),
        (None, None) => {}
    }
    if submitted {
        title.push_str(&format!(" and press {}", bold(keys.return_name())));
    }
    title
}

fn click_title(step: &DraftStep, verb: &str, modifiers: Modifiers, keys: KeyPlatform) -> String {
    let verb = with_click_prefix(verb, modifiers, keys);
    match (step.label(), step.app_name()) {
        (Some(label), Some(app)) => format!("{verb} {} in {}", bold(label), escape(app)),
        (Some(label), None) => format!("{verb} {}", bold(label)),
        (None, Some(app)) => format!("{verb} in {}", escape(app)),
        (None, None) => verb,
    }
}

/// `verb` ("Click") with the held modifiers before it: "⌘-click",
/// "Ctrl+click".
fn with_click_prefix(verb: &str, modifiers: Modifiers, keys: KeyPlatform) -> String {
    let held = keys.click_prefix(modifiers);
    if held.is_empty() {
        verb.to_owned()
    } else {
        format!("{held}{}", verb.to_lowercase())
    }
}

/// The printable part of the text a key inserts: control and function-key
/// characters are dropped, the rest is kept (a text expander may send
/// "a\tb" as one key).
fn printable(s: &str) -> String {
    s.chars().filter(|&c| is_printable_char(c)).collect()
}

fn distance(a: Point, b: Point) -> f64 {
    (a.x - b.x).hypot(a.y - b.y)
}

fn center(r: Rect) -> Point {
    Point {
        x: r.x + r.width / 2.0,
        y: r.y + r.height / 2.0,
    }
}

fn contains(r: Rect, p: Point) -> bool {
    const SLOP: f64 = 1.0;
    p.x >= r.x - SLOP
        && p.x <= r.x + r.width + SLOP
        && p.y >= r.y - SLOP
        && p.y <= r.y + r.height + SLOP
}

/// Escapes text for the guide Markdown subset. Runs of whitespace (including
/// line breaks) become one space and the ends are trimmed, as normalizing
/// would do.
pub(crate) fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (i, word) in text.split_whitespace().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        for c in word.chars() {
            if matches!(
                c,
                '\\' | '*' | '_' | '`' | '[' | ']' | '<' | '>' | '&' | '~'
            ) {
                out.push('\\');
            }
            out.push(c);
        }
    }
    out
}

fn bold(text: &str) -> String {
    format!("**{}**", escape(text))
}

/// An inline code span holding `code` verbatim.
fn code_span(code: &str) -> String {
    let code = code.replace(['\n', '\r'], " ");
    let longest_run = code.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest_run + 1);
    let pad =
        !code.trim().is_empty() && (code.starts_with([' ', '`']) || code.ends_with([' ', '`']));
    let pad = if pad { " " } else { "" };
    format!("{fence}{pad}{code}{pad}{fence}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown;

    /// The default config, reading keys as a Mac's: the traces and the
    /// key codes below were recorded on one.
    fn mac() -> StepConfig {
        StepConfig {
            keys: KeyPlatform::Mac,
            ..StepConfig::default()
        }
    }

    fn of(event: &CaptureEvent) -> Option<LiveAction> {
        LiveAction::of_on(event, KeyPlatform::Mac)
    }

    fn trace(name: &str) -> Vec<TimedEvent> {
        let path = format!("{}/testdata/traces/{name}.json", env!("CARGO_MANIFEST_DIR"));
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path}: {e}"))
    }

    fn detect(name: &str) -> Vec<DraftStep> {
        let steps = detect_steps(&trace(name), &mac());
        for step in &steps {
            assert_eq!(
                markdown::normalize(&step.title),
                step.title,
                "title is not normalized Markdown"
            );
        }
        steps
    }

    fn summary(steps: &[DraftStep]) -> Vec<(StepKind, u64, &str)> {
        steps
            .iter()
            .map(|s| (s.kind, s.at.0, s.title.as_str()))
            .collect()
    }

    const MS: u64 = 1_000_000;

    #[test]
    fn clicks() {
        let steps = detect("clicks");
        assert_eq!(
            summary(&steps),
            vec![
                (StepKind::Click, 1000 * MS, "Click **Save** in Finder"),
                (
                    StepKind::DoubleClick,
                    3000 * MS,
                    "Double-click **Report.pdf** in Finder"
                ),
                (
                    StepKind::RightClick,
                    5000 * MS,
                    "Right-click **Report.pdf** in Finder"
                ),
                (StepKind::RightClick, 7000 * MS, "Right-click in Finder"),
                (
                    StepKind::Click,
                    9000 * MS,
                    "⌘-click **Report.pdf** in Finder"
                ),
                // The pressed element wins over the focused one under the point.
                (StepKind::Click, 11000 * MS, "Click **Share** in Finder"),
                // An unlabelled pressed element falls back to the focus...
                (
                    StepKind::Click,
                    13000 * MS,
                    "Click **Report.pdf** in Finder"
                ),
                // ...and is kept when nothing else matches.
                (StepKind::Click, 15000 * MS, "Click in Finder"),
            ]
        );
        assert_eq!(
            steps[5].element.as_ref().unwrap().role.as_deref(),
            Some("AXButton")
        );
        assert_eq!(
            steps[7].element.as_ref().unwrap().role.as_deref(),
            Some("AXGroup")
        );
        assert_eq!(steps[0].point, Some(Point { x: 410.0, y: 300.0 }));
        assert_eq!(
            steps[1].element.as_ref().and_then(RawElement::best_label),
            Some("Report.pdf")
        );
        assert!(steps
            .iter()
            .all(|s| s.end_point.is_none() && s.keystrokes.is_none()));
    }

    #[test]
    fn typing() {
        let steps = detect("typing");
        assert_eq!(
            summary(&steps),
            vec![
                (
                    StepKind::Type,
                    1000 * MS,
                    "Type `staging` into **Environment**"
                ),
                (StepKind::Type, 2000 * MS, "Type `eu-west` into **Region**"),
                (
                    StepKind::Type,
                    5000 * MS,
                    "Type `hello world` into **Region**"
                ),
                (
                    StepKind::Type,
                    8000 * MS,
                    "Type `abc` into **Name** and press **Return**"
                ),
                (StepKind::Shortcut, 9000 * MS, "Press **Return**"),
                (StepKind::Type, 10000 * MS, "Type ``a`b`` in Terminal"),
            ]
        );
        assert_eq!(steps[0].keystrokes.as_deref(), Some("staging"));
        assert_eq!(steps[0].point, Some(Point { x: 150.0, y: 110.0 }));
        // Backspace removed the typo; the idle gap split "hello world" off.
        assert_eq!(steps[1].keystrokes.as_deref(), Some("eu-west"));
        assert_eq!(steps[5].keystrokes.as_deref(), Some("a`b"));
    }

    #[test]
    fn secure_typing_is_masked() {
        let steps = detect("secure");
        assert_eq!(
            summary(&steps),
            vec![
                (
                    StepKind::Type,
                    1000 * MS,
                    "Type `••••••••` into **Password** and press **Return**"
                ),
                (StepKind::Type, 3000 * MS, "Type `••••••••` into **PIN**"),
                (StepKind::Type, 5100 * MS, "Type `bob` into **User**"),
                // Keys before the late secure focus join the password run.
                (
                    StepKind::Type,
                    5500 * MS,
                    "Type `••••••••` into **Password** and press **Return**"
                ),
                // Auto-type: Return ended the run before the late secure focus.
                (StepKind::Type, 7005 * MS, "Type `b` into **Login**"),
                (
                    StepKind::Type,
                    7020 * MS,
                    "Type `••••••••` into **Passcode** and press **Return**"
                ),
                // A focus loss ended the run before the late secure focus.
                (StepKind::Type, 8010 * MS, "Type `a` into **Login**"),
                (
                    StepKind::Type,
                    8030 * MS,
                    "Type `••••••••` into **Passcode**"
                ),
                // P6: normal login with Tab; the username stays plain.
                (StepKind::Type, 9100 * MS, "Type `bob` into **Account**"),
                (
                    StepKind::Type,
                    9500 * MS,
                    "Type `••••••••` into **Passcode** and press **Return**"
                ),
                // P7: a click into the password field; the username stays plain.
                (StepKind::Type, 10100 * MS, "Type `bob` into **Account**"),
                (
                    StepKind::Click,
                    10400 * MS,
                    "Click **Passcode** in Keychain Access"
                ),
                // P4: a focus loss between password keys does not split the run.
                (StepKind::Type, 11005 * MS, "Type `b` into **Account**"),
                (
                    StepKind::Type,
                    11030 * MS,
                    "Type `••••••••` into **Passcode** and press **Return**"
                ),
            ]
        );
        for i in [0, 3, 5, 7, 9, 13] {
            assert_eq!(
                steps[i].keystrokes.as_deref(),
                Some(SECURE_MASK),
                "step {i}"
            );
        }
        assert_eq!(steps[3].point, Some(Point { x: 60.0, y: 110.0 }));
        assert_eq!(steps[5].point, Some(Point { x: 60.0, y: 170.0 }));
        assert_eq!(
            steps[7].element.as_ref().and_then(RawElement::best_label),
            Some("Passcode")
        );
        let json = serde_json::to_string(&steps).unwrap();
        // "´" is what the option dead key in the PIN field produced.
        for secret in ["hunter2", "hunt", "4321", "´", "zq9", "zq", "\"z", "!"] {
            assert!(!json.contains(secret), "{secret} leaked: {json}");
        }
        for step in &steps {
            let keys = step.keystrokes.as_deref().unwrap_or_default();
            assert!(!keys.contains(['z', 'q', 's']), "leaked: {keys}");
        }
    }

    #[test]
    fn click_before_secure_focus_keeps_previous_run() {
        // Typing, then a click into the password field: the focus change is
        // explained by the click, so the earlier run is not masked.
        let mut events = trace("secure");
        events.retain(|ev| ev.ts.0 < 7120 * MS);
        let click = |ts, up| TimedEvent {
            ts: Timestamp(ts * MS),
            event: if up {
                CaptureEvent::MouseUp {
                    point: Point { x: 60.0, y: 170.0 },
                    button: MouseButton::Left,
                    click_count: 1,
                    modifiers: Modifiers::default(),
                }
            } else {
                CaptureEvent::MouseDown {
                    point: Point { x: 60.0, y: 170.0 },
                    button: MouseButton::Left,
                    click_count: 1,
                    modifiers: Modifiers::default(),
                    element: None,
                }
            },
        };
        let mut secure_focus = trace("secure");
        secure_focus.retain(|ev| ev.ts.0 == 7120 * MS);
        events.extend([click(7080, false), click(7100, true)]);
        events.extend(secure_focus);
        let steps = detect_steps(&events, &mac());
        let typed = &steps[steps.len() - 2];
        assert_eq!(typed.keystrokes.as_deref(), Some("zq"));
        assert_eq!(steps.last().unwrap().kind, StepKind::Click);
    }

    #[test]
    fn late_secure_focus_outside_settle_window_is_not_masked() {
        let mut events = trace("secure");
        // Move the secure focus change well after the last key.
        for ev in &mut events {
            if ev.ts.0 == 5800 * MS {
                ev.ts = Timestamp(5850 * MS);
            }
        }
        let cfg = StepConfig {
            focus_settle_ms: 100,
            ..mac()
        };
        let steps = detect_steps(&events, &cfg);
        assert_eq!(steps[3].keystrokes.as_deref(), Some("zq9"));
        assert_eq!(steps[4].keystrokes.as_deref(), Some(SECURE_MASK));
    }

    #[test]
    fn edge_cases() {
        let steps = detect("edges");
        assert_eq!(
            summary(&steps),
            vec![
                (StepKind::Click, 1000 * MS, "Click in Finder"),
                (StepKind::Click, 1200 * MS, "Click in Finder"),
                (StepKind::Drag, 2000 * MS, "Drag in Finder"),
                (StepKind::Type, 3200 * MS, "Type `b` into **Note**"),
                (StepKind::Shortcut, 5200 * MS, "Press **Return**"),
                (StepKind::Type, 6100 * MS, "Type `a@bxy` into **Email**"),
                (StepKind::RightClick, 7000 * MS, "Right-click in Finder"),
                (StepKind::DoubleClick, 8000 * MS, "Triple-click in Finder"),
                (StepKind::Click, 9000 * MS, "Click in Finder"),
            ]
        );
        assert_eq!(steps[2].point, Some(Point { x: 100.0, y: 100.0 }));
        assert_eq!(steps[2].end_point, Some(Point { x: 300.0, y: 100.0 }));
        assert_eq!(steps[4].keystrokes.as_deref(), Some("↩"));
        assert_eq!(steps[5].keystrokes.as_deref(), Some("a@bxy"));
    }

    #[test]
    fn events_are_sorted_first() {
        let events = trace("typing");
        let mut shuffled = events.clone();
        shuffled.reverse();
        let cfg = mac();
        assert_eq!(detect_steps(&shuffled, &cfg), detect_steps(&events, &cfg));
    }

    #[test]
    fn app_names_are_trimmed() {
        let point = Point { x: 1.0, y: 1.0 };
        let at = |ts, event| TimedEvent {
            ts: Timestamp(ts),
            event,
        };
        let events = [
            at(
                0,
                CaptureEvent::AppActivated {
                    app: AppRef {
                        pid: 1,
                        name: Some(" *Odd*  App_ ".into()),
                        bundle_id: None,
                    },
                },
            ),
            at(
                1,
                CaptureEvent::MouseDown {
                    point,
                    button: MouseButton::Left,
                    click_count: 1,
                    modifiers: Modifiers::default(),
                    element: None,
                },
            ),
            at(
                2,
                CaptureEvent::MouseUp {
                    point,
                    button: MouseButton::Left,
                    click_count: 1,
                    modifiers: Modifiers::default(),
                },
            ),
        ];
        let steps = detect_steps(&events, &mac());
        assert_eq!(steps[0].title, "Click in \\*Odd\\* App\\_");
        assert_eq!(markdown::normalize(&steps[0].title), steps[0].title);
    }

    #[test]
    fn shortcuts() {
        let steps = detect("shortcuts");
        assert_eq!(
            summary(&steps),
            vec![
                (StepKind::Type, 1000 * MS, "Type `Hi` in TextEdit"),
                (StepKind::Shortcut, 2000 * MS, "Press **⌘S**"),
                (StepKind::Shortcut, 3000 * MS, "Press **⌃⌥⇧⌘↩**"),
                (StepKind::Shortcut, 4000 * MS, "Press **⌘F5**"),
                (StepKind::Shortcut, 5000 * MS, "Press **Esc**"),
            ]
        );
        assert_eq!(steps[1].keystrokes.as_deref(), Some("⌘S"));
    }

    #[test]
    fn drag_scroll_and_manual() {
        let steps = detect("drag");
        assert_eq!(
            summary(&steps),
            vec![
                (StepKind::Drag, 1000 * MS, "Drag **Report.pdf** in Finder"),
                (StepKind::Click, 3000 * MS, "Click in Finder"),
                (StepKind::Manual, 5000 * MS, "Manual step"),
            ]
        );
        assert_eq!(steps[0].point, Some(Point { x: 100.0, y: 100.0 }));
        assert_eq!(steps[0].end_point, Some(Point { x: 400.0, y: 250.0 }));
        // Jitter within the threshold is still a click.
        assert_eq!(steps[1].end_point, None);
    }

    #[test]
    fn into_step_maps_points_and_element() {
        let step = detect("drag").remove(0);
        let image = StepImage {
            full: "steps/a.webp".into(),
            crop: None,
            crop_rect: None,
            after: None,
            show: Default::default(),
        };
        let id = Ulid::new();
        let step = step
            .map_points(|p| Point {
                x: p.x * 2.0,
                y: p.y * 2.0,
            })
            .into_step(id, image.clone());
        assert_eq!(step.id, id);
        assert_eq!(step.kind, StepKind::Drag);
        assert_eq!(step.point, Some(Point { x: 200.0, y: 200.0 }));
        assert_eq!(step.end_point, Some(Point { x: 800.0, y: 500.0 }));
        let element = step.element.unwrap();
        assert_eq!(element.label.as_deref(), Some("Report.pdf"));
        assert_eq!(element.app.as_deref(), Some("Finder"));
        assert_eq!(step.image, image);
        assert_eq!(step.ai_state, AiState::default());

        let manual = detect("drag").remove(2).into_step(id, image);
        let element = manual.element.unwrap();
        assert_eq!(element.app.as_deref(), Some("Finder"));
        assert_eq!(element.label, None);
    }

    #[test]
    fn idle_threshold_is_configurable() {
        let events = trace("typing");
        let cfg = StepConfig {
            typing_idle_ms: 60_000,
            ..mac()
        };
        let steps = detect_steps(&events, &cfg);
        assert!(steps
            .iter()
            .any(|s| s.keystrokes.as_deref() == Some("eu-westhello world")));
    }

    #[test]
    fn empty_trace() {
        assert!(detect_steps(&[], &mac()).is_empty());
    }

    #[test]
    fn live_titles_read_like_step_titles() {
        let down = |button, click_count, modifiers| CaptureEvent::MouseDown {
            point: Point { x: 1.0, y: 2.0 },
            button,
            click_count,
            modifiers,
            element: None,
        };
        let cmd = Modifiers {
            command: true,
            ..Modifiers::default()
        };
        let ctrl = Modifiers {
            control: true,
            ..Modifiers::default()
        };
        let click = of(&down(MouseButton::Left, 1, Modifiers::default())).unwrap();
        assert_eq!(
            click.title(Some("Submit report"), Some("Safari")),
            "Click **Submit report** in Safari"
        );
        assert_eq!(click.title(Some("  "), Some("Safari")), "Click in Safari");
        assert_eq!(click.title(None, None), "Click");
        let right = of(&down(MouseButton::Left, 1, ctrl)).unwrap();
        assert_eq!(right.title(Some("File"), None), "Right-click **File**");
        let double = of(&down(MouseButton::Left, 2, cmd)).unwrap();
        assert_eq!(
            double.title(None, Some("Finder")),
            "⌘-double-click in Finder"
        );

        let key = |code, characters: &str, modifiers| CaptureEvent::KeyDown {
            key: Key {
                code,
                characters: Some(characters.into()),
                characters_ignoring_modifiers: Some(characters.into()),
            },
            modifiers,
            is_repeat: false,
            secure: false,
        };
        let save = of(&key(1, "s", cmd)).unwrap();
        assert_eq!(save.title(None, Some("TextEdit")), "Press **⌘S**");
        assert_eq!(
            of(&key(36, "\r", Modifiers::default()))
                .unwrap()
                .title(None, None),
            "Press **Return**"
        );
        // Typed text never shows: the field may turn out to be secure.
        let typed = of(&key(0, "p", Modifiers::default())).unwrap();
        assert_eq!(typed, LiveAction::Type);
        assert_eq!(typed.title(None, Some("Notes")), "Type in Notes");
        assert_eq!(
            typed.title(Some("Name"), Some("Notes")),
            "Type into **Name**"
        );
        let option = Modifiers {
            option: true,
            ..Modifiers::default()
        };
        assert_eq!(of(&key(37, "@", option)), Some(LiveAction::Type));
        assert_eq!(
            of(&CaptureEvent::ManualStep)
                .unwrap()
                .title(Some("x"), Some("y")),
            "Manual step"
        );
        assert_eq!(
            of(&CaptureEvent::FlagsChanged {
                modifiers: Modifiers::default()
            }),
            None
        );
        for title in [
            click.title(Some("*a* [b]"), Some("C_d")),
            plain_title("Click  **Submit**\n now"),
        ] {
            assert_eq!(markdown::normalize(&title), title);
        }
        assert_eq!(
            markdown::to_plain_text(&plain_title("Click **Submit**")),
            "Click **Submit**"
        );
    }

    #[test]
    fn windows_keys_and_clicks_read_as_windows_writes_them() {
        let win = StepConfig {
            keys: KeyPlatform::Windows,
            ..StepConfig::default()
        };
        let ctrl = Modifiers {
            control: true,
            ..Modifiers::default()
        };
        let key = |ms: u64, code: u16, chars: &str, base: &str, modifiers| TimedEvent {
            ts: Timestamp(ms * MS),
            event: CaptureEvent::KeyDown {
                key: Key {
                    code,
                    characters: Some(chars.into()),
                    characters_ignoring_modifiers: Some(base.into()),
                },
                modifiers,
                is_repeat: false,
                secure: false,
            },
        };
        let press = |ms: u64, modifiers| {
            let point = Point { x: 5.0, y: 5.0 };
            [
                TimedEvent {
                    ts: Timestamp(ms * MS),
                    event: CaptureEvent::MouseDown {
                        point,
                        button: MouseButton::Left,
                        click_count: 1,
                        modifiers,
                        element: None,
                    },
                },
                TimedEvent {
                    ts: Timestamp((ms + 50) * MS),
                    event: CaptureEvent::MouseUp {
                        point,
                        button: MouseButton::Left,
                        click_count: 1,
                        modifiers,
                    },
                },
            ]
        };
        let mut events = vec![
            key(0, 0x48, "h", "h", Modifiers::default()),
            key(100, 0x49, "i", "i", Modifiers::default()),
            // VK_RETURN: 36 would be VK_HOME.
            key(200, 0x0D, "\r", "\r", Modifiers::default()),
            // Ctrl types a control character; the label reads the base key.
            key(3000, 0x53, "\u{13}", "s", ctrl),
        ];
        events.extend(press(5000, ctrl));
        let steps = detect_steps(&events, &win);
        assert_eq!(
            summary(&steps),
            vec![
                (StepKind::Type, 0, "Type `hi` and press **Enter**"),
                (StepKind::Shortcut, 3000 * MS, "Press **Ctrl+S**"),
                // Control does not make a right click on Windows.
                (StepKind::Click, 5000 * MS, "Ctrl+click"),
            ]
        );
        assert_eq!(
            LiveAction::of_on(&events[3].event, KeyPlatform::Windows),
            Some(LiveAction::Press {
                label: "Ctrl+S".into()
            })
        );
        assert_eq!(
            LiveAction::of_on(&events[2].event, KeyPlatform::Windows)
                .unwrap()
                .title(None, None),
            "Press **Enter**"
        );
    }

    #[test]
    fn markdown_helpers() {
        assert_eq!(bold("a*b_c"), "**a\\*b\\_c**");
        assert_eq!(code_span("plain"), "`plain`");
        assert_eq!(code_span("``x"), "``` ``x ```");
        let label = r"C# [draft] <b>_x_</b> *a* \ ~y~ !z & | `q`";
        let title = format!("Click {} in {}", bold(label), escape(label));
        assert_eq!(markdown::normalize(&title), title);
        assert_eq!(
            markdown::to_plain_text(&title),
            format!("Click {label} in {label}")
        );
    }
}
