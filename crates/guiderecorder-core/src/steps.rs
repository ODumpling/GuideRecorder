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
//! - scrolling is summed until the next step and merged into it (its
//!   screenshot shows the scrolled view), except that a click or drag on
//!   something the scrolling brought into view gets a "Scroll to …" step
//!   of its own first (see `# Scrolling`);
//! - [`CaptureEvent::ManualStep`] produces a manual step (the recording
//!   session drops the hotkey's own key press, so it is not filtered here);
//! - a web address change without a click may produce a "Go to …" step
//!   (see `# Web addresses`).
//!
//! # Scrolling
//!
//! Scroll events carry how far the content moved and, from the first event
//! of a burst, the frame of the scrolled area under the pointer. A click
//! (or the start of a drag) after scrolling is on something that was not
//! visible before when its point is inside that area but its point moved
//! back by the scrolling is not: then a [`StepKind::Manual`] step "Scroll
//! to **Label**" (or "Scroll down in App" without a label) comes first,
//! at the same time, point and element as the click, so it shows the
//! scrolled view with the target in it. Scrolling without a known area
//! (older traces, a failed lookup), outside the clicked area, or followed
//! by anything else, makes no step. Scrolling in another area starts the
//! sum again.
//!
//! # Web addresses
//!
//! [`CaptureEvent::UrlChanged`] reports the frontmost browser window's
//! address; every step keeps the one in effect when it started
//! ([`DraftStep::url`]). When it changes from one page to another:
//!
//! - after a click or drag (a link, a button), a shortcut or Return (back,
//!   reload, a form), or typing without Return: no step;
//! - after typing an address (see [`web::looks_like_address`]) and Return,
//!   that typing step becomes "Go to [address](url)";
//! - with no step since the previous change: a redirect if it came within
//!   [`StepConfig::url_settle_ms`] of it (no step), else a [`StepKind::Manual`]
//!   step "Go to [address](url)".
//!
//! The first address seen in a browser (recording started there, or the
//! browser came to the front) is not a change. Private windows and
//! excluded apps report no address (the recording session sees to that).
//!
//! # Typed text
//!
//! When the recording session reads the field's text
//! ([`CaptureEvent::FieldValue`]) during a run, the run's text is what that
//! value added to the value read before the run (after a click into the
//! field, or at the end of the previous run in it): the common start and
//! end are cut off. Without a value from before, a single-line field's
//! whole value is taken. This gets text composed by an input method
//! (Japanese, Chinese, Korean) and accents from dead keys right, which key
//! presses alone do not. Otherwise (no value, a value longer than
//! [`MAX_FIELD_VALUE_CHARS`], nothing added) the text is rebuilt from the
//! key presses. A secure field's value is never read, and a run in one is
//! masked whatever was read.
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
//! Limitations: without a field value (see `# Typed text`), typed text is
//! rebuilt from key presses. Cursor movement is then ignored (typing `a`,
//! ←, `b` records "ab"), Backspace only removes text typed in the same run,
//! input methods record the raw keystrokes rather than the composed text,
//! and dead keys without option (e.g. US-International `'`) lose the
//! accent. The Return that commits a composition reads as "press Return"
//! either way. Option dead keys (⌥E then `e` for "é") may split a run:
//! the dead key reads as a "Press **⌥E**" shortcut if it reports no
//! characters, or as a separate "´" if it reports the accent.

use serde::{Deserialize, Serialize};
use ulid::Ulid;

use crate::events::{
    AppRef, CaptureEvent, Key, Modifiers, MouseButton, RawElement, TimedEvent, Timestamp,
};
use crate::keyboard::{is_printable_char, KeyPlatform, NamedKey};
use crate::markdown;
use crate::model::{AiState, Point, Rect, Step, StepImage, StepKind};
use crate::web;

/// Replaces anything typed into a secure field. Fixed length, so the
/// length of the secret is not revealed either.
pub const SECURE_MASK: &str = "••••••••";

/// Longest field value, in characters, that typed text is taken from (see
/// `# Typed text`); a longer one (a document) is not used.
pub const MAX_FIELD_VALUE_CHARS: usize = 2000;

