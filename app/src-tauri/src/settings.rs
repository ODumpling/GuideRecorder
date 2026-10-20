//! App settings (design #439 "Settings"): a JSON file in Application
//! Support (`dev.dufflebear.guiderecorder/settings.json`), written atomically, with a
//! schema version.
//!
//! API keys are not here: they live in the Keychain (core's
//! `ai::keychain`). Each recording reads the recording settings when it
//! starts (`recording::session_config`); the record and pause shortcuts
//! are registered at startup and again on every change
//! (`hotkeys::apply`).
//!
//! Paths in settings (the library, the default export folder and the logo)
//! are trusted later without another dialog, so the webview may only set
//! them to a path a native dialog returned (see [`Settings::check_paths`]).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use guiderecorder_core::ai::claude::{DEFAULT_GUIDE_MODEL, DEFAULT_STEP_MODEL};
use guiderecorder_core::library::{write_atomic, Durability};
use guiderecorder_core::model::Audience;
use guiderecorder_core::redact::{self, RedactionMode};
use serde::{Deserialize, Serialize};

use crate::editor::{AppError, ErrorKind, Result};
use crate::export::{ChosenPaths, Format};

/// The settings format this app writes. Version 2 changed the default
/// shortcuts (see [`migrate`]).
pub const SETTINGS_VERSION: u32 = 2;

/// File name inside the app's Application Support folder.
pub const SETTINGS_FILE: &str = "settings.json";

/// Largest settings file read; a real one is a few kB.
const MAX_SETTINGS_BYTES: u64 = 1024 * 1024;

/// `~/Library/Application Support/dev.dufflebear.guiderecorder/settings.json` (macOS;
/// the index lives next to it). `None` without a home folder.
pub fn default_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
    Some(
        PathBuf::from(home)
            .join("Library/Application Support/dev.dufflebear.guiderecorder")
            .join(SETTINGS_FILE),
    )
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub version: u32,
    pub general: General,
    pub recording: Recording,
    pub ai: Ai,
    pub export: ExportDefaults,
    pub redaction: Redaction,
    pub mcp: Mcp,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            general: General::default(),
            recording: Recording::default(),
            ai: Ai::default(),
            export: ExportDefaults::default(),
            redaction: Redaction::default(),
            mcp: Mcp::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct General {
    /// The library folder; `None` is `~/Documents/GuideRecorder`.
    /// `$GUIDERECORDER_LIBRARY` overrides it.
    pub library: Option<PathBuf>,
    /// Desired state; the login item itself is the truth (see the
    /// `set_launch_at_login` command).
    pub launch_at_login: bool,
    /// Placeholder: there is no menu bar icon yet.
    pub menu_bar_icon: bool,
    /// First-run setup was finished (or skipped through to the end).
    pub onboarded: bool,
    /// Check for updates at startup and every few hours (release builds;
    /// see `updater`).
    pub check_for_updates: bool,
    /// The app's colour theme; the windows apply it themselves.
    pub theme: Theme,
}

impl Default for General {
    fn default() -> Self {
        Self {
            library: None,
            launch_at_login: false,
            menu_bar_icon: false,
            onboarded: false,
            check_for_updates: true,
            theme: Theme::default(),
        }
    }
}

