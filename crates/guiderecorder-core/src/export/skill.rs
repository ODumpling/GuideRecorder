//! Agent skill writer over [`RenderGuide`]: a folder an AI agent can use
//! to repeat the recorded task.
//!
//! # Layout
//!
//! ```text
//! SKILL.md              the steps as instructions (Agent Skills format)
//! steps.json            the steps as data, with element locators and keys
//! replay.js             optional: a macOS replay script (JXA), or
//! replay.ps1            for a guide recorded on Windows, a PowerShell one
//! images/NN-slug.png    each step's image, as the Markdown export writes it
//! ```
//!
//! # SKILL.md
//!
//! YAML front matter with `name` ([`skill_name`]: the title as lowercase
//! hyphenated ASCII, at most 64 characters) and `description` (what the
//! guide does and when to use it, at most 1024 characters, no angle
//! brackets), then Markdown: the purpose (summary and intro), the
//! prerequisites (the guide's, plus the apps the steps use), the steps
//! with their element, keys and image, notes and sections in place, the
//! expected result and the files next to it. Guide text is escaped as the
//! [Markdown writer](super::md) escapes it; the fixed words are English
//! whatever the guide language, since the reader is an agent.
//!
//! # steps.json
//!
//! [`SkillSteps`], camelCase like `guide.json`: per step its number, kind,
//! plain-text title, Markdown description, section, image, element locator
//! (app, bundle id, window, role, label), click point (and drag end) in
//! full-screenshot pixels, keystrokes as recorded plus parsed [`Keys`], what
//! the author said during the step (narration, when recorded), and the
//! element [`confidence`](crate::confidence). Guides store no element
//! frame, so the point is the only geometry. SKILL.md shows the narration
//! too, as context for the step.
//!
//! # replay.js
//!
//! A JavaScript for Automation script ([`replay_script`]) with the steps
//! embedded, run with `osascript -l JavaScript replay.js`. It finds each
//! element through System Events (app, then window, then an element of
//! the recorded role whose title, description or value is the label) and
//! presses it or types into it; `--dry-run` only reports what it would do
//! and whether each element is there now. It stops at the first step it
//! can't do safely: an unclear or missing element, more than one match, a
//! masked password, a manual step, a double click or a drag. macOS only.
//!
//! # replay.ps1
//!
//! The Windows counterpart ([`replay_script`] for a guide [recorded
//! on](recorded_on) Windows), a Windows PowerShell 5.1 script (PowerShell 7
//! runs it too) run with `powershell -ExecutionPolicy Bypass -File
//! replay.ps1 -DryRun`. It finds each element through UI Automation (the
//! app's processes by executable name, then the window by title, then a
//! descendant whose control type is one the recorder reports as the
//! recorded AX role and whose name, help text, label or value is the
//! label), invokes, toggles, selects or expands it, and types and presses
//! keys with `SendInput`. It has the same options (`-DryRun`, `-From`,
//! `-To`, `-Delay`) and stops at the same steps, and also at a window it
//! can't bring to the front. The steps are embedded in a single-quoted
//! here-string as one line of ASCII JSON (every other character and `'`
//! escaped), so no title can end it; the file is written with a UTF-8 byte
//! order mark, which Windows PowerShell needs to read a script as UTF-8.
//!
//! Images have blur and pixelate burned in like every export. Output is
//! deterministic: the same guide and images give the same bytes.

use std::fs;
use std::path::Path;

use serde::Serialize;

use super::ast::{plain_text, Block};
use super::md::{
    blocks, code_span, decision_text, escape_line, heading_text, image_names, inlines_single_line,
    io_err, prepare_out_dir, quote, remove_stale_images, single_line, slug, Labels, Result,
};
use super::{render_step_image, ExportReport, ImageOptions, RenderGuide, RenderItem, RenderStep};
use crate::confidence::{describe_issues, element_issues, needs_element, ElementIssue};
use crate::library::{write_atomic, Durability, GuideHandle};
use crate::model::{ElementInfo, Point, StepKind};
use crate::steps::SECURE_MASK;

/// File name of the instructions inside the output folder.
pub const SKILL_FILE: &str = "SKILL.md";
/// File name of the structured steps.
pub const STEPS_FILE: &str = "steps.json";
/// File name of the macOS replay script, for every guide not recorded on
/// Windows.
pub const REPLAY_FILE: &str = "replay.js";
/// File name of the Windows replay script, for guides recorded there.
pub const REPLAY_WINDOWS_FILE: &str = "replay.ps1";
/// Folder (inside the output folder) the step images are written to.
pub const IMAGES_DIR: &str = super::md::IMAGES_DIR;
/// `steps.json`'s `format`.
pub const STEPS_FORMAT: &str = "guiderecorder.skill-steps";
/// `steps.json`'s `version`; bumped when a field changes meaning.
pub const STEPS_VERSION: u32 = 1;
/// What `replay.ps1` starts with.
const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";

/// Longest skill name the Agent Skills format allows.
pub const MAX_NAME_CHARS: usize = 64;
/// Longest skill description the Agent Skills format allows.
pub const MAX_DESCRIPTION_CHARS: usize = 1024;

/// The replay script around its data: `__GUIDERECORDER_STEPS__` is
/// replaced by `steps.json` as one line.
const REPLAY_TEMPLATE: &str = include_str!("templates/replay.js");
/// The Windows replay script, with the same placeholder.
const REPLAY_WINDOWS_TEMPLATE: &str = include_str!("templates/replay.ps1");
const REPLAY_PLACEHOLDER: &str = "__GUIDERECORDER_STEPS__";

/// Options for [`export`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SkillOptions {
    /// How step images are rendered. The default keeps full resolution.
    pub image: ImageOptions,
    /// Write into a non-empty `out_dir`, replacing the files and images of
    /// an earlier export.
    pub overwrite: bool,
    /// Also write the replay script ([`replay_file`]).
    pub replay: bool,
}

// ---------------------------------------------------------------------------
// steps.json

/// The contents of `steps.json`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillSteps {
    /// Always [`STEPS_FORMAT`].
    pub format: &'static str,
    /// Always [`STEPS_VERSION`].
    pub version: u32,
    pub guide: SkillGuide,
    pub steps: Vec<SkillStep>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillGuide {
    pub id: String,
    /// The skill's `name` ([`skill_name`]).
    pub name: String,
    /// Plain text.
    pub title: String,
    /// Plain text.
    pub summary: String,
    pub language: String,
    /// Where the guide was recorded, as far as its steps tell
    /// ([`recorded_on`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recorded_on: Option<Platform>,
}

/// The platform a guide was recorded on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Platform {
    Macos,
    Windows,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillStep {
    /// 1-based, as in every export.
    pub number: u32,
    pub id: String,
    pub kind: StepKind,
    /// Plain text.
    pub title: String,
    /// Markdown, as stored.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// What the author said during the step while recording, as
    /// transcribed; plain text.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub narration: String,
    /// The title of the section the step is in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    /// The step image, relative to the skill folder.
    pub image: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub element: Option<ElementInfo>,
    /// Click point (or drag start) in full-screenshot pixels.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub point: Option<Point>,
    /// Where a drag ends, in full-screenshot pixels.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_point: Option<Point>,
    /// As recorded, e.g. `⌘S` or the typed text; passwords stay masked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keystrokes: Option<String>,
    /// The keystrokes parsed ([`parse_keys`]), if they could be.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keys: Option<Keys>,
    /// The web page the step was on, when recorded in a browser.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub confidence: Confidence,
}

/// How clearly a step's element is identified (see
/// [`crate::confidence`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Confidence {
    pub level: ConfidenceLevel,
    /// Why it is low; empty otherwise.
    pub issues: Vec<ElementIssue>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ConfidenceLevel {
    /// The element can be found by app, window, role and label.
    High,
    /// Something is missing or vague: see the issues.
    Low,
    /// The step acts on no element (a shortcut or a manual step).
    NotNeeded,
}

