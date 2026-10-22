//! Brand presets and layout templates: how every export of a library
//! looks, chosen by name instead of set up for each export.
//!
//! A [`Preset`] names a logo, colours, fonts, cover page fields, header and
//! footer text, page numbering, a [`Layout`] and optional custom CSS for
//! HTML. [`Preset::branding`] turns it into the [`Branding`] every writer
//! takes (checked with [`Branding::check`] as usual).
//!
//! # Files
//!
//! Presets live in the library, so a synced or shared library folder
//! carries them along:
//!
//! ```text
//! <root>/
//!   .presets/
//!     acme.preset.json      one preset, logo included (base64)
//!     library.json          { "default": "acme" }: the library's default
//! ```
//!
//! The folder is hidden, so the library never lists it as a guide folder.
//! A preset's id is its file stem (`acme`), made from its name when it is
//! first saved ([`preset_id`]) and kept when the name changes. Each file is
//! self-contained (the logo is embedded), so copying one file shares a
//! preset. Files are written atomically; a file that can't be read is
//! listed with its error rather than hiding the others.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use base64::Engine as _;
use serde::{Deserialize, Serialize};

use super::md::{Branding, MAX_LOGO_BYTES};
use crate::library::{write_atomic, Durability};

/// The hidden folder in a library root that holds its presets.
pub const PRESETS_DIR: &str = ".presets";
/// Extension of a preset file: `<id>.preset.json`.
pub const PRESET_EXTENSION: &str = ".preset.json";
/// The library's preset settings (its default preset) in [`PRESETS_DIR`].
pub const LIBRARY_FILE: &str = "library.json";
/// The preset format this version writes.
pub const PRESET_VERSION: u32 = 1;
/// Largest preset file read: a 2 MB logo in base64 plus the CSS.
pub const MAX_PRESET_BYTES: u64 = 4 * 1024 * 1024;
/// Longest custom stylesheet, in bytes.
pub const MAX_CSS_BYTES: usize = 100 * 1024;
/// Longest header, footer or cover field, in characters.
pub const MAX_TEXT_CHARS: usize = 200;
/// Longest preset name, in characters.
pub const MAX_NAME_CHARS: usize = 80;
/// Longest font family name, in characters.
pub const MAX_FAMILY_CHARS: usize = 64;

/// How steps are laid out on the page (HTML, PDF and Word; Markdown keeps
/// only [`Layout::Checklist`]; the other formats ignore it).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Layout {
    /// The step's image, then its text.
    #[default]
    ImageThenText,
    /// The image beside the text (stacked again on a narrow HTML page).
    SideBySide,
    /// Smaller images, about half the text width.
    Compact,
    /// A box to tick per step and its text, without images.
    Checklist,
}

impl Layout {
    pub const ALL: [Layout; 4] = [
        Layout::ImageThenText,
        Layout::SideBySide,
        Layout::Compact,
        Layout::Checklist,
    ];

    /// The name the CLI and the preset files use, e.g. `side-by-side`.
    pub fn name(self) -> &'static str {
        match self {
            Layout::ImageThenText => "image-then-text",
            Layout::SideBySide => "side-by-side",
            Layout::Compact => "compact",
            Layout::Checklist => "checklist",
        }
    }

    pub fn from_name(name: &str) -> Option<Layout> {
        Layout::ALL.into_iter().find(|l| l.name() == name)
    }

    /// Whether steps show their images.
    pub fn has_images(self) -> bool {
        self != Layout::Checklist
    }
}

/// A kind of typeface. The PDF has only bundled fonts, so it always uses
/// one of these: Lato, Libertinus Serif or DejaVu Sans Mono.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FontKind {
    #[default]
    Sans,
    Serif,
    Mono,
}

/// A font: a kind, and optionally a family by name (such as the company's
/// brand font) that HTML and Word use where it is installed, falling back
/// to the kind.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Font {
    pub kind: FontKind,
    /// Letters, digits, spaces, `-`, `_` and `.`; at most
    /// [`MAX_FAMILY_CHARS`]. Blank means none.
    pub family: Option<String>,
}

impl Font {
    /// The family, trimmed; `None` when blank.
    pub fn family(&self) -> Option<&str> {
        self.family
            .as_deref()
            .map(str::trim)
            .filter(|f| !f.is_empty())
    }
}