/// The app's colour theme (Settings → Appearance). Settings saved before
/// there was a choice have no `theme` and get Arctic.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Theme {
    #[default]
    Arctic,
    Paper,
    PaperDark,
    Sage,
    Steel,
    Nebula,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CaptureArea {
    /// Every display.
    #[default]
    Display,
    /// One display, picked at each recording.
    OneDisplay,
    Window,
    Region,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Hotkeys {
    /// Start and stop recording.
    pub record: String,
    pub pause: String,
    /// Add a manual step while recording.
    pub manual_step: String,
}

impl Default for Hotkeys {
    fn default() -> Self {
        Self {
            record: "Ctrl+Alt+R".into(),
            pause: "Ctrl+Alt+P".into(),
            manual_step: "Ctrl+Alt+S".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Recording {
    pub hotkeys: Hotkeys,
    pub capture_area: CaptureArea,
    /// Also capture a frame after each action.
    pub after_frames: bool,
    /// How long after the action that frame is taken.
    pub after_frame_delay_ms: u32,
    /// Bundle ids of apps during which nothing is recorded while one is
    /// frontmost. A settings file without the field gets
    /// [`default_excluded_apps`]; a saved empty list stays empty.
    pub excluded_apps: Vec<String>,
    /// Typing pauses longer than this end a typing step.
    pub typing_idle_ms: u32,
    /// Briefly show a ring on screen where each click was captured.
    pub click_ring: bool,
}

impl Default for Recording {
    fn default() -> Self {
        Self {
            hotkeys: Hotkeys::default(),
            capture_area: CaptureArea::default(),
            after_frames: true,
            after_frame_delay_ms: 400,
            excluded_apps: default_excluded_apps(),
            typing_idle_ms: 1500,
            click_ring: false,
        }
    }
}

/// The suggested exclusion list: password managers, Keychain Access,
/// Passwords and Messages (the capture crate's
/// `DEFAULT_EXCLUDED_BUNDLE_IDS`). Empty where there is no capture backend.
pub fn default_excluded_apps() -> Vec<String> {
    #[cfg(target_os = "macos")]
    {
        guiderecorder_capture::macos::session::default_excluded_bundle_ids()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Vec::new()
    }
}

/// Who writes the guide text. Settings saved before there was a choice
/// have no `provider` and stay on Anthropic.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Provider {
    #[default]
    Anthropic,
    /// Any OpenAI-compatible API at [`Endpoint::base_url`], with an
    /// optional key.
    OpenAiCompatible,
    /// Ollama on this Mac; no key.
    Ollama,
    /// LM Studio on this Mac; no key.
    LmStudio,
}

/// An OpenAI-compatible provider's settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Endpoint {
    /// Up to the version segment, e.g. `https://api.openai.com/v1`; empty:
    /// the local preset's default (required for a custom API).
    pub base_url: String,
    /// Writes each step, and rewrites; empty until chosen.
    pub step_model: String,
    /// Writes the guide fields; empty until chosen.
    pub guide_model: String,
    /// The step model takes images. Off: steps are written from their
    /// element, keystrokes and kind only.
    pub images: bool,
}

impl Default for Endpoint {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            step_model: String::new(),
            guide_model: String::new(),
            images: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Ai {
    pub enabled: bool,
    /// Write the guide text when a recording stops.
    pub auto_run: bool,
    pub provider: Provider,
    /// Anthropic's step model.
    pub step_model: String,
    /// Anthropic's guide model.
    pub guide_model: String,
    pub open_ai_compatible: Endpoint,
    pub ollama: Endpoint,
    pub lm_studio: Endpoint,
    /// BCP 47 tag of the language to write in; empty: the guide's own.
    pub language: String,
    pub audience: Audience,
}

impl Default for Ai {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_run: true,
            provider: Provider::default(),
            step_model: DEFAULT_STEP_MODEL.into(),
            guide_model: DEFAULT_GUIDE_MODEL.into(),
            open_ai_compatible: Endpoint::default(),
            ollama: Endpoint::default(),
            lm_studio: Endpoint::default(),
            language: String::new(),
            audience: Audience::default(),
        }
    }
}

impl Ai {
    /// The selected OpenAI-compatible provider's settings; `None` for
    /// Anthropic.
    pub fn endpoint(&self) -> Option<&Endpoint> {
        match self.provider {
            Provider::Anthropic => None,
            Provider::OpenAiCompatible => Some(&self.open_ai_compatible),
            Provider::Ollama => Some(&self.ollama),
            Provider::LmStudio => Some(&self.lm_studio),
        }
    }

    /// Whether step images are sent: always for Anthropic.
    pub fn images(&self) -> bool {
        self.endpoint().is_none_or(|e| e.images)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ExportDefaults {
    pub format: Format,
    /// The folder Markdown and HTML exports go into, and where the save
    /// dialog starts.
    pub folder: Option<PathBuf>,
    pub logo: Option<PathBuf>,
    pub accent: Option<String>,
}

/// On-device redaction of new recordings (see `redaction`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Redaction {
    /// What happens to sensitive text found in a recording's screenshots.
    pub mode: RedactionMode,
    /// The user's own regular expressions (Rust `regex` syntax).
    pub patterns: Vec<String>,
    /// Words and phrases to blur wherever they appear, ignoring case.
    pub words: Vec<String>,
}

/// The MCP server setup shown in Settings → MCP.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Mcp {
    /// Let MCP clients edit and create guides: the setup command passes
    /// `--writable`. Only the command changes; a server already set up
    /// keeps the flags it was added with.
    pub writable: bool,
}

/// Limits for the stored values.
pub const TYPING_IDLE_MS: std::ops::RangeInclusive<u32> = 300..=10_000;
pub const AFTER_FRAME_DELAY_MS: std::ops::RangeInclusive<u32> = 0..=3_000;
pub const MAX_EXCLUDED_APPS: usize = 100;

const MODIFIERS: &[&str] = &[
    "CmdOrCtrl",
    "CommandOrControl",
    "Cmd",
    "Command",
    "Super",
    "Ctrl",
    "Control",
    "Alt",
    "Option",
    "Shift",
];

/// A global shortcut as Tauri spells it: modifiers (at least one besides
/// Shift) plus one key, e.g. `CmdOrCtrl+Shift+R`. Empty means none.
pub fn is_hotkey(value: &str) -> bool {
    if value.is_empty() {
        return true;
    }
    let parts: Vec<&str> = value.split('+').collect();
    let Some((key, mods)) = parts.split_last() else {
        return false;
    };
    let key_ok = (key.len() == 1 && key.bytes().all(|b| b.is_ascii_alphanumeric()))
        || (key.len() <= 3
            && key.starts_with('F')
            && key[1..].parse::<u8>().is_ok_and(|n| (1..=20).contains(&n)))
        || ["Space", "Enter", "Tab", "Escape", "Backspace", "Delete"].contains(key);
    let mods_ok = mods.iter().all(|m| MODIFIERS.contains(m))
        && mods.iter().any(|m| *m != "Shift")
        && (1..mods.len()).all(|i| !mods[..i].contains(&mods[i]));
    key_ok && mods_ok
}

/// Whether `a` and `b` are the same shortcut (neither empty), with
/// modifier aliases and order ignored: `Alt+Ctrl+R` and `Control+Option+R`
/// are both `Ctrl+Alt+R`.
fn same_hotkey(a: &str, b: &str) -> bool {
    fn canonical(value: &str) -> Option<(Vec<&'static str>, String)> {
        let mut parts = value.split('+');
        let key = parts.next_back()?.to_ascii_uppercase();
        let mut mods: Vec<&'static str> = parts
            .map(|m| match m {
                "CmdOrCtrl" | "CommandOrControl" | "Cmd" | "Command" | "Super" => "Cmd",
                "Ctrl" | "Control" => "Ctrl",
                "Alt" | "Option" => "Alt",
                _ => "Shift",
            })
            .collect();
        mods.sort_unstable();
        mods.dedup();
        Some((mods, key))
    }
    !a.is_empty() && !b.is_empty() && canonical(a) == canonical(b)
}

/// A model id such as `claude-haiku-5-5`, `qwen3-vl:8b` or
/// `lmstudio-community/gemma-3-12b`.
pub fn is_model_id(value: &str) -> bool {
    (1..=200).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._:@/".contains(&b))
}

/// Empty, or an `http` or `https` URL without spaces or a query.
pub fn is_base_url(value: &str) -> bool {
    let rest = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"));
    value.is_empty()
        || (value.len() <= 2048
            && rest.is_some_and(|rest| {
                !rest.is_empty()
                    && !rest.starts_with('/')
                    && rest
                        .bytes()
                        .all(|b| b.is_ascii_graphic() && !b"?#".contains(&b))
            }))
}

/// Empty, or a BCP 47-looking tag (`en`, `de-CH`, `zh-Hant-TW`).
pub fn is_language(value: &str) -> bool {
    value.is_empty()
        || (value.len() <= 35
            && value.split('-').all(|part| {
                (1..=8).contains(&part.len()) && part.bytes().all(|b| b.is_ascii_alphanumeric())
            }))
}

/// A full bundle id such as `com.1password.1password`: at least three
/// non-empty dot-separated parts. Exclusion also matches `<id>.` as a
/// prefix (for helpers), so a shorter entry like `com.apple` would leave
/// out every Apple app.
pub fn is_bundle_id(value: &str) -> bool {
    has_bundle_id_chars(value)
        && value.split('.').count() >= 3
        && value.split('.').all(|part| !part.is_empty())
}

/// Made of bundle-id characters and dotted, but not a full bundle id
/// (`com.apple`, `com..x`): it would exclude far more than one app.
fn is_broad_bundle_id(value: &str) -> bool {
    has_bundle_id_chars(value) && value.contains('.') && !is_bundle_id(value)
}

fn has_bundle_id_chars(value: &str) -> bool {
    (1..=255).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._".contains(&b))
}

/// Why `app` can't be an excluded app.
fn bundle_id_error(app: &str) -> String {
    if is_broad_bundle_id(app) {
        format!(
            "{app:?} is too broad: it would exclude every app whose id starts with it; \
             use a full bundle id such as com.1password.1password"
        )
    } else {
        format!("{app:?} is not an app bundle id")
    }
}

/// `#rgb` or `#rrggbb`.
pub fn is_accent(value: &str) -> bool {
    let hex = value.strip_prefix('#').unwrap_or("");
    (hex.len() == 3 || hex.len() == 6) && hex.bytes().all(|b| b.is_ascii_hexdigit())
}

fn invalid(message: impl Into<String>) -> AppError {
    AppError::new(ErrorKind::Invalid, message)
}

impl Settings {
    /// Checks every value against the limits above.
    pub fn validate(&self) -> Result<()> {
        let r = &self.recording;
        let keys = [
            ("record", &r.hotkeys.record),
            ("pause", &r.hotkeys.pause),
            ("manual step", &r.hotkeys.manual_step),
        ];
        for (name, key) in keys {
            if !is_hotkey(key) {
                return Err(invalid(format!("the {name} shortcut {key:?} is not valid")));
            }
        }
        for (i, (_, a)) in keys.iter().enumerate() {
            if keys[..i].iter().any(|(_, b)| same_hotkey(a, b)) {
                return Err(invalid(format!("the shortcut {a} is used twice")));
            }
        }
        if !TYPING_IDLE_MS.contains(&r.typing_idle_ms) {
            return Err(invalid(format!(
                "the typing pause must be {}–{} ms",
                TYPING_IDLE_MS.start(),
                TYPING_IDLE_MS.end()
            )));
        }
        if !AFTER_FRAME_DELAY_MS.contains(&r.after_frame_delay_ms) {
            return Err(invalid(format!(
                "the after-frame delay must be {}–{} ms",
                AFTER_FRAME_DELAY_MS.start(),
                AFTER_FRAME_DELAY_MS.end()
            )));
        }
        if r.excluded_apps.len() > MAX_EXCLUDED_APPS {
            return Err(invalid(format!(
                "at most {MAX_EXCLUDED_APPS} excluded apps"
            )));
        }
        if let Some(app) = r.excluded_apps.iter().find(|a| !is_bundle_id(a)) {
            return Err(invalid(bundle_id_error(app)));
        }
        for model in [&self.ai.step_model, &self.ai.guide_model] {
            if !is_model_id(model) {
                return Err(invalid(format!("{model:?} is not a model id")));
            }
        }
        for e in [
            &self.ai.open_ai_compatible,
            &self.ai.ollama,
            &self.ai.lm_studio,
        ] {
            for model in [&e.step_model, &e.guide_model] {
                if !model.is_empty() && !is_model_id(model) {
                    return Err(invalid(format!("{model:?} is not a model id")));
                }
            }
            if !is_base_url(&e.base_url) {
                return Err(invalid(format!(
                    "{:?} is not an http or https URL",
                    e.base_url
                )));
            }
        }
        if !is_language(&self.ai.language) {
            return Err(invalid(format!(
                "{:?} is not a language tag",
                self.ai.language
            )));
        }
        if let Some(accent) = self.export.accent.as_deref().filter(|a| !is_accent(a)) {
            return Err(invalid(format!(
                "accent {accent:?} is not a hex colour (#rgb or #rrggbb)"
            )));
        }
        let r = &self.redaction;
        if r.patterns.len() > redact::detect::MAX_PATTERNS {
            return Err(invalid(format!(
                "at most {} patterns to blur",
                redact::detect::MAX_PATTERNS
            )));
        }
        if r.words.len() > redact::detect::MAX_WORDS {
            return Err(invalid(format!(
                "at most {} words to blur",
                redact::detect::MAX_WORDS
            )));
        }
        for pattern in &r.patterns {
            redact::check_pattern(pattern).map_err(|e| invalid(e.0))?;
        }
        for word in &r.words {
            redact::check_word(word).map_err(|e| invalid(e.0))?;
        }
        Ok(())
    }

    /// Refuses a changed path the user did not pick in a native dialog: a
    /// path may stay as it was, be cleared, or become one the dialogs
    /// returned. The library changes only through `switch_library`.
    pub fn check_paths(&self, old: &Settings, chosen: &ChosenPaths) -> Result<()> {
        if self.general.library != old.general.library {
            return Err(invalid("change the library folder with Choose…"));
        }
        let changed = |new: &Option<PathBuf>, old: &Option<PathBuf>| {
            new.as_ref().filter(|n| Some(*n) != old.as_ref()).cloned()
        };
        if let Some(folder) = changed(&self.export.folder, &old.export.folder) {
            if !chosen.is_folder(&folder) {
                return Err(invalid(format!(
                    "{} was not chosen as the export folder",
                    folder.display()
                )));
            }
        }
        if let Some(logo) = changed(&self.export.logo, &old.export.logo) {
            if !chosen.is_logo(&logo) {
                return Err(invalid(format!(
                    "{} was not chosen as the logo",
                    logo.display()
                )));
            }
        }
        Ok(())
    }
}

/// How the stored settings were found at startup.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadState {
    /// A newer GuideRecorder wrote them: shown, not saved over.
    pub read_only: bool,
    /// Why the file could not be used (it was set aside), if so.
    pub error: Option<String>,
}

