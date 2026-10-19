//! DOCX writer over [`RenderGuide`] (docx-rs). See design scratchpad #443.
//!
//! # Layout
//!
//! - Title (style `Title`), summary (`Subtitle`), the review details line
//!   when [`DocxOptions::review_details`] is on, intro, then a
//!   prerequisites heading with its text.
//! - Sections are `Heading1`; steps are `Heading2` headings numbered
//!   "1. ", "2. ", ... followed by the step image and the description, in
//!   the same order as the Markdown and HTML writers. A step with an empty
//!   title is headed "N. Step N" ("N. Page N" in an annotation guide), as
//!   in Markdown.
//! - A step with hotspots has a legend right under its image: one
//!   paragraph per hotspot, "N. " and the bold title, then the description
//!   with its line breaks kept. The image shows the numbered markers.
//! - Notes are shaded paragraphs with a coloured left border, starting with
//!   a bold label ("Tip:", "Warning:", "Note:").
//! - Fixed words (prerequisites heading, note labels, "Step") come from
//!   [`Labels`]: [`DocxOptions::labels`], or by default
//!   [`Labels::for_language`] of the guide's language.
//! - With a [`CheckedBranding`] logo (the checked, upright PNG), it sits
//!   above the title, at most [`LOGO_HEIGHT_PX`] high (at 96 dpi) and the
//!   text width wide. The accent colours the `Heading1` and `Heading2`
//!   styles and the left border of neutral notes; tips and warnings keep
//!   their green and orange, which carry meaning.
//! - The Markdown subset maps to runs: bold, italic, code (monospace on a
//!   grey background), hyperlinks (real `w:hyperlink` elements with an
//!   external relationship), hard breaks, and bullet and numbered lists
//!   (every list gets its own numbering instance, so numbered lists restart
//!   at their own start number).
//!
//! # Images
//!
//! Step images come from [`render_step_image`], so annotations are drawn and
//! blur is burned in. They are shown at the text width, keeping the aspect
//! ratio, except that an image whose 96 dpi size is narrower stays at that
//! size, so small crops are not blown up, and a very tall image is scaled
//! down to fit [`MAX_IMAGE_HEIGHT_RATIO`] of the text height. A missing or
//! broken image becomes the renderer's placeholder and a warning in the
//! [`ExportReport`].
//!
//! Known docx-rs limitation (0.4.22): every image is written with
//! `<wp:docPr id="1" name="Figure" />`, and docx-rs offers no way to set
//! that id. The OOXML spec wants docPr ids unique per document. Word,
//! LibreOffice and textutil open such files without complaint, but a strict
//! validator flags them. Fixing it needs a docx-rs change or post-processing
//! of `word/document.xml`; neither is done here.
//!
//! # Escaping
//!
//! docx-rs XML-escapes text and hyperlink targets. Characters that XML 1.0
//! does not allow at all (most C0 controls) are dropped here first, since
//! Word refuses a document that contains them.
//!
//! # Determinism
//!
//! The same guide and options give the same bytes: paragraph, image and
//! hyperlink ids are assigned per document here instead of by docx-rs'
//! process-wide counters, hyperlink relationships (which docx-rs collects
//! in a `HashMap`) are sorted by id, the core-properties dates come from
//! [`DocxOptions::timestamp`] (1970-01-01 when unset), and zip entries carry
//! the fixed 1980-01-01 date of the zip crate without its `time` feature.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::ast::{plain_text, Block, Inline};
pub use super::md::{Branding, CheckedBranding};
use super::md::{Labels, Logo};
use super::raster::{render_step_image, ImageOptions};
use super::render::{RenderGuide, RenderItem, RenderNote, RenderStep};
use super::report::ExportReport;
use crate::library::GuideHandle;
use crate::model::{GuideKind, NoteKind};
use chrono::{DateTime, Utc};
use docx_rs::{
    AbstractNumbering, AlignmentType, BorderType, BreakType, Docx, Hyperlink, HyperlinkData,
    HyperlinkType, IndentLevel, Level, LevelJc, LevelOverride, LevelText, NumberFormat, Numbering,
    NumberingId, PageMargin, Paragraph, ParagraphBorder, ParagraphBorderPosition, ParagraphBorders,
    Pic, Run, RunFonts, Shading, ShdType, SpecialIndentType, Start, Style, StyleType,
};
use serde::{Deserialize, Serialize};