/// The text and heading fonts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Fonts {
    pub body: Font,
    pub heading: Font,
}

/// Colours as `#rgb` or `#rrggbb`; blank or absent keeps the format's own.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Colors {
    /// Step numbers, links, rules and decision boxes (and headings unless
    /// `heading` is set).
    pub accent: Option<String>,
    /// Section and step headings and the title.
    pub heading: Option<String>,
    /// Body text. HTML uses it in the light appearance only, so dark mode
    /// stays readable.
    pub text: Option<String>,
}

/// What a cover page shows besides the title, summary and cover image.
/// Blank fields are left out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CoverFields {
    pub company: String,
    pub author: String,
    /// The document's version, e.g. `2.1`.
    pub version: String,
    /// Show the day the guide was last changed.
    pub date: bool,
}

impl Default for CoverFields {
    fn default() -> Self {
        Self {
            company: String::new(),
            author: String::new(),
            version: String::new(),
            date: true,
        }
    }
}

impl CoverFields {
    /// Whether any text field is filled in.
    pub fn has_text(&self) -> bool {
        [&self.company, &self.author, &self.version]
            .iter()
            .any(|f| !f.trim().is_empty())
    }
}

/// A named brand preset, as stored in `<id>.preset.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Preset {
    pub version: u32,
    pub name: String,
    /// A PNG or JPEG, base64; checked like any logo ([`Branding::check`]).
    pub logo: Option<String>,
    pub colors: Colors,
    pub fonts: Fonts,
    pub cover: CoverFields,
    /// Text at the top of every page after the cover (PDF and Word) or of
    /// the page (HTML). Blank: the PDF shows the guide title as before.
    pub header: String,
    /// Text at the bottom of every page (PDF and Word) or of the page (HTML).
    pub footer: String,
    /// Number the pages (PDF and Word).
    pub page_numbers: bool,
    pub layout: Layout,
    /// Added after the stylesheet of an HTML export (not the walkthrough).
    /// At most [`MAX_CSS_BYTES`], without `<`.
    pub css: String,
}

impl Default for Preset {
    fn default() -> Self {
        Self {
            version: PRESET_VERSION,
            name: String::new(),
            logo: None,
            colors: Colors::default(),
            fonts: Fonts::default(),
            cover: CoverFields::default(),
            header: String::new(),
            footer: String::new(),
            page_numbers: true,
            layout: Layout::default(),
            css: String::new(),
        }
    }
}

/// Why a preset was refused or could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum PresetError {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("{path}: not a valid preset: {source}")]
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("{0}: the preset file is over {MAX_PRESET_BYTES} bytes")]
    TooLarge(PathBuf),
    #[error("{0}: the preset was made by a newer version of GuideRecorder")]
    Newer(PathBuf),
    #[error("no preset {0:?} in this library")]
    NotFound(String),
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T, E = PresetError> = std::result::Result<T, E>;

fn invalid(message: impl Into<String>) -> PresetError {
    PresetError::Invalid(message.into())
}