/// The settings file and its current contents.
pub struct SettingsStore {
    path: Option<PathBuf>,
    state: Mutex<(Settings, LoadState)>,
}

/// `get_settings`'s result.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsView {
    pub settings: Settings,
    pub read_only: bool,
    pub load_error: Option<String>,
}

impl SettingsStore {
    /// Loads `path`. A missing file gives the defaults. An unreadable or
    /// corrupt one is renamed aside (`settings.json.corrupt-<secs>`) and
    /// the defaults are used. A newer version is used but never written.
    /// Without a path the settings live in memory only.
    pub fn load(path: Option<PathBuf>) -> Self {
        let (settings, state) = match &path {
            Some(path) => read(path),
            None => (Settings::default(), LoadState::default()),
        };
        if let Some(error) = &state.error {
            eprintln!("guiderecorder: settings: {error}");
        }
        Self {
            path,
            state: Mutex::new((settings, state)),
        }
    }

    pub fn get(&self) -> Settings {
        self.state.lock().unwrap().0.clone()
    }

    pub fn view(&self) -> SettingsView {
        let state = self.state.lock().unwrap();
        SettingsView {
            settings: state.0.clone(),
            read_only: state.1.read_only,
            load_error: state.1.error.clone(),
        }
    }

    /// Validates and stores `settings` from the UI (see
    /// [`Settings::check_paths`]). The version is always ours.
    pub fn update(&self, mut settings: Settings, chosen: &ChosenPaths) -> Result<Settings> {
        let mut state = self.state.lock().unwrap();
        if state.1.read_only {
            return Err(AppError::new(
                ErrorKind::ReadOnly,
                "these settings were saved by a newer GuideRecorder and can't be changed here",
            ));
        }
        settings.version = SETTINGS_VERSION;
        settings.validate()?;
        settings.check_paths(&state.0, chosen)?;
        self.write(&settings)?;
        state.0 = settings.clone();
        Ok(settings)
    }