impl Confidence {
    fn of(step: &RenderStep) -> Self {
        let issues = element_issues(&step.step);
        let level = if !needs_element(step.step.kind) {
            ConfidenceLevel::NotNeeded
        } else if issues.is_empty() {
            ConfidenceLevel::High
        } else {
            ConfidenceLevel::Low
        };
        Self { level, issues }
    }
}

/// A step's keystrokes, parsed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Keys {
    /// Text typed into the element; `submit`: Return was pressed after it.
    Text { text: String, submit: bool },
    /// Typed into a password field and masked when recorded, so it is not
    /// known.
    Secret { submit: bool },
    /// One key, with the modifiers held. `key` is a lowercase character or
    /// a name: `return`, `tab`, `space`, `backspace`, `escape`,
    /// `forwardDelete`, `left`, `right`, `up`, `down`, `home`, `end`,
    /// `pageUp`, `pageDown`, `f1`…`f24`.
    Shortcut {
        modifiers: Vec<Modifier>,
        key: String,
    },
}

/// A modifier key. On Windows, Ctrl is `control`, Alt is `option` and the
/// Windows key is `command`, as the recorder names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Modifier {
    Control,
    Option,
    Shift,
    Command,
}

/// Return as the typing step records it after the text.
const SUBMIT_MARKS: [char; 2] = ['⏎', '↩'];

/// `keystrokes` of a step of `kind`, parsed: typed text (with a trailing
/// Return mark taken as `submit`), a masked password, or a shortcut in
/// either platform's notation (`⌃⌥⇧⌘S`, `Ctrl+Shift+S`). `None` for other
/// kinds and for labels it can't read (e.g. `Key 255`).
pub fn parse_keys(kind: StepKind, keystrokes: &str) -> Option<Keys> {
    match kind {
        StepKind::Type => {
            let (text, submit) = match keystrokes.strip_suffix(SUBMIT_MARKS) {
                Some(text) => (text, true),
                None => (keystrokes, false),
            };
            if text.is_empty() {
                return submit.then(|| Keys::Shortcut {
                    modifiers: Vec::new(),
                    key: "return".into(),
                });
            }
            if text == SECURE_MASK || text.chars().all(|c| c == '•') {
                return Some(Keys::Secret { submit });
            }
            Some(Keys::Text {
                text: text.to_owned(),
                submit,
            })
        }
        StepKind::Shortcut => parse_shortcut(keystrokes.trim()),
        _ => None,
    }
}

/// A shortcut label in macOS (`⌘S`) or Windows (`Ctrl+S`) notation.
fn parse_shortcut(label: &str) -> Option<Keys> {
    let mac_key = label.trim_start_matches(['⌃', '⌥', '⇧', '⌘']);
    let mac_mods = &label[..label.len() - mac_key.len()];
    if !mac_mods.is_empty() || !label.contains('+') || label == "+" {
        let modifiers = [
            ('⌃', Modifier::Control),
            ('⌥', Modifier::Option),
            ('⇧', Modifier::Shift),
            ('⌘', Modifier::Command),
        ]
        .into_iter()
        .filter(|(symbol, _)| mac_mods.contains(*symbol))
        .map(|(_, modifier)| modifier)
        .collect();
        return Some(Keys::Shortcut {
            modifiers,
            key: key_name(mac_key)?,
        });
    }
    // `Ctrl++` holds Ctrl and presses `+`.
    let (mods, key) = match label.strip_suffix("++") {
        Some(mods) => (mods, "+"),
        None => label.rsplit_once('+')?,
    };
    let mut modifiers = Vec::new();
    for name in mods.split('+') {
        let modifier = match name.to_ascii_lowercase().as_str() {
            "ctrl" => Modifier::Control,
            "alt" => Modifier::Option,
            "shift" => Modifier::Shift,
            "win" => Modifier::Command,
            _ => return None,
        };
        modifiers.push(modifier);
    }
    // In the order the replay holds them, whatever the label's.
    modifiers.sort_by_key(|m| *m as u8);
    modifiers.dedup();
    Some(Keys::Shortcut {
        modifiers,
        key: key_name(key)?,
    })
}

/// The [`Keys::Shortcut`] name of a key as a label shows it.
fn key_name(key: &str) -> Option<String> {
    let name = match key {
        "↩" | "⏎" | "Enter" | "Return" => "return",
        "⇥" | "Tab" => "tab",
        "Space" => "space",
        "⌫" | "Backspace" => "backspace",
        "⎋" | "Esc" | "Escape" => "escape",
        "⌦" | "Delete" | "Del" => "forwardDelete",
        "←" => "left",
        "→" => "right",
        "↑" => "up",
        "↓" => "down",
        "Home" => "home",
        "End" => "end",
        "Page Up" => "pageUp",
        "Page Down" => "pageDown",
        _ => {
            if let Some(n) = key
                .strip_prefix('F')
                .and_then(|n| n.parse::<u8>().ok())
                .filter(|n| (1..=24).contains(n))
            {
                return Some(format!("f{n}"));
            }
            let mut chars = key.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else {
                return None;
            };
            if c.is_control() || c.is_whitespace() {
                return None;
            }
            return Some(c.to_lowercase().collect());
        }
    };
    Some(name.to_owned())
}

/// Where `guide` was recorded, from its steps: Windows if an element's
/// bundle id is an executable (`.exe`) or a shortcut uses `Ctrl+`/`Alt+`
/// notation, macOS if a bundle id or a shortcut says so, else unknown.
pub fn recorded_on(guide: &RenderGuide) -> Option<Platform> {
    let mut mac = false;
    for step in guide.steps() {
        let step = &step.step;
        if let Some(id) = step.element.as_ref().and_then(|e| e.bundle_id.as_deref()) {
            if id.to_ascii_lowercase().ends_with(".exe") {
                return Some(Platform::Windows);
            }
            mac |= !id.trim().is_empty();
        }
        if step.kind == StepKind::Shortcut {
            let keys = step.keystrokes.as_deref().unwrap_or_default();
            if ["Ctrl+", "Alt+", "Win+"].iter().any(|m| keys.contains(m)) {
                return Some(Platform::Windows);
            }
            mac |= keys.contains(['⌃', '⌥', '⇧', '⌘']);
        }
    }
    mac.then_some(Platform::Macos)
}

/// The skill `name` for a guide title: lowercase ASCII words joined by
/// `-` ([`slug`]), without the words the format reserves (`anthropic`,
/// `claude`), at most [`MAX_NAME_CHARS`]; `guide` if nothing is left.
pub fn skill_name(title: &str) -> String {
    let slug = slug(title);
    // `slug` falls back to "step" when nothing is left.
    if slug == "step" && !title.to_lowercase().contains("step") {
        return "guide".into();
    }
    let mut name = String::new();
    for word in slug
        .split('-')
        .filter(|w| !w.is_empty() && *w != "anthropic" && *w != "claude")
    {
        if name.len() + word.len() + usize::from(!name.is_empty()) > MAX_NAME_CHARS {
            break;
        }
        if !name.is_empty() {
            name.push('-');
        }
        name.push_str(word);
    }
    if name.is_empty() {
        name.push_str("guide");
    }
    name
}