impl Preset {
    /// Checks what [`Branding::check`] does not: the name, text lengths,
    /// font families and the custom CSS. The logo and colours are checked
    /// by [`Branding::check`] on [`Preset::branding`].
    pub fn validate(&self) -> Result<()> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err(invalid("a preset needs a name"));
        }
        if name.chars().count() > MAX_NAME_CHARS {
            return Err(invalid(format!(
                "the preset name is longer than {MAX_NAME_CHARS} characters"
            )));
        }
        let texts = [
            ("header", &self.header),
            ("footer", &self.footer),
            ("company", &self.cover.company),
            ("author", &self.cover.author),
            ("version", &self.cover.version),
        ];
        for (what, text) in texts {
            check_text(what, text)?;
        }
        for (what, font) in [("text", &self.fonts.body), ("heading", &self.fonts.heading)] {
            if let Some(family) = font.family().filter(|f| !is_font_family(f)) {
                return Err(invalid(format!(
                    "the {what} font {family:?} is not a font name (letters, digits, spaces, - _ .; at most {MAX_FAMILY_CHARS})"
                )));
            }
        }
        check_css(&self.css)?;
        Ok(())
    }

    /// The logo's bytes, decoded from base64; `None` without one.
    pub fn logo_bytes(&self) -> Result<Option<Vec<u8>>> {
        let Some(logo) = self
            .logo
            .as_deref()
            .map(str::trim)
            .filter(|l| !l.is_empty())
        else {
            return Ok(None);
        };
        // Base64 grows by 4/3; refuse before decoding anything bigger.
        if logo.len() > MAX_LOGO_BYTES.div_ceil(3) * 4 + 4 {
            return Err(invalid("the logo is over 2 MB"));
        }
        base64::engine::general_purpose::STANDARD
            .decode(logo)
            .map(Some)
            .map_err(|_| invalid("the logo is not valid base64"))
    }

    /// Stores `bytes` (a PNG or JPEG) as the logo.
    pub fn set_logo(&mut self, bytes: Option<&[u8]>) {
        self.logo = bytes.map(|b| base64::engine::general_purpose::STANDARD.encode(b));
    }

    /// The branding the writers take, validated first ([`Preset::validate`]).
    pub fn branding(&self) -> Result<Branding> {
        self.validate()?;
        let given = |s: &Option<String>| {
            s.as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        let text = |s: &str| s.trim().to_owned();
        Ok(Branding {
            logo: self.logo_bytes()?,
            accent: given(&self.colors.accent),
            heading_color: given(&self.colors.heading),
            text_color: given(&self.colors.text),
            fonts: self.fonts.clone(),
            cover: Some(CoverFields {
                company: text(&self.cover.company),
                author: text(&self.cover.author),
                version: text(&self.cover.version),
                date: self.cover.date,
            }),
            header: text(&self.header),
            footer: text(&self.footer),
            page_numbers: Some(self.page_numbers),
            layout: self.layout,
            css: self.css.clone(),
        })
    }
}

fn check_text(what: &str, text: &str) -> Result<()> {
    if text.chars().count() > MAX_TEXT_CHARS {
        return Err(invalid(format!(
            "the {what} is longer than {MAX_TEXT_CHARS} characters"
        )));
    }
    if text.chars().any(char::is_control) {
        return Err(invalid(format!(
            "the {what} has control characters or line breaks"
        )));
    }
    Ok(())
}

/// Whether `family` is a font family name: letters (any script), digits,
/// spaces, `-`, `_` and `.`, at most [`MAX_FAMILY_CHARS`].
pub fn is_font_family(family: &str) -> bool {
    !family.is_empty()
        && family.chars().count() <= MAX_FAMILY_CHARS
        && family
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.'))
}

/// Custom CSS: at most [`MAX_CSS_BYTES`] and no `<`, so it can't close the
/// `<style>` element it is written into. What it can do is limited by the
/// page's Content-Security-Policy (no requests; only `data:` images).
pub fn check_css(css: &str) -> Result<()> {
    if css.len() > MAX_CSS_BYTES {
        return Err(invalid(format!(
            "the custom CSS is over {} kB",
            MAX_CSS_BYTES / 1024
        )));
    }
    if css.contains('<') {
        return Err(invalid("the custom CSS can't contain <"));
    }
    Ok(())
}

/// A file stem for a preset named `name`: lowercase letters and digits
/// separated by `-`, at most 40 bytes; `preset` when nothing is left.
pub fn preset_id(name: &str) -> String {
    let mut id = String::new();
    for c in name.trim().chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            id.push(c);
        } else if !id.is_empty() && !id.ends_with('-') {
            id.push('-');
        }
        if id.len() >= 40 {
            break;
        }
    }
    let id = id.trim_end_matches('-');
    if id.is_empty() {
        "preset".to_owned()
    } else {
        id.to_owned()
    }
}

/// Whether `id` is a preset id [`PresetStore`] accepts: what
/// [`preset_id`] makes, possibly with a `-N` suffix.
pub fn is_preset_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 48
        && !id.starts_with('-')
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// A preset as listed: its id, and the preset or why it can't be read.
#[derive(Debug)]
pub struct PresetEntry {
    pub id: String,
    pub preset: Result<Preset>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct LibraryPresets {
    default: Option<String>,
}

/// The presets of the library at `root` (see the module docs).
#[derive(Debug, Clone)]
pub struct PresetStore {
    dir: PathBuf,
}

impl PresetStore {
    pub fn new(library_root: impl AsRef<Path>) -> Self {
        Self {
            dir: library_root.as_ref().join(PRESETS_DIR),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}{PRESET_EXTENSION}"))
    }