    /// Applies `f` to the stored settings and saves them, for changes the
    /// app itself makes (the library switch, the login item).
    pub fn modify(&self, f: impl FnOnce(&mut Settings)) -> Result<Settings> {
        let mut state = self.state.lock().unwrap();
        if state.1.read_only {
            return Err(AppError::new(
                ErrorKind::ReadOnly,
                "these settings were saved by a newer GuideRecorder and can't be changed here",
            ));
        }
        let mut settings = state.0.clone();
        f(&mut settings);
        self.write(&settings)?;
        state.0 = settings.clone();
        Ok(settings)
    }

    fn write(&self, settings: &Settings) -> Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| AppError::new(ErrorKind::Io, format!("{}: {e}", dir.display())))?;
        }
        let mut json = serde_json::to_vec_pretty(settings)
            .map_err(|e| AppError::new(ErrorKind::Io, e.to_string()))?;
        json.push(b'\n');
        write_atomic(path, &json, Durability::Full)?;
        Ok(())
    }
}

fn read(path: &Path) -> (Settings, LoadState) {
    let bytes = match read_capped(path) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return (Settings::default(), LoadState::default()),
        // Not corruption: never rename it, and never save over it.
        Err(e) => {
            return keep_read_only(
                Settings::default(),
                format!(
                    "cannot read {}: {e}; using the defaults, and changes can't be saved until it is readable",
                    path.display()
                ),
            )
        }
    };
    let value: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(e) => return set_aside(path, format!("{} is not valid JSON: {e}", path.display())),
    };
    let version = value
        .get("version")
        .and_then(|v| v.as_u64())
        .unwrap_or(u64::from(SETTINGS_VERSION));
    let read_only = version > u64::from(SETTINGS_VERSION);
    let decoded = serde_json::from_value::<Settings>(value).map(|mut settings| {
        if !read_only {
            migrate(&mut settings, version);
        }
        let dropped = drop_invalid_excluded_apps(&mut settings);
        (settings, dropped)
    });
    match decoded {
        Ok((settings, dropped)) if read_only || settings.validate().is_ok() => (
            settings,
            LoadState {
                read_only,
                error: dropped.map(|e| format!("{}: {e}", path.display())),
            },
        ),
        Ok((settings, dropped)) => {
            // Keep what is valid: fall back field group by field group.
            let error = settings
                .validate()
                .err()
                .map(|e| format!("{}; using the default there", e.message));
            let error = [dropped, error].into_iter().flatten().collect::<Vec<_>>();
            (
                sanitize(settings),
                LoadState {
                    read_only: false,
                    error: Some(format!("{}: {}", path.display(), error.join("; "))),
                },
            )
        }
        // A newer GuideRecorder's settings this one can't decode (a new enum
        // value, a changed type): leave the file alone.
        Err(e) if read_only => keep_read_only(
            Settings::default(),
            format!(
                "{} was saved by a newer GuideRecorder (settings version {version}) and can't be read here ({e}); \
                 showing the defaults, and changes can't be saved",
                path.display()
            ),
        ),
        Err(e) => set_aside(path, format!("{}: {e}", path.display())),
    }
}

/// Brings settings saved by an older version up to date, in memory; the
/// next save writes them as [`SETTINGS_VERSION`].
///
/// Version 1 saved ⇧⌘R and ⇧⌘P as the default record and pause
/// shortcuts, but the app always registered ⌃⌥R and ⌃⌥P. Now that the
/// saved shortcuts are used, one still at its old default becomes the one
/// that actually worked, rather than taking over ⇧⌘P (the command palette
/// in many apps), unless another shortcut already has it. The manual-step
/// shortcut was always read from the settings, so it is kept.
fn migrate(settings: &mut Settings, version: u64) {
    if version < 2 {
        let defaults = Hotkeys::default();
        let h = &mut settings.recording.hotkeys;
        if h.record == "CmdOrCtrl+Shift+R"
            && ![&h.pause, &h.manual_step]
                .iter()
                .any(|o| same_hotkey(o, &defaults.record))
        {
            h.record = defaults.record;
        }
        if h.pause == "CmdOrCtrl+Shift+P"
            && ![&h.record, &h.manual_step]
                .iter()
                .any(|o| same_hotkey(o, &defaults.pause))
        {
            h.pause = defaults.pause;
        }
    }
}

/// Drops the excluded apps that are not full bundle ids (such as a broad
/// `com.apple` from a hand-edited file), so one bad entry neither resets
/// the rest of the recording settings nor excludes a whole vendor's apps.
/// Also applied to read-only settings, in memory only. Returns why, if any
/// were dropped; the next save writes the list without them.
fn drop_invalid_excluded_apps(settings: &mut Settings) -> Option<String> {
    let (kept, dropped): (Vec<String>, Vec<String>) =
        std::mem::take(&mut settings.recording.excluded_apps)
            .into_iter()
            .partition(|app| is_bundle_id(app));
    settings.recording.excluded_apps = kept;
    let reasons: Vec<String> = dropped.iter().map(|app| bundle_id_error(app)).collect();
    (!reasons.is_empty()).then(|| format!("ignored excluded apps: {}", reasons.join("; ")))
}