/// Roles of fields that hold one line, whose whole value can be taken as
/// typed when there is no value from before the run.
const SINGLE_LINE_ROLES: &[&str] = &["AXTextField", "AXComboBox", "AXSearchField"];

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
    /// A web address change this soon after the previous one, with no step
    /// in between, is a redirect: it adds no "Go to" step.
    pub url_settle_ms: u64,
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
            url_settle_ms: 3000,
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
    /// Web address of the browser page the step started on; for a "Go to"
    /// step, the page it goes to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
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
            url: self.url,
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
        url: None,
        nav: None,
        scroll: None,
        field_value: None,
        last_submitted: None,
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
    /// The frontmost browser window's web address, as last reported.
    url: Option<String>,
    /// The last change of `url` from one page to another (or to the first
    /// page seen), for the "Go to" rule.
    nav: Option<Nav>,
    /// Scrolling since the last step.
    scroll: Option<Scroll>,
    /// The focused field's text, as last read outside a typing run: the
    /// value before the next run.
    field_value: Option<String>,
    /// The newest step if it is typing ended by Return.
    last_submitted: Option<usize>,
}

struct LastTyped {
    index: usize,
    last: Timestamp,
    submitted: bool,
}

/// A change of web address.
struct Nav {
    at: Timestamp,
    /// How many steps there were then.
    steps: usize,
}

/// Scrolling summed since the last step (see `# Scrolling`).
#[derive(Debug, Clone, Copy)]
struct Scroll {
    /// How far the content moved, in points.
    dx: f64,
    dy: f64,
    /// The scrolled area.
    region: Option<Rect>,
}

