//! PPTX writer over [`RenderGuide`]: a slide deck to present a guide.
//!
//! # Slides
//!
//! - A title slide with the guide's title and summary; its speaker notes
//!   hold the intro and prerequisites.
//! - One slide per step, titled "N. Title" ("N. Step N" when untitled, as
//!   in the other writers), with the step image on the left and the
//!   description beside it, or the image across the slide when there is no
//!   description. A step's hotspots follow its description as a numbered
//!   list matching the markers on the image. The speaker notes repeat the
//!   description and hotspots, for the presenter.
//! - A slide per section, with just its title (and "For: Admin" when it
//!   names audiences), per note, with its label ("Tip", "Warning", "Note")
//!   as the title and its text as the body, and per decision, with "Decision:
//!   question" as the title and an option per bullet naming its step.
//!
//! Slides are 16:9. Every slide uses one "Title Only" layout, so the title
//! is a real title placeholder (PowerPoint's outline and screen readers
//! find it); everything else is placed on the slide. Text boxes shrink
//! their text to fit when opened in PowerPoint.
//!
//! The Markdown subset maps to runs: bold, italic, code (Menlo), links
//! (with a hyperlink relationship) and line breaks; lists become bulleted or
//! numbered paragraphs.
//!
//! # Images
//!
//! Step images come from [`render_step_image`](super::render_step_image), so
//! annotations are drawn and blurs are burned in. A missing or broken image
//! becomes the renderer's placeholder and a warning in the
//! [`ExportReport`].
//!
//! # Determinism
//!
//! The same guide and options give the same bytes: part names and ids
//! follow item order, the core-properties dates come from
//! [`PptxOptions::timestamp`] (1970-01-01 when unset), and zip entries carry
//! the zip crate's fixed 1980-01-01 date.