/// Settings that are shown but never saved, with why.
fn keep_read_only(settings: Settings, error: String) -> (Settings, LoadState) {
    (
        settings,
        LoadState {
            read_only: true,
            error: Some(error),
        },
    )
}

/// Replaces each invalid group with its defaults.
fn sanitize(mut settings: Settings) -> Settings {
    let defaults = Settings::default();
    let check = |s: &Settings| s.validate().is_ok();
    let mut probe = defaults.clone();
    probe.recording = settings.recording.clone();
    if !check(&probe) {
        settings.recording = defaults.recording.clone();
    }
    let mut probe = defaults.clone();
    probe.ai = settings.ai.clone();
    if !check(&probe) {
        settings.ai = defaults.ai.clone();
    }
    let mut probe = defaults.clone();
    probe.export = settings.export.clone();
    if !check(&probe) {
        settings.export.accent = None;
    }
    // Keeps the mode and every usable pattern and word.
    let r = &mut settings.redaction;
    r.patterns.retain(|p| redact::check_pattern(p).is_ok());
    r.patterns.truncate(redact::detect::MAX_PATTERNS);
    r.words.retain(|w| redact::check_word(w).is_ok());
    r.words.truncate(redact::detect::MAX_WORDS);
    settings.version = SETTINGS_VERSION;
    settings
}

/// Reads at most [`MAX_SETTINGS_BYTES`]; `None` if there is no file.
fn read_capped(path: &Path) -> std::io::Result<Option<Vec<u8>>> {
    use std::io::Read;
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other("not a regular file"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_SETTINGS_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_SETTINGS_BYTES {
        return Err(std::io::Error::other("the file is too large"));
    }
    Ok(Some(bytes))
}

/// Renames an unusable settings file aside and returns the defaults.
fn set_aside(path: &Path, error: String) -> (Settings, LoadState) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut aside = path.as_os_str().to_owned();
    aside.push(format!(".corrupt-{secs}"));
    let note = match std::fs::rename(path, &aside) {
        Ok(()) => format!("{error}; it was renamed to {}", Path::new(&aside).display()),
        Err(_) => error,
    };
    (
        Settings::default(),
        LoadState {
            read_only: false,
            error: Some(note),
        },
    )
}

