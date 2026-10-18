//! PDF writer over [`RenderGuide`] via embedded Typst (typst + typst-pdf).
//! See design scratchpad #443.
//!
//! # Layout
//!
//! The template is `pdf/guide.typ`:
//!
//! - A cover page with the title ("Untitled" / "Unbenannt" when blank),
//!   summary and date (the [`PdfOptions::timestamp`] day; no date without
//!   one), and the optional [`CheckedBranding`] logo (top of the cover; the
//!   checked, upright PNG) and accent colour (cover rule and section
//!   headings).
//! - Every later page has the guide title as a header and "page / total" as
//!   a footer.
//! - Intro, then a prerequisites heading with its text. Sections are level 1
//!   headings; steps are level 2 headings "1. ", "2. ", ... (so both show up
//!   as PDF bookmarks), followed by the step image and the description, in
//!   the same order as the other writers. A step with an empty title is
//!   headed "N. Step N" ("N. Page N" in an annotation guide).
//! - Step images show their hotspots as numbered markers; a numbered legend
//!   (bold title, then the description with its line breaks kept) follows
//!   the image, before the description.
//! - A step's heading and image are one unbreakable block that sticks to the
//!   description, so the heading, image and the start of the description
//!   always share a page; the description itself flows on across pages,
//!   however long it is. Images are shown at their 96 dpi size, at
//!   most the text width, and at most [`MAX_IMAGE_HEIGHT_RATIO`] of the text
//!   height, keeping the aspect ratio. A missing or broken image becomes the
//!   renderer's placeholder and a warning in the [`ExportReport`].
//! - Notes are tinted boxes with a coloured left rule and a label from
//!   [`Labels`] ([`PdfOptions::labels`], or [`Labels::for_language`]).
//! - The Markdown subset maps to bold, italic, code (monospace on a grey
//!   background), links (to the AST's already validated URLs), hard breaks
//!   and lists. A URL longer than Typst allows ([`MAX_URL_LEN`] bytes) is
//!   shown as plain text, with a note in [`ExportReport::notes`].
//!
//! # Fonts
//!
//! Only bundled fonts are loaded, never system fonts, so output is the same
//! on every machine: Lato (regular, bold, italic, bold italic) from
//! `assets/fonts` for text, and Typst's own fonts from `typst-assets`
//! (DejaVu Sans Mono, New Computer Modern, Libertinus Serif) for code and
//! as a fallback for characters Lato lacks, such as `⌘ ⇧ ⌥`.
//!
//! # Injection boundary
//!
//! No guide text is ever spliced into Typst source. The template is a fixed
//! file; all guide text reaches it as JSON strings in a virtual
//! `/guide.json`, and the template only places those strings as text. So
//! `#panic("x")`, `$x$`, `@ref` or `<label>` in a guide print literally.
//! The [`World`] serves exactly the template, `/guide.json`, the step images
//! and the logo from memory: it has no file system access, rejects every
//! package import, and offers no other source file.
//!
//! # Determinism
//!
//! The same guide and options give the same bytes: fonts are bundled,
//! `datetime.today()` and the PDF dates come from
//! [`PdfOptions::timestamp`] (1970-01-01 when unset, clamped to Typst's
//! years -9999 to 9999), and the PDF document id is derived from the guide
//! id.
//!
//! # Errors
//!
//! Typst diagnostics become [`PdfError::Typst`]; Typst warnings go to
//! [`ExportReport::notes`]. A panic inside Typst is caught and reported as
//! [`PdfError::Typst`] too.

use std::any::Any;
use std::collections::HashMap;
use std::fs;
use std::io;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use chrono::{DateTime, Datelike, NaiveDate, Timelike, Utc};
use serde_json::{json, Value};
use typst::diag::{FileError, FileResult, PackageError, Severity, SourceDiagnostic};
use typst::foundations::{Bytes, Datetime, Duration as TypstDuration, Smart};
use typst::layout::{Frame, FrameItem, Point};
use typst::syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World, WorldExt};
use typst_pdf::Timestamp;