/// Paper size of the document (DOCX and PDF). Serialized as `a4` or
/// `letter`, as the app and the CLI name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Paper {
    #[default]
    A4,
    Letter,
}

impl Paper {
    /// Width and height in twips (1/1440 inch).
    fn size(self) -> (u32, u32) {
        match self {
            Paper::A4 => (11906, 16838),
            Paper::Letter => (12240, 15840),
        }
    }
}

/// Options for [`export`].
#[derive(Debug, Clone, PartialEq)]
pub struct DocxOptions {
    /// Replace an existing file at the output path.
    pub overwrite: bool,
    /// How step images are rendered. `max_width` bounds the embedded PNG's
    /// pixel width (and so the file size); the displayed size is set by the
    /// page, see the module docs.
    pub image: ImageOptions,
    /// Core-properties created and modified date, e.g. `guide.updated_at`.
    /// `None` writes 1970-01-01, so output never depends on the clock.
    pub timestamp: Option<DateTime<Utc>>,
    pub paper: Paper,
    /// Fixed words; `None` uses [`Labels::for_language`] of the guide's
    /// language.
    pub labels: Option<Labels>,
    pub branding: CheckedBranding,
    /// Show the version, owner and last review day under the summary (see
    /// [`Labels::review_details`]); off by default.
    pub review_details: bool,
}