struct Typing {
    at: Timestamp,
    last: Timestamp,
    element: Option<RawElement>,
    text: String,
    secure: bool,
    /// Ended by Return, to be mentioned in the title.
    submitted: bool,
    url: Option<String>,
    /// The field's text before the run and as last read during it.
    before: Option<String>,
    value: Option<String>,
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
    url: Option<String>,
    /// Scrolling just before the press.
    scroll: Option<Scroll>,
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
                    url: self.url.clone(),
                    scroll: self.scroll.take(),
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
            CaptureEvent::Scroll {
                delta_x,
                delta_y,
                region,
                ..
            } => {
                // Scrolling somewhere else: what came before is not about
                // the next click.
                if let (Some(scroll), Some(region)) = (&self.scroll, region) {
                    if scroll.region.is_some_and(|r| r != *region) {
                        self.scroll = None;
                    }
                }
                let scroll = self.scroll.get_or_insert(Scroll {
                    dx: 0.0,
                    dy: 0.0,
                    region: None,
                });
                scroll.dx += delta_x;
                scroll.dy += delta_y;
                scroll.region = scroll.region.or(*region);
            }
            CaptureEvent::FlagsChanged { .. } => {}
            CaptureEvent::KeyDown {
                key,
                modifiers,
                is_repeat,
                secure,
            } => {
                // Scrolling before typing or a shortcut is merged into it.
                self.scroll = None;
                self.key_down(ts, key, *modifiers, *is_repeat, *secure);
            }
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
                // A value read before belongs to the field focused then.
                if element.is_some() && self.focus != *element {
                    self.field_value = None;
                }
                self.focus = element.clone();
            }
            CaptureEvent::AppActivated { app } => {
                if self.app.as_ref() != Some(app) {
                    self.end_run();
                    self.scroll = None;
                    self.field_value = None;
                }
                self.app = Some(app.clone());
            }
            CaptureEvent::UrlChanged { url } => self.url_changed(ts, url.clone()),
            CaptureEvent::FieldValue { value } => match &mut self.typing {
                // An empty read (a submitted form already cleared, a scrub)
                // keeps the last one.
                Some(typing) if !typing.secure && !value.is_empty() => {
                    typing.value = Some(value.clone());
                }
                Some(_) => {}
                None => self.field_value = Some(value.clone()),
            },
            CaptureEvent::ManualStep => {
                self.scroll = None;
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
            url: self.url.clone(),
            title: String::new(),
        }
    }

    /// Applies a change of web address (see `# Web addresses`).
    fn url_changed(&mut self, ts: Timestamp, url: Option<String>) {
        let previous = std::mem::replace(&mut self.url, url.clone());
        let Some(url) = url else {
            // Left the browser, or a private window: what comes back is a
            // first page again.
            self.nav = None;
            return;
        };
        if previous.as_ref() == Some(&url) {
            return;
        }
        self.scroll = None;
        let nav = Nav {
            at: ts,
            steps: self.steps.len(),
        };
        let Some(last) = self.nav.replace(nav).filter(|_| previous.is_some()) else {
            return;
        };
        if self.steps.len() <= last.steps {
            // Nothing done since the last change: a redirect, or the page
            // moved on by itself (or by something not recorded).
            if ts.millis_since(last.at) > self.cfg.url_settle_ms {
                let mut step = self.draft(StepKind::Manual, ts);
                step.title = go_to_title(&url);
                self.steps.push(step);
                if let Some(nav) = &mut self.nav {
                    nav.steps = self.steps.len();
                }
            }
            return;
        }
        // An address typed and submitted: that is where the step goes. A
        // click, a shortcut, a Return or other typing explains the change
        // by itself.
        let newest = self.steps.len() - 1;
        let step = &mut self.steps[newest];
        let typed_address = step.kind == StepKind::Type
            && self.last_submitted == Some(newest)
            && step
                .keystrokes
                .as_deref()
                .is_some_and(|keys| keys != SECURE_MASK && web::looks_like_address(keys));
        if typed_address {
            step.title = go_to_title(&url);
            step.url = Some(url);
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
            url: self.url.clone(),
            before: self.field_value.take(),
            value: None,
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
        step.url = typing.url;
        let has_text = !typing.text.is_empty();
        let shown = if typing.secure {
            self.field_value = None;
            SECURE_MASK.to_owned()
        } else {
            // The value is also what the field holds before the next run.
            self.field_value = typing.value.clone();
            typing
                .value
                .as_deref()
                .and_then(|after| {
                    typed_value(typing.before.as_deref(), after, typing.element.as_ref())
                })
                .unwrap_or(typing.text)
        };
        step.element = typing.element;
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
        if typing.submitted {
            self.last_submitted = Some(self.steps.len() - 1);
        }
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
        self.field_value = None;
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
        if let Some(scroll) = press
            .scroll
            .filter(|scroll| scroll.revealed(press.point) && press.click_count < 2)
        {
            let mut step = self.draft(StepKind::Manual, press.at);
            step.point = Some(press.point);
            step.element = element.clone();
            step.url = press.url.clone();
            step.title = scroll_title(&step, scroll);
            self.steps.push(step);
        }
        if press.dragged {
            let mut step = self.draft(StepKind::Drag, press.at);
            step.url = press.url;
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
        step.url = press.url;
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

impl Scroll {
    /// Whether a press at `point`, after this scrolling, is on something it
    /// brought into view: `point` is in the scrolled area, but was not
    /// before the content moved (see `# Scrolling`).
    fn revealed(&self, point: Point) -> bool {
        let Some(region) = self.region else {
            return false;
        };
        let before = Point {
            x: point.x - self.dx,
            y: point.y - self.dy,
        };
        contains(region, point) && !contains(region, before)
    }

    /// "down", "up", "right" or "left": where the view went, along the axis
    /// it moved most. Content moving up means scrolling down.
    fn direction(&self) -> &'static str {
        if self.dy.abs() >= self.dx.abs() {
            if self.dy <= 0.0 {
                "down"
            } else {
                "up"
            }
        } else if self.dx <= 0.0 {
            "right"
        } else {
            "left"
        }
    }
}

/// "Scroll to **Label** in App", or "Scroll down in App" without a label.
fn scroll_title(step: &DraftStep, scroll: Scroll) -> String {
    match (step.label(), step.app_name()) {
        (Some(label), Some(app)) => format!("Scroll to {} in {}", bold(label), escape(app)),
        (Some(label), None) => format!("Scroll to {}", bold(label)),
        (None, Some(app)) => format!("Scroll {} in {}", scroll.direction(), escape(app)),
        (None, None) => format!("Scroll {}", scroll.direction()),
    }
}

/// "Go to [example.com/settings](https://example.com/settings)", in
/// normalized guide Markdown.
fn go_to_title(url: &str) -> String {
    markdown::normalize(&format!(
        "Go to [{}](<{}>)",
        escape(&web::display_url(url)),
        url.replace(['<', '>'], "")
    ))
}

/// The text a typing run typed, from the field's value: what `after` (read
/// during the run) added to `before` (read before it), or with nothing from
/// before, all of a single-line field's `after`. `None` when the value
/// cannot say (too long, nothing added, a multi-line field without a value
/// from before).
fn typed_value(before: Option<&str>, after: &str, element: Option<&RawElement>) -> Option<String> {
    if after.chars().count() > MAX_FIELD_VALUE_CHARS {
        return None;
    }
    let added = match before {
        Some(before) => inserted_text(before, after),
        None => {
            let single_line = element.is_some_and(|el| {
                [&el.role, &el.subrole]
                    .into_iter()
                    .flatten()
                    .any(|role| SINGLE_LINE_ROLES.contains(&role.as_str()))
            });
            if !single_line {
                return None;
            }
            after.to_owned()
        }
    };
    let added = printable(&added);
    (!added.trim().is_empty()).then_some(added)
}

/// What `after` has that `before` does not, between their common start
/// and end: the text typed (or pasted) over any selection.
fn inserted_text(before: &str, after: &str) -> String {
    let before: Vec<char> = before.chars().collect();
    let after: Vec<char> = after.chars().collect();
    let start = before
        .iter()
        .zip(&after)
        .take_while(|(a, b)| a == b)
        .count();
    let end = before[start..]
        .iter()
        .rev()
        .zip(after[start..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    after[start..after.len() - end].iter().collect()
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

    fn at(ms: u64, event: CaptureEvent) -> TimedEvent {
        TimedEvent {
            ts: Timestamp(ms * MS),
            event,
        }
    }

    fn browser(ms: u64) -> TimedEvent {
        at(
            ms,
            CaptureEvent::AppActivated {
                app: AppRef {
                    pid: 7,
                    name: Some("Safari".into()),
                    bundle_id: Some("com.apple.Safari".into()),
                },
            },
        )
    }

    /// A left click at `ms` on `element` (its frame around the point).
    fn click_at(ms: u64, point: Point, label: Option<&str>) -> [TimedEvent; 2] {
        let element = label.map(|label| RawElement {
            role: Some("AXButton".into()),
            title: Some(label.into()),
            frame: Some(Rect {
                x: point.x - 40.0,
                y: point.y - 10.0,
                width: 80.0,
                height: 20.0,
            }),
            ..RawElement::default()
        });
        [
            at(
                ms,
                CaptureEvent::MouseDown {
                    point,
                    button: MouseButton::Left,
                    click_count: 1,
                    modifiers: Modifiers::default(),
                    element,
                },
            ),
            at(
                ms + 50,
                CaptureEvent::MouseUp {
                    point,
                    button: MouseButton::Left,
                    click_count: 1,
                    modifiers: Modifiers::default(),
                },
            ),
        ]
    }

    fn scroll(ms: u64, delta_y: f64, region: Option<Rect>) -> TimedEvent {
        at(
            ms,
            CaptureEvent::Scroll {
                point: Point { x: 400.0, y: 400.0 },
                delta_x: 0.0,
                delta_y,
                region,
            },
        )
    }

    /// A key typing `chars` (none for a dead key), with Mac key codes.
    fn key_at(ms: u64, code: u16, chars: Option<&str>, modifiers: Modifiers) -> TimedEvent {
        at(
            ms,
            CaptureEvent::KeyDown {
                key: Key {
                    code,
                    characters: chars.map(Into::into),
                    characters_ignoring_modifiers: chars.map(Into::into),
                },
                modifiers,
                is_repeat: false,
                secure: false,
            },
        )
    }

    /// Keys typing `text` from `ms`, 100 ms apart.
    fn type_at(ms: u64, text: &str) -> Vec<TimedEvent> {
        text.chars()
            .zip(0..)
            .map(|(c, i)| key_at(ms + i * 100, 0, Some(&c.to_string()), Modifiers::default()))
            .collect()
    }

    const RETURN: u16 = 36;

    fn url_at(ms: u64, url: Option<&str>) -> TimedEvent {
        at(
            ms,
            CaptureEvent::UrlChanged {
                url: url.map(Into::into),
            },
        )
    }

    fn value_at(ms: u64, value: &str) -> TimedEvent {
        at(
            ms,
            CaptureEvent::FieldValue {
                value: value.into(),
            },
        )
    }

    fn focus_at(ms: u64, label: &str, role: &str, secure: bool) -> TimedEvent {
        at(
            ms,
            CaptureEvent::FocusChanged {
                element: Some(RawElement {
                    role: Some(role.into()),
                    subrole: secure.then(|| "AXSecureTextField".into()),
                    title: Some(label.into()),
                    is_secure: secure,
                    ..RawElement::default()
                }),
            },
        )
    }

    fn detect_events(events: Vec<TimedEvent>) -> Vec<DraftStep> {
        let steps = detect_steps(&events, &mac());
        for step in &steps {
            assert_eq!(markdown::normalize(&step.title), step.title);
        }
        steps
    }

    #[test]
    fn scrolling_to_a_click_target_is_a_step() {
        // The page: y 100 to 700.
        let page = Some(Rect {
            x: 0.0,
            y: 100.0,
            width: 800.0,
            height: 600.0,
        });
        let target = Point { x: 400.0, y: 300.0 };
        let mut events = vec![browser(0)];
        // 600 points down: the button was at y 900, below the page.
        events.extend([scroll(1000, -300.0, page), scroll(1100, -300.0, None)]);
        events.extend(click_at(2000, target, Some("Delete account")));
        // A little scrolling: it was in view already.
        events.push(scroll(3000, -50.0, page));
        events.extend(click_at(3500, target, Some("Confirm")));
        // Scrolling, then typing: merged into the typing.
        events.push(scroll(4000, -600.0, page));
        events.extend(type_at(4500, "hi"));
        events.extend(click_at(6000, target, Some("Send")));
        // No known area (an older trace): merged.
        events.push(scroll(7000, -2000.0, None));
        events.extend(click_at(7500, target, Some("Next")));
        // A click outside the scrolled area (a toolbar): merged.
        events.push(scroll(8000, -600.0, page));
        events.extend(click_at(8500, Point { x: 400.0, y: 50.0 }, Some("Back")));
        // Scrolled in a sidebar, then in the page: only the page's counts.
        let sidebar = Some(Rect {
            x: 0.0,
            y: 100.0,
            width: 100.0,
            height: 600.0,
        });
        events.push(scroll(9000, -5000.0, sidebar));
        events.push(scroll(9100, -100.0, page));
        events.extend(click_at(9500, target, Some("Save")));
        // Up, with nothing to name.
        events.push(scroll(10000, 800.0, page));
        events.extend(click_at(10500, target, None));

        let steps = detect_events(events);
        assert_eq!(
            summary(&steps),
            vec![
                (
                    StepKind::Manual,
                    2000 * MS,
                    "Scroll to **Delete account** in Safari"
                ),
                (
                    StepKind::Click,
                    2000 * MS,
                    "Click **Delete account** in Safari"
                ),
                (StepKind::Click, 3500 * MS, "Click **Confirm** in Safari"),
                (StepKind::Type, 4500 * MS, "Type `hi` in Safari"),
                (StepKind::Click, 6000 * MS, "Click **Send** in Safari"),
                (StepKind::Click, 7500 * MS, "Click **Next** in Safari"),
                (StepKind::Click, 8500 * MS, "Click **Back** in Safari"),
                (StepKind::Click, 9500 * MS, "Click **Save** in Safari"),
                (StepKind::Manual, 10500 * MS, "Scroll up in Safari"),
                (StepKind::Click, 10500 * MS, "Click in Safari"),
            ]
        );
        // The scroll step points at what it scrolls to, as its click does.
        assert_eq!(steps[0].point, Some(target));
        assert_eq!(
            steps[0].element.as_ref().and_then(RawElement::best_label),
            Some("Delete account")
        );
        assert_eq!(steps[0].keystrokes, None);
    }

    #[test]
    fn web_addresses_and_go_to_steps() {
        let link = Point { x: 10.0, y: 10.0 };
        let mut events = vec![browser(0)];
        // The page the recording starts on: no step.
        events.push(url_at(100, Some("https://example.com/")));
        events.extend(click_at(1000, link, Some("Docs")));
        // A link: the click explains it.
        events.push(url_at(1500, Some("https://example.com/docs")));
        events.extend(click_at(2000, link, Some("Search")));
        // An address typed into the address bar and submitted.
        events.push(focus_at(
            2900,
            "Address and search bar",
            "AXTextField",
            false,
        ));
        events.extend(type_at(3000, "github.com"));
        events.push(key_at(4100, RETURN, Some("\r"), Modifiers::default()));
        events.push(url_at(4300, Some("https://github.com/")));
        // A redirect soon after, with nothing done since: no step.
        events.push(url_at(4800, Some("https://github.com/login")));
        // Later, with nothing done since: the page moved on.
        events.push(url_at(15_000, Some("https://github.com/dashboard")));
        // A search typed into the address bar stays typing.
        events.extend(type_at(20_000, "rust"));
        events.push(key_at(20_500, RETURN, Some("\r"), Modifiers::default()));
        events.push(url_at(21_000, Some("https://duckduckgo.com/?q=rust")));
        // ⌘[ goes back: the shortcut explains it.
        let cmd = Modifiers {
            command: true,
            ..Modifiers::default()
        };
        events.push(key_at(25_000, 33, Some("["), cmd));
        events.push(url_at(25_500, Some("https://github.com/dashboard")));
        // A private window reports no address; the next one is a first page.
        events.push(url_at(30_000, None));
        events.extend(click_at(30_500, link, Some("New")));
        events.push(url_at(40_000, Some("https://example.org/")));
        events.extend(click_at(41_000, link, Some("More")));

        let steps = detect_events(events);
        assert_eq!(
            summary(&steps),
            vec![
                (StepKind::Click, 1000 * MS, "Click **Docs** in Safari"),
                (StepKind::Click, 2000 * MS, "Click **Search** in Safari"),
                (
                    StepKind::Type,
                    3000 * MS,
                    "Go to [github.com](https://github.com/)"
                ),
                (
                    StepKind::Manual,
                    15_000 * MS,
                    "Go to [github.com/dashboard](https://github.com/dashboard)"
                ),
                (
                    StepKind::Type,
                    20_000 * MS,
                    "Type `rust` into **Address and search bar** and press **Return**"
                ),
                (StepKind::Shortcut, 25_000 * MS, "Press **⌘\\[**"),
                (StepKind::Click, 30_500 * MS, "Click **New** in Safari"),
                (StepKind::Click, 41_000 * MS, "Click **More** in Safari"),
            ]
        );
        let urls: Vec<Option<&str>> = steps.iter().map(|s| s.url.as_deref()).collect();
        assert_eq!(
            urls,
            [
                Some("https://example.com/"),
                Some("https://example.com/docs"),
                Some("https://github.com/"),
                Some("https://github.com/dashboard"),
                Some("https://github.com/dashboard"),
                Some("https://duckduckgo.com/?q=rust"),
                None,
                Some("https://example.org/"),
            ]
        );
        // The typed address is kept as typed.
        assert_eq!(steps[2].keystrokes.as_deref(), Some("github.com"));
        let step = steps[3].clone().into_step(
            Ulid::new(),
            StepImage {
                full: String::new(),
                crop: None,
                crop_rect: None,
                after: None,
                show: Default::default(),
            },
        );
        assert_eq!(step.url.as_deref(), Some("https://github.com/dashboard"));
    }

    #[test]
    fn typed_text_comes_from_the_field_value() {
        let mut events = vec![browser(0)];
        // Japanese input: romaji keys, composed into kanji. The click into
        // the empty field read its value.
        events.push(focus_at(500, "Name", "AXTextField", false));
        events.push(value_at(600, ""));
        events.extend(type_at(1000, "nihongo"));
        events.push(value_at(2200, "日本語"));
        // A dead key (no characters) then "e": the accent is in the value.
        events.push(focus_at(5000, "City", "AXTextField", false));
        events.extend(type_at(5100, "Caf"));
        events.push(key_at(5400, 39, None, Modifiers::default()));
        events.extend(type_at(5500, "e"));
        // Read just before Return ended the run.
        events.push(value_at(5899, "Café"));
        events.push(key_at(5900, RETURN, Some("\r"), Modifiers::default()));
        // Text added to what was there: only the new text.
        events.push(focus_at(8000, "Title", "AXTextField", false));
        events.push(value_at(8100, "Invoice 2023"));
        events.push(key_at(8500, 51, Some("\u{7f}"), Modifiers::default()));
        events.extend(type_at(8600, "4-Q1"));
        events.push(value_at(9500, "Invoice 2024-Q1"));
        // A second run in the same field: what the first left is before.
        events.extend(type_at(12_000, "!"));
        events.push(value_at(12_500, "Invoice 2024-Q1!"));
        // A multi-line field without a value from before: the keys.
        events.push(focus_at(15_000, "Notes", "AXTextArea", false));
        events.extend(type_at(15_100, "ok"));
        events.push(value_at(15_600, "Long notes\nok"));
        // A secure field: never the value.
        events.push(focus_at(18_000, "Password", "AXTextField", true));
        events.extend(type_at(18_100, "ab"));
        events.push(value_at(18_600, "hunter2"));

        let steps = detect_events(events);
        let shown: Vec<(&str, &str)> = steps
            .iter()
            .map(|s| {
                (
                    s.title.as_str(),
                    s.keystrokes.as_deref().unwrap_or_default(),
                )
            })
            .collect();
        assert_eq!(
            shown,
            [
                ("Type `日本語` into **Name**", "日本語"),
                ("Type `Café` into **City** and press **Return**", "Café"),
                ("Type `4-Q1` into **Title**", "4-Q1"),
                ("Type `!` into **Title**", "!"),
                ("Type `ok` into **Notes**", "ok"),
                ("Type `••••••••` into **Password**", SECURE_MASK),
            ]
        );
        assert!(!serde_json::to_string(&steps).unwrap().contains("hunter2"));
    }

    #[test]
    fn field_values_that_cannot_say_fall_back_to_keys() {
        let el = RawElement {
            role: Some("AXTextField".into()),
            ..RawElement::default()
        };
        let area = RawElement {
            role: Some("AXTextArea".into()),
            ..RawElement::default()
        };
        assert_eq!(typed_value(None, "abc", Some(&el)).as_deref(), Some("abc"));
        assert_eq!(typed_value(None, "abc", Some(&area)), None);
        assert_eq!(typed_value(None, "abc", None), None);
        assert_eq!(typed_value(Some("abc"), "abc", Some(&el)), None);
        assert_eq!(typed_value(Some("abc"), "ab", Some(&el)), None);
        assert_eq!(typed_value(Some(""), "  ", Some(&el)), None);
        let long = "x".repeat(MAX_FIELD_VALUE_CHARS + 1);
        assert_eq!(typed_value(Some(""), &long, Some(&el)), None);
        assert_eq!(inserted_text("Hello world", "Hello big world"), "big ");
        assert_eq!(inserted_text("aa", "aaa"), "a");
        assert_eq!(inserted_text("", "日本語"), "日本語");
        assert_eq!(inserted_text("abc", "xyz"), "xyz");
        assert_eq!(
            go_to_title("https://example.com/a_b"),
            "Go to [example.com/a\\_b](https://example.com/a_b)"
        );
        let title = go_to_title("https://en.wikipedia.org/wiki/Rust_(language)?a=<b>");
        assert_eq!(markdown::normalize(&title), title);
        assert_eq!(
            markdown::to_plain_text(&title),
            "Go to en.wikipedia.org/wiki/Rust_(language)?a=<b>"
        );
    }
}