use super::annotate::DEFAULT_HOTSPOT_COLOR;
use super::ast::{plain_text, Block, Inline};
use super::docx::Paper;
use super::md::Labels;
pub use super::md::{Branding, CheckedBranding};
use super::raster::{render_step_image, ImageOptions};
use super::render::{RenderGuide, RenderItem};
use super::report::ExportReport;
use crate::library::GuideHandle;
use crate::model::NoteKind;

/// The Typst template; see the module docs.
const TEMPLATE: &str = include_str!("pdf/guide.typ");

/// The bundled Lato faces; Typst's own fonts are added in [`FONT_SET`].
const LATO: &[&[u8]] = &[
    include_bytes!("../../assets/fonts/Lato-Regular.ttf"),
    include_bytes!("../../assets/fonts/Lato-Bold.ttf"),
    include_bytes!("../../assets/fonts/Lato-Italic.ttf"),
    include_bytes!("../../assets/fonts/Lato-BoldItalic.ttf"),
];

/// Longest link URL Typst accepts, in bytes.
pub const MAX_URL_LEN: usize = 8000;

/// Largest share of the text height a step image may take, leaving room for
/// the heading and the first description block on the same page.
pub const MAX_IMAGE_HEIGHT_RATIO: f64 = 0.5;

/// Accent colour when [`CheckedBranding::accent`] is unset.
const DEFAULT_ACCENT: [u8; 3] = [0x1a, 0x73, 0xe8];

/// Options for [`export`] and [`render`].
#[derive(Debug, Clone, PartialEq)]
pub struct PdfOptions {
    /// Replace an existing file at the output path.
    pub overwrite: bool,
    /// How step images are rendered. `max_width` bounds the embedded
    /// image's pixel width (and so the file size); the displayed size is set
    /// by the page, see the module docs.
    pub image: ImageOptions,
    /// Cover date, `datetime.today()` and the PDF creation date, e.g.
    /// `guide.updated_at`. `None` shows no cover date and uses 1970-01-01
    /// elsewhere, so output never depends on the clock.
    pub timestamp: Option<DateTime<Utc>>,
    pub paper: Paper,
    /// Fixed words; `None` uses [`Labels::for_language`] of the guide's
    /// language.
    pub labels: Option<Labels>,
    pub branding: CheckedBranding,
}