impl Default for DocxOptions {
    fn default() -> Self {
        Self {
            overwrite: false,
            image: ImageOptions {
                max_width: Some(1800),
                ..ImageOptions::default()
            },
            timestamp: None,
            paper: Paper::default(),
            labels: None,
            branding: CheckedBranding::default(),
            review_details: false,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DocxError {
    #[error("{} already exists", .0.display())]
    Exists(PathBuf),
    #[error(transparent)]
    Io(#[from] io::Error),
    /// docx-rs failed to write the package.
    #[error("could not write the DOCX package: {0}")]
    Package(String),
}

pub type Result<T, E = DocxError> = std::result::Result<T, E>;

/// Page margins in twips (1 inch).
const MARGIN: u32 = 1440;
const EMU_PER_TWIP: u64 = 635;
/// EMU per pixel at 96 dpi.
const EMU_PER_PX: u64 = 9525;
/// Largest share of the text height an image may take.
pub const MAX_IMAGE_HEIGHT_RATIO: f64 = 0.75;

/// docx-rs always defines abstract numbering 1 and numbering 1; ours start
/// after them.
const BULLET_ABSTRACT: usize = 2;
const DECIMAL_ABSTRACT: usize = 3;
const FIRST_NUMBERING: usize = 2;
const LIST_LEVELS: usize = 9;
/// Left indent per list level, and the hanging indent of the marker.
const LIST_INDENT: i32 = 720;
const LIST_HANGING: i32 = 360;

const CODE_FONT: &str = "Consolas";
const CODE_FILL: &str = "F2F2F2";
const LINK_COLOR: &str = "0563C1";
/// Largest displayed logo height, in pixels at 96 dpi (about 1.3 cm).
pub const LOGO_HEIGHT_PX: u32 = 48;

/// Writes `guide` as a DOCX file at `out_path`, reading step images from the
/// bundle at `handle`. The file is written to a temporary file next to it
/// and renamed into place, so a failed export leaves nothing behind.
pub fn export(
    guide: &RenderGuide,
    handle: &GuideHandle,
    out_path: impl AsRef<Path>,
    opts: &DocxOptions,
) -> Result<ExportReport> {
    let out_path = out_path.as_ref();
    if !opts.overwrite && out_path.exists() {
        return Err(DocxError::Exists(out_path.to_path_buf()));
    }
    let mut report = ExportReport::default();
    let labels = opts
        .labels
        .clone()
        .unwrap_or_else(|| Labels::for_language(&guide.language));
    let docx = Writer::new(handle, opts, labels, &opts.branding, &mut report)
        .document(guide, opts.branding.logo());

    let dir = match out_path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    let mut tmp = tempfile::Builder::new()
        .prefix(".guiderecorder-export-")
        .suffix(".docx.tmp")
        .tempfile_in(dir)?;
    let mut xml = docx.build();
    xml.document_rels = sort_hyperlink_rels(&xml.document_rels);
    xml.pack(tmp.as_file_mut())
        .map_err(|e| DocxError::Package(e.to_string()))?;
    tmp.as_file().sync_all()?;
    if opts.overwrite {
        tmp.persist(out_path).map_err(|e| e.error)?;
    } else {
        tmp.persist_noclobber(out_path).map_err(|e| {
            if e.error.kind() == io::ErrorKind::AlreadyExists {
                DocxError::Exists(out_path.to_path_buf())
            } else {
                DocxError::Io(e.error)
            }
        })?;
    }
    if let Some(dir) = out_path.parent().filter(|d| !d.as_os_str().is_empty()) {
        // Make the rename durable; not all platforms can open a directory.
        if let Ok(dir) = fs::File::open(dir) {
            let _ = dir.sync_all();
        }
    }
    report.files.push(out_path.to_path_buf());
    Ok(report)
}

/// `word/_rels/document.xml.rels` with the `rIdHyperlinkN` relationships
/// in `N` order after all others. docx-rs writes them in `HashMap` order,
/// which changes from run to run.
fn sort_hyperlink_rels(xml: &[u8]) -> Vec<u8> {
    const OPEN: &str = "<Relationship ";
    let Ok(text) = std::str::from_utf8(xml) else {
        return xml.to_vec();
    };
    let (Some(first), Some(end)) = (text.find(OPEN), text.rfind("</Relationships>")) else {
        return xml.to_vec();
    };
    let mut others = Vec::new();
    let mut links = Vec::new();
    for element in text[first..end].split(OPEN).filter(|e| !e.is_empty()) {
        let number = element
            .strip_prefix("Id=\"rIdHyperlink")
            .and_then(|rest| rest.split('"').next())
            .and_then(|n| n.parse::<u64>().ok());
        match number {
            Some(n) => links.push((n, element)),
            None => others.push(element),
        }
    }
    links.sort_by_key(|(n, _)| *n);
    let mut out = String::with_capacity(text.len());
    out.push_str(&text[..first]);
    for element in others.into_iter().chain(links.into_iter().map(|(_, e)| e)) {
        out.push_str(OPEN);
        out.push_str(element);
    }
    out.push_str(&text[end..]);
    out.into_bytes()
}

/// Inline formatting in effect while walking the AST.
#[derive(Debug, Clone, Copy, Default)]
struct Format {
    bold: bool,
    italic: bool,
    code: bool,
    link: bool,
}

/// Paragraph decoration shared by every paragraph of a note.
#[derive(Debug, Clone, Copy)]
struct NoteStyle {
    fill: &'static str,
    /// Left border colour; `None` takes the accent.
    border: Option<&'static str>,
}

impl NoteStyle {
    /// `accent`: whether a branding accent replaces the neutral border.
    fn of(kind: NoteKind, accent: bool) -> Self {
        match kind {
            NoteKind::Tip => Self {
                fill: "E8F5E9",
                border: Some("2E7D32"),
            },
            NoteKind::Warning => Self {
                fill: "FFF4E5",
                border: Some("ED6C02"),
            },
            NoteKind::Note => Self {
                fill: "E8F0FE",
                border: (!accent).then_some("1A73E8"),
            },
        }
    }
}

/// Where a block sits: inside a note and/or a list item.
#[derive(Debug, Clone, Copy, Default)]
struct Context {
    note: Option<NoteStyle>,
    /// Numbering id and level of the enclosing list item.
    list: Option<(usize, usize)>,
}

/// A paragraph child: docx-rs has no common type for runs and hyperlinks.
enum Piece {
    Run(Box<Run>),
    Link(Hyperlink),
}

impl Piece {
    fn run(run: Run) -> Self {
        Self::Run(Box::new(run))
    }
}

struct Writer<'a> {
    handle: &'a GuideHandle,
    opts: &'a DocxOptions,
    labels: Labels,
    /// The guide's kind, for the "Step" or "Page" fallback title.
    kind: GuideKind,
    /// Branding accent as `RRGGBB`.
    accent: Option<String>,
    report: &'a mut ExportReport,
    docx: Docx,
    next_paragraph: u32,
    next_numbering: usize,
    next_link: usize,
    next_image: usize,
    /// Text area size in EMU.
    text_width: u64,
    text_height: u64,
}

impl<'a> Writer<'a> {
    fn new(
        handle: &'a GuideHandle,
        opts: &'a DocxOptions,
        labels: Labels,
        branding: &CheckedBranding,
        report: &'a mut ExportReport,
    ) -> Self {
        let (page_w, page_h) = opts.paper.size();
        let date = opts
            .timestamp
            .unwrap_or(DateTime::UNIX_EPOCH)
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();
        let docx = Docx::new()
            .page_size(page_w, page_h)
            .page_margin(
                PageMargin::new()
                    .top(MARGIN as i32)
                    .bottom(MARGIN as i32)
                    .left(MARGIN as i32)
                    .right(MARGIN as i32)
                    .header(720)
                    .footer(720),
            )
            .created_at(&date)
            .updated_at(&date)
            .default_size(22)
            .add_abstract_numbering(list_levels(BULLET_ABSTRACT, false))
            .add_abstract_numbering(list_levels(DECIMAL_ABSTRACT, true));
        let accent = branding
            .accent()
            .map(|[r, g, b]| format!("{r:02X}{g:02X}{b:02X}"));
        let docx = styles(accent.as_deref())
            .into_iter()
            .fold(docx, Docx::add_style);
        Self {
            handle,
            opts,
            labels,
            kind: GuideKind::default(),
            accent,
            report,
            docx,
            next_paragraph: 1,
            next_numbering: FIRST_NUMBERING,
            next_link: 1,
            next_image: 1,
            text_width: u64::from(page_w - 2 * MARGIN) * EMU_PER_TWIP,
            text_height: u64::from(page_h - 2 * MARGIN) * EMU_PER_TWIP,
        }
    }

    fn document(mut self, guide: &RenderGuide, logo: Option<&Logo>) -> Docx {
        self.kind = guide.kind;
        if let Some(logo) = logo {
            self.logo(logo);
        }
        let title = self
            .paragraph()
            .style("Title")
            .add_run(text_run(&guide.title));
        self.push(title);
        if !guide.summary.trim().is_empty() {
            let summary = self
                .paragraph()
                .style("Subtitle")
                .add_run(text_run(&guide.summary));
            self.push(summary);
        }
        let details = self
            .opts
            .review_details
            .then(|| self.labels.review_details(&guide.review))
            .flatten();
        if let Some(details) = details {
            let details = self
                .paragraph()
                .add_run(text_run(&details).size(20).color("595959"));
            self.push(details);
        }
        self.blocks(&guide.intro, Context::default());
        if !guide.prerequisites.is_empty() {
            let heading = self.labels.prerequisites.clone();
            self.heading("Heading1", &heading);
            self.blocks(&guide.prerequisites, Context::default());
        }
        for item in &guide.items {
            match item {
                RenderItem::Section(section) => self.heading("Heading1", &section.title),
                RenderItem::Step(step) => self.step(step),
                RenderItem::Note(note) => self.note(note),
            }
        }
        self.docx
    }

    /// A paragraph with a per-document id (docx-rs' own ids come from a
    /// thread-local counter).
    fn paragraph(&mut self) -> Paragraph {
        let id = self.next_paragraph;
        self.next_paragraph += 1;
        Paragraph::new().id(format!("{id:08x}"))
    }

    fn push(&mut self, paragraph: Paragraph) {
        self.docx = std::mem::take(&mut self.docx).add_paragraph(paragraph);
    }

    fn heading(&mut self, style: &str, text: &str) {
        let p = self
            .paragraph()
            .style(style)
            .keep_next(true)
            .add_run(text_run(text));
        self.push(p);
    }

    fn step(&mut self, step: &RenderStep) {
        let mut pieces = vec![Piece::run(text_run(&format!("{}. ", step.number)))];
        if plain_text(&step.title).trim().is_empty() {
            // Same fallback as the Markdown writer: "2. Step 2".
            let title = format!("{} {}", self.labels.step_kind(self.kind), step.number);
            pieces.push(Piece::run(text_run(&title)));
        } else {
            self.inlines(&step.title, Format::default(), &mut pieces);
        }
        let heading = self.paragraph().style("Heading2").keep_next(true);
        let heading = add_pieces(heading, pieces);
        self.push(heading);
        self.image(step);
        self.hotspot_legend(step);
        self.blocks(&step.description, Context::default());
    }

    /// One paragraph per hotspot: "N. " and the bold title, then the
    /// description lines after line breaks. Hanging-indented like a list.
    fn hotspot_legend(&mut self, step: &RenderStep) {
        for (number, hotspot) in step.hotspots() {
            let mut p = self
                .paragraph()
                .indent(
                    Some(LIST_INDENT),
                    Some(SpecialIndentType::Hanging(LIST_HANGING)),
                    None,
                    None,
                )
                .add_run(text_run(&format!("{number}.")).add_tab());
            let title = hotspot.title.trim();
            let mut first = true;
            if !title.is_empty() {
                p = p.add_run(text_run(title).bold());
                first = false;
            }
            for line in hotspot.description.lines().map(str::trim) {
                if line.is_empty() {
                    continue;
                }
                if !first {
                    p = p.add_run(Run::new().add_break(BreakType::TextWrapping));
                }
                p = p.add_run(text_run(line));
                first = false;
            }
            self.push(p);
        }
    }

    fn image(&mut self, step: &RenderStep) {
        let image = render_step_image(self.handle, &step.step, &self.opts.image);
        self.report.add_warnings(step.step.id, image.warnings);
        let (width, height) = self.display_size(image.width, image.height);
        let id = self.next_image;
        self.next_image += 1;
        let pic = Pic::new_with_dimensions(image.png, image.width, image.height)
            .id(format!("rIdImage{id}"))
            .size(width, height);
        let p = self
            .paragraph()
            .align(AlignmentType::Center)
            .keep_lines(true)
            .add_run(Run::new().add_image(pic));
        self.push(p);
    }

    fn logo(&mut self, logo: &Logo) {
        let (px_w, px_h) = (u64::from(logo.width()), u64::from(logo.height()));
        let mut height = px_h.min(u64::from(LOGO_HEIGHT_PX)) * EMU_PER_PX;
        let mut width = height * px_w / px_h;
        if width > self.text_width {
            width = self.text_width;
            height = (width * px_h / px_w).max(1);
        }
        let clamp = |v: u64| v.clamp(1, u64::from(u32::MAX)) as u32;
        let id = self.next_image;
        self.next_image += 1;
        let pic = Pic::new_with_dimensions(logo.png().to_vec(), logo.width(), logo.height())
            .id(format!("rIdImage{id}"))
            .size(clamp(width), clamp(height));
        let p = self
            .paragraph()
            .keep_next(true)
            .add_run(Run::new().add_image(pic));
        self.push(p);
    }

    /// Displayed size in EMU: the text width, or the 96 dpi size if
    /// narrower, then shrunk to fit the height cap; aspect ratio kept.
    fn display_size(&self, px_w: u32, px_h: u32) -> (u32, u32) {
        let (px_w, px_h) = (u64::from(px_w.max(1)), u64::from(px_h.max(1)));
        let mut width = (px_w * EMU_PER_PX).min(self.text_width);
        let mut height = width * px_h / px_w;
        let max_height = (self.text_height as f64 * MAX_IMAGE_HEIGHT_RATIO) as u64;
        if height > max_height {
            height = max_height;
            width = height * px_w / px_h;
        }
        let clamp = |v: u64| v.clamp(1, u64::from(u32::MAX)) as u32;
        (clamp(width), clamp(height))
    }

    fn note(&mut self, note: &RenderNote) {
        let style = NoteStyle::of(note.kind, self.accent.is_some());
        let label = format!("{}: ", self.labels.note_kind(note.kind));
        let label = Piece::run(Run::new().bold().add_text(xml_text(&label)));
        let ctx = Context {
            note: Some(style),
            list: None,
        };
        match note.body.split_first() {
            // The label leads the first paragraph.
            Some((Block::Paragraph(inlines), rest)) => {
                let mut pieces = vec![label];
                self.inlines(inlines, Format::default(), &mut pieces);
                let p = add_pieces(self.paragraph(), pieces);
                let p = self.decorate(p, ctx);
                self.push(p);
                self.blocks(rest, ctx);
            }
            // The label gets a paragraph of its own.
            _ => {
                let p = add_pieces(self.paragraph(), vec![label]);
                let p = self.decorate(p, ctx);
                self.push(p);
                self.blocks(&note.body, ctx);
            }
        }
    }

    /// Applies note shading and border.
    fn decorate(&self, mut p: Paragraph, ctx: Context) -> Paragraph {
        if let Some(note) = ctx.note {
            let border = ParagraphBorder::new(ParagraphBorderPosition::Left)
                .val(BorderType::Single)
                .size(24)
                .space(8)
                .color(note.border.or(self.accent.as_deref()).unwrap_or_default());
            p.property = p
                .property
                .shading(
                    Shading::new()
                        .shd_type(ShdType::Clear)
                        .color("auto")
                        .fill(note.fill),
                )
                .set_borders(ParagraphBorders::with_empty().set(border));
        }
        p
    }

    fn blocks(&mut self, blocks: &[Block], ctx: Context) {
        for block in blocks {
            match block {
                Block::Paragraph(inlines) => {
                    let mut pieces = Vec::new();
                    self.inlines(inlines, Format::default(), &mut pieces);
                    let mut p = add_pieces(self.paragraph(), pieces);
                    if let Some((_, level)) = ctx.list {
                        // A continuation paragraph of a list item: indented
                        // like the item's text, without a marker.
                        p = p.indent(Some(list_indent(level)), None, None, None);
                    }
                    let p = self.decorate(p, ctx);
                    self.push(p);
                }
                Block::List { start, items } => self.list(*start, items, ctx),
            }
        }
    }

    fn list(&mut self, start: Option<u64>, items: &[Vec<Block>], ctx: Context) {
        let level = ctx
            .list
            .map_or(0, |(_, level)| (level + 1).min(LIST_LEVELS - 1));
        let id = self.next_numbering;
        self.next_numbering += 1;
        let numbering = match start {
            Some(start) => Numbering::new(id, DECIMAL_ABSTRACT)
                .add_override(LevelOverride::new(level).start(usize::try_from(start).unwrap_or(1))),
            None => Numbering::new(id, BULLET_ABSTRACT),
        };
        self.docx = std::mem::take(&mut self.docx).add_numbering(numbering);
        let item_ctx = Context {
            note: ctx.note,
            list: Some((id, level)),
        };
        for item in items {
            match item.split_first() {
                // The marker goes on the item's first paragraph.
                Some((Block::Paragraph(inlines), rest)) => {
                    let mut pieces = Vec::new();
                    self.inlines(inlines, Format::default(), &mut pieces);
                    let p = add_pieces(self.paragraph(), pieces)
                        .numbering(NumberingId::new(id), IndentLevel::new(level));
                    let p = self.decorate(p, ctx);
                    self.push(p);
                    self.blocks(rest, item_ctx);
                }
                // An item starting with a nested list (or empty) still shows
                // its marker.
                _ => {
                    let p = self
                        .paragraph()
                        .numbering(NumberingId::new(id), IndentLevel::new(level));
                    let p = self.decorate(p, ctx);
                    self.push(p);
                    self.blocks(item, item_ctx);
                }
            }
        }
    }

    fn inlines(&mut self, inlines: &[Inline], fmt: Format, out: &mut Vec<Piece>) {
        for inline in inlines {
            match inline {
                Inline::Text(text) => out.push(Piece::run(styled_run(text, fmt))),
                Inline::Code(text) => {
                    out.push(Piece::run(styled_run(text, Format { code: true, ..fmt })))
                }
                Inline::Strong(children) => {
                    self.inlines(children, Format { bold: true, ..fmt }, out)
                }
                Inline::Emphasis(children) => self.inlines(
                    children,
                    Format {
                        italic: true,
                        ..fmt
                    },
                    out,
                ),
                Inline::Break => {
                    out.push(Piece::run(Run::new().add_break(BreakType::TextWrapping)))
                }
                // Links don't nest; keep the text of an inner one.
                Inline::Link { children, .. } if fmt.link => self.inlines(children, fmt, out),
                Inline::Link { url, children, .. } => {
                    let fmt = Format { link: true, ..fmt };
                    let mut inner = Vec::new();
                    self.inlines(children, fmt, &mut inner);
                    if inner.is_empty() {
                        inner.push(Piece::run(styled_run(url, fmt)));
                    }
                    let link =
                        inner
                            .into_iter()
                            .fold(self.hyperlink(url), |link, piece| match piece {
                                Piece::Run(run) => link.add_run(*run),
                                Piece::Link(_) => unreachable!("links don't nest"),
                            });
                    out.push(Piece::Link(link));
                }
            }
        }
    }

    /// An external hyperlink with a per-document relationship id.
    fn hyperlink(&mut self, url: &str) -> Hyperlink {
        let mut link = Hyperlink::new(xml_text(url), HyperlinkType::External);
        if let HyperlinkData::External { rid, .. } = &mut link.link {
            *rid = format!("rIdHyperlink{}", self.next_link);
        }
        self.next_link += 1;
        link
    }
}

fn add_pieces(paragraph: Paragraph, pieces: Vec<Piece>) -> Paragraph {
    pieces.into_iter().fold(paragraph, |p, piece| match piece {
        Piece::Run(run) => p.add_run(*run),
        Piece::Link(link) => p.add_hyperlink(link),
    })
}

fn text_run(text: &str) -> Run {
    Run::new().add_text(xml_text(text))
}

fn styled_run(text: &str, fmt: Format) -> Run {
    let mut run = text_run(text);
    if fmt.bold {
        run = run.bold();
    }
    if fmt.italic {
        run = run.italic();
    }
    if fmt.code {
        run = run
            .fonts(
                RunFonts::new()
                    .ascii(CODE_FONT)
                    .hi_ansi(CODE_FONT)
                    .cs(CODE_FONT),
            )
            .shading(
                Shading::new()
                    .shd_type(ShdType::Clear)
                    .color("auto")
                    .fill(CODE_FILL),
            );
    }
    if fmt.link {
        run = run.color(LINK_COLOR).underline("single");
    }
    run
}

/// Drops characters XML 1.0 forbids (docx-rs escapes the rest). Newlines
/// become spaces, since docx-rs removes them from text runs.
fn xml_text(text: &str) -> String {
    text.chars()
        .filter_map(|c| match c {
            '\n' | '\r' => Some(' '),
            '\t' => Some(c),
            '\u{0}'..='\u{1F}' | '\u{FFFE}' | '\u{FFFF}' => None,
            _ => Some(c),
        })
        .collect()
}

fn list_indent(level: usize) -> i32 {
    LIST_INDENT * (level as i32 + 1)
}

/// An abstract numbering with all nine levels: bullets (•, ◦, ▪) or
/// numbers (1., a., i.).
fn list_levels(id: usize, numbered: bool) -> AbstractNumbering {
    (0..LIST_LEVELS).fold(AbstractNumbering::new(id), |abs, level| {
        let (format, text) = if numbered {
            let format = ["decimal", "lowerLetter", "lowerRoman"][level % 3];
            (format, format!("%{}.", level + 1))
        } else {
            ("bullet", ["•", "◦", "▪"][level % 3].to_owned())
        };
        abs.add_level(
            Level::new(
                level,
                Start::new(1),
                NumberFormat::new(format),
                LevelText::new(text),
                LevelJc::new("left"),
            )
            .indent(
                Some(list_indent(level)),
                Some(SpecialIndentType::Hanging(LIST_HANGING)),
                None,
                None,
            ),
        )
    })
}

/// The paragraph styles; headings take the accent (`RRGGBB`) if any.
fn styles(accent: Option<&str>) -> Vec<Style> {
    let paragraph = |id: &str, name: &str| {
        Style::new(id, StyleType::Paragraph)
            .name(name)
            .based_on("Normal")
            .next("Normal")
            .q_format(true)
    };
    let heading = |style: Style| match accent {
        Some(accent) => style.color(accent),
        None => style,
    };
    vec![
        paragraph("Title", "Title").size(52).bold(),
        paragraph("Subtitle", "Subtitle")
            .size(28)
            .italic()
            .color("595959"),
        heading(
            paragraph("Heading1", "heading 1")
                .size(32)
                .bold()
                .outline_lvl(0),
        ),
        heading(
            paragraph("Heading2", "heading 2")
                .size(26)
                .bold()
                .outline_lvl(1),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use ulid::Ulid;

    fn writer_size(paper: Paper, px: (u32, u32)) -> (u32, u32) {
        let handle = GuideHandle {
            id: Ulid::nil(),
            path: PathBuf::from("/nonexistent.guiderecorder"),
        };
        let opts = DocxOptions {
            paper,
            ..DocxOptions::default()
        };
        let mut report = ExportReport::default();
        Writer::new(
            &handle,
            &opts,
            Labels::for_language("en"),
            &CheckedBranding::default(),
            &mut report,
        )
        .display_size(px.0, px.1)
    }

    #[test]
    fn paper_names_round_trip() {
        for (paper, name) in [(Paper::A4, "\"a4\""), (Paper::Letter, "\"letter\"")] {
            assert_eq!(serde_json::to_string(&paper).unwrap(), name);
            assert_eq!(serde_json::from_str::<Paper>(name).unwrap(), paper);
        }
        assert!(serde_json::from_str::<Paper>("\"legal\"").is_err());
    }

    #[test]
    fn wide_images_fit_the_text_width() {
        // A4 text width: (11906 - 2880) twips.
        let text_width = 9026 * 635;
        assert_eq!(
            writer_size(Paper::A4, (2000, 1000)),
            (text_width, text_width / 2)
        );
        let letter = 9360 * 635;
        assert_eq!(
            writer_size(Paper::Letter, (3000, 1500)),
            (letter, letter / 2)
        );
    }

    #[test]
    fn small_images_keep_their_96_dpi_size() {
        assert_eq!(writer_size(Paper::A4, (64, 40)), (64 * 9525, 40 * 9525));
    }

    #[test]
    fn tall_images_fit_the_height_cap() {
        let max_height = ((16838 - 2880) as f64 * 635.0 * MAX_IMAGE_HEIGHT_RATIO) as u32;
        let (w, h) = writer_size(Paper::A4, (1000, 10000));
        assert_eq!(h, max_height);
        assert_eq!(w, max_height / 10);
    }

    #[test]
    fn hyperlink_rels_are_sorted() {
        let rels = concat!(
            r#"<?xml version="1.0"?><Relationships xmlns="x">"#,
            r#"<Relationship Id="rId1" Target="styles.xml" />"#,
            r#"<Relationship Id="rIdHyperlink10" Target="j" TargetMode="External" />"#,
            r#"<Relationship Id="rIdImage1" Target="media/a.png" />"#,
            r#"<Relationship Id="rIdHyperlink2" Target="b" TargetMode="External" />"#,
            r#"</Relationships>"#,
        );
        let sorted = String::from_utf8(sort_hyperlink_rels(rels.as_bytes())).unwrap();
        assert_eq!(
            sorted,
            concat!(
                r#"<?xml version="1.0"?><Relationships xmlns="x">"#,
                r#"<Relationship Id="rId1" Target="styles.xml" />"#,
                r#"<Relationship Id="rIdImage1" Target="media/a.png" />"#,
                r#"<Relationship Id="rIdHyperlink2" Target="b" TargetMode="External" />"#,
                r#"<Relationship Id="rIdHyperlink10" Target="j" TargetMode="External" />"#,
                r#"</Relationships>"#,
            )
        );
        assert_eq!(sort_hyperlink_rels(b"\xff"), b"\xff");
    }

    #[test]
    fn xml_text_drops_forbidden_characters() {
        assert_eq!(xml_text("a\u{0}b\u{7}c\td\ne\u{FFFE}"), "abc\td e");
        assert_eq!(xml_text("<&>\"'"), "<&>\"'");
    }
}