use std::fmt::Write as _;
use std::io::{self, Seek, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use super::annotate::escape;
use super::ast::{plain_text, Block, Inline};
use super::md::Labels;
use super::persist;
use super::raster::ImageOptions;
use super::render::{RenderDecision, RenderGuide, RenderItem, RenderStep};
use super::report::ExportReport;
use crate::model::GuideKind;

/// Slide size in EMU (13.333 × 7.5 in, 16:9).
const SLIDE_W: i64 = 12_192_000;
const SLIDE_H: i64 = 6_858_000;
/// Notes page size in EMU (7.5 × 10 in).
const NOTES_W: i64 = 6_858_000;
const NOTES_H: i64 = 9_144_000;
const MARGIN: i64 = 457_200;
const GAP: i64 = 304_800;
const TITLE_Y: i64 = 304_800;
const TITLE_H: i64 = 914_400;
const CONTENT_Y: i64 = TITLE_Y + TITLE_H + 152_400;
/// Width of the image beside a description.
const IMAGE_W: i64 = 7_315_200;
/// EMU per pixel at 96 dpi.
const EMU_PER_PX: i64 = 9_525;

const NS_A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const NS_R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const NS_P: &str = "http://schemas.openxmlformats.org/presentationml/2006/main";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const CT: &str = "application/vnd.openxmlformats-officedocument.presentationml";

/// Options for [`export`].
#[derive(Debug, Clone, PartialEq)]
pub struct PptxOptions {
    /// Replace an existing file at the output path.
    pub overwrite: bool,
    /// How step images are rendered. `max_width` bounds the embedded PNG's
    /// pixel width.
    pub image: ImageOptions,
    /// Core-properties created and modified date, e.g. `guide.updated_at`.
    /// `None` writes 1970-01-01, so output never depends on the clock.
    pub timestamp: Option<DateTime<Utc>>,
    /// Fixed words; `None` uses [`Labels::for_language`] of the guide's
    /// language.
    pub labels: Option<Labels>,
}

impl Default for PptxOptions {
    fn default() -> Self {
        Self {
            overwrite: false,
            image: ImageOptions {
                max_width: Some(1600),
                ..ImageOptions::default()
            },
            timestamp: None,
            labels: None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PptxError {
    #[error("{0} already exists")]
    Exists(PathBuf),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("packaging failed: {0}")]
    Package(String),
}

pub type Result<T> = std::result::Result<T, PptxError>;

/// Writes `guide` as a PPTX file to `out_path`.
pub fn export(
    guide: &RenderGuide,
    out_path: impl AsRef<Path>,
    opts: &PptxOptions,
) -> Result<ExportReport> {
    let out = out_path.as_ref();
    let exists = |e: io::Error| {
        if e.kind() == io::ErrorKind::AlreadyExists {
            PptxError::Exists(out.to_path_buf())
        } else {
            PptxError::Io(e)
        }
    };
    persist::check_free(out, opts.overwrite).map_err(exists)?;
    let mut report = ExportReport::default();
    let labels = opts
        .labels
        .clone()
        .unwrap_or_else(|| Labels::for_language(&guide.language));
    let slides = Deck::new(guide, opts, &labels, &mut report).slides;
    let mut bytes = io::Cursor::new(Vec::new());
    package(&mut bytes, guide, &slides, opts.timestamp)
        .map_err(|e| PptxError::Package(e.to_string()))?;
    persist::write_file(out, opts.overwrite, "pptx", |file| {
        file.write_all(bytes.get_ref())
    })
    .map_err(exists)?;
    report.files.push(out.to_path_buf());
    Ok(report)
}

// ---------------------------------------------------------------------------
// Slides

/// One slide's parts.
struct Slide {
    /// `ppt/slides/slideN.xml`.
    xml: String,
    /// The slide's PNG, if any.
    image: Option<Vec<u8>>,
    /// Hyperlink targets, as relationships `rIdLink1`, `rIdLink2`, ...
    links: Vec<String>,
    /// Speaker notes as `<a:p>` paragraphs; empty for none.
    notes: String,
}

struct Deck<'a> {
    guide: &'a RenderGuide,
    opts: &'a PptxOptions,
    labels: &'a Labels,
    lang: String,
    slides: Vec<Slide>,
}

impl<'a> Deck<'a> {
    fn new(
        guide: &'a RenderGuide,
        opts: &'a PptxOptions,
        labels: &'a Labels,
        report: &mut ExportReport,
    ) -> Self {
        let mut deck = Self {
            guide,
            opts,
            labels,
            lang: escape(guide.language.trim()),
            slides: Vec::new(),
        };
        deck.title_slide();
        for item in &guide.items {
            match item {
                RenderItem::Step(step) => deck.step_slide(step, report),
                RenderItem::Section(section) => {
                    let audiences: Vec<Block> = labels
                        .audiences(&section.audiences)
                        .map(|text| Block::Paragraph(vec![Inline::Text(text)]))
                        .into_iter()
                        .collect();
                    deck.text_slide(&section.title, &audiences)
                }
                RenderItem::Decision(decision) => {
                    let (title, body) = decision_slide(decision, labels, guide.kind);
                    deck.text_slide(&title, &body)
                }
                RenderItem::Note(note) => {
                    let label = labels.note_kind(note.kind).to_owned();
                    deck.text_slide(&label, &note.body)
                }
            }
        }
        deck
    }

    fn lang_attr(&self) -> String {
        if self.lang.is_empty() {
            String::new()
        } else {
            format!(r#" lang="{}""#, self.lang)
        }
    }

    fn title_slide(&mut self) {
        let mut links = Vec::new();
        let mut shapes = title_shape(
            &self.run(&self.guide.title, Style::default()),
            (
                MARGIN,
                SLIDE_H / 2 - 1_371_600,
                SLIDE_W - 2 * MARGIN,
                1_371_600,
            ),
            Some(4000),
            "b",
            true,
        );
        if !self.guide.summary.trim().is_empty() {
            let body = paragraph("", &self.run(&self.guide.summary, Style::default()));
            shapes.push_str(&text_box(
                3,
                "Summary",
                (MARGIN, SLIDE_H / 2 + GAP, SLIDE_W - 2 * MARGIN, 1_371_600),
                &body,
                Some(2000),
                true,
            ));
        }
        let mut notes = String::new();
        for (heading, blocks) in [
            (None, &self.guide.intro),
            (Some(&self.labels.prerequisites), &self.guide.prerequisites),
        ] {
            if blocks.is_empty() {
                continue;
            }
            if let Some(heading) = heading {
                notes.push_str(&paragraph(
                    "",
                    &self.run(
                        heading,
                        Style {
                            bold: true,
                            ..Style::default()
                        },
                    ),
                ));
            }
            notes.push_str(&self.blocks(blocks, &mut links));
        }
        self.slides.push(Slide {
            xml: slide_xml(&shapes),
            image: None,
            links,
            notes,
        });
    }

    fn step_slide(&mut self, step: &RenderStep, report: &mut ExportReport) {
        let mut links = Vec::new();
        let kind = self.labels.step_kind(self.guide.kind);
        let mut title = format!("{}. ", step.number);
        let mut title_xml = self.run(&title, Style::default());
        if step.title.is_empty() {
            title.push_str(&format!("{kind} {}", step.number));
            title_xml = self.run(&title, Style::default());
        } else {
            title.push_str(&plain_text(&step.title));
            title_xml.push_str(&self.inlines(&step.title, Style::default(), &mut links));
        }

        let mut body = self.blocks(&step.description, &mut links);
        for (number, hotspot) in step.hotspots() {
            let mut runs = self.run(
                &format!("{number}. {}", hotspot.title.trim()),
                Style {
                    bold: true,
                    ..Style::default()
                },
            );
            if !hotspot.description.trim().is_empty() {
                runs.push_str(&self.run(
                    &format!(": {}", hotspot.description.trim()),
                    Style::default(),
                ));
            }
            body.push_str(&paragraph("", &runs));
        }

        let rendered = self.guide.step_image(step, &self.opts.image);
        report.add_warnings(step.step.id, rendered.warnings.iter().cloned());
        let content_h = SLIDE_H - CONTENT_Y - MARGIN;
        let image_box = if body.is_empty() {
            (MARGIN, CONTENT_Y, SLIDE_W - 2 * MARGIN, content_h)
        } else {
            (MARGIN, CONTENT_Y, IMAGE_W, content_h)
        };
        let mut shapes = title_shape(
            &title_xml,
            (MARGIN, TITLE_Y, SLIDE_W - 2 * MARGIN, TITLE_H),
            None,
            "b",
            false,
        );
        shapes.push_str(&picture(
            3,
            &format!("{kind} {}", step.number),
            &title,
            fit(rendered.width, rendered.height, image_box),
        ));
        if !body.is_empty() {
            let x = MARGIN + IMAGE_W + GAP;
            shapes.push_str(&text_box(
                4,
                "Description",
                (x, CONTENT_Y, SLIDE_W - MARGIN - x, content_h),
                &body,
                Some(1600),
                false,
            ));
        }
        self.slides.push(Slide {
            xml: slide_xml(&shapes),
            image: Some(rendered.png),
            links,
            notes: body,
        });
    }

    fn text_slide(&mut self, title: &str, body: &[Block]) {
        let mut links = Vec::new();
        let title_xml = self.run(title, Style::default());
        let mut shapes;
        if body.is_empty() {
            shapes = title_shape(
                &title_xml,
                (
                    MARGIN,
                    SLIDE_H / 2 - TITLE_H,
                    SLIDE_W - 2 * MARGIN,
                    2 * TITLE_H,
                ),
                Some(3600),
                "ctr",
                true,
            );
        } else {
            shapes = title_shape(
                &title_xml,
                (MARGIN, TITLE_Y, SLIDE_W - 2 * MARGIN, TITLE_H),
                None,
                "b",
                false,
            );
            let text = self.blocks(body, &mut links);
            shapes.push_str(&text_box(
                3,
                "Body",
                (
                    MARGIN,
                    CONTENT_Y,
                    SLIDE_W - 2 * MARGIN,
                    SLIDE_H - CONTENT_Y - MARGIN,
                ),
                &text,
                Some(2000),
                false,
            ));
        }
        self.slides.push(Slide {
            xml: slide_xml(&shapes),
            image: None,
            links,
            notes: String::new(),
        });
    }

    /// A plain-text run (line breaks kept).
    fn run(&self, text: &str, style: Style) -> String {
        let mut out = String::new();
        for (i, line) in text.split('\n').enumerate() {
            if i > 0 {
                out.push_str(&format!("<a:br><a:rPr{}/></a:br>", self.lang_attr()));
            }
            if !line.is_empty() {
                out.push_str(&self.text_run(line, style, None));
            }
        }
        out
    }

    fn text_run(&self, text: &str, style: Style, link: Option<usize>) -> String {
        let mut attrs = self.lang_attr();
        if style.bold {
            attrs.push_str(r#" b="1""#);
        }
        if style.italic {
            attrs.push_str(r#" i="1""#);
        }
        let mut inner = String::new();
        if style.code {
            inner.push_str(r#"<a:latin typeface="Menlo"/><a:cs typeface="Menlo"/>"#);
        }
        if let Some(n) = link {
            inner.push_str(&format!(r#"<a:hlinkClick r:id="rIdLink{n}"/>"#));
        }
        let rpr = if inner.is_empty() {
            format!("<a:rPr{attrs} dirty=\"0\"/>")
        } else {
            format!("<a:rPr{attrs} dirty=\"0\">{inner}</a:rPr>")
        };
        format!("<a:r>{rpr}<a:t>{}</a:t></a:r>", escape(text))
    }

    fn inlines(&self, inlines: &[Inline], style: Style, links: &mut Vec<String>) -> String {
        self.inlines_linked(inlines, style, None, links)
    }

    fn inlines_linked(
        &self,
        inlines: &[Inline],
        style: Style,
        link: Option<usize>,
        links: &mut Vec<String>,
    ) -> String {
        let mut out = String::new();
        for inline in inlines {
            match inline {
                Inline::Text(text) => out.push_str(&self.text_run(text, style, link)),
                Inline::Code(text) => out.push_str(&self.text_run(
                    text,
                    Style {
                        code: true,
                        ..style
                    },
                    link,
                )),
                Inline::Strong(children) => out.push_str(&self.inlines_linked(
                    children,
                    Style {
                        bold: true,
                        ..style
                    },
                    link,
                    links,
                )),
                Inline::Emphasis(children) => out.push_str(&self.inlines_linked(
                    children,
                    Style {
                        italic: true,
                        ..style
                    },
                    link,
                    links,
                )),
                Inline::Link { url, children, .. } => {
                    links.push(url.clone());
                    let n = links.len();
                    out.push_str(&self.inlines_linked(children, style, Some(n), links));
                }
                Inline::Break => {
                    out.push_str(&format!("<a:br><a:rPr{}/></a:br>", self.lang_attr()))
                }
            }
        }
        out
    }

    fn blocks(&self, blocks: &[Block], links: &mut Vec<String>) -> String {
        let mut out = String::new();
        self.push_blocks(blocks, 0, links, &mut out);
        out
    }

    fn push_blocks(&self, blocks: &[Block], level: u32, links: &mut Vec<String>, out: &mut String) {
        for block in blocks {
            match block {
                Block::Paragraph(inlines) => {
                    let ppr = if level == 0 {
                        String::new()
                    } else {
                        list_ppr(level, None)
                    };
                    out.push_str(&paragraph(
                        &ppr,
                        &self.inlines(inlines, Style::default(), links),
                    ));
                }
                Block::List { start, items } => {
                    for item in items {
                        let mut first = true;
                        for block in item {
                            match block {
                                Block::Paragraph(inlines) if first => {
                                    out.push_str(&paragraph(
                                        &list_ppr(level + 1, Some(*start)),
                                        &self.inlines(inlines, Style::default(), links),
                                    ));
                                }
                                other => self.push_blocks(
                                    std::slice::from_ref(other),
                                    level + 1,
                                    links,
                                    out,
                                ),
                            }
                            first = false;
                        }
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Style {
    bold: bool,
    italic: bool,
    code: bool,
}

/// Paragraph properties of a list item at `level` (1 for a top-level
/// list): a bullet, or a number when `numbered` holds a start.
/// A decision's slide: "Decision: question" as the title, and a bullet per
/// option, "**Label:** Go to Step 5", as the body. Slides don't link to
/// each other; the step is named.
fn decision_slide(
    decision: &RenderDecision,
    labels: &Labels,
    kind: GuideKind,
) -> (String, Vec<Block>) {
    let question = plain_text(&decision.question).replace('\n', " ");
    let title = match question.trim() {
        "" => labels.decision.clone(),
        question => format!("{}: {question}", labels.decision),
    };
    let items: Vec<Vec<Block>> = decision
        .options
        .iter()
        .map(|option| {
            let mut inlines = Vec::new();
            if !option.label.is_empty() {
                inlines.push(Inline::Strong(vec![Inline::Text(format!(
                    "{}:",
                    option.label
                ))]));
                inlines.push(Inline::Text(" ".into()));
            }
            inlines.push(Inline::Text(labels.go_to(&option.target, kind)));
            vec![Block::Paragraph(inlines)]
        })
        .collect();
    let body = if items.is_empty() {
        Vec::new()
    } else {
        vec![Block::List { start: None, items }]
    };
    (title, body)
}

fn list_ppr(level: u32, numbered: Option<Option<u64>>) -> String {
    let indent = 285_750;
    let mar = i64::from(level) * indent;
    let lvl = level.saturating_sub(1).min(8);
    let bullet = match numbered {
        Some(Some(start)) => format!(
            r#"<a:buFont typeface="+mj-lt"/><a:buAutoNum type="arabicPeriod" startAt="{}"/>"#,
            start.clamp(1, 32_767)
        ),
        Some(None) => r#"<a:buFont typeface="Arial"/><a:buChar char="•"/>"#.to_owned(),
        // A paragraph continuing a list item: indented, no bullet.
        None => "<a:buNone/>".to_owned(),
    };
    let indent = if numbered.is_some() { -indent } else { 0 };
    format!(r#"<a:pPr marL="{mar}" lvl="{lvl}" indent="{indent}">{bullet}</a:pPr>"#)
}

fn paragraph(ppr: &str, runs: &str) -> String {
    format!("<a:p>{ppr}{runs}</a:p>")
}

/// `width` × `height` pixels fitted into `(x, y, w, h)` EMU, centred.
fn fit(width: u32, height: u32, (x, y, w, h): (i64, i64, i64, i64)) -> (i64, i64, i64, i64) {
    let (iw, ih) = (i64::from(width.max(1)), i64::from(height.max(1)));
    // Never larger than the image at 96 dpi... unless that is tiny.
    let (mut cw, mut ch) = (w, w * ih / iw);
    if ch > h {
        ch = h;
        cw = h * iw / ih;
    }
    let natural = iw * EMU_PER_PX;
    if natural * 2 < cw {
        cw = natural * 2;
        ch = cw * ih / iw;
    }
    (x + (w - cw) / 2, y + (h - ch) / 2, cw, ch)
}

fn xfrm((x, y, w, h): (i64, i64, i64, i64)) -> String {
    format!(r#"<a:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="{w}" cy="{h}"/></a:xfrm>"#)
}

/// The slide's title placeholder at `rect`, optionally at `size`
/// hundredths of a point, anchored `anchor` (`b`, `ctr`) and optionally
/// centred.
fn title_shape(
    runs: &str,
    rect: (i64, i64, i64, i64),
    size: Option<u32>,
    anchor: &str,
    centred: bool,
) -> String {
    let runs = match size {
        Some(sz) => runs.replace("<a:rPr", &format!(r#"<a:rPr sz="{sz}""#)),
        None => runs.to_owned(),
    };
    let align = if centred {
        r#"<a:pPr algn="ctr"/>"#
    } else {
        ""
    };
    format!(
        r#"<p:sp><p:nvSpPr><p:cNvPr id="2" name="Title"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr><p:spPr>{}</p:spPr><p:txBody><a:bodyPr anchor="{anchor}"><a:normAutofit/></a:bodyPr><a:lstStyle/><a:p>{align}{runs}</a:p></p:txBody></p:sp>"#,
        xfrm(rect)
    )
}

/// A text box of `paragraphs`, optionally at `size` hundredths of a point.
fn text_box(
    id: u32,
    name: &str,
    rect: (i64, i64, i64, i64),
    paragraphs: &str,
    size: Option<u32>,
    centred: bool,
) -> String {
    let mut paragraphs = match size {
        Some(sz) => paragraphs.replace("<a:rPr", &format!(r#"<a:rPr sz="{sz}""#)),
        None => paragraphs.to_owned(),
    };
    if centred {
        paragraphs = paragraphs.replace("<a:p>", r#"<a:p><a:pPr algn="ctr"/>"#);
    }
    format!(
        r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="{name}"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr>{}<a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:noFill/></p:spPr><p:txBody><a:bodyPr wrap="square" lIns="0" tIns="0" rIns="0" bIns="0" anchor="t"><a:normAutofit/></a:bodyPr><a:lstStyle/>{paragraphs}</p:txBody></p:sp>"#,
        xfrm(rect)
    )
}

fn picture(id: u32, name: &str, descr: &str, rect: (i64, i64, i64, i64)) -> String {
    format!(
        r#"<p:pic><p:nvPicPr><p:cNvPr id="{id}" name="{}" descr="{}"/><p:cNvPicPr><a:picLocks noChangeAspect="1"/></p:cNvPicPr><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="rIdImage"/><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr>{}<a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:ln w="9525"><a:solidFill><a:srgbClr val="D1D1D6"/></a:solidFill></a:ln></p:spPr></p:pic>"#,
        escape(name),
        escape(descr),
        xfrm(rect)
    )
}

const GROUP: &str = r#"<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>"#;

fn slide_xml(shapes: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="{NS_A}" xmlns:r="{NS_R}" xmlns:p="{NS_P}"><p:cSld><p:spTree>{GROUP}{shapes}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>"#
    )
}

// ---------------------------------------------------------------------------
// Package

fn package<W: Write + Seek>(
    writer: W,
    guide: &RenderGuide,
    slides: &[Slide],
    timestamp: Option<DateTime<Utc>>,
) -> zip::result::ZipResult<()> {
    let mut zip = ZipWriter::new(writer);
    let deflate = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let put = |zip: &mut ZipWriter<W>, name: &str, data: &[u8], compress: bool| {
        zip.start_file(name, if compress { deflate } else { stored })?;
        zip.write_all(data)?;
        Ok::<_, zip::result::ZipError>(())
    };

    // [Content_Types].xml
    let mut types = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Override PartName="/ppt/presentation.xml" ContentType="{CT}.presentation.main+xml"/><Override PartName="/ppt/slideMasters/slideMaster1.xml" ContentType="{CT}.slideMaster+xml"/><Override PartName="/ppt/slideLayouts/slideLayout1.xml" ContentType="{CT}.slideLayout+xml"/><Override PartName="/ppt/notesMasters/notesMaster1.xml" ContentType="{CT}.notesMaster+xml"/><Override PartName="/ppt/theme/theme1.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/><Override PartName="/ppt/theme/theme2.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/><Override PartName="/ppt/presProps.xml" ContentType="{CT}.presProps+xml"/><Override PartName="/ppt/viewProps.xml" ContentType="{CT}.viewProps+xml"/><Override PartName="/ppt/tableStyles.xml" ContentType="{CT}.tableStyles+xml"/><Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/><Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/>"#
    );
    for (i, slide) in slides.iter().enumerate() {
        let n = i + 1;
        let _ = write!(
            types,
            r#"<Override PartName="/ppt/slides/slide{n}.xml" ContentType="{CT}.slide+xml"/>"#
        );
        if !slide.notes.is_empty() {
            let _ = write!(
                types,
                r#"<Override PartName="/ppt/notesSlides/notesSlide{n}.xml" ContentType="{CT}.notesSlide+xml"/>"#
            );
        }
    }
    types.push_str("</Types>");
    put(&mut zip, "[Content_Types].xml", types.as_bytes(), true)?;

    put(
        &mut zip,
        "_rels/.rels",
        rels(&[
            ("rId1", "officeDocument", "ppt/presentation.xml"),
            (
                "rId2",
                "http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties",
                "docProps/core.xml",
            ),
            ("rId3", "extended-properties", "docProps/app.xml"),
        ])
        .as_bytes(),
        true,
    )?;
    let date = timestamp
        .unwrap_or_default()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    put(
        &mut zip,
        "docProps/core.xml",
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><dc:title>{}</dc:title><dc:language>{}</dc:language><dcterms:created xsi:type="dcterms:W3CDTF">{date}</dcterms:created><dcterms:modified xsi:type="dcterms:W3CDTF">{date}</dcterms:modified></cp:coreProperties>"#,
            escape(&guide.title),
            escape(guide.language.trim()),
        )
        .as_bytes(),
        true,
    )?;
    put(
        &mut zip,
        "docProps/app.xml",
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties"><Application>GuideRecorder</Application><PresentationFormat>Widescreen</PresentationFormat><Slides>{}</Slides></Properties>"#,
            slides.len()
        )
        .as_bytes(),
        true,
    )?;

    // Presentation.
    let mut ids = String::new();
    let mut pres_rels = vec![
        (
            "rId1".to_owned(),
            "slideMaster".to_owned(),
            "slideMasters/slideMaster1.xml".to_owned(),
        ),
        (
            "rId2".to_owned(),
            "notesMaster".to_owned(),
            "notesMasters/notesMaster1.xml".to_owned(),
        ),
        (
            "rId3".to_owned(),
            "theme".to_owned(),
            "theme/theme1.xml".to_owned(),
        ),
        (
            "rId4".to_owned(),
            "presProps".to_owned(),
            "presProps.xml".to_owned(),
        ),
        (
            "rId5".to_owned(),
            "viewProps".to_owned(),
            "viewProps.xml".to_owned(),
        ),
        (
            "rId6".to_owned(),
            "tableStyles".to_owned(),
            "tableStyles.xml".to_owned(),
        ),
    ];
    for i in 0..slides.len() {
        let rid = format!("rId{}", 7 + i);
        let _ = write!(ids, r#"<p:sldId id="{}" r:id="{rid}"/>"#, 256 + i);
        pres_rels.push((rid, "slide".into(), format!("slides/slide{}.xml", i + 1)));
    }
    put(
        &mut zip,
        "ppt/presentation.xml",
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation xmlns:a="{NS_A}" xmlns:r="{NS_R}" xmlns:p="{NS_P}" saveSubsetFonts="1"><p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rId1"/></p:sldMasterIdLst><p:notesMasterIdLst><p:notesMasterId r:id="rId2"/></p:notesMasterIdLst><p:sldIdLst>{ids}</p:sldIdLst><p:sldSz cx="{SLIDE_W}" cy="{SLIDE_H}"/><p:notesSz cx="{NOTES_W}" cy="{NOTES_H}"/></p:presentation>"#
        )
        .as_bytes(),
        true,
    )?;
    let pres_rels: Vec<(&str, &str, &str)> = pres_rels
        .iter()
        .map(|(a, b, c)| (a.as_str(), b.as_str(), c.as_str()))
        .collect();
    put(
        &mut zip,
        "ppt/_rels/presentation.xml.rels",
        rels(&pres_rels).as_bytes(),
        true,
    )?;
    put(
        &mut zip,
        "ppt/presProps.xml",
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentationPr xmlns:a="{NS_A}" xmlns:r="{NS_R}" xmlns:p="{NS_P}"/>"#
        )
        .as_bytes(),
        true,
    )?;
    put(
        &mut zip,
        "ppt/viewProps.xml",
        format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:viewPr xmlns:a="{NS_A}" xmlns:r="{NS_R}" xmlns:p="{NS_P}"><p:normalViewPr><p:restoredLeft sz="15620"/><p:restoredTop sz="94660"/></p:normalViewPr><p:gridSpacing cx="76200" cy="76200"/></p:viewPr>"#)
        .as_bytes(),
        true,
    )?;
    put(
        &mut zip,
        "ppt/tableStyles.xml",
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<a:tblStyleLst xmlns:a="{NS_A}" def="{{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}}"/>"#
        )
        .as_bytes(),
        true,
    )?;

    // Master, layout, notes master, themes.
    put(
        &mut zip,
        "ppt/slideMasters/slideMaster1.xml",
        slide_master().as_bytes(),
        true,
    )?;
    put(
        &mut zip,
        "ppt/slideMasters/_rels/slideMaster1.xml.rels",
        rels(&[
            ("rId1", "slideLayout", "../slideLayouts/slideLayout1.xml"),
            ("rId2", "theme", "../theme/theme1.xml"),
        ])
        .as_bytes(),
        true,
    )?;
    put(
        &mut zip,
        "ppt/slideLayouts/slideLayout1.xml",
        slide_layout().as_bytes(),
        true,
    )?;
    put(
        &mut zip,
        "ppt/slideLayouts/_rels/slideLayout1.xml.rels",
        rels(&[("rId1", "slideMaster", "../slideMasters/slideMaster1.xml")]).as_bytes(),
        true,
    )?;
    put(
        &mut zip,
        "ppt/notesMasters/notesMaster1.xml",
        notes_master().as_bytes(),
        true,
    )?;
    put(
        &mut zip,
        "ppt/notesMasters/_rels/notesMaster1.xml.rels",
        rels(&[("rId1", "theme", "../theme/theme2.xml")]).as_bytes(),
        true,
    )?;
    put(
        &mut zip,
        "ppt/theme/theme1.xml",
        theme("Office Theme").as_bytes(),
        true,
    )?;
    put(
        &mut zip,
        "ppt/theme/theme2.xml",
        theme("Notes Theme").as_bytes(),
        true,
    )?;

    // Slides.
    for (i, slide) in slides.iter().enumerate() {
        let n = i + 1;
        put(
            &mut zip,
            &format!("ppt/slides/slide{n}.xml"),
            slide.xml.as_bytes(),
            true,
        )?;
        let mut slide_rels = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdLayout" Type="{REL}/slideLayout" Target="../slideLayouts/slideLayout1.xml"/>"#
        );
        if slide.image.is_some() {
            let _ = write!(
                slide_rels,
                r#"<Relationship Id="rIdImage" Type="{REL}/image" Target="../media/image{n}.png"/>"#
            );
        }
        if !slide.notes.is_empty() {
            let _ = write!(
                slide_rels,
                r#"<Relationship Id="rIdNotes" Type="{REL}/notesSlide" Target="../notesSlides/notesSlide{n}.xml"/>"#
            );
        }
        for (k, url) in slide.links.iter().enumerate() {
            let _ = write!(
                slide_rels,
                r#"<Relationship Id="rIdLink{}" Type="{REL}/hyperlink" Target="{}" TargetMode="External"/>"#,
                k + 1,
                escape(url)
            );
        }
        slide_rels.push_str("</Relationships>");
        put(
            &mut zip,
            &format!("ppt/slides/_rels/slide{n}.xml.rels"),
            slide_rels.as_bytes(),
            true,
        )?;
        if let Some(png) = &slide.image {
            // PNG is compressed already.
            put(&mut zip, &format!("ppt/media/image{n}.png"), png, false)?;
        }
        if !slide.notes.is_empty() {
            // Links in notes are kept as text: notes slides get no
            // hyperlink relationships.
            let notes = strip_links(&slide.notes);
            put(
                &mut zip,
                &format!("ppt/notesSlides/notesSlide{n}.xml"),
                notes_slide(&notes).as_bytes(),
                true,
            )?;
            put(
                &mut zip,
                &format!("ppt/notesSlides/_rels/notesSlide{n}.xml.rels"),
                rels(&[
                    ("rId1", "notesMaster", "../notesMasters/notesMaster1.xml"),
                    ("rId2", "slide", &format!("../slides/slide{n}.xml")),
                ])
                .as_bytes(),
                true,
            )?;
        }
    }
    zip.finish()?;
    Ok(())
}

/// `paragraphs` without hyperlink elements.
fn strip_links(paragraphs: &str) -> String {
    let mut out = String::with_capacity(paragraphs.len());
    let mut rest = paragraphs;
    while let Some(start) = rest.find("<a:hlinkClick ") {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let end = rest.find("/>").map_or(rest.len(), |e| e + 2);
        rest = &rest[end..];
    }
    out.push_str(rest);
    // An `<a:rPr ...></a:rPr>` left empty is still valid XML.
    out
}

/// A relationships part; a `kind` without a scheme is an officeDocument
/// relationship type.
fn rels(entries: &[(&str, &str, &str)]) -> String {
    let mut out = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
    );
    for (id, kind, target) in entries {
        let kind = if kind.starts_with("http") {
            (*kind).to_owned()
        } else {
            format!("{REL}/{kind}")
        };
        let _ = write!(
            out,
            r#"<Relationship Id="{id}" Type="{kind}" Target="{target}"/>"#
        );
    }
    out.push_str("</Relationships>");
    out
}

const CLR_MAP: &str = r#"bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink""#;

fn title_placeholder() -> String {
    format!(
        r#"<p:sp><p:nvSpPr><p:cNvPr id="2" name="Title Placeholder 1"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr><p:spPr>{}</p:spPr><p:txBody><a:bodyPr anchor="b"><a:normAutofit/></a:bodyPr><a:lstStyle/><a:p><a:endParaRPr lang="en-US"/></a:p></p:txBody></p:sp>"#,
        xfrm((MARGIN, TITLE_Y, SLIDE_W - 2 * MARGIN, TITLE_H))
    )
}

fn slide_master() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sldMaster xmlns:a="{NS_A}" xmlns:r="{NS_R}" xmlns:p="{NS_P}"><p:cSld><p:bg><p:bgRef idx="1001"><a:schemeClr val="bg1"/></p:bgRef></p:bg><p:spTree>{GROUP}{}</p:spTree></p:cSld><p:clrMap {CLR_MAP}/><p:sldLayoutIdLst><p:sldLayoutId id="2147483649" r:id="rId1"/></p:sldLayoutIdLst><p:txStyles><p:titleStyle><a:lvl1pPr algn="l"><a:defRPr sz="2800" b="1" kern="1200"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill><a:latin typeface="+mj-lt"/><a:ea typeface="+mj-ea"/><a:cs typeface="+mj-cs"/></a:defRPr></a:lvl1pPr></p:titleStyle><p:bodyStyle><a:lvl1pPr><a:defRPr sz="1800" kern="1200"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill><a:latin typeface="+mn-lt"/><a:ea typeface="+mn-ea"/><a:cs typeface="+mn-cs"/></a:defRPr></a:lvl1pPr></p:bodyStyle><p:otherStyle><a:defPPr><a:defRPr lang="en-US"/></a:defPPr><a:lvl1pPr><a:defRPr sz="1800" kern="1200"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill><a:latin typeface="+mn-lt"/><a:ea typeface="+mn-ea"/><a:cs typeface="+mn-cs"/></a:defRPr></a:lvl1pPr></p:otherStyle></p:txStyles></p:sldMaster>"#,
        title_placeholder()
    )
}

fn slide_layout() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sldLayout xmlns:a="{NS_A}" xmlns:r="{NS_R}" xmlns:p="{NS_P}" type="titleOnly" preserve="1"><p:cSld name="Title Only"><p:spTree>{GROUP}{}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sldLayout>"#,
        title_placeholder()
    )
}

/// Slide image and notes body placeholders of the notes pages.
fn notes_placeholders(body: &str) -> String {
    format!(
        r#"<p:sp><p:nvSpPr><p:cNvPr id="2" name="Slide Image Placeholder 1"/><p:cNvSpPr><a:spLocks noGrp="1" noRot="1" noChangeAspect="1"/></p:cNvSpPr><p:nvPr><p:ph type="sldImg" idx="2"/></p:nvPr></p:nvSpPr><p:spPr>{}<a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:noFill/><a:ln w="12700"><a:solidFill><a:prstClr val="black"/></a:solidFill></a:ln></p:spPr></p:sp><p:sp><p:nvSpPr><p:cNvPr id="3" name="Notes Placeholder 2"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="body" sz="quarter" idx="3"/></p:nvPr></p:nvSpPr><p:spPr>{}</p:spPr><p:txBody><a:bodyPr/><a:lstStyle/>{body}</p:txBody></p:sp>"#,
        xfrm((381_000, 685_800, 6_096_000, 3_429_000)),
        xfrm((685_800, 4_343_400, 5_486_400, 4_114_800)),
    )
}

fn notes_master() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:notesMaster xmlns:a="{NS_A}" xmlns:r="{NS_R}" xmlns:p="{NS_P}"><p:cSld><p:bg><p:bgRef idx="1001"><a:schemeClr val="bg1"/></p:bgRef></p:bg><p:spTree>{GROUP}{}</p:spTree></p:cSld><p:clrMap {CLR_MAP}/><p:notesStyle><a:lvl1pPr marL="0" algn="l" defTabSz="914400" rtl="0" eaLnBrk="1" latinLnBrk="0" hangingPunct="1"><a:defRPr sz="1200" kern="1200"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill><a:latin typeface="+mn-lt"/><a:ea typeface="+mn-ea"/><a:cs typeface="+mn-cs"/></a:defRPr></a:lvl1pPr></p:notesStyle></p:notesMaster>"#,
        notes_placeholders("<a:p><a:endParaRPr lang=\"en-US\"/></a:p>")
    )
}

fn notes_slide(paragraphs: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:notes xmlns:a="{NS_A}" xmlns:r="{NS_R}" xmlns:p="{NS_P}"><p:cSld><p:spTree>{GROUP}{}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:notes>"#,
        notes_placeholders(paragraphs)
    )
}

/// A plain Office-style theme: system colours, Helvetica Neue.
fn theme(name: &str) -> String {
    let fills = r#"<a:solidFill><a:schemeClr val="phClr"/></a:solidFill>"#.repeat(3);
    let lines =
        r#"<a:ln w="9525"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln>"#.repeat(3);
    let effects = "<a:effectStyle><a:effectLst/></a:effectStyle>".repeat(3);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<a:theme xmlns:a="{NS_A}" name="{name}"><a:themeElements><a:clrScheme name="GuideRecorder"><a:dk1><a:srgbClr val="1D1D1F"/></a:dk1><a:lt1><a:srgbClr val="FFFFFF"/></a:lt1><a:dk2><a:srgbClr val="3A3A3C"/></a:dk2><a:lt2><a:srgbClr val="F2F2F7"/></a:lt2><a:accent1><a:srgbClr val="007AFF"/></a:accent1><a:accent2><a:srgbClr val="FF3B30"/></a:accent2><a:accent3><a:srgbClr val="34C759"/></a:accent3><a:accent4><a:srgbClr val="FF9500"/></a:accent4><a:accent5><a:srgbClr val="AF52DE"/></a:accent5><a:accent6><a:srgbClr val="5AC8FA"/></a:accent6><a:hlink><a:srgbClr val="007AFF"/></a:hlink><a:folHlink><a:srgbClr val="5856D6"/></a:folHlink></a:clrScheme><a:fontScheme name="GuideRecorder"><a:majorFont><a:latin typeface="Helvetica Neue"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont><a:minorFont><a:latin typeface="Helvetica Neue"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont></a:fontScheme><a:fmtScheme name="GuideRecorder"><a:fillStyleLst>{fills}</a:fillStyleLst><a:lnStyleLst>{lines}</a:lnStyleLst><a:effectStyleLst>{effects}</a:effectStyleLst><a:bgFillStyleLst>{fills}</a:bgFillStyleLst></a:fmtScheme></a:themeElements><a:objectDefaults/><a:extraClrSchemeLst/></a:theme>"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn images_fit_their_box_centred() {
        let (x, y, w, h) = fit(1600, 900, (0, 0, 1600 * EMU_PER_PX, 1600 * EMU_PER_PX));
        assert_eq!((x, w), (0, 1600 * EMU_PER_PX));
        assert_eq!(h, 900 * EMU_PER_PX);
        assert_eq!(y, (1600 - 900) * EMU_PER_PX / 2);
        // A tiny image is shown at most twice its 96 dpi size.
        let (_, _, w, _) = fit(10, 10, (0, 0, 10_000_000, 10_000_000));
        assert_eq!(w, 20 * EMU_PER_PX);
    }

    #[test]
    fn strip_links_removes_hyperlinks_only() {
        let xml =
            r#"<a:r><a:rPr dirty="0"><a:hlinkClick r:id="rIdLink1"/></a:rPr><a:t>x</a:t></a:r>"#;
        assert_eq!(
            strip_links(xml),
            r#"<a:r><a:rPr dirty="0"></a:rPr><a:t>x</a:t></a:r>"#
        );
    }
}