impl Default for PdfOptions {
    fn default() -> Self {
        Self {
            overwrite: false,
            image: ImageOptions {
                max_width: Some(1600),
                ..ImageOptions::default()
            },
            timestamp: None,
            paper: Paper::default(),
            labels: None,
            branding: CheckedBranding::default(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PdfError {
    #[error("{} already exists", .0.display())]
    Exists(PathBuf),
    #[error(transparent)]
    Io(#[from] io::Error),
    /// Typst failed to compile the document or write the PDF.
    #[error("could not typeset the PDF: {}", .0.join("; "))]
    Typst(Vec<String>),
}

pub type Result<T, E = PdfError> = std::result::Result<T, E>;

/// A typeset guide, before it is written anywhere.
#[derive(Debug, Clone)]
pub struct PdfOutput {
    /// The PDF file.
    pub pdf: Vec<u8>,
    /// The text of each page as laid out, one line per line of text (in
    /// layout order, which is reading order for this template).
    pub page_text: Vec<String>,
    /// Step image warnings, plus Typst warnings (formatted like errors in
    /// [`PdfError::Typst`]) and other degradations in `notes`.
    pub report: ExportReport,
    /// Time spent typesetting and writing the PDF, excluding image
    /// rendering.
    pub compile_time: Duration,
}

impl PdfOutput {
    pub fn page_count(&self) -> usize {
        self.page_text.len()
    }
}

/// Writes `guide` as a PDF file at `out_path`, reading step images from the
/// bundle at `handle`. The file is written to a temporary file next to it
/// and renamed into place, so a failed export leaves nothing behind.
pub fn export(
    guide: &RenderGuide,
    handle: &GuideHandle,
    out_path: impl AsRef<Path>,
    opts: &PdfOptions,
) -> Result<ExportReport> {
    let out_path = out_path.as_ref();
    if !opts.overwrite && out_path.exists() {
        return Err(PdfError::Exists(out_path.to_path_buf()));
    }
    let output = render(guide, handle, opts)?;
    write_atomically(out_path, &output.pdf, opts.overwrite)?;
    let mut report = output.report;
    report.files.push(out_path.to_path_buf());
    Ok(report)
}

/// Typesets `guide` in memory. [`export`] is this plus writing the file.
pub fn render(guide: &RenderGuide, handle: &GuideHandle, opts: &PdfOptions) -> Result<PdfOutput> {
    let branding = &opts.branding;
    let accent = branding.accent().unwrap_or(DEFAULT_ACCENT);
    let mut report = ExportReport::default();
    let labels = opts
        .labels
        .clone()
        .unwrap_or_else(|| Labels::for_language(&guide.language));
    let now = typst_datetime(opts.timestamp);
    let mut files = Vec::new();
    let data = guide_data(
        guide,
        handle,
        opts,
        &labels,
        &now,
        accent,
        branding.logo().map(|l| (l.width(), l.height())),
        &mut files,
        &mut report,
    );
    files.push(("/guide.json".to_owned(), data.to_string().into_bytes()));
    if let Some(logo) = branding.logo() {
        files.push(("/logo".to_owned(), logo.png().to_vec()));
    }

    let started = Instant::now();
    let world = GuideWorld::new(files, now);
    // Typst memoizes across compilations; drop what older exports left, on
    // every path out of here.
    let _evict = EvictOnDrop;
    let warned = guard(|| typst::compile(&world))?;
    report.notes.extend(world.format(&warned.warnings));
    let document = warned
        .output
        .map_err(|e| PdfError::Typst(world.format(&e)))?;
    let pdf_opts = typst_pdf::PdfOptions {
        ident: Smart::Custom(guide.id.to_string()),
        creator: Smart::Custom(Some("GuideRecorder".into())),
        timestamp: Some(Timestamp::new_utc(world.now)),
        ..typst_pdf::PdfOptions::default()
    };
    let pdf = guard(|| typst_pdf::pdf(&document, &pdf_opts))?
        .map_err(|e| PdfError::Typst(world.format(&e)))?;
    let compile_time = started.elapsed();
    let page_text = document
        .pages()
        .iter()
        .map(|page| page_text(&page.frame))
        .collect();
    Ok(PdfOutput {
        pdf,
        page_text,
        report,
        compile_time,
    })
}

/// Runs a Typst call, turning a panic into [`PdfError::Typst`].
fn guard<T>(f: impl FnOnce() -> T) -> Result<T> {
    catch_unwind(AssertUnwindSafe(f)).map_err(|panic| {
        PdfError::Typst(vec![format!("Typst panicked: {}", panic_message(&*panic))])
    })
}

fn panic_message(panic: &(dyn Any + Send)) -> &str {
    panic
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("unknown panic")
}

struct EvictOnDrop;

impl Drop for EvictOnDrop {
    fn drop(&mut self) {
        typst::comemo::evict(10);
    }
}

fn write_atomically(out_path: &Path, bytes: &[u8], overwrite: bool) -> Result<()> {
    let dir = match out_path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    let mut tmp = tempfile::Builder::new()
        .prefix(".guiderecorder-export-")
        .suffix(".pdf.tmp")
        .tempfile_in(dir)?;
    io::Write::write_all(tmp.as_file_mut(), bytes)?;
    tmp.as_file().sync_all()?;
    if overwrite {
        tmp.persist(out_path).map_err(|e| e.error)?;
    } else {
        tmp.persist_noclobber(out_path).map_err(|e| {
            if e.error.kind() == io::ErrorKind::AlreadyExists {
                PdfError::Exists(out_path.to_path_buf())
            } else {
                PdfError::Io(e.error)
            }
        })?;
    }
    if let Some(dir) = out_path.parent().filter(|d| !d.as_os_str().is_empty()) {
        // Make the rename durable; not all platforms can open a directory.
        if let Ok(dir) = fs::File::open(dir) {
            let _ = dir.sync_all();
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Guide data for the template

/// The `/guide.json` the template reads, rendering step images into `files`
/// as `/steps/N.png`.
#[allow(clippy::too_many_arguments)]
fn guide_data(
    guide: &RenderGuide,
    handle: &GuideHandle,
    opts: &PdfOptions,
    labels: &Labels,
    now: &Datetime,
    accent: [u8; 3],
    logo: Option<(u32, u32)>,
    files: &mut Vec<(String, Vec<u8>)>,
    report: &mut ExportReport,
) -> Value {
    let mut cx = Cx::default();
    let items: Vec<Value> = guide
        .items
        .iter()
        .map(|item| match item {
            RenderItem::Section(section) => json!({
                "kind": "section",
                "title": clean(&section.title),
            }),
            RenderItem::Note(note) => json!({
                "kind": "note",
                "note": note_kind(note.kind),
                "label": clean(labels.note_kind(note.kind)),
                "body": cx.blocks(&note.body),
            }),
            RenderItem::Step(step) => {
                let image = render_step_image(handle, &step.step, &opts.image);
                report.add_warnings(step.step.id, image.warnings);
                let path = format!("/steps/{}.png", step.number);
                files.push((path.clone(), image.png));
                let title = if plain_text(&step.title).trim().is_empty() {
                    Vec::new()
                } else {
                    cx.inlines(&step.title)
                };
                let color_of = |number: u32| {
                    let placed = image.hotspots.iter().find(|h| h.number == number);
                    placed.and_then(|h| h.color.as_deref())
                };
                let hotspots: Vec<Value> = step
                    .hotspots()
                    .map(|(number, hotspot)| {
                        json!({
                            "number": number,
                            "color": hotspot_color(color_of(number)),
                            "title": clean(hotspot.title.trim()),
                            "lines": hotspot
                                .description
                                .lines()
                                .map(|line| clean(line.trim()))
                                .filter(|line| !line.is_empty())
                                .collect::<Vec<_>>(),
                        })
                    })
                    .collect();
                json!({
                    "kind": "step",
                    "number": step.number,
                    "title": title,
                    "description": cx.blocks(&step.description),
                    "image": {
                        "path": path,
                        "width": image.width.max(1),
                        "height": image.height.max(1),
                        "alt": clean(&plain_text(&step.title)),
                    },
                    "hotspots": hotspots,
                })
            }
        })
        .collect();
    let intro = cx.blocks(&guide.intro);
    let prerequisites = cx.blocks(&guide.prerequisites);
    report.notes.append(&mut cx.notes);
    let [r, g, b] = accent;
    let (_, text_height) = text_area_pt(opts.paper);
    let title = match clean(&guide.title).trim() {
        "" => untitled(&guide.language).to_owned(),
        title => title.to_owned(),
    };
    let date = opts.timestamp.and_then(|_| {
        Some(format!(
            "{:04}-{:02}-{:02}",
            now.year()?,
            now.month()?,
            now.day()?
        ))
    });
    json!({
        "title": title,
        "summary": clean(guide.summary.trim()),
        "lang": typst_lang(&guide.language),
        "date": date,
        "paper": match opts.paper {
            Paper::A4 => "a4",
            Paper::Letter => "us-letter",
        },
        "accent": format!("#{r:02x}{g:02x}{b:02x}"),
        "logo": logo.map(|(width, height)| json!({
            "path": "/logo",
            "width": width,
            "height": height,
        })),
        "max_image_height": (text_height * MAX_IMAGE_HEIGHT_RATIO).round(),
        "labels": {
            "prerequisites": clean(&labels.prerequisites),
            "step": clean(labels.step_kind(guide.kind)),
        },
        "intro": intro,
        "prerequisites": prerequisites,
        "items": items,
    })
}

/// Text area of the template's page (2 cm side margins, 2.6 cm top, 2.4 cm
/// bottom) in points.
fn text_area_pt(paper: Paper) -> (f64, f64) {
    const CM: f64 = 72.0 / 2.54;
    let (w, h) = match paper {
        Paper::A4 => (21.0 * CM, 29.7 * CM),
        Paper::Letter => (612.0, 792.0),
    };
    (w - 4.0 * CM, h - 5.0 * CM)
}

fn note_kind(kind: NoteKind) -> &'static str {
    match kind {
        NoteKind::Tip => "tip",
        NoteKind::Warning => "warning",
        NoteKind::Note => "note",
    }
}

/// A language Typst accepts (ISO 639 letters), from a BCP 47 tag; English
/// otherwise.
fn typst_lang(language: &str) -> String {
    let primary = language.split(['-', '_']).next().unwrap_or_default();
    if (2..=3).contains(&primary.len()) && primary.bytes().all(|b| b.is_ascii_alphabetic()) {
        primary.to_ascii_lowercase()
    } else {
        "en".into()
    }
}

/// Title of a guide whose title is blank.
fn untitled(language: &str) -> &'static str {
    match typst_lang(language).as_str() {
        "de" => "Unbenannt",
        _ => "Untitled",
    }
}

/// `timestamp` (or 1970-01-01) as a Typst datetime, clamped to the years
/// Typst supports (-9999 to 9999), so an odd `updated_at` can't fail an
/// export.
fn typst_datetime(timestamp: Option<DateTime<Utc>>) -> Datetime {
    let bound = |y, m, d, h, mi, s| {
        NaiveDate::from_ymd_opt(y, m, d)
            .and_then(|d| d.and_hms_opt(h, mi, s))
            .map(|t| t.and_utc())
            .unwrap_or(DateTime::UNIX_EPOCH)
    };
    let t = timestamp
        .unwrap_or(DateTime::UNIX_EPOCH)
        .clamp(bound(-9999, 1, 1, 0, 0, 0), bound(9999, 12, 31, 23, 59, 59));
    Datetime::from_ymd_hms(
        t.year(),
        t.month() as u8,
        t.day() as u8,
        t.hour() as u8,
        t.minute() as u8,
        t.second() as u8,
    )
    .or_else(|| Datetime::from_ymd(1970, 1, 1))
    .expect("1970-01-01 is a valid Typst date")
}

/// Builds the JSON for Markdown content, collecting notes on anything it
/// had to degrade.
#[derive(Default)]
struct Cx {
    notes: Vec<String>,
}

impl Cx {
    fn blocks(&mut self, content: &[Block]) -> Value {
        Value::Array(
            content
                .iter()
                .map(|block| match block {
                    Block::Paragraph(text) => json!({ "p": self.inlines(text) }),
                    Block::List { start, items } => json!({
                        "list": items.iter().map(|item| self.blocks(item)).collect::<Vec<_>>(),
                        "start": start,
                    }),
                })
                .collect(),
        )
    }

    fn inlines(&mut self, content: &[Inline]) -> Vec<Value> {
        let mut out = Vec::new();
        for inline in content {
            match inline {
                Inline::Text(text) => out.push(json!({ "t": "text", "v": clean(text) })),
                Inline::Code(text) => out.push(json!({ "t": "code", "v": clean(text) })),
                Inline::Strong(children) => {
                    out.push(json!({ "t": "strong", "c": self.inlines(children) }))
                }
                Inline::Emphasis(children) => {
                    out.push(json!({ "t": "em", "c": self.inlines(children) }))
                }
                Inline::Link { url, children, .. } => {
                    let mut body = self.inlines(children);
                    if body.is_empty() {
                        body.push(json!({ "t": "text", "v": clean(url) }));
                    }
                    if url.len() > MAX_URL_LEN {
                        self.notes.push(format!(
                            "a link URL of {} bytes is longer than the PDF allows ({MAX_URL_LEN}) \
                             and was left out; its text is kept",
                            url.len()
                        ));
                        out.extend(body);
                    } else {
                        out.push(json!({ "t": "link", "url": url, "c": body }));
                    }
                }
                Inline::Break => out.push(json!({ "t": "br" })),
            }
        }
        out
    }
}

/// Text as Typst should show it: line breaks (only hard breaks are kept, as
/// [`Inline::Break`]), line and paragraph separators and tabs become
/// spaces; control characters and the bidi embedding, override and isolate
/// controls (which could make text display in a misleading order) are
/// dropped.
fn clean(text: &str) -> String {
    text.chars()
        .filter_map(|c| match c {
            '\n' | '\r' | '\t' | '\u{2028}' | '\u{2029}' => Some(' '),
            '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' => None,
            c if c.is_control() => None,
            c => Some(c),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Typst world

struct Fonts {
    book: LazyHash<FontBook>,
    fonts: Vec<Font>,
}

static FONT_SET: LazyLock<Fonts> = LazyLock::new(|| {
    let fonts: Vec<Font> = LATO
        .iter()
        .copied()
        .chain(typst_assets::fonts())
        .flat_map(|data| Font::iter(Bytes::new(data)))
        .collect();
    Fonts {
        book: LazyHash::new(FontBook::from_fonts(&fonts)),
        fonts,
    }
});

static LIBRARY: LazyLock<LazyHash<Library>> =
    LazyLock::new(|| LazyHash::new(Library::builder().build()));

/// A sealed world: the template plus in-memory files, bundled fonts, a fixed
/// date, no file system and no packages.
struct GuideWorld {
    main: Source,
    files: HashMap<FileId, Bytes>,
    now: Datetime,
}

impl GuideWorld {
    fn new(files: Vec<(String, Vec<u8>)>, now: Datetime) -> Self {
        Self::with_main(TEMPLATE, files, now)
    }

    fn with_main(main: &str, files: Vec<(String, Vec<u8>)>, now: Datetime) -> Self {
        let main = Source::new(file_id("/main.typ"), main.to_owned());
        let files = files
            .into_iter()
            .map(|(path, bytes)| (file_id(&path), Bytes::new(bytes)))
            .collect();
        Self { main, files, now }
    }

    /// "error: message (main.typ:12:5)" plus hints, for each diagnostic.
    fn format(&self, diagnostics: &[SourceDiagnostic]) -> Vec<String> {
        diagnostics
            .iter()
            .map(|d| {
                let severity = match d.severity {
                    Severity::Error => "error",
                    Severity::Warning => "warning",
                };
                let mut out = format!("{severity}: {}", d.message);
                let at = d.span.id().zip(self.range(d.span)).and_then(|(id, range)| {
                    let source = self.source(id).ok()?;
                    let (line, col) = source.lines().byte_to_line_column(range.start)?;
                    Some(format!(
                        "{}:{}:{}",
                        id.get().vpath().get_with_slash(),
                        line + 1,
                        col + 1
                    ))
                });
                if let Some(at) = at {
                    out += &format!(" ({at})");
                }
                for hint in &d.hints {
                    out += &format!(" hint: {}", hint.v);
                }
                out
            })
            .collect()
    }
}

fn file_id(path: &str) -> FileId {
    let vpath = VirtualPath::new(path).expect("valid virtual path");
    RootedPath::new(VirtualRoot::Project, vpath).intern()
}

impl World for GuideWorld {
    fn library(&self) -> &LazyHash<Library> {
        &LIBRARY
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &FONT_SET.book
    }

    fn main(&self) -> FileId {
        self.main.id()
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.main.id() {
            Ok(self.main.clone())
        } else {
            Err(self.file(id).err().unwrap_or(FileError::NotSource))
        }
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        let path = id.get();
        if let VirtualRoot::Package(spec) = path.root() {
            return Err(FileError::Package(PackageError::Other(Some(
                format!("packages are not available ({spec})").into(),
            ))));
        }
        if id == self.main.id() {
            return Ok(Bytes::from_string(self.main.clone()));
        }
        self.files
            .get(&id)
            .cloned()
            .ok_or_else(|| FileError::NotFound(path.vpath().get_with_slash().into()))
    }

    fn font(&self, index: usize) -> Option<Font> {
        FONT_SET.fonts.get(index).cloned()
    }

    /// The timestamp's UTC day, whatever the offset: output must not depend
    /// on where it is made.
    fn today(&self, _offset: Option<TypstDuration>) -> Option<Datetime> {
        Datetime::from_ymd(self.now.year()?, self.now.month()?, self.now.day()?)
    }
}

/// The legend badge colour of a hotspot as `#rrggbb` (which Typst's `rgb`
/// takes): its own colour, hex or CSS name, parsed as the image marker's
/// is, so the two match (alpha dropped); else, or if transparent, the
/// default hotspot colour.
fn hotspot_color(color: Option<&str>) -> String {
    let parsed = color
        .and_then(|c| c.trim().parse::<svgtypes::Color>().ok())
        .filter(|c| c.alpha > 0);
    match parsed {
        Some(c) => format!("#{:02x}{:02x}{:02x}", c.red, c.green, c.blue),
        None => DEFAULT_HOTSPOT_COLOR.to_ascii_lowercase(),
    }
}

/// Text of a laid-out page: runs on the same baseline are joined (with a
/// space where there is a visible gap, e.g. after a list marker), a new
/// baseline starts a new line.
fn page_text(frame: &Frame) -> String {
    let mut runs = Vec::new();
    collect_text(frame, Point::zero(), &mut runs);
    let mut out = String::new();
    let mut last: Option<TextRun> = None;
    for run in runs {
        if let Some(prev) = &last {
            if (run.y - prev.y).abs() > 1.0 {
                out.push('\n');
            } else if run.x - prev.end > 1.0
                && !prev.text.ends_with(' ')
                && !run.text.starts_with(' ')
            {
                out.push(' ');
            }
        }
        out.push_str(&run.text);
        last = Some(run);
    }
    out
}

struct TextRun {
    x: f64,
    end: f64,
    y: f64,
    text: String,
}

fn collect_text(frame: &Frame, origin: Point, out: &mut Vec<TextRun>) {
    for (pos, item) in frame.items() {
        let pos = origin + *pos;
        match item {
            FrameItem::Group(group) => {
                let offset = Point::new(group.transform.tx, group.transform.ty);
                collect_text(&group.frame, pos + offset, out);
            }
            FrameItem::Text(text) => out.push(TextRun {
                x: pos.x.to_pt(),
                end: (pos.x + text.width()).to_pt(),
                y: pos.y.to_pt(),
                text: text.text.to_string(),
            }),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn hotspot_legend_colors_match_the_markers() {
        use super::hotspot_color;
        let default = super::DEFAULT_HOTSPOT_COLOR.to_ascii_lowercase();
        assert_eq!(hotspot_color(Some("#16A34A")), "#16a34a");
        assert_eq!(hotspot_color(Some("#abc")), "#aabbcc");
        assert_eq!(hotspot_color(Some(" Red ")), "#ff0000");
        assert_eq!(hotspot_color(Some("teal")), "#008080");
        assert_eq!(hotspot_color(Some("#ff000080")), "#ff0000");
        assert_eq!(hotspot_color(Some("transparent")), default);
        assert_eq!(hotspot_color(Some("not a colour")), default);
        assert_eq!(hotspot_color(None), default);
    }

    use super::*;

    /// Compiles `main` in a sealed world; the error or warning messages.
    fn compile(main: &str) -> std::result::Result<Vec<String>, Vec<String>> {
        let files = vec![("/data.txt".to_owned(), b"hello".to_vec())];
        let world = GuideWorld::with_main(main, files, typst_datetime(None));
        let warned = typst::compile(&world);
        let warnings = world.format(&warned.warnings);
        warned
            .output
            .map(|document| {
                let _ = typst_pdf::pdf(&document, &typst_pdf::PdfOptions::default());
                warnings
            })
            .map_err(|e| world.format(&e))
    }

    #[test]
    fn world_serves_only_its_own_files() {
        assert!(compile("#read(\"/data.txt\")").is_ok());
        for main in [
            "#read(\"/etc/passwd\")",
            "#read(\"../../../../etc/passwd\")",
            "#image(\"/Users/x.png\")",
            "#import \"/other.typ\": *",
            "#import \"@preview/cetz:0.3.0\": *",
            "#include \"/data.txt\"",
        ] {
            let errors = compile(main).expect_err(main);
            assert!(errors[0].starts_with("error: "), "{errors:?}");
        }
        let errors = compile("#import \"@preview/cetz:0.3.0\": *").unwrap_err();
        assert!(
            errors[0].contains("packages are not available"),
            "{errors:?}"
        );
    }

    #[test]
    fn compile_errors_become_diagnostics_not_panics() {
        let errors = compile("Hi\n#panic(\"x\")").unwrap_err();
        assert_eq!(errors, ["error: panicked with: x (/main.typ:2:2)"]);
    }

    #[test]
    fn today_is_fixed() {
        assert!(
            compile("#assert.eq(datetime.today(), datetime(year: 1970, month: 1, day: 1))").is_ok()
        );
        let t = "2026-10-14T23:30:00Z".parse().unwrap();
        let world = GuideWorld::with_main("", Vec::new(), typst_datetime(Some(t)));
        assert_eq!(world.today(None), Datetime::from_ymd(2026, 10, 14));
        // A Typst offset (e.g. `datetime.today(offset: 5)`) doesn't move it.
        assert!(compile("#datetime.today(offset: 5)").is_ok());
    }

    #[test]
    fn only_bundled_fonts_are_known() {
        let mut families: Vec<&str> = FONT_SET.book.families().map(|(name, _)| name).collect();
        families.sort();
        assert_eq!(
            families,
            [
                "DejaVu Sans Mono",
                "Lato",
                "Libertinus Serif",
                "New Computer Modern",
                "New Computer Modern Math",
            ]
        );
        // All four Lato faces.
        let lato = FONT_SET.book.select_family("lato").count();
        assert_eq!(lato, 4);
    }

    #[test]
    fn key_symbols_have_a_glyph() {
        for c in ['⌘', '⇧', '⌥', '⌃', '⏎', '⌫', '→', '•', 'é', 'ß'] {
            assert!(
                FONT_SET
                    .fonts
                    .iter()
                    .any(|f| f.info().coverage.contains(c as u32)),
                "no font has {c}"
            );
        }
    }

    #[test]
    fn typst_panics_become_errors() {
        let err = guard(|| -> u8 { panic!("layout bug") }).unwrap_err();
        assert!(
            matches!(&err, PdfError::Typst(m) if m == &["Typst panicked: layout bug"]),
            "{err}"
        );
        let err = guard(|| -> u8 { panic!("{}", String::from("owned")) }).unwrap_err();
        assert!(matches!(&err, PdfError::Typst(m) if m[0].ends_with("owned")));
        assert_eq!(guard(|| 7).unwrap(), 7);
    }

    #[test]
    fn datetimes_are_clamped_to_typst_years() {
        let at = |s: &str| typst_datetime(Some(s.parse().unwrap()));
        assert_eq!(
            at("+10000-01-01T00:00:00Z"),
            Datetime::from_ymd_hms(9999, 12, 31, 23, 59, 59).unwrap()
        );
        assert_eq!(
            at("-10000-06-01T00:00:00Z"),
            Datetime::from_ymd_hms(-9999, 1, 1, 0, 0, 0).unwrap()
        );
        assert_eq!(
            at("2026-10-14T08:09:10Z"),
            Datetime::from_ymd_hms(2026, 10, 14, 8, 9, 10).unwrap()
        );
        assert_eq!(
            typst_datetime(None),
            Datetime::from_ymd_hms(1970, 1, 1, 0, 0, 0).unwrap()
        );
    }

    #[test]
    fn untitled_follows_the_language() {
        assert_eq!(untitled("de-CH"), "Unbenannt");
        assert_eq!(untitled("fr"), "Untitled");
        assert_eq!(untitled(""), "Untitled");
    }

    #[test]
    fn languages_are_reduced_to_iso_639() {
        assert_eq!(typst_lang("de-DE"), "de");
        assert_eq!(typst_lang("EN_us"), "en");
        assert_eq!(typst_lang("gsw"), "gsw");
        assert_eq!(typst_lang(""), "en");
        assert_eq!(typst_lang("x\"); #panic()"), "en");
        assert_eq!(typst_lang("1234"), "en");
    }

    #[test]
    fn clean_drops_control_characters() {
        assert_eq!(clean("a\u{0}b\u{7}c\td\ne\u{1b}"), "abc d e");
        assert_eq!(clean("a\u{2028}b\u{2029}c"), "a b c");
        assert_eq!(
            clean(
                "\u{202A}a\u{202B}b\u{202C}c\u{202D}d\u{202E}e\u{2066}f\u{2067}g\u{2068}h\u{2069}"
            ),
            "abcdefgh"
        );
        // Marks that only hint direction stay.
        assert_eq!(clean("a\u{200E}b\u{200F}"), "a\u{200E}b\u{200F}");
        assert_eq!(clean("#$*_`<@[]\\"), "#$*_`<@[]\\");
    }

    #[test]
    fn image_height_cap_is_half_the_text_height() {
        let (w, h) = text_area_pt(Paper::A4);
        assert!(
            (w - 481.9).abs() < 0.1 && (h - 700.2).abs() < 0.1,
            "{w} {h}"
        );
        let (w, h) = text_area_pt(Paper::Letter);
        assert!(
            (w - 498.6).abs() < 0.1 && (h - 650.3).abs() < 0.1,
            "{w} {h}"
        );
    }
}