    fn io(path: &Path) -> impl FnOnce(io::Error) -> PresetError + '_ {
        move |source| PresetError::Io {
            path: path.to_path_buf(),
            source,
        }
    }

    /// Every preset, by name (ids break ties). No folder: none.
    pub fn list(&self) -> Result<Vec<PresetEntry>> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(Self::io(&self.dir)(e)),
        };
        let mut presets = Vec::new();
        for entry in entries {
            let entry = entry.map_err(Self::io(&self.dir))?;
            let name = entry.file_name();
            let Some(id) = name
                .to_str()
                .and_then(|n| n.strip_suffix(PRESET_EXTENSION))
                .filter(|id| is_preset_id(id))
            else {
                continue;
            };
            presets.push(PresetEntry {
                id: id.to_owned(),
                preset: self.read(id),
            });
        }
        let key = |e: &PresetEntry| {
            let name = e.preset.as_ref().map(|p| p.name.to_lowercase()).ok();
            (name, e.id.clone())
        };
        presets.sort_by_key(key);
        Ok(presets)
    }

    /// The preset with id `id`.
    pub fn read(&self, id: &str) -> Result<Preset> {
        if !is_preset_id(id) {
            return Err(PresetError::NotFound(id.to_owned()));
        }
        let path = self.path(id);
        let file = match fs::File::open(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(PresetError::NotFound(id.to_owned()))
            }
            Err(e) => return Err(Self::io(&path)(e)),
        };
        let mut bytes = Vec::new();
        file.take(MAX_PRESET_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(Self::io(&path))?;
        if bytes.len() as u64 > MAX_PRESET_BYTES {
            return Err(PresetError::TooLarge(path));
        }
        let preset: Preset =
            serde_json::from_slice(&bytes).map_err(|source| PresetError::Json {
                path: path.clone(),
                source,
            })?;
        if preset.version > PRESET_VERSION {
            return Err(PresetError::Newer(path));
        }
        Ok(preset)
    }

    /// The preset whose id is `key`, or else whose name is `key` (ignoring
    /// case and surrounding spaces): what the CLI's `--preset` takes.
    pub fn find(&self, key: &str) -> Result<(String, Preset)> {
        let key = key.trim();
        if is_preset_id(key) {
            match self.read(key) {
                Ok(preset) => return Ok((key.to_owned(), preset)),
                Err(PresetError::NotFound(_)) => {}
                Err(e) => return Err(e),
            }
        }
        let wanted = key.to_lowercase();
        self.list()?
            .into_iter()
            .find_map(|e| match e.preset {
                Ok(p) if p.name.trim().to_lowercase() == wanted => Some((e.id, p)),
                _ => None,
            })
            .ok_or_else(|| PresetError::NotFound(key.to_owned()))
    }

    /// Saves `preset` (validated first, logo and colours included) as `id`,
    /// or as a new preset with an id made from its name; returns the id.
    pub fn save(&self, id: Option<&str>, preset: &Preset) -> Result<String> {
        preset
            .branding()?
            .check()
            .map_err(|e| invalid(e.to_string()))?;
        let id = match id {
            Some(id) if is_preset_id(id) => id.to_owned(),
            Some(id) => return Err(PresetError::NotFound(id.to_owned())),
            None => self.new_id(&preset.name)?,
        };
        fs::create_dir_all(&self.dir).map_err(Self::io(&self.dir))?;
        let preset = Preset {
            version: PRESET_VERSION,
            name: preset.name.trim().to_owned(),
            ..preset.clone()
        };
        let mut json = serde_json::to_vec_pretty(&preset).map_err(|source| PresetError::Json {
            path: self.path(&id),
            source,
        })?;
        json.push(b'\n');
        let path = self.path(&id);
        write_atomic(&path, &json, Durability::Full).map_err(|e| invalid(e.to_string()))?;
        Ok(id)
    }

    /// An unused id for `name`: its [`preset_id`], else with `-2`, `-3`, ...
    fn new_id(&self, name: &str) -> Result<String> {
        let base = preset_id(name);
        for n in 1.. {
            let id = if n == 1 {
                base.clone()
            } else {
                format!("{base}-{n}")
            };
            match fs::symlink_metadata(self.path(&id)) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(id),
                Err(e) => return Err(Self::io(&self.path(&id))(e)),
                Ok(_) => {}
            }
        }
        unreachable!()
    }

    /// Deletes the preset `id`; it stops being the default if it was.
    pub fn delete(&self, id: &str) -> Result<()> {
        if !is_preset_id(id) {
            return Err(PresetError::NotFound(id.to_owned()));
        }
        let path = self.path(id);
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(PresetError::NotFound(id.to_owned()))
            }
            Err(e) => return Err(Self::io(&path)(e)),
        }
        if self.default_id()?.as_deref() == Some(id) {
            self.set_default(None)?;
        }
        Ok(())
    }

    /// The id of the library's default preset, if it names one that
    /// exists.
    pub fn default_id(&self) -> Result<Option<String>> {
        let path = self.dir.join(LIBRARY_FILE);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(Self::io(&path)(e)),
        };
        // An unreadable file is no default rather than an error: exports
        // still work, and saving a default replaces it.
        let settings: LibraryPresets = serde_json::from_slice(&bytes).unwrap_or_default();
        Ok(settings
            .default
            .filter(|id| is_preset_id(id) && self.path(id).is_file()))
    }

    /// Makes `id` the library's default preset (`None`: no default).
    pub fn set_default(&self, id: Option<&str>) -> Result<()> {
        if let Some(id) = id {
            if !is_preset_id(id) || !self.path(id).is_file() {
                return Err(PresetError::NotFound(id.to_owned()));
            }
        }
        fs::create_dir_all(&self.dir).map_err(Self::io(&self.dir))?;
        let settings = LibraryPresets {
            default: id.map(str::to_owned),
        };
        let mut json = serde_json::to_vec_pretty(&settings).expect("serializable");
        json.push(b'\n');
        write_atomic(&self.dir.join(LIBRARY_FILE), &json, Durability::Full)
            .map_err(|e| invalid(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(name: &str) -> Preset {
        Preset {
            name: name.into(),
            ..Preset::default()
        }
    }

    #[test]
    fn ids_come_from_names() {
        assert_eq!(preset_id("Acme Corp"), "acme-corp");
        assert_eq!(preset_id("  Ünïcode / Brand!  "), "n-code-brand");
        assert_eq!(preset_id("***"), "preset");
        assert!(preset_id(&"x".repeat(90)).len() <= 40);
        assert!(is_preset_id("acme-2"));
        for bad in ["", "-x", "Acme", "a/b", "..", "a.b"] {
            assert!(!is_preset_id(bad), "{bad}");
        }
    }

    #[test]
    fn layouts_round_trip_by_name() {
        for layout in Layout::ALL {
            assert_eq!(Layout::from_name(layout.name()), Some(layout));
            let json = serde_json::to_string(&layout).unwrap();
            assert_eq!(json, format!("\"{}\"", layout.name()));
        }
        assert_eq!(Layout::from_name("grid"), None);
    }

    #[test]
    fn a_preset_file_fills_in_what_it_leaves_out() {
        let preset: Preset = serde_json::from_str(r#"{"name":"Acme"}"#).unwrap();
        assert_eq!(preset, named("Acme"));
        assert!(preset.page_numbers);
        assert!(preset.cover.date);
    }

    #[test]
    fn validation_refuses_what_could_break_out() {
        let mut preset = named("Acme");
        preset.css = "body{} </style><script>".into();
        assert!(preset.validate().is_err());
        preset.css = "body { color: red }".into();
        preset.validate().unwrap();
        preset.fonts.body.family = Some("Inter\"; x".into());
        assert!(preset.validate().is_err());
        preset.fonts.body.family = Some("Source Sans 3".into());
        preset.validate().unwrap();
        preset.footer = "a\nb".into();
        assert!(preset.validate().is_err());
        assert!(named(" ").validate().is_err());
    }

    #[test]
    fn branding_carries_the_preset() {
        let mut preset = named("Acme");
        preset.colors.accent = Some(" #C0F ".into());
        preset.colors.heading = Some("".into());
        preset.header = "  Internal ".into();
        preset.layout = Layout::Compact;
        preset.page_numbers = false;
        let branding = preset.branding().unwrap();
        assert_eq!(branding.accent.as_deref(), Some("#C0F"));
        assert_eq!(branding.heading_color, None);
        assert_eq!(branding.header, "Internal");
        assert_eq!(branding.page_numbers, Some(false));
        let checked = branding.check().unwrap();
        assert_eq!(checked.accent_hex().as_deref(), Some("#cc00ff"));
        assert_eq!(checked.layout(), Layout::Compact);
    }

    #[test]
    fn the_logo_round_trips_through_base64() {
        let mut preset = named("Acme");
        preset.set_logo(Some(b"\x89PNG..."));
        assert_eq!(preset.logo_bytes().unwrap().unwrap(), b"\x89PNG...");
        preset.logo = Some("not base64!".into());
        assert!(preset.logo_bytes().is_err());
    }

    #[test]
    fn the_store_saves_lists_finds_and_deletes() {
        let dir = tempfile::tempdir().unwrap();
        let store = PresetStore::new(dir.path());
        assert!(store.list().unwrap().is_empty());
        assert_eq!(store.default_id().unwrap(), None);

        let acme = store.save(None, &named("Acme")).unwrap();
        let again = store.save(None, &named("ACME")).unwrap();
        let zeta = store.save(None, &named("Zeta")).unwrap();
        assert_eq!((acme.as_str(), again.as_str()), ("acme", "acme-2"));
        assert!(dir.path().join(".presets/acme.preset.json").is_file());

        let names: Vec<_> = store
            .list()
            .unwrap()
            .into_iter()
            .map(|e| (e.id, e.preset.unwrap().name))
            .collect();
        assert_eq!(
            names,
            [
                ("acme".to_owned(), "Acme".to_owned()),
                ("acme-2".to_owned(), "ACME".to_owned()),
                ("zeta".to_owned(), "Zeta".to_owned()),
            ]
        );

        // Renaming keeps the id.
        store.save(Some(&zeta), &named("Omega")).unwrap();
        assert_eq!(store.read(&zeta).unwrap().name, "Omega");
        assert_eq!(store.find("omega").unwrap().0, "zeta");
        assert_eq!(store.find("zeta").unwrap().1.name, "Omega");
        assert!(matches!(store.find("nope"), Err(PresetError::NotFound(_))));

        store.set_default(Some(&zeta)).unwrap();
        assert_eq!(store.default_id().unwrap().as_deref(), Some("zeta"));
        assert!(store.set_default(Some("missing")).is_err());
        store.delete(&zeta).unwrap();
        assert_eq!(store.default_id().unwrap(), None);
        assert!(matches!(store.read(&zeta), Err(PresetError::NotFound(_))));
    }

    #[test]
    fn a_broken_file_is_listed_with_its_error() {
        let dir = tempfile::tempdir().unwrap();
        let store = PresetStore::new(dir.path());
        store.save(None, &named("Good")).unwrap();
        fs::write(store.dir().join("bad.preset.json"), "{").unwrap();
        fs::write(
            store.dir().join("newer.preset.json"),
            r#"{"version": 99, "name": "Newer"}"#,
        )
        .unwrap();
        fs::write(store.dir().join("Not An Id.preset.json"), "{}").unwrap();
        let entries = store.list().unwrap();
        assert_eq!(entries.len(), 3);
        let failed: Vec<_> = entries
            .iter()
            .filter(|e| e.preset.is_err())
            .map(|e| e.id.as_str())
            .collect();
        assert_eq!(failed, ["bad", "newer"]);
    }

    #[test]
    fn saving_refuses_an_invalid_preset() {
        let dir = tempfile::tempdir().unwrap();
        let store = PresetStore::new(dir.path());
        let mut preset = named("Acme");
        preset.colors.accent = Some("blue".into());
        assert!(store.save(None, &preset).is_err());
        assert!(!store.dir().exists());
        assert!(store.save(Some("../x"), &named("Acme")).is_err());
    }
}