/// `steps.json` for `guide`, with image `i` at `images/<names[i]>`.
pub fn steps(guide: &RenderGuide, image_names: &[String]) -> SkillSteps {
    let mut section = None;
    let mut names = image_names.iter();
    let mut steps = Vec::new();
    for item in &guide.items {
        match item {
            RenderItem::Section(s) => {
                section = Some(single_line(&s.title)).filter(|t| !t.is_empty())
            }
            RenderItem::Note(_) | RenderItem::Decision(_) => {}
            RenderItem::Step(step) => {
                let stored = &step.step;
                steps.push(SkillStep {
                    number: step.number,
                    id: stored.id.to_string(),
                    kind: stored.kind,
                    title: single_line(&plain_text(&step.title)),
                    description: stored.description.trim().to_owned(),
                    narration: single_line(&stored.narration),
                    section: section.clone(),
                    image: names
                        .next()
                        .map(|name| format!("{IMAGES_DIR}/{name}"))
                        .unwrap_or_default(),
                    element: stored.element.clone(),
                    point: stored.point,
                    end_point: stored.end_point,
                    keystrokes: stored.keystrokes.clone(),
                    keys: stored
                        .keystrokes
                        .as_deref()
                        .and_then(|k| parse_keys(stored.kind, k)),
                    url: stored.url.clone(),
                    confidence: Confidence::of(step),
                });
            }
        }
    }
    SkillSteps {
        format: STEPS_FORMAT,
        version: STEPS_VERSION,
        guide: SkillGuide {
            id: guide.id.to_string(),
            name: skill_name(&guide.title),
            title: single_line(&guide.title),
            summary: single_line(&guide.summary),
            language: guide.language.clone(),
            recorded_on: recorded_on(guide),
        },
        steps,
    }
}

/// `steps` as `steps.json`: pretty-printed, with a final newline.
pub fn steps_json(steps: &SkillSteps) -> String {
    let mut json = serde_json::to_string_pretty(steps).expect("steps serialize");
    json.push('\n');
    json
}

/// The replay script's file name for steps recorded on `platform`:
/// [`REPLAY_WINDOWS_FILE`] for Windows, else [`REPLAY_FILE`].
pub fn replay_file(platform: Option<Platform>) -> &'static str {
    match platform {
        Some(Platform::Windows) => REPLAY_WINDOWS_FILE,
        Some(Platform::Macos) | None => REPLAY_FILE,
    }
}

/// A replay script and the file name it goes by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayScript {
    /// [`REPLAY_FILE`] or [`REPLAY_WINDOWS_FILE`].
    pub file: &'static str,
    /// The script, without the byte order mark [`export`] writes before
    /// `replay.ps1`.
    pub script: String,
}

/// The replay script for `steps` (see the module docs and the script's
/// own header): `replay.ps1` for steps recorded on Windows, else
/// `replay.js`. The steps are embedded as one line of JSON; for
/// JavaScript with U+2028 and U+2029 escaped so no engine reads them as
/// line ends, for PowerShell as ASCII (see [`ascii_json`]).
pub fn replay_script(steps: &SkillSteps) -> ReplayScript {
    let json = serde_json::to_string(steps).expect("steps serialize");
    let file = replay_file(steps.guide.recorded_on);
    let script = if file == REPLAY_WINDOWS_FILE {
        REPLAY_WINDOWS_TEMPLATE.replacen(REPLAY_PLACEHOLDER, &ascii_json(&json), 1)
    } else {
        let json = json
            .replace('\u{2028}', "\\u2028")
            .replace('\u{2029}', "\\u2029");
        REPLAY_TEMPLATE.replacen(REPLAY_PLACEHOLDER, &json, 1)
    };
    ReplayScript { file, script }
}