/// `s` quoted for a POSIX shell when it needs it.
pub fn shell_quote(s: &str) -> String {
    let plain = !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./:@%+=,".contains(&b));
    if plain {
        s.to_owned()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// The `claude mcp add` command that registers the GuideRecorder MCP server for
/// the library at `library`, with its write tools if `writable`.
pub fn mcp_add_command(mcp: &Path, library: &Path, writable: bool) -> String {
    format!(
        "claude mcp add guiderecorder -- {} --library {}{}",
        shell_quote(&mcp.to_string_lossy()),
        shell_quote_home(&library.to_string_lossy()),
        if writable { " --writable" } else { "" }
    )
}

/// Like [`shell_quote`], but a leading `~/` stays unquoted so the shell
/// still expands it to the home folder.
pub fn shell_quote_home(s: &str) -> String {
    match s.strip_prefix("~/") {
        Some("") => "~/".to_owned(),
        Some(rest) => format!("~/{}", shell_quote(rest)),
        None => shell_quote(s),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn scratch() -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/guiderecorder-app-tests")
            .join(format!(
                "settings-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn defaults_are_valid_and_round_trip() {
        let settings = Settings::default();
        settings.validate().unwrap();
        let json = serde_json::to_value(&settings).unwrap();
        assert_eq!(json["version"], SETTINGS_VERSION);
        assert_eq!(json["recording"]["typingIdleMs"], 1500);
        assert_eq!(json["recording"]["clickRing"], false);
        assert_eq!(json["ai"]["stepModel"], DEFAULT_STEP_MODEL);
        assert_eq!(json["export"]["format"], "pdf");
        assert_eq!(
            serde_json::from_value::<Settings>(json.clone()).unwrap(),
            settings
        );
        // Missing fields take their defaults.
        let partial: Settings =
            serde_json::from_str(r#"{"version":1,"ai":{"enabled":false}}"#).unwrap();
        assert!(!partial.ai.enabled);
        assert_eq!(partial.ai.guide_model, DEFAULT_GUIDE_MODEL);
        assert_eq!(json["ai"]["provider"], "anthropic");
        assert_eq!(json["ai"]["openAiCompatible"]["images"], true);
        assert_eq!(json["ai"]["lmStudio"]["baseUrl"], "");
    }

    #[test]
    fn ai_settings_from_before_providers_stay_on_anthropic() {
        let dir = scratch();
        let path = dir.join("settings.json");
        fs::write(
            &path,
            br#"{"version":2,"ai":{"enabled":true,"autoRun":false,
                "stepModel":"claude-sonnet-5-5","guideModel":"claude-opus-5-5",
                "language":"de","audience":"expert"}}"#,
        )
        .unwrap();
        let store = SettingsStore::load(Some(path.clone()));
        let view = store.view();
        assert!(view.load_error.is_none());
        let ai = &view.settings.ai;
        assert_eq!(ai.provider, Provider::Anthropic);
        assert_eq!(ai.step_model, "claude-sonnet-5-5");
        assert_eq!(ai.guide_model, "claude-opus-5-5");
        assert!(!ai.auto_run);
        assert_eq!(ai.language, "de");
        assert_eq!(ai.ollama, Endpoint::default());
        assert!(ai.endpoint().is_none() && ai.images());

        // A provider and its endpoint round-trip.
        let mut settings = store.get();
        settings.ai.provider = Provider::Ollama;
        settings.ai.ollama = Endpoint {
            base_url: String::new(),
            step_model: "qwen3-vl:8b".into(),
            guide_model: "hf.co/org/model:Q4_K_M".into(),
            images: false,
        };
        store.update(settings, &ChosenPaths::default()).unwrap();
        let saved = SettingsStore::load(Some(path)).get();
        assert_eq!(saved.ai.provider, Provider::Ollama);
        assert_eq!(saved.ai.endpoint(), Some(&saved.ai.ollama));
        assert_eq!(saved.ai.ollama.guide_model, "hf.co/org/model:Q4_K_M");
        assert!(!saved.ai.images());
        assert_eq!(saved.ai.step_model, "claude-sonnet-5-5");
    }

    #[test]
    fn excluded_apps_default_only_when_missing() {
        let defaults = default_excluded_apps();
        #[cfg(target_os = "macos")]
        {
            assert!(defaults.contains(&"com.1password.1password".to_owned()));
            assert!(defaults.contains(&"com.bitwarden.desktop".to_owned()));
            assert!(defaults.contains(&"com.apple.keychainaccess".to_owned()));
            assert!(defaults.contains(&"com.apple.MobileSMS".to_owned()));
        }
        assert!(defaults.len() <= MAX_EXCLUDED_APPS);
        assert!(defaults.iter().all(|id| is_bundle_id(id)), "{defaults:?}");
        assert_eq!(Settings::default().recording.excluded_apps, defaults);

        // No recording section, or one without the field: the defaults.
        let missing: Settings = serde_json::from_str(r#"{"version":1}"#).unwrap();
        assert_eq!(missing.recording.excluded_apps, defaults);
        let missing: Settings =
            serde_json::from_str(r#"{"version":1,"recording":{"typingIdleMs":900}}"#).unwrap();
        assert_eq!(missing.recording.excluded_apps, defaults);
        // A saved empty list stays empty.
        let empty: Settings =
            serde_json::from_str(r#"{"version":1,"recording":{"excludedApps":[]}}"#).unwrap();
        assert!(empty.recording.excluded_apps.is_empty());
        let mine: Settings = serde_json::from_str(
            r#"{"version":1,"recording":{"excludedApps":["com.apple.Safari"]}}"#,
        )
        .unwrap();
        assert_eq!(mine.recording.excluded_apps, ["com.apple.Safari"]);
    }

    #[test]
    fn value_checks() {
        for ok in [
            "",
            "CmdOrCtrl+Shift+R",
            "Alt+F5",
            "Ctrl+Option+Space",
            "Cmd+1",
        ] {
            assert!(is_hotkey(ok), "{ok}");
        }
        for bad in [
            "R",
            "Shift+R",
            "Cmd+",
            "Cmd+Cmd+R",
            "Hyper+R",
            "Cmd+F21",
            "Cmd+RR",
        ] {
            assert!(!is_hotkey(bad), "{bad}");
        }
        assert!(is_model_id("claude-sonnet-5-5"));
        assert!(is_model_id("qwen3-vl:8b") && is_model_id("lmstudio-community/gemma-3-12b"));
        assert!(!is_model_id("") && !is_model_id("a b"));
        for ok in [
            "",
            "https://api.openai.com/v1",
            "http://localhost:11434/v1",
            "http://192.168.1.5:8080",
        ] {
            assert!(is_base_url(ok), "{ok}");
        }
        for bad in [
            "localhost:11434",
            "ftp://x",
            "https://",
            "https:///v1",
            "https://a b",
            "https://x/v1?key=1",
        ] {
            assert!(!is_base_url(bad), "{bad}");
        }
        assert!(same_hotkey("Ctrl+Alt+R", "Option+Control+R"));
        assert!(same_hotkey("CmdOrCtrl+Shift+R", "Shift+Cmd+R"));
        assert!(!same_hotkey("Ctrl+Alt+R", "Ctrl+Alt+Shift+R"));
        assert!(!same_hotkey("", ""));
        assert!(is_language("") && is_language("de-CH") && is_language("zh-Hant-TW"));
        assert!(!is_language("en_US") && !is_language("-en"));
        for ok in [
            "com.1password.1password",
            "2BUA8C4S2C.com.agilebits.onepassword7-helper",
            "com.apple.Safari",
        ] {
            assert!(is_bundle_id(ok), "{ok}");
        }
        assert!(!is_bundle_id("Safari") && !is_bundle_id("com.x/y"));
        // Too broad: these would exclude every app under them.
        for broad in [
            "com.apple",
            "com",
            "com.",
            ".com.apple",
            "com..apple",
            "com.apple.",
        ] {
            assert!(!is_bundle_id(broad), "{broad}");
        }
        assert!(is_broad_bundle_id("com.apple") && !is_broad_bundle_id("Safari"));
        assert!(is_accent("#abc") && is_accent("#A1B2C3"));
        assert!(!is_accent("abc") && !is_accent("#abcd") && !is_accent("red"));

        let mut s = Settings::default();
        s.recording.hotkeys.pause = s.recording.hotkeys.record.clone();
        assert!(s.validate().unwrap_err().message.contains("used twice"));
        let mut s = Settings::default();
        s.recording.hotkeys.pause = "Alt+Ctrl+R".into();
        assert!(s.validate().unwrap_err().message.contains("used twice"));
        let mut s = Settings::default();
        s.recording.typing_idle_ms = 50;
        assert!(s.validate().is_err());
        let mut s = Settings::default();
        s.recording.excluded_apps = vec!["com.apple.Safari".into(), "nope".into()];
        assert!(s.validate().unwrap_err().message.contains("nope"));
        let mut s = Settings::default();
        s.recording.excluded_apps = vec!["com.apple".into()];
        let message = s.validate().unwrap_err().message;
        assert!(message.contains("\"com.apple\" is too broad"), "{message}");
        let mut s = Settings::default();
        s.export.accent = Some("blue".into());
        assert!(s.validate().is_err());
        let mut s = Settings::default();
        s.ai.ollama.step_model = "a b".into();
        assert!(s.validate().is_err());
        let mut s = Settings::default();
        s.ai.open_ai_compatible.base_url = "api.example.com".into();
        assert!(s.validate().unwrap_err().message.contains("URL"));
    }

    #[test]
    fn redaction_rules_are_checked_and_kept_on_load() {
        let settings = Settings::default();
        assert_eq!(
            serde_json::to_value(&settings).unwrap()["redaction"],
            serde_json::json!({ "mode": "suggest", "patterns": [], "words": [] })
        );
        let mut s = Settings {
            redaction: Redaction {
                mode: RedactionMode::Apply,
                patterns: vec![r"INC-\d+".into()],
                words: vec!["Acme Corp".into()],
            },
            ..Settings::default()
        };
        s.validate().unwrap();
        s.redaction.patterns.push("(".into());
        let message = s.validate().unwrap_err().message;
        assert!(message.contains(r#""(" is not valid"#), "{message}");
        s.redaction.patterns.pop();
        s.redaction.words.push("  ".into());
        assert!(s.validate().is_err());
        s.redaction.words.pop();
        s.redaction.patterns = vec![r"\d".into(); redact::detect::MAX_PATTERNS + 1];
        assert!(s.validate().is_err());

        // A stored bad pattern or word goes; the rest stays.
        let path = scratch().join(SETTINGS_FILE);
        fs::write(
            &path,
            r#"{"version":2,"redaction":{"mode":"apply","patterns":["(","INC-\\d+"],"words":["Acme",""]}}"#,
        )
        .unwrap();
        let store = SettingsStore::load(Some(path));
        let r = store.get().redaction;
        assert_eq!(r.mode, RedactionMode::Apply);
        assert_eq!(r.patterns, [r"INC-\d+"]);
        assert_eq!(r.words, ["Acme"]);
        assert!(store.view().load_error.is_some());
    }

    #[test]
    fn paths_change_only_to_chosen_ones() {
        let chosen = ChosenPaths::default();
        let old = Settings::default();
        let mut new = old.clone();
        new.export.folder = Some("/Users/me/Exports".into());
        assert!(new.check_paths(&old, &chosen).is_err());
        chosen.add_folder("/Users/me/Exports".into());
        new.check_paths(&old, &chosen).unwrap();
        // Unchanged or cleared paths need no dialog.
        new.check_paths(&new, &ChosenPaths::default()).unwrap();
        Settings::default()
            .check_paths(&new, &ChosenPaths::default())
            .unwrap();

        let mut logo = old.clone();
        logo.export.logo = Some("/etc/passwd".into());
        assert!(logo.check_paths(&old, &chosen).is_err());
        let mut library = old.clone();
        library.general.library = Some("/tmp/x".into());
        assert!(library.check_paths(&old, &chosen).is_err());
    }

    #[test]
    fn stores_atomically_and_reloads() {
        let dir = scratch();
        let path = dir.join("app/settings.json");
        let store = SettingsStore::load(Some(path.clone()));
        assert_eq!(store.get(), Settings::default());
        assert!(!path.exists(), "loading never writes");

        let mut s = store.get();
        s.ai.language = "de".into();
        s.version = 99;
        let saved = store.update(s, &ChosenPaths::default()).unwrap();
        assert_eq!(saved.version, SETTINGS_VERSION);
        let reloaded = SettingsStore::load(Some(path.clone()));
        assert_eq!(reloaded.get().ai.language, "de");
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);

        let mut bad = store.get();
        bad.recording.typing_idle_ms = 1;
        assert_eq!(
            store.update(bad, &ChosenPaths::default()).unwrap_err().kind,
            ErrorKind::Invalid
        );
        assert_eq!(store.get().recording.typing_idle_ms, 1500);

        let library = store
            .modify(|s| s.general.library = Some("/lib".into()))
            .unwrap();
        assert_eq!(library.general.library, Some(PathBuf::from("/lib")));
    }

    #[test]
    fn corrupt_files_are_set_aside_and_newer_ones_kept() {
        let dir = scratch();
        let path = dir.join("settings.json");
        fs::write(&path, b"{ not json").unwrap();
        let store = SettingsStore::load(Some(path.clone()));
        assert_eq!(store.get(), Settings::default());
        assert!(store.view().load_error.unwrap().contains("renamed"));
        assert!(!path.exists());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);

        fs::write(&path, br#"{"version":3,"ai":{"language":"fr"}}"#).unwrap();
        let store = SettingsStore::load(Some(path.clone()));
        assert!(store.view().read_only);
        assert_eq!(store.get().ai.language, "fr");
        let err = store
            .update(store.get(), &ChosenPaths::default())
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::ReadOnly);
        assert!(store.modify(|_| {}).is_err());
        assert!(fs::read_to_string(&path).unwrap().contains("\"version\":3"));

        // A newer file this version can't decode: defaults, read-only, and
        // the file is neither renamed nor saved over.
        let newer = br#"{"version":3,"export":{"format":"keynote"}}"#;
        fs::write(&path, newer).unwrap();
        let store = SettingsStore::load(Some(path.clone()));
        let view = store.view();
        assert!(view.read_only);
        assert_eq!(view.settings, Settings::default());
        assert!(view.load_error.unwrap().contains("newer GuideRecorder"));
        assert_eq!(
            store
                .update(store.get(), &ChosenPaths::default())
                .unwrap_err()
                .kind,
            ErrorKind::ReadOnly
        );
        assert!(store.modify(|_| {}).is_err());
        assert_eq!(fs::read(&path).unwrap(), newer);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 2, "nothing set aside");

        // Invalid values fall back per group; the rest is kept.
        fs::write(
            &path,
            br#"{"version":1,"recording":{"typingIdleMs":5},"ai":{"language":"it"}}"#,
        )
        .unwrap();
        let store = SettingsStore::load(Some(path));
        assert_eq!(store.get().recording.typing_idle_ms, 1500);
        assert_eq!(store.get().ai.language, "it");
        assert!(store.view().load_error.is_some());
    }

    #[test]
    fn broad_excluded_apps_are_dropped_on_load() {
        let dir = scratch();
        let path = dir.join("settings.json");
        // Hand-edited: a broad entry next to valid ones and a custom shortcut.
        fs::write(
            &path,
            br#"{"version":1,"recording":{"hotkeys":{"pause":"Ctrl+Alt+K"},
                "excludedApps":["com.1password.1password","com.apple","com.apple.Safari"]}}"#,
        )
        .unwrap();
        let store = SettingsStore::load(Some(path.clone()));
        let view = store.view();
        // Only the broad entry goes; the rest of Recording is kept.
        assert_eq!(
            view.settings.recording.excluded_apps,
            ["com.1password.1password", "com.apple.Safari"]
        );
        assert_eq!(view.settings.recording.hotkeys.pause, "Ctrl+Alt+K");
        assert!(!view.read_only);
        let error = view.load_error.unwrap();
        assert!(error.contains("\"com.apple\" is too broad"), "{error}");
        // Saving writes the list without it.
        store.update(store.get(), &ChosenPaths::default()).unwrap();
        assert!(!fs::read_to_string(&path).unwrap().contains("\"com.apple\""));

        // Alongside another invalid value: both are reported, and only that
        // value's group falls back.
        fs::write(
            &path,
            br#"{"version":1,"recording":{"excludedApps":["com.apple"]},"ai":{"language":"en_US"}}"#,
        )
        .unwrap();
        let view = SettingsStore::load(Some(path.clone())).view();
        assert!(view.settings.recording.excluded_apps.is_empty());
        assert_eq!(view.settings.ai.language, "");
        let error = view.load_error.unwrap();
        assert!(
            error.contains("too broad") && error.contains("en_US"),
            "{error}"
        );

        // A newer, read-only file is left alone on disk but not applied.
        let newer = br#"{"version":3,"recording":{"excludedApps":["com.apple"]}}"#;
        fs::write(&path, newer).unwrap();
        let view = SettingsStore::load(Some(path.clone())).view();
        assert!(view.read_only);
        assert!(view.settings.recording.excluded_apps.is_empty());
        assert!(view.load_error.unwrap().contains("too broad"));
        assert_eq!(fs::read(&path).unwrap(), newer);
    }

    #[test]
    fn old_default_shortcuts_are_migrated() {
        let dir = scratch();
        let path = dir.join("settings.json");
        // Version 1 defaults for record and manual step, a custom pause.
        fs::write(
            &path,
            br#"{"version":1,"recording":{"hotkeys":{"record":"CmdOrCtrl+Shift+R",
                "pause":"Ctrl+Alt+K","manualStep":"CmdOrCtrl+Shift+M"}}}"#,
        )
        .unwrap();
        let store = SettingsStore::load(Some(path.clone()));
        let view = store.view();
        assert!(view.load_error.is_none());
        let hotkeys = &view.settings.recording.hotkeys;
        assert_eq!(hotkeys.record, "Ctrl+Alt+R");
        assert_eq!(hotkeys.pause, "Ctrl+Alt+K");
        // It always worked as stored.
        assert_eq!(hotkeys.manual_step, "CmdOrCtrl+Shift+M");
        store.update(store.get(), &ChosenPaths::default()).unwrap();
        let saved: Settings = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved.version, 2);
        assert_eq!(saved.recording.hotkeys, store.get().recording.hotkeys);

        // A new default another shortcut already has is not taken, so
        // nothing is reset as a duplicate.
        fs::write(
            &path,
            br#"{"version":1,"recording":{"hotkeys":{"record":"CmdOrCtrl+Shift+R",
                "pause":"Alt+Ctrl+R","manualStep":"CmdOrCtrl+Shift+M"}}}"#,
        )
        .unwrap();
        let view = SettingsStore::load(Some(path.clone())).view();
        assert!(view.load_error.is_none());
        let hotkeys = &view.settings.recording.hotkeys;
        assert_eq!(hotkeys.record, "CmdOrCtrl+Shift+R");
        assert_eq!(hotkeys.pause, "Alt+Ctrl+R");

        // Chosen in version 2: kept.
        fs::write(
            &path,
            br#"{"version":2,"recording":{"hotkeys":{"pause":"CmdOrCtrl+Shift+P"}}}"#,
        )
        .unwrap();
        let store = SettingsStore::load(Some(path));
        assert_eq!(store.get().recording.hotkeys.pause, "CmdOrCtrl+Shift+P");
    }

    #[test]
    fn unreadable_files_are_kept_read_only() {
        let dir = scratch();
        // Not a regular file (a folder in its place).
        let path = dir.join("settings.json");
        fs::create_dir(&path).unwrap();
        let store = SettingsStore::load(Some(path.clone()));
        assert!(store.view().read_only);
        assert!(store.view().load_error.unwrap().contains("cannot read"));
        assert!(path.is_dir());
        assert!(store.modify(|_| {}).is_err());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1, "nothing renamed");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let locked = dir.join("locked.json");
            fs::write(&locked, br#"{"version":1}"#).unwrap();
            fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
            // Root can read anything; the check only means something otherwise.
            if fs::read(&locked).is_err() {
                let store = SettingsStore::load(Some(locked.clone()));
                assert!(store.view().read_only);
                assert!(locked.exists());
            }
            fs::set_permissions(&locked, fs::Permissions::from_mode(0o600)).unwrap();
        }
    }

    #[test]
    fn mcp_command_keeps_the_home_tilde() {
        assert_eq!(
            mcp_add_command(
                Path::new("/a/mcp"),
                Path::new("~/Documents/GuideRecorder"),
                false
            ),
            "claude mcp add guiderecorder -- /a/mcp --library ~/Documents/GuideRecorder"
        );
        assert_eq!(shell_quote_home("~/My Guides"), "~/'My Guides'");
        assert_eq!(shell_quote_home("~user/x"), "'~user/x'");
        assert_eq!(shell_quote_home("~/"), "~/");
    }

    #[test]
    fn mcp_command_adds_writable_when_allowed() {
        assert_eq!(
            mcp_add_command(Path::new("/a/mcp"), Path::new("/lib"), true),
            "claude mcp add guiderecorder -- /a/mcp --library /lib --writable"
        );
        // Off unless chosen, also in a settings file from before the option.
        let old: Settings = serde_json::from_str(r#"{"version":2}"#).unwrap();
        assert!(!old.mcp.writable);
    }

    #[test]
    fn theme_defaults_to_arctic_and_round_trips() {
        let old: Settings =
            serde_json::from_str(r#"{"version":2,"general":{"onboarded":true}}"#).unwrap();
        assert_eq!(old.general.theme, Theme::Arctic);
        let mut settings = Settings::default();
        settings.general.theme = Theme::PaperDark;
        let json = serde_json::to_value(&settings).unwrap();
        assert_eq!(json["general"]["theme"], "paperDark");
        let back: Settings = serde_json::from_value(json).unwrap();
        assert_eq!(back.general.theme, Theme::PaperDark);
    }

    #[test]
    fn mcp_command_quotes_paths() {
        assert_eq!(
            mcp_add_command(
                Path::new("/Applications/GuideRecorder.app/Contents/MacOS/guiderecorder-mcp"),
                Path::new("/Users/me/Documents/GuideRecorder"),
                false
            ),
            "claude mcp add guiderecorder -- /Applications/GuideRecorder.app/Contents/MacOS/guiderecorder-mcp --library /Users/me/Documents/GuideRecorder"
        );
        assert_eq!(
            mcp_add_command(Path::new("/a b/mcp"), Path::new("/it's"), false),
            r"claude mcp add guiderecorder -- '/a b/mcp' --library '/it'\''s'"
        );
    }
}