/// One line of JSON with every non-ASCII character and `'` written as a
/// `\uXXXX` escape (both only occur in strings), so it is safe inside a
/// PowerShell single-quoted here-string, which only a line starting with
/// `'@` ends, whatever encoding the script is read in.
fn ascii_json(json: &str) -> String {
    let mut out = String::with_capacity(json.len());
    for c in json.chars() {
        if c.is_ascii() && c != '\'' && c != '\n' && c != '\r' {
            out.push(c);
        } else {
            for unit in c.encode_utf16(&mut [0; 2]) {
                out.push_str(&format!("\\u{unit:04x}"));
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// SKILL.md

/// `SKILL.md` for `guide`, with image `i` at `images/<names[i]>`; `replay`:
/// whether the replay script ([`replay_file`]) is next to it.
pub fn skill_md(guide: &RenderGuide, image_names: &[String], replay: bool) -> String {
    let data = steps(guide, image_names);
    let labels = Labels::for_language("en");
    let title = single_line(&guide.title);
    let mut out: Vec<String> = Vec::new();

    out.push(format!(
        "---\nname: {}\ndescription: {}\n---",
        data.guide.name,
        yaml_string(&description(guide, &data))
    ));
    if !title.is_empty() {
        out.push(format!("# {}", heading_text(&escape_line(&title))));
    }

    out.push("## Purpose".into());
    let summary = single_line(&guide.summary);
    if !summary.is_empty() {
        out.push(escape_line(&summary));
    }
    if !guide.intro.is_empty() {
        out.push(blocks(&guide.intro));
    }
    if summary.is_empty() && guide.intro.is_empty() {
        let task = if title.is_empty() {
            "the recorded task"
        } else {
            &title
        };
        out.push(escape_line(&format!("Carry out “{task}” as recorded.")));
    }

    let apps = apps(&data);
    if !guide.prerequisites.is_empty() || !apps.is_empty() {
        out.push(format!("## {}", labels.prerequisites));
        if !guide.prerequisites.is_empty() {
            out.push(blocks(&guide.prerequisites));
        }
        if !apps.is_empty() {
            out.push(escape_line(&format!("The steps use: {}.", apps.join(", "))));
        }
    }

    out.push("## Steps".into());
    if data.steps.is_empty() {
        out.push("This guide has no steps.".into());
    } else {
        out.push(
            "Do the steps in order. Each names the element to act on (its role, label, window \
             and app) and the keys to press; check the screen against the step's image before \
             you act."
                .into(),
        );
        if guide.decisions().next().is_some() {
            out.push(
                "At a decision, answer its question from what is on the screen and go where \
                 the matching option says; the other steps are then skipped or repeated."
                    .into(),
            );
        }
    }
    let has_sections = guide
        .items
        .iter()
        .any(|item| matches!(item, RenderItem::Section(_)));
    let step_heading = if has_sections { "####" } else { "###" };
    let mut data_steps = data.steps.iter();
    for item in &guide.items {
        match item {
            RenderItem::Section(section) => {
                let title = escape_line(&single_line(&section.title));
                out.push(format!("### {}", heading_text(&title)));
                if let Some(audiences) = labels.audiences(&section.audiences) {
                    out.push(format!("*{}*", escape_line(&audiences)));
                }
            }
            RenderItem::Decision(decision) => {
                out.push(quote(&decision_text(decision, &labels, guide.kind)));
            }
            RenderItem::Step(step) => {
                let Some(data) = data_steps.next() else {
                    continue;
                };
                let title = inlines_single_line(&step.title);
                let title = if title.is_empty() {
                    escape_line(labels.step_kind(guide.kind))
                } else {
                    title
                };
                out.push(format!(
                    "{step_heading} {}. {}",
                    step.number,
                    heading_text(&title)
                ));
                out.push(step_facts(data));
                if !step.description.is_empty() {
                    out.push(blocks(&step.description));
                }
                if !data.image.is_empty() {
                    let alt = escape_line(&format!("Step {}", step.number));
                    out.push(format!("![{alt}]({})", data.image));
                }
                if data.confidence.level == ConfidenceLevel::Low {
                    out.push(quote(&format!(
                        "**Unclear element:** {}. Find it from the image and the description, \
                         not by its label.",
                        escape_line(&capitalize(&describe_issues(&data.confidence.issues)))
                    )));
                }
            }
            RenderItem::Note(note) => {
                let label = format!("**{}:**", labels.note_kind(note.kind));
                let body = blocks(&note.body);
                let text = match note.body.first() {
                    None => label,
                    // A list can't follow the label on its line.
                    Some(Block::List { .. }) => format!("{label}\n\n{body}"),
                    Some(Block::Paragraph(_)) => format!("{label} {body}"),
                };
                out.push(quote(&text));
            }
        }
    }

    out.push("## Expected result".into());
    match data.steps.last() {
        Some(last) => {
            let task = if title.is_empty() {
                "the task".to_owned()
            } else {
                format!("“{title}”")
            };
            let mut text = escape_line(&format!(
                "When every step is done, {task} is complete. The last step was “{}”",
                if last.title.is_empty() {
                    "untitled"
                } else {
                    &last.title
                }
            ));
            if last.image.is_empty() {
                text.push('.');
            } else {
                text.push_str(&format!(
                    "; its image, `{}`, shows the screen then.",
                    last.image
                ));
            }
            out.push(text);
        }
        None => out.push("Nothing to do: the guide has no steps.".into()),
    }

    let unclear: Vec<u32> = data
        .steps
        .iter()
        .filter(|s| s.confidence.level == ConfidenceLevel::Low)
        .map(|s| s.number)
        .collect();
    if !unclear.is_empty() {
        out.push("## Unclear elements".into());
        out.push(format!(
            "The element of {} was not identified clearly when it was recorded (see \
             `confidence` in `{STEPS_FILE}`). Rely on the images there, and expect the replay \
             script to stop at {}.",
            step_list(&unclear),
            if unclear.len() == 1 {
                "it"
            } else {
                "the first"
            }
        ));
    }

    out.push("## Files".into());
    let mut files = vec![
        format!(
            "- `{STEPS_FILE}`: the steps as data, with element locators (app, window, role, \
             label), click points in screenshot pixels, keys and the confidence of each element."
        ),
        format!("- `{IMAGES_DIR}/`: an image of each step, with sensitive areas blurred."),
    ];
    if replay {
        let script = match replay_file(data.guide.recorded_on) {
            REPLAY_WINDOWS_FILE => format!(
                "- `{REPLAY_WINDOWS_FILE}`: a Windows PowerShell script that repeats the steps \
                 through UI Automation. Run `powershell -ExecutionPolicy Bypass -File \
                 {REPLAY_WINDOWS_FILE} -DryRun` first:"
            ),
            _ => format!(
                "- `{REPLAY_FILE}`: a macOS script that repeats the steps through the \
                 accessibility API. Run `osascript -l JavaScript {REPLAY_FILE} --dry-run` first:"
            ),
        };
        files.push(format!(
            "{script} it only reports what it would do and whether each element is there. The \
             script stops at a step it can't do safely."
        ));
    }
    out.push(files.join("\n"));

    let mut doc = out.join("\n\n");
    doc.push('\n');
    doc
}

/// The front matter `description`: the purpose and when to use the skill,
/// on one line, without angle brackets (no XML tags), at most
/// [`MAX_DESCRIPTION_CHARS`].
fn description(guide: &RenderGuide, data: &SkillSteps) -> String {
    let title = single_line(&guide.title);
    let summary = single_line(&guide.summary);
    let intro = guide
        .intro
        .first()
        .map(|block| match block {
            Block::Paragraph(inlines) => single_line(&plain_text(inlines)),
            Block::List { .. } => String::new(),
        })
        .unwrap_or_default();
    let purpose = [summary, intro, title.clone()]
        .into_iter()
        .find(|s| !s.is_empty())
        .unwrap_or_else(|| "A recorded task.".into());
    let mut text = purpose;
    if !text.ends_with(['.', '!', '?']) {
        text.push('.');
    }
    let count = data.steps.len();
    let apps = apps(data);
    let task = if title.is_empty() {
        "the task this guide records".to_owned()
    } else {
        format!("“{title}”")
    };
    text.push_str(&format!(
        " Use when asked to do {task}: {count} recorded step{}{}.",
        if count == 1 { "" } else { "s" },
        if apps.is_empty() {
            String::new()
        } else {
            format!(" in {}", apps.join(", "))
        }
    ));
    let text: String = text.chars().filter(|c| !matches!(c, '<' | '>')).collect();
    if text.chars().count() <= MAX_DESCRIPTION_CHARS {
        return text;
    }
    let mut cut: String = text.chars().take(MAX_DESCRIPTION_CHARS - 1).collect();
    cut.push('…');
    cut
}

/// `text` as a double-quoted YAML scalar.
fn yaml_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The apps the steps use, in order of first use: `Safari`, or the
/// bundle id when there is no name.
fn apps(data: &SkillSteps) -> Vec<String> {
    let mut apps: Vec<String> = Vec::new();
    for element in data.steps.iter().filter_map(|s| s.element.as_ref()) {
        let name = element
            .app
            .as_deref()
            .or(element.bundle_id.as_deref())
            .map(single_line)
            .filter(|n| !n.is_empty());
        if let Some(name) = name {
            if !apps.contains(&name) {
                apps.push(name);
            }
        }
    }
    apps
}

/// The list under a step's heading: what to act on and the keys.
fn step_facts(step: &SkillStep) -> String {
    let mut lines = vec![format!("- Action: {}", action(step.kind))];
    if let Some(element) = &step.element {
        let locator = locator(element);
        if !locator.is_empty() {
            lines.push(format!("- Element: {}", escape_line(&locator)));
        }
    }
    let keys = match (&step.keys, step.keystrokes.as_deref()) {
        (Some(Keys::Text { text, submit }), _) => Some(format!(
            "type {}{}",
            code_span(text),
            if *submit { ", then press Return" } else { "" }
        )),
        (Some(Keys::Secret { submit }), _) => Some(format!(
            "type the password (masked when recorded: ask the user for it){}",
            if *submit { ", then press Return" } else { "" }
        )),
        (Some(Keys::Shortcut { .. }), Some(raw)) | (None, Some(raw)) => {
            Some(format!("press {}", code_span(raw.trim())))
        }
        _ => None,
    };
    if let Some(keys) = keys.filter(|k| !k.ends_with("press ")) {
        lines.push(format!("- Keys: {keys}"));
    }
    if let Some(url) = &step.url {
        lines.push(format!("- Web page: {}", escape_line(url)));
    }
    if !step.narration.is_empty() {
        lines.push(format!(
            "- Narration (the author, while recording): {}",
            escape_line(&format!("“{}”", step.narration))
        ));
    }
    lines.join("\n")
}

fn action(kind: StepKind) -> &'static str {
    match kind {
        StepKind::Click => "click",
        StepKind::DoubleClick => "double-click",
        StepKind::RightClick => "right-click",
        StepKind::Drag => "drag",
        StepKind::Type => "type into a field",
        StepKind::Shortcut => "press keys",
        StepKind::Manual => "by hand (no recorded action)",
    }
}

/// `button “Save” in window “Untitled” of TextEdit (com.apple.TextEdit)`,
/// leaving out what is not known.
fn locator(element: &ElementInfo) -> String {
    let text = |v: &Option<String>| v.as_deref().map(single_line).filter(|v| !v.is_empty());
    let mut parts = Vec::new();
    let role = text(&element.role).map(|r| role_name(&r));
    match (role, text(&element.label)) {
        (Some(role), Some(label)) => parts.push(format!("{role} “{label}”")),
        (Some(role), None) => parts.push(format!("{role} (no label)")),
        (None, Some(label)) => parts.push(format!("“{label}”")),
        (None, None) => {}
    }
    if let Some(window) = text(&element.window) {
        parts.push(format!("in window “{window}”"));
    }
    match (text(&element.app), text(&element.bundle_id)) {
        (Some(app), Some(id)) => parts.push(format!("of {app} ({id})")),
        (Some(app), None) => parts.push(format!("of {app}")),
        (None, Some(id)) => parts.push(format!("of {id}")),
        (None, None) => {}
    }
    parts.join(" ")
}

/// An AX role as words: `AXPopUpButton` → `pop up button (AXPopUpButton)`.
fn role_name(role: &str) -> String {
    let Some(rest) = role.strip_prefix("AX").filter(|r| !r.is_empty()) else {
        return role.to_owned();
    };
    let mut words = String::new();
    for (i, c) in rest.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            words.push(' ');
        }
        words.extend(c.to_lowercase());
    }
    format!("{words} ({role})")
}

/// `[2, 5, 7]` as "steps 2, 5 and 7".
fn step_list(numbers: &[u32]) -> String {
    let n: Vec<String> = numbers.iter().map(u32::to_string).collect();
    match n.as_slice() {
        [one] => format!("step {one}"),
        [rest @ .., last] => format!("steps {} and {last}", rest.join(", ")),
        [] => String::new(),
    }
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

// ---------------------------------------------------------------------------
// Export

/// Writes `<out_dir>/SKILL.md`, `steps.json`, `images/NN-<slug>.png` and,
/// with `opts.replay`, the replay script (`replay.js`, or `replay.ps1` with
/// a UTF-8 byte order mark for a guide recorded on Windows), with step
/// images read from the bundle at `handle`. Steps whose element is unclear are listed in the report's
/// notes.
///
/// Refuses a non-empty `out_dir` unless `opts.overwrite`; with it, earlier
/// `NN-*.png` images this export did not write are removed, and so is an
/// earlier `replay.js` or `replay.ps1` this export did not write (it would
/// replay old steps). A broken step image becomes a placeholder plus a warning.
pub fn export(
    guide: &RenderGuide,
    handle: &GuideHandle,
    out_dir: impl AsRef<Path>,
    opts: &SkillOptions,
) -> Result<ExportReport> {
    let out_dir = out_dir.as_ref();
    prepare_out_dir(out_dir, opts.overwrite)?;
    let images_dir = out_dir.join(IMAGES_DIR);
    fs::create_dir_all(&images_dir).map_err(io_err(&images_dir))?;

    let names = image_names(guide);
    let mut report = ExportReport::default();
    let mut images = Vec::new();
    for (step, name) in guide.steps().zip(&names) {
        let image = render_step_image(handle, &step.step, &opts.image);
        report.add_warnings(step.step.id, image.warnings);
        let path = images_dir.join(name);
        write_atomic(&path, &image.png, Durability::RenameOnly)?;
        images.push(path);
    }

    let data = steps(guide, &names);
    let skill = out_dir.join(SKILL_FILE);
    write_atomic(
        &skill,
        skill_md(guide, &names, opts.replay).as_bytes(),
        Durability::RenameOnly,
    )?;
    report.files.push(skill);
    let steps_path = out_dir.join(STEPS_FILE);
    write_atomic(
        &steps_path,
        steps_json(&data).as_bytes(),
        Durability::RenameOnly,
    )?;
    report.files.push(steps_path);
    let script = opts.replay.then(|| replay_script(&data));
    if let Some(script) = &script {
        let path = out_dir.join(script.file);
        let mut bytes = Vec::with_capacity(script.script.len() + 3);
        if script.file == REPLAY_WINDOWS_FILE {
            // Windows PowerShell reads a script without one as ANSI.
            bytes.extend_from_slice(UTF8_BOM);
        }
        bytes.extend_from_slice(script.script.as_bytes());
        write_atomic(&path, &bytes, Durability::RenameOnly)?;
        make_executable(&path)?;
        report.files.push(path);
    }
    if opts.overwrite {
        // An earlier script would replay old steps.
        for file in [REPLAY_FILE, REPLAY_WINDOWS_FILE] {
            let stale = out_dir.join(file);
            if script.as_ref().is_none_or(|s| s.file != file) && stale.is_file() {
                fs::remove_file(&stale).map_err(io_err(&stale))?;
            }
        }
    }
    report.files.extend(images);
    report.notes.extend(unclear_notes(&data));

    if opts.overwrite {
        remove_stale_images(&images_dir, &names)?;
    }
    Ok(report)
}

/// "Step 3: its element is unclear (no label)" for every unclear step.
pub fn unclear_notes(data: &SkillSteps) -> Vec<String> {
    data.steps
        .iter()
        .filter(|s| s.confidence.level == ConfidenceLevel::Low)
        .map(|s| {
            format!(
                "Step {}: its element is unclear ({}), so agents and the replay script may not find it",
                s.number,
                describe_issues(&s.confidence.issues)
            )
        })
        .collect()
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).map_err(io_err(path))
}

#[cfg(not(unix))]
fn make_executable(_: &Path) -> Result<()> {
    Ok(())
}

/// A skill export in memory, for a preview.
#[derive(Debug, Clone)]
pub struct SkillPreview {
    /// `SKILL.md` as [`export`] writes it.
    pub skill_md: String,
    pub steps_json: String,
    /// The replay script, when asked for.
    pub replay: Option<ReplayScript>,
    /// The PNG images `SKILL.md` links, by their link (`images/<name>`).
    pub images: Vec<(String, Vec<u8>)>,
    /// The step image warnings and unclear-element notes; no files.
    pub report: ExportReport,
}

/// What [`export`] would write, without writing it, at `opts.image`'s
/// size.
pub fn preview(guide: &RenderGuide, handle: &GuideHandle, opts: &SkillOptions) -> SkillPreview {
    let names = image_names(guide);
    let mut report = ExportReport::default();
    let mut images = Vec::new();
    for (step, name) in guide.steps().zip(&names) {
        let image = render_step_image(handle, &step.step, &opts.image);
        report.add_warnings(step.step.id, image.warnings);
        images.push((format!("{IMAGES_DIR}/{name}"), image.png));
    }
    let data = steps(guide, &names);
    report.notes.extend(unclear_notes(&data));
    SkillPreview {
        skill_md: skill_md(guide, &names, opts.replay),
        steps_json: steps_json(&data),
        replay: opts.replay.then(|| replay_script(&data)),
        images,
        report,
    }
}

/// [`skill_md`], `steps.json` and (with `replay`) the replay script for
/// `guide`, without images: what an agent reads over MCP. The image links
/// are the export's (`images/NN-slug.png`).
pub fn render_text(guide: &RenderGuide, replay: bool) -> (String, String, Option<ReplayScript>) {
    let names = image_names(guide);
    let data = steps(guide, &names);
    (
        skill_md(guide, &names, replay),
        steps_json(&data),
        replay.then(|| replay_script(&data)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Guide, ImageShow, Item, Note, NoteKind, Section, Step, StepImage};
    use crate::Ulid;
    use serde_json::json;
    use std::path::PathBuf;

    fn element(role: &str, label: &str) -> ElementInfo {
        ElementInfo {
            role: Some(role.into()),
            label: Some(label.into()),
            app: Some("TextEdit".into()),
            bundle_id: Some("com.apple.TextEdit".into()),
            window: Some("Untitled".into()),
        }
    }

    fn step(title: &str, kind: StepKind, element: Option<ElementInfo>, keys: Option<&str>) -> Item {
        Item::Step(Step {
            id: Ulid::from_parts(1, title.len() as u128),
            title: title.into(),
            description: String::new(),
            kind,
            point: Some(Point { x: 10.0, y: 20.0 }),
            end_point: None,
            element,
            keystrokes: keys.map(Into::into),
            url: None,
            image: StepImage {
                full: "steps/x.webp".into(),
                crop: None,
                crop_rect: None,
                after: None,
                show: ImageShow::Full,
            },
            annotations: Vec::new(),
            narration: String::new(),
            ai_state: Default::default(),
        })
    }

    fn render(guide: &Guide) -> RenderGuide {
        let handle = GuideHandle {
            id: guide.id,
            path: PathBuf::from("/nonexistent.guiderecorder"),
        };
        RenderGuide::new(guide, &handle)
    }

    fn sample() -> Guide {
        let mut guide = Guide::new("Save a *note* in TextEdit", "en");
        guide.id = Ulid::from_parts(7, 7);
        guide.summary = "Write and save a note.".into();
        guide.prerequisites = "- TextEdit is open".into();
        guide.items = vec![
            Item::Section(Section {
                id: Ulid::from_parts(2, 1),
                title: "Write".into(),
                audiences: Vec::new(),
            }),
            step(
                "Type the text",
                StepKind::Type,
                Some(element("AXTextArea", "Body")),
                Some("Hello"),
            ),
            Item::Note(Note {
                id: Ulid::from_parts(2, 2),
                kind: NoteKind::Tip,
                body: "Any text works.".into(),
            }),
            step("Press **⌘S**", StepKind::Shortcut, None, Some("⌘S")),
            step(
                "Click in the sidebar",
                StepKind::Click,
                Some(ElementInfo {
                    label: None,
                    ..element("AXGroup", "")
                }),
                None,
            ),
            step("Check the file", StepKind::Manual, None, None),
        ];
        guide
    }

    fn names(guide: &RenderGuide) -> Vec<String> {
        image_names(guide)
    }

    #[test]
    fn web_steps_name_their_page() {
        let mut guide = sample();
        if let Item::Step(step) = &mut guide.items[3] {
            step.url = Some("https://example.com/notes".into());
        }
        let rendered = render(&guide);
        let data = steps(&rendered, &names(&rendered));
        let json = serde_json::to_value(&data).unwrap();
        assert_eq!(json["steps"][1]["url"], "https://example.com/notes");
        assert!(json["steps"][0].get("url").is_none());
        let md = skill_md(&rendered, &names(&rendered), false);
        assert!(md.contains("- Web page: https://example.com/notes"), "{md}");
    }

    #[test]
    fn names_skills_in_lowercase_hyphenated_ascii() {
        assert_eq!(skill_name("Rotate an API key"), "rotate-an-api-key");
        assert_eq!(skill_name("Drucker: Fach öffnen!"), "drucker-fach-oeffnen");
        assert_eq!(
            skill_name("Ask Claude via the Anthropic console"),
            "ask-via-the-console"
        );
        assert_eq!(skill_name("★★★"), "guide");
        assert_eq!(skill_name(""), "guide");
        assert_eq!(skill_name("Step"), "step");
        let long = skill_name(&"word ".repeat(40));
        assert!(
            long.len() <= MAX_NAME_CHARS && !long.ends_with('-'),
            "{long}"
        );
        for name in [skill_name("Ünïcödé — Tëst 42"), long] {
            assert!(name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'));
        }
    }

    #[test]
    fn parses_typed_text_secrets_and_shortcuts() {
        let shortcut = |mods: &[Modifier], key: &str| {
            Some(Keys::Shortcut {
                modifiers: mods.to_vec(),
                key: key.into(),
            })
        };
        use Modifier::*;
        assert_eq!(
            parse_keys(StepKind::Type, "alex@example.com⏎"),
            Some(Keys::Text {
                text: "alex@example.com".into(),
                submit: true
            })
        );
        assert_eq!(
            parse_keys(StepKind::Type, "a b"),
            Some(Keys::Text {
                text: "a b".into(),
                submit: false
            })
        );
        assert_eq!(
            parse_keys(StepKind::Type, "••••••••⏎"),
            Some(Keys::Secret { submit: true })
        );
        assert_eq!(
            parse_keys(StepKind::Type, SECURE_MASK),
            Some(Keys::Secret { submit: false })
        );
        assert_eq!(parse_keys(StepKind::Type, "⏎"), shortcut(&[], "return"));
        assert_eq!(
            parse_keys(StepKind::Shortcut, "⌘S"),
            shortcut(&[Command], "s")
        );
        assert_eq!(
            parse_keys(StepKind::Shortcut, "⌃⌥⇧⌘↩"),
            shortcut(&[Control, Option, Shift, Command], "return")
        );
        assert_eq!(
            parse_keys(StepKind::Shortcut, "⌘F5"),
            shortcut(&[Command], "f5")
        );
        assert_eq!(parse_keys(StepKind::Shortcut, "⎋"), shortcut(&[], "escape"));
        assert_eq!(
            parse_keys(StepKind::Shortcut, "⌘+"),
            shortcut(&[Command], "+")
        );
        assert_eq!(
            parse_keys(StepKind::Shortcut, "Ctrl+S"),
            shortcut(&[Control], "s")
        );
        assert_eq!(
            parse_keys(StepKind::Shortcut, "Win+Shift+Page Down"),
            shortcut(&[Shift, Command], "pageDown")
        );
        assert_eq!(
            parse_keys(StepKind::Shortcut, "Ctrl++"),
            shortcut(&[Control], "+")
        );
        assert_eq!(
            parse_keys(StepKind::Shortcut, "Enter"),
            shortcut(&[], "return")
        );
        assert_eq!(
            parse_keys(StepKind::Shortcut, "Esc"),
            shortcut(&[], "escape")
        );
        assert_eq!(parse_keys(StepKind::Shortcut, "Ctrl+Key 255"), None);
        assert_eq!(parse_keys(StepKind::Shortcut, "Hyper+S"), None);
        assert_eq!(parse_keys(StepKind::Click, "⌘S"), None);
    }

    #[test]
    fn steps_json_has_locators_keys_and_confidence() {
        let guide = render(&sample());
        let data = steps(&guide, &names(&guide));
        let value: serde_json::Value = serde_json::from_str(&steps_json(&data)).unwrap();
        assert_eq!(value["format"], STEPS_FORMAT);
        assert_eq!(value["version"], 1);
        assert_eq!(
            value["guide"],
            json!({
                "id": Ulid::from_parts(7, 7).to_string(),
                "name": "save-a-note-in-textedit",
                "title": "Save a *note* in TextEdit",
                "summary": "Write and save a note.",
                "language": "en",
                "recordedOn": "macos",
            })
        );
        let steps = value["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 4);
        assert_eq!(
            steps[0],
            json!({
                "number": 1,
                "id": Ulid::from_parts(1, 13).to_string(),
                "kind": "type",
                "title": "Type the text",
                "section": "Write",
                "image": "images/01-type-the-text.png",
                "element": {
                    "role": "AXTextArea",
                    "label": "Body",
                    "app": "TextEdit",
                    "bundleId": "com.apple.TextEdit",
                    "window": "Untitled",
                },
                "point": { "x": 10.0, "y": 20.0 },
                "keystrokes": "Hello",
                "keys": { "type": "text", "text": "Hello", "submit": false },
                "confidence": { "level": "high", "issues": [] },
            })
        );
        assert_eq!(steps[1]["title"], "Press ⌘S");
        assert_eq!(
            steps[1]["keys"],
            json!({ "type": "shortcut", "modifiers": ["command"], "key": "s" })
        );
        assert_eq!(steps[1]["confidence"]["level"], "notNeeded");
        assert!(steps[1].get("element").is_none());
        assert_eq!(
            steps[2]["confidence"],
            json!({ "level": "low", "issues": ["noLabel", "genericRole"] })
        );
        assert_eq!(steps[3]["kind"], "manual");
        assert_eq!(steps[3]["confidence"]["level"], "notNeeded");
        assert_eq!(steps[3]["section"], "Write");
    }

    #[test]
    fn recorded_on_reads_bundle_ids_and_shortcut_notation() {
        let mut guide = Guide::new("T", "en");
        assert_eq!(recorded_on(&render(&guide)), None);
        guide.items = vec![step("Save", StepKind::Shortcut, None, Some("Ctrl+S"))];
        assert_eq!(recorded_on(&render(&guide)), Some(Platform::Windows));
        let mut el = element("AXButton", "OK");
        el.bundle_id = Some("Notepad.EXE".into());
        guide.items = vec![step("OK", StepKind::Click, Some(el), None)];
        assert_eq!(recorded_on(&render(&guide)), Some(Platform::Windows));
        guide.items = vec![step("Save", StepKind::Shortcut, None, Some("⌘S"))];
        assert_eq!(recorded_on(&render(&guide)), Some(Platform::Macos));
    }

    #[test]
    fn skill_md_has_front_matter_purpose_steps_and_result() {
        let guide = render(&sample());
        let md = skill_md(&guide, &names(&guide), true);
        let expected_head = "---\n\
            name: save-a-note-in-textedit\n\
            description: \"Write and save a note. Use when asked to do “Save a *note* in \
            TextEdit”: 4 recorded steps in TextEdit.\"\n\
            ---\n\n\
            # Save a \\*note\\* in TextEdit\n\n\
            ## Purpose\n\n\
            Write and save a note.\n\n\
            ## Prerequisites\n\n\
            - TextEdit is open\n\n\
            The steps use: TextEdit.\n\n\
            ## Steps\n\n";
        assert!(md.starts_with(expected_head), "{md}");
        for part in [
            "### Write\n\n#### 1. Type the text\n\n\
             - Action: type into a field\n\
             - Element: text area (AXTextArea) “Body” in window “Untitled” of TextEdit \
             (com.apple.TextEdit)\n\
             - Keys: type `Hello`\n\n\
             ![Step 1](images/01-type-the-text.png)\n\n\
             > **Tip:** Any text works.\n\n",
            "#### 2. Press **⌘S**\n\n- Action: press keys\n- Keys: press `⌘S`\n\n",
            "- Element: group (AXGroup) (no label) in window “Untitled” of TextEdit",
            "> **Unclear element:** No label and a generic role. Find it",
            "#### 4. Check the file\n\n- Action: by hand (no recorded action)\n\n",
            "## Expected result\n\nWhen every step is done, “Save a \\*note\\* in TextEdit” is \
             complete. The last step was “Check the file”; its image, \
             `images/04-check-the-file.png`, shows the screen then.\n\n",
            "## Unclear elements\n\nThe element of step 3 was not identified clearly",
            "- `replay.js`: a macOS script",
        ] {
            assert!(md.contains(part), "missing {part:?} in\n{md}");
        }
        assert!(md.ends_with(".\n"));
        let without = skill_md(&guide, &names(&guide), false);
        assert!(!without.contains("replay.js"));
    }

    #[test]
    fn front_matter_stays_valid_yaml_for_any_title() {
        let mut guide = Guide::new("Say \"hi\" \\ <b>now</b>", "en");
        guide.summary = "Line one\nline: two # three".into();
        let rendered = render(&guide);
        let md = skill_md(&rendered, &names(&rendered), false);
        let front = md.split("---\n").nth(1).unwrap();
        assert_eq!(
            front,
            "name: say-hi-b-now-b\n\
             description: \"Line one line: two # three. Use when asked to do “Say \\\"hi\\\" \
             \\\\ bnow/b”: 0 recorded steps.\"\n"
        );
        assert!(md.contains("This guide has no steps."));
        assert!(md.contains("Nothing to do"));
        let long = Guide {
            summary: "x".repeat(2000),
            ..Guide::new("T", "en")
        };
        let long = render(&long);
        let data = steps(&long, &[]);
        assert_eq!(
            description(&long, &data).chars().count(),
            MAX_DESCRIPTION_CHARS
        );
    }

    #[test]
    fn replay_script_embeds_the_steps_once() {
        let guide = render(&sample());
        let data = steps(&guide, &names(&guide));
        let ReplayScript { file, script } = replay_script(&data);
        assert_eq!(file, REPLAY_FILE);
        assert!(script.starts_with("#!/usr/bin/osascript -l JavaScript\n"));
        assert!(!script.contains(REPLAY_PLACEHOLDER));
        let line = script
            .lines()
            .find(|l| l.starts_with("const GUIDE = "))
            .expect("data line");
        let json = line
            .strip_prefix("const GUIDE = ")
            .and_then(|l| l.strip_suffix(';'))
            .unwrap();
        let embedded: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(embedded, serde_json::to_value(&data).unwrap());
        for part in [
            "function run(argv)",
            "--dry-run",
            "System Events",
            "entireContents",
        ] {
            assert!(script.contains(part), "{part}");
        }
        // Descriptions are kept as stored, line separators and all.
        let mut odd = sample();
        if let Item::Step(step) = &mut odd.items[1] {
            step.description = "a\u{2028}b".into();
        }
        let script = replay_script(&steps(&render(&odd), &[])).script;
        assert!(!script.contains('\u{2028}') && script.contains("a\\u2028b"));
    }

    /// A title no embedding may trip over: quotes, a here-string end,
    /// PowerShell's `$` and backtick, non-ASCII and line separators.
    const NASTY: &str = "Say \"hi\" '@\n'@ $env:USERNAME `n — café 😀 \u{2028}\u{2029}\u{85} end";

    /// [`sample`] as recorded on Windows, against an app that is not
    /// running, with [`NASTY`] text in it.
    fn windows_sample() -> Guide {
        let mut guide = sample();
        guide.title = NASTY.into();
        for item in &mut guide.items {
            if let Item::Step(step) = item {
                if let Some(el) = &mut step.element {
                    el.app = Some("Not Running".into());
                    el.bundle_id = Some("guiderecorder-not-running.exe".into());
                    el.window = Some(NASTY.into());
                    el.role = Some("AXTextField".into());
                }
                if step.kind == StepKind::Shortcut {
                    step.keystrokes = Some("Ctrl+S".into());
                }
                if step.kind == StepKind::Type {
                    step.keystrokes = Some(format!("{NASTY}⏎"));
                }
            }
        }
        guide
    }

    /// The JSON embedded in `replay.ps1`.
    fn powershell_json(script: &str) -> serde_json::Value {
        let (_, rest) = script.split_once("$Guide = @'\n").expect("here-string");
        let (json, _) = rest.split_once("\n'@ | ConvertFrom-Json").expect("end");
        assert!(!json.contains('\n'), "one line");
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn windows_guides_get_the_powershell_script() {
        assert_eq!(replay_file(Some(Platform::Windows)), REPLAY_WINDOWS_FILE);
        assert_eq!(replay_file(Some(Platform::Macos)), REPLAY_FILE);
        assert_eq!(replay_file(None), REPLAY_FILE);
        assert_eq!(
            REPLAY_WINDOWS_TEMPLATE.matches(REPLAY_PLACEHOLDER).count(),
            1
        );

        let guide = render(&windows_sample());
        let data = steps(&guide, &names(&guide));
        assert_eq!(data.guide.recorded_on, Some(Platform::Windows));
        let ReplayScript { file, script } = replay_script(&data);
        assert_eq!(file, REPLAY_WINDOWS_FILE);
        assert!(script.starts_with("# Replays a GuideRecorder guide on Windows"));
        assert!(!script.contains(REPLAY_PLACEHOLDER));
        // ASCII only, so no `'@` and nothing a code page could misread.
        assert!(script.is_ascii());
        assert_eq!(
            script.matches("\n'@").count(),
            2,
            "the two here-strings end"
        );
        assert_eq!(
            powershell_json(&script),
            serde_json::to_value(&data).unwrap()
        );
        for part in ["-DryRun", "UIAutomationClient", "SendInput", "-From $("] {
            assert!(script.contains(part), "{part}");
        }
    }

    #[test]
    fn skill_md_names_the_script_for_the_platform() {
        let guide = render(&windows_sample());
        let md = skill_md(&guide, &names(&guide), true);
        assert!(
            md.contains(
                "- `replay.ps1`: a Windows PowerShell script that repeats the steps through UI \
                 Automation. Run `powershell -ExecutionPolicy Bypass -File replay.ps1 -DryRun` \
                 first: it only reports what it would do and whether each element is there. The \
                 script stops at a step it can't do safely."
            ),
            "{md}"
        );
        assert!(!md.contains("replay.js"));
        assert!(!skill_md(&guide, &names(&guide), false).contains("replay.ps1"));
        let mac = render(&sample());
        assert!(!skill_md(&mac, &names(&mac), true).contains("replay.ps1"));
    }

    #[test]
    fn export_writes_the_platforms_script_and_removes_the_other() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("skill");
        let mac = sample();
        let windows = windows_sample();
        let handle = GuideHandle {
            id: mac.id,
            path: dir.path().join("missing.guiderecorder"),
        };
        let opts = SkillOptions {
            replay: true,
            overwrite: true,
            ..SkillOptions::default()
        };
        let render_mac = RenderGuide::new(&mac, &handle);
        let render_windows = RenderGuide::new(&windows, &handle);

        export(&render_mac, &handle, &out, &opts).unwrap();
        assert!(out.join(REPLAY_FILE).is_file());
        let report = export(&render_windows, &handle, &out, &opts).unwrap();
        assert!(report.files.contains(&out.join(REPLAY_WINDOWS_FILE)));
        assert!(!out.join(REPLAY_FILE).exists(), "the old script is gone");
        let bytes = fs::read(out.join(REPLAY_WINDOWS_FILE)).unwrap();
        let script = replay_script(&steps(&render_windows, &image_names(&render_windows)));
        assert_eq!(bytes[..3], *UTF8_BOM);
        assert_eq!(bytes[3..], *script.script.as_bytes());

        export(&render_mac, &handle, &out, &opts).unwrap();
        assert!(out.join(REPLAY_FILE).is_file());
        assert!(!out.join(REPLAY_WINDOWS_FILE).exists());
        assert!(!fs::read(out.join(REPLAY_FILE))
            .unwrap()
            .starts_with(UTF8_BOM));

        export(&render_windows, &handle, &out, &opts).unwrap();
        let without = SkillOptions {
            replay: false,
            ..opts.clone()
        };
        export(&render_windows, &handle, &out, &without).unwrap();
        assert!(!out.join(REPLAY_FILE).exists() && !out.join(REPLAY_WINDOWS_FILE).exists());

        let preview = preview(&render_windows, &handle, &opts);
        assert_eq!(preview.replay.unwrap().file, REPLAY_WINDOWS_FILE);
        let (_, _, text) = render_text(&render_windows, true);
        assert_eq!(text.unwrap(), script);
    }

    /// Runs the exported `replay.ps1` in Windows PowerShell: it parses
    /// without errors, reads the embedded steps back as written, and stops
    /// as it should when the app is not running.
    #[cfg(windows)]
    #[test]
    fn powershell_parses_and_dry_runs_the_script() {
        use std::process::Command;

        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("skill");
        let guide = windows_sample();
        let handle = GuideHandle {
            id: guide.id,
            path: dir.path().join("missing.guiderecorder"),
        };
        let opts = SkillOptions {
            replay: true,
            ..SkillOptions::default()
        };
        export(&RenderGuide::new(&guide, &handle), &handle, &out, &opts).unwrap();
        let script = out.join(REPLAY_WINDOWS_FILE);
        let quoted = script.to_string_lossy().replace('\'', "''");
        let powershell = |command: String| {
            let output = Command::new("powershell")
                .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass"])
                .arg("-Command")
                .arg(format!(
                    "[Console]::OutputEncoding = [Text.Encoding]::UTF8; {command}; exit $LASTEXITCODE"
                ))
                .output()
                .expect("powershell runs");
            let text = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n")
                + &String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
            (output.status.code(), text)
        };

        let (code, text) = powershell(format!(
            "$errors = $null; [System.Management.Automation.Language.Parser]::ParseFile('{quoted}', \
             [ref]$null, [ref]$errors) | Out-Null; Write-Output \"errors: $($errors.Count)\""
        ));
        assert_eq!((code, text.trim()), (Some(0), "errors: 0"));

        let (code, text) = powershell(format!("& '{quoted}' -Help"));
        assert_eq!(code, Some(0), "{text}");
        assert!(text.starts_with("Usage: powershell"), "{text}");

        let (code, text) = powershell(format!("& '{quoted}' -DryRun"));
        assert_eq!(code, Some(0), "{text}");
        let title = single_line(NASTY);
        assert!(
            text.starts_with(&format!(
                "Dry run of \"{}\": 4 of 4 steps.\n",
                title.replace('"', "\\\"")
            )),
            "{text}"
        );
        for part in [
            "Step 1: Type the text\n  would type \"",
            "not found now: Not Running is not running",
            "Step 2: Press ⌘S\n  would press Ctrl+S\n  ok",
            "would stop: its element is unclear (noLabel)",
            "Step 4: Check the file\n  would do it by hand\n  would stop: it is a manual step",
            "3 step(s) need attention.",
        ] {
            assert!(text.contains(part), "missing {part:?} in\n{text}");
        }

        // A real run stops at the first step it can't do, with a non-zero
        // exit code.
        let (code, text) = powershell(format!("& '{quoted}' -Delay 0"));
        assert_eq!(code, Some(1), "{text}");
        assert!(
            text.contains(
                "Step 1: Type the text\n  stopped: Not Running is not running. Do it by hand, \
                 then run again with -From 2."
            ),
            "{text}"
        );
    }

    #[test]
    fn narration_is_context_in_both_files() {
        let mut guide = sample();
        if let Item::Step(step) = &mut guide.items[1] {
            step.narration = "now type a *short*\ngreeting".into();
        }
        let guide = render(&guide);
        let names = names(&guide);
        let data = steps(&guide, &names);
        assert_eq!(data.steps[0].narration, "now type a *short* greeting");
        let json: serde_json::Value = serde_json::from_str(&steps_json(&data)).unwrap();
        assert_eq!(json["steps"][0]["narration"], "now type a *short* greeting");
        assert!(json["steps"][1].get("narration").is_none());
        let md = skill_md(&guide, &names, false);
        assert!(
            md.contains(
                "- Keys: type `Hello`\n- Narration (the author, while recording): \
                 “now type a \\*short\\* greeting”\n"
            ),
            "{md}"
        );
        assert_eq!(md.matches("- Narration").count(), 1);
    }

    #[test]
    fn notes_name_unclear_steps() {
        let guide = render(&sample());
        let data = steps(&guide, &names(&guide));
        assert_eq!(
            unclear_notes(&data),
            [
                "Step 3: its element is unclear (no label and a generic role), so agents and the \
              replay script may not find it"
            ]
        );
    }

    #[test]
    fn writes_the_folder_and_replaces_an_earlier_export() {
        let dir = tempfile::tempdir().unwrap();
        let guide = sample();
        let handle = GuideHandle {
            id: guide.id,
            path: dir.path().join("missing.guiderecorder"),
        };
        let render = RenderGuide::new(&guide, &handle);
        let out = dir.path().join("skill");
        let opts = SkillOptions {
            replay: true,
            ..SkillOptions::default()
        };
        let report = export(&render, &handle, &out, &opts).unwrap();
        let files: Vec<String> = report
            .files
            .iter()
            .map(|f| {
                f.strip_prefix(&out)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(
            files,
            [
                "SKILL.md",
                "steps.json",
                "replay.js",
                "images/01-type-the-text.png",
                "images/02-press-s.png",
                "images/03-click-in-the-sidebar.png",
                "images/04-check-the-file.png",
            ]
        );
        // Missing screenshots become placeholders with a warning each.
        assert_eq!(report.warnings.len(), 4);
        assert_eq!(report.notes.len(), 1);
        assert!(matches!(
            export(&render, &handle, &out, &opts),
            Err(super::super::md::ExportError::NotEmpty(_))
        ));

        let again = SkillOptions {
            overwrite: true,
            ..SkillOptions::default()
        };
        let report = export(&render, &handle, &out, &again).unwrap();
        assert!(!out.join(REPLAY_FILE).exists());
        assert_eq!(report.files.len(), 6);
        assert_eq!(
            fs::read_to_string(out.join(SKILL_FILE)).unwrap(),
            skill_md(&render, &image_names(&render), false)
        );

        // The preview is the export with replay.js, in memory.
        let preview = preview(&render, &handle, &opts);
        assert_eq!(
            preview.skill_md,
            skill_md(&render, &image_names(&render), true)
        );
        assert_eq!(
            preview.steps_json,
            fs::read_to_string(out.join(STEPS_FILE)).unwrap()
        );
        assert_eq!(preview.images.len(), 4);
        assert_eq!(preview.images[0].0, "images/01-type-the-text.png");
        assert!(preview.replay.is_some());
        assert_eq!(preview.report.notes.len(), 1);
    }
}
