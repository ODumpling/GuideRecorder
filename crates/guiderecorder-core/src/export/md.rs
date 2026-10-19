//! Markdown writer over [`RenderGuide`]: `guide.md` plus `images/NN-slug.png`.
//!
//! # Layout
//!
//! ```text
//! # Title
//!
//! Summary
//!
//! Intro
//!
//! ## Prerequisites
//!
//! ...
//!
//! ## Section            (sections are H2; steps are H3 when the guide has
//!                        sections, H2 otherwise)
//!
//! ### 1. Click **Save**
//!
//! ![Click Save](images/01-click-save.png)
//!
//! 1) **Save**\
//!    Saves the file
//!
//! Description
//!
//! > **Tip:** note body
//! ```
//!
//! Step images have their hotspots drawn in as numbered markers, and the
//! numbered legend under the image gives each hotspot's title (bold) and
//! description, line breaks kept. The legend uses `1)` markers so a
//! description that starts with a `1.` list can't continue it. In an
//! annotation guide ([`GuideKind::Annotated`]) an untitled step is headed
//! "Page" instead of "Step".
//!
//! With a branding logo ([`CheckedBranding`]), it is written as
//! `images/logo.png` (the checked, re-encoded PNG) and shown right under
//! the title. The accent colour
//! has no place in Markdown and is ignored (but still validated).
//!
//! # Escaping
//!
//! The [`ast`](super::ast) holds raw text, so every `Text` node is escaped
//! for CommonMark: inline metacharacters (`\ * _ [ ] < > & | ~ $` and the
//! backtick; `$` for renderers with math) are backslash-escaped everywhere,
//! a `!` right before a link is escaped so it can't become an image, and
//! block markers (`#`, `>`, `-`, `+`, `=`, `1.`) are escaped at the start
//! of a line. The result parses back to the same text, with no raw HTML,
//! autolinks, images or entities.
//!
//! Output is deterministic: the same guide and images give the same bytes.

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Cursor};
use std::path::{Path, PathBuf};

use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader};

use super::ast::{plain_text, Block, Inline};
use super::{render_step_image, ExportReport, ImageOptions, RenderGuide, RenderItem, RenderStep};
use crate::library::{write_atomic, Durability, GuideHandle, LibraryError};
use crate::model::{GuideKind, NoteKind, ReviewInfo};

/// File name of the Markdown document inside the output folder.
pub const GUIDE_FILE: &str = "guide.md";
/// Folder (inside the output folder) the step images are written to.
pub const IMAGES_DIR: &str = "images";
/// File name of the branding logo inside [`IMAGES_DIR`].
pub const LOGO_FILE: &str = "logo.png";

/// Options for [`export`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MdOptions {
    /// How step images are rendered. The default keeps full resolution.
    pub image: ImageOptions,
    /// Write into a non-empty `out_dir`, replacing `guide.md` and the
    /// images of an earlier export.
    pub overwrite: bool,
    /// Fixed labels; `None` picks them from the guide language.
    pub labels: Option<Labels>,
    /// Logo under the title; off by default. The accent is not used.
    pub branding: CheckedBranding,
    /// Show the version, owner and last review day under the summary (see
    /// [`Labels::review_details`]); off by default.
    pub review_details: bool,
}

/// Errors from [`export`] (and the HTML writer's file output).
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    /// `out_dir` already has files and `overwrite` is off.
    #[error("{0}: output folder is not empty (set overwrite to replace it)")]
    NotEmpty(PathBuf),
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error(transparent)]
    Write(#[from] LibraryError),
    /// An HTML template failure.
    #[error("html: {0}")]
    Html(String),
}

pub type Result<T, E = ExportError> = std::result::Result<T, E>;

/// Fixed words a writer adds around the guide text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Labels {
    pub prerequisites: String,
    pub step: String,
    /// What a step of an annotation guide ([`GuideKind::Annotated`]) is
    /// called.
    pub page: String,
    pub tip: String,
    pub warning: String,
    pub note: String,
    /// The review details line (see [`Labels::review_details`]).
    pub version: String,
    pub owner: String,
    pub last_reviewed: String,
}

impl Labels {
    /// Labels for a BCP 47 language tag: German for `de*`, English
    /// otherwise.
    pub fn for_language(language: &str) -> Self {
        let primary = language
            .split(['-', '_'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let [prerequisites, step, page, tip, warning, note, version, owner, last_reviewed] =
            match primary.as_str() {
                "de" => [
                    "Voraussetzungen",
                    "Schritt",
                    "Seite",
                    "Tipp",
                    "Warnung",
                    "Hinweis",
                    "Version",
                    "Verantwortlich",
                    "Zuletzt geprüft",
                ],
                _ => [
                    "Prerequisites",
                    "Step",
                    "Page",
                    "Tip",
                    "Warning",
                    "Note",
                    "Version",
                    "Owner",
                    "Last reviewed",
                ],
            };
        Self {
            prerequisites: prerequisites.into(),
            step: step.into(),
            page: page.into(),
            tip: tip.into(),
            warning: warning.into(),
            note: note.into(),
            version: version.into(),
            owner: owner.into(),
            last_reviewed: last_reviewed.into(),
        }
    }

    /// One plain-text line with the guide's version, owner and last review
    /// day, those that are set: `Version: 1.2 · Owner: IT · Last reviewed:
    /// 2026-10-19`. `None` when none is. Writers show it under the summary
    /// when their `review_details` option is on.
    pub fn review_details(&self, review: &ReviewInfo) -> Option<String> {
        let text = |value: &Option<String>| {
            value
                .as_deref()
                .map(|v| v.split_whitespace().collect::<Vec<_>>().join(" "))
                .filter(|v| !v.is_empty())
        };
        let parts: Vec<String> = [
            (&self.version, text(&review.version)),
            (&self.owner, text(&review.owner)),
            (
                &self.last_reviewed,
                review.last_reviewed.map(|d| d.to_string()),
            ),
        ]
        .into_iter()
        .filter_map(|(label, value)| Some(format!("{label}: {}", value?)))
        .collect();
        (!parts.is_empty()).then(|| parts.join(" · "))
    }

    /// What a step is called in a guide of `kind`: "Step", or "Page" in an
    /// annotation guide.
    pub fn step_kind(&self, kind: GuideKind) -> &str {
        match kind {
            GuideKind::Walkthrough => &self.step,
            GuideKind::Annotated => &self.page,
        }
    }

    /// The label of a note kind.
    pub fn note_kind(&self, kind: NoteKind) -> &str {
        match kind {
            NoteKind::Tip => &self.tip,
            NoteKind::Warning => &self.warning,
            NoteKind::Note => &self.note,
        }
    }
}

// ---------------------------------------------------------------------------
// Branding

/// Largest logo file accepted, in bytes.
pub const MAX_LOGO_BYTES: usize = 2 * 1024 * 1024;
/// Largest logo width or height accepted, in pixels.
pub const MAX_LOGO_SIDE: u32 = 2000;

/// Branding as given by the user: a logo and an accent colour. Turn it into
/// a [`CheckedBranding`] with [`Branding::check`]; the writers' options take
/// that, so a writer never sees an invalid logo and never decodes it again.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Branding {
    /// PNG or JPEG bytes, at most [`MAX_LOGO_BYTES`] and
    /// [`MAX_LOGO_SIDE`] px on either side. SVG is refused: it can carry
    /// scripts and external references, and not every format can show it.
    pub logo: Option<Vec<u8>>,
    /// Hex colour, `#rgb` or `#rrggbb`.
    pub accent: Option<String>,
}

/// Why a [`Branding`] was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BrandingError {
    #[error("the logo is an SVG image; use a PNG or JPEG logo")]
    SvgLogo,
    #[error("the logo is not a PNG or JPEG image")]
    LogoFormat,
    #[error("the logo is {0} bytes; the limit is {MAX_LOGO_BYTES} bytes (2 MB)")]
    LogoTooLarge(usize),
    #[error("the logo is {width}×{height} px; the limit is {MAX_LOGO_SIDE} px on either side")]
    LogoDimensions { width: u32, height: u32 },
    #[error("the logo cannot be read: {0}")]
    LogoUnreadable(String),
    #[error("accent {0:?} is not a hex colour (#rgb or #rrggbb)")]
    Accent(String),
}

/// A validated logo, re-encoded as PNG.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Logo {
    png: Vec<u8>,
    width: u32,
    height: u32,
}

impl Logo {
    /// PNG bytes: decoded once, turned upright by its EXIF orientation and
    /// re-encoded, so no metadata (EXIF, text chunks) is carried along.
    pub fn png(&self) -> &[u8] {
        &self.png
    }

    /// Width in pixels, upright.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels, upright.
    pub fn height(&self) -> u32 {
        self.height
    }
}

/// Branding that passed [`Branding::check`], as every writer takes it.
/// Off ([`Default`]) unless built by `check`.
///
/// Where each writer puts it:
///
/// - Markdown: the logo as `images/logo.png` under the title; no accent.
/// - HTML: the logo above the title (a data URI); the accent on step
///   numbers, links and the header rule.
/// - DOCX: the logo above the title; the accent on headings and on the
///   border of neutral notes (tips and warnings keep their green and
///   orange, which carry meaning).
/// - PDF: the logo at the top of the cover page; the accent on the cover
///   rule and section headings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckedBranding {
    logo: Option<Logo>,
    accent: Option<[u8; 3]>,
}

impl CheckedBranding {
    pub fn logo(&self) -> Option<&Logo> {
        self.logo.as_ref()
    }

    /// The accent as RGB.
    pub fn accent(&self) -> Option<[u8; 3]> {
        self.accent
    }

    /// The accent as lowercase `#rrggbb`.
    pub fn accent_hex(&self) -> Option<String> {
        self.accent
            .map(|[r, g, b]| format!("#{r:02x}{g:02x}{b:02x}"))
    }
}

impl Branding {
    /// Validates the accent colour and the logo (format, file size, pixel
    /// size from the header before decoding), then decodes the logo once
    /// and re-encodes it as an upright PNG.
    pub fn check(&self) -> Result<CheckedBranding, BrandingError> {
        let accent = self.accent.as_deref().map(parse_accent).transpose()?;
        let logo = self.logo.as_deref().map(check_logo).transpose()?;
        Ok(CheckedBranding { logo, accent })
    }
}

/// `#rgb` or `#rrggbb` as RGB.
fn parse_accent(value: &str) -> Result<[u8; 3], BrandingError> {
    let invalid = || BrandingError::Accent(value.to_owned());
    let hex = value.strip_prefix('#').ok_or_else(invalid)?;
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(invalid());
    }
    let digit = |i: usize| u8::from_str_radix(&hex[i..=i], 16).map_err(|_| invalid());
    match hex.len() {
        3 => Ok([digit(0)? * 17, digit(1)? * 17, digit(2)? * 17]),
        6 => {
            let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| invalid());
            Ok([byte(0)?, byte(2)?, byte(4)?])
        }
        _ => Err(invalid()),
    }
}

fn check_logo(bytes: &[u8]) -> Result<Logo, BrandingError> {
    if bytes.len() > MAX_LOGO_BYTES {
        return Err(BrandingError::LogoTooLarge(bytes.len()));
    }
    let format = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        ImageFormat::Png
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        ImageFormat::Jpeg
    } else if looks_like_svg(bytes) {
        return Err(BrandingError::SvgLogo);
    } else {
        return Err(BrandingError::LogoFormat);
    };
    let unreadable = |e: image::ImageError| BrandingError::LogoUnreadable(e.to_string());
    let mut reader = ImageReader::new(Cursor::new(bytes));
    reader.set_format(format);
    let mut decoder = reader.into_decoder().map_err(unreadable)?;
    // The header first, so an oversized image is refused before decoding.
    // A quarter turn swaps the sides, which the square limit doesn't mind.
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 || width > MAX_LOGO_SIDE || height > MAX_LOGO_SIDE {
        return Err(BrandingError::LogoDimensions { width, height });
    }
    let orientation = decoder.orientation().map_err(unreadable)?;
    let mut image = DynamicImage::from_decoder(decoder).map_err(unreadable)?;
    image.apply_orientation(orientation);
    let mut png = Vec::new();
    if image
        .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .is_err()
    {
        // A colour type PNG can't hold (e.g. 32-bit float): store RGBA.
        png.clear();
        DynamicImage::ImageRgba8(image.to_rgba8())
            .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
            .map_err(unreadable)?;
    }
    Ok(Logo {
        png,
        width: image.width(),
        height: image.height(),
    })
}

/// An XML or SVG document: `<` after an optional BOM and whitespace.
fn looks_like_svg(bytes: &[u8]) -> bool {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    bytes
        .iter()
        .find(|b| !b.is_ascii_whitespace())
        .is_some_and(|&b| b == b'<')
}

// ---------------------------------------------------------------------------
// Export

/// Writes `<out_dir>/guide.md` and `<out_dir>/images/NN-<slug>.png`, with
/// step images read from the bundle at `handle` (normally `guide.handle`).
///
/// Refuses a non-empty `out_dir` unless `opts.overwrite`; with it, earlier
/// `NN-*.png` images this export did not write are removed, and nothing
/// else is touched (an earlier `images/logo.png` stays, unreferenced, when
/// this export has no logo). A broken step image becomes a placeholder
/// plus a warning in the report.
pub fn export(
    guide: &RenderGuide,
    handle: &GuideHandle,
    out_dir: impl AsRef<Path>,
    opts: &MdOptions,
) -> Result<ExportReport> {
    let out_dir = out_dir.as_ref();
    let logo = opts.branding.logo().map(Logo::png);
    prepare_out_dir(out_dir, opts.overwrite)?;
    let images_dir = out_dir.join(IMAGES_DIR);
    fs::create_dir_all(&images_dir).map_err(io_err(&images_dir))?;

    let labels = opts
        .labels
        .clone()
        .unwrap_or_else(|| Labels::for_language(&guide.language));
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

    let logo_path = images_dir.join(LOGO_FILE);
    if let Some(png) = &logo {
        write_atomic(&logo_path, png, Durability::RenameOnly)?;
    }

    let details = opts
        .review_details
        .then(|| labels.review_details(&guide.review))
        .flatten();
    let markdown = render_document(guide, &names, &labels, logo.is_some(), details.as_deref());
    let doc = out_dir.join(GUIDE_FILE);
    write_atomic(&doc, markdown.as_bytes(), Durability::RenameOnly)?;
    report.files.push(doc);
    if logo.is_some() {
        report.files.push(logo_path.clone());
    }
    report.files.extend(images);

    if opts.overwrite {
        remove_stale_images(&images_dir, &names)?;
    }
    Ok(report)
}

/// The Markdown document for `guide`, with image `i` at
/// `images/<names[i]>`. Exposed for tests and previews; [`export`] writes
/// it.
pub fn render(guide: &RenderGuide, image_names: &[String], labels: &Labels) -> String {
    render_with_logo(guide, image_names, labels, false)
}

/// [`render`], plus `images/logo.png` right under the title when `logo`.
pub fn render_with_logo(
    guide: &RenderGuide,
    image_names: &[String],
    labels: &Labels,
    logo: bool,
) -> String {
    render_document(guide, image_names, labels, logo, None)
}

/// [`render_with_logo`], plus a `details` line (the review details) under
/// the summary.
fn render_document(
    guide: &RenderGuide,
    image_names: &[String],
    labels: &Labels,
    logo: bool,
    details: Option<&str>,
) -> String {
    let mut out = Vec::new();
    let title = escape_line(&single_line(&guide.title));
    if !title.is_empty() {
        push_block(&mut out, &format!("# {}", heading_text(&title)));
    }
    if logo {
        push_block(&mut out, &format!("![]({IMAGES_DIR}/{LOGO_FILE})"));
    }
    let summary = single_line(&guide.summary);
    if !summary.is_empty() {
        push_block(&mut out, &escape_line(&summary));
    }
    if let Some(details) = details {
        push_block(&mut out, &format!("*{}*", escape_line(details)));
    }
    if !guide.intro.is_empty() {
        push_block(&mut out, &blocks(&guide.intro));
    }
    if !guide.prerequisites.is_empty() {
        push_block(
            &mut out,
            &format!("## {}", heading_text(&escape_line(&labels.prerequisites))),
        );
        push_block(&mut out, &blocks(&guide.prerequisites));
    }

    let has_sections = guide
        .items
        .iter()
        .any(|item| matches!(item, RenderItem::Section(_)));
    let step_heading = if has_sections { "###" } else { "##" };
    let mut names = image_names.iter();
    for item in &guide.items {
        match item {
            RenderItem::Section(section) => {
                let title = escape_line(&single_line(&section.title));
                push_block(&mut out, &format!("## {}", heading_text(&title)));
            }
            RenderItem::Step(step) => {
                let title = inlines_single_line(&step.title);
                let title = if title.is_empty() {
                    escape_line(labels.step_kind(guide.kind))
                } else {
                    title
                };
                push_block(
                    &mut out,
                    &format!("{step_heading} {}. {}", step.number, heading_text(&title)),
                );
                if let Some(name) = names.next() {
                    let alt = escape_line(&single_line(&plain_text(&step.title)));
                    push_block(&mut out, &format!("![{alt}]({IMAGES_DIR}/{name})"));
                }
                if let Some(legend) = hotspot_legend(step) {
                    push_block(&mut out, &legend);
                }
                if !step.description.is_empty() {
                    push_block(&mut out, &blocks(&step.description));
                }
            }
            RenderItem::Note(note) => {
                let label = format!("**{}:**", escape_line(labels.note_kind(note.kind)));
                let body = match note.body.split_first() {
                    Some((Block::Paragraph(first), rest)) => {
                        let mut body = format!("{label} {}", paragraph(first, ""));
                        if !rest.is_empty() {
                            body.push_str("\n\n");
                            body.push_str(&blocks(rest));
                        }
                        body
                    }
                    Some(_) => format!("{label}\n\n{}", blocks(&note.body)),
                    None => label,
                };
                push_block(&mut out, &quote(&body));
            }
        }
    }
    let mut doc = out.join("\n\n");
    doc.push('\n');
    doc
}

/// The numbered list of `step`'s hotspots, if it has any: `N) **Title**`,
/// then the description lines as hard-broken continuation lines. Empty
/// lines are dropped.
fn hotspot_legend(step: &RenderStep) -> Option<String> {
    let items: Vec<String> = step
        .hotspots()
        .map(|(number, hotspot)| {
            let marker = format!("{number}) ");
            let mut lines = Vec::new();
            let title = escape_line(&single_line(&hotspot.title));
            if !title.is_empty() {
                lines.push(format!("**{title}**"));
            }
            lines.extend(
                hotspot
                    .description
                    .lines()
                    .map(escape_line)
                    .filter(|line| !line.is_empty()),
            );
            let indent = " ".repeat(marker.len());
            format!("{marker}{}", lines.join(&format!("\\\n{indent}")))
                .trim_end()
                .to_owned()
        })
        .collect();
    (!items.is_empty()).then(|| items.join("\n"))
}

/// `NN-<slug>.png` for every step in order. `NN` is the step number,
/// zero-padded to two digits (more for guides with 100+ steps).
pub fn image_names(guide: &RenderGuide) -> Vec<String> {
    let count = guide.steps().count();
    let width = count.to_string().len().max(2);
    guide
        .steps()
        .map(|step| {
            format!(
                "{:0width$}-{}.png",
                step.number,
                slug(&plain_text(&step.title))
            )
        })
        .collect()
}

/// Lowercase ASCII words joined by `-`, at most 48 bytes; `step` if
/// nothing is left. German umlauts and ß are transliterated; other
/// non-ASCII letters are dropped.
pub fn slug(text: &str) -> String {
    const MAX: usize = 48;
    let mut out = String::new();
    let mut dash = false;
    for c in text.chars() {
        let piece = match c.to_lowercase().next().unwrap_or(c) {
            c if c.is_ascii_alphanumeric() => c.to_string(),
            'ä' => "ae".into(),
            'ö' => "oe".into(),
            'ü' => "ue".into(),
            'ß' => "ss".into(),
            c if c.is_alphanumeric() => continue,
            _ => {
                dash = true;
                continue;
            }
        };
        if dash && !out.is_empty() {
            out.push('-');
        }
        dash = false;
        out.push_str(&piece);
    }
    if out.len() > MAX {
        out.truncate(MAX);
        while out.ends_with('-') {
            out.pop();
        }
    }
    if out.is_empty() {
        out.push_str("step");
    }
    out
}

// ---------------------------------------------------------------------------
// Output folder

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> ExportError + '_ {
    move |source| ExportError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Creates `out_dir`, or checks that it is empty unless `overwrite`.
pub(super) fn prepare_out_dir(out_dir: &Path, overwrite: bool) -> Result<()> {
    match fs::read_dir(out_dir) {
        Ok(mut entries) => {
            if !overwrite && entries.next().is_some() {
                return Err(ExportError::NotEmpty(out_dir.to_path_buf()));
            }
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(out_dir).map_err(io_err(out_dir))
        }
        Err(e) => Err(io_err(out_dir)(e)),
    }
}

/// Removes `NN-*.png` files in `images_dir` that are not in `keep`.
fn remove_stale_images(images_dir: &Path, keep: &[String]) -> Result<()> {
    let keep: BTreeSet<&str> = keep.iter().map(String::as_str).collect();
    for entry in fs::read_dir(images_dir).map_err(io_err(images_dir))? {
        let entry = entry.map_err(io_err(images_dir))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if keep.contains(name) || !is_export_image_name(name) {
            continue;
        }
        if entry.file_type().map_err(io_err(images_dir))?.is_file() {
            let path = entry.path();
            fs::remove_file(&path).map_err(io_err(&path))?;
        }
    }
    Ok(())
}

/// Whether `name` looks like `NN-slug.png` from [`image_names`].
fn is_export_image_name(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".png") else {
        return false;
    };
    let Some((number, slug)) = stem.split_once('-') else {
        return false;
    };
    number.len() >= 2
        && number.bytes().all(|b| b.is_ascii_digit())
        && !slug.is_empty()
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

// ---------------------------------------------------------------------------
// Serializer

/// Appends a block; blocks are separated by one blank line.
fn push_block(out: &mut Vec<String>, block: &str) {
    out.push(block.to_owned());
}

/// Collapses line breaks and runs of whitespace into single spaces.
fn single_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Blocks as Markdown, without a trailing newline.
fn blocks(blocks: &[Block]) -> String {
    blocks_indented(blocks, "")
}

/// Blocks with every line after the first prefixed by `indent` (the first
/// line follows a list marker or sits at the margin).
fn blocks_indented(blocks: &[Block], indent: &str) -> String {
    let mut out = String::new();
    for (i, block) in blocks.iter().enumerate() {
        if i > 0 {
            // A list right after a paragraph stays tight inside list items.
            let tight = !indent.is_empty()
                && matches!(blocks[i - 1], Block::Paragraph(_))
                && matches!(block, Block::List { .. });
            out.push('\n');
            if !tight {
                out.push('\n');
            }
            out.push_str(indent);
        }
        match block {
            Block::Paragraph(inlines) => out.push_str(&paragraph(inlines, indent)),
            Block::List { start, items } => out.push_str(&list(*start, items, indent)),
        }
    }
    out
}

fn list(start: Option<u64>, items: &[Vec<Block>], indent: &str) -> String {
    let mut out = String::new();
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push('\n');
            out.push_str(indent);
        }
        let marker = match start {
            Some(start) => format!("{}. ", start.saturating_add(i as u64)),
            None => "- ".into(),
        };
        let inner = format!("{indent}{}", " ".repeat(marker.len()));
        out.push_str(&marker);
        if item.is_empty() {
            continue;
        }
        out.push_str(&blocks_indented(item, &inner));
    }
    out
}

/// A paragraph; lines after a hard break are prefixed by `indent`.
fn paragraph(inlines: &[Inline], indent: &str) -> String {
    let mut lines = vec![String::new()];
    push_inlines(inlines, &mut lines, false);
    lines
        .iter()
        .map(|line| escape_line_start(line.trim()))
        .collect::<Vec<_>>()
        .join(&format!("\\\n{indent}"))
}

/// Inlines on one line (hard breaks become spaces), for headings.
fn inlines_single_line(inlines: &[Inline]) -> String {
    let mut lines = vec![String::new()];
    push_inlines(inlines, &mut lines, true);
    escape_line_start(lines[0].trim())
}

fn push_inlines(inlines: &[Inline], lines: &mut Vec<String>, single: bool) {
    for inline in inlines {
        match inline {
            Inline::Text(text) => {
                let line = lines.last_mut().expect("at least one line");
                line.push_str(&escape_inline(&text.replace(['\n', '\r'], " ")));
            }
            Inline::Code(code) => {
                let line = lines.last_mut().expect("at least one line");
                line.push_str(&code_span(code));
            }
            Inline::Strong(children) | Inline::Emphasis(children) => {
                let mark = if matches!(inline, Inline::Strong(_)) {
                    "**"
                } else {
                    "*"
                };
                // Render on one line: a delimiter run can't span a break
                // cleanly, and empty emphasis would print literal stars.
                let mut inner = vec![String::new()];
                push_inlines(children, &mut inner, true);
                let inner = inner.concat();
                let inner = inner.trim();
                if !inner.is_empty() {
                    let line = lines.last_mut().expect("at least one line");
                    line.push_str(mark);
                    line.push_str(inner);
                    line.push_str(mark);
                }
            }
            Inline::Link {
                url,
                title,
                children,
            } => {
                let mut inner = vec![String::new()];
                push_inlines(children, &mut inner, true);
                let text = inner.concat();
                let line = lines.last_mut().expect("at least one line");
                // `!` right before `[` would turn the link into an image,
                // which viewers load on open.
                if ends_with_unescaped(line, '!') {
                    line.insert(line.len() - 1, '\\');
                }
                line.push('[');
                line.push_str(text.trim());
                line.push_str("](");
                line.push_str(&link_destination(url));
                if !title.is_empty() {
                    line.push_str(" \"");
                    line.push_str(&link_title(title));
                    line.push('"');
                }
                line.push(')');
            }
            Inline::Break if single => lines.last_mut().expect("line").push(' '),
            Inline::Break => lines.push(String::new()),
        }
    }
}

/// Backslash-escapes the inline metacharacters of `text`.
fn escape_inline(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(
            c,
            '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '&' | '|' | '~' | '$'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Whether `line` ends with `c` not escaped by a backslash (an odd run of
/// backslashes before it escapes it).
fn ends_with_unescaped(line: &str, c: char) -> bool {
    let Some(rest) = line.strip_suffix(c) else {
        return false;
    };
    let backslashes = rest.len() - rest.trim_end_matches('\\').len();
    backslashes % 2 == 0
}

/// A whole line of plain text: inline escapes plus line-start escapes.
fn escape_line(text: &str) -> String {
    escape_line_start(escape_inline(text).trim())
}

/// Escapes a block marker at the start of an already inline-escaped line:
/// `#`, `>`, `-`, `+`, `=`, `:` and ordered list markers (`1.`, `1)`).
fn escape_line_start(line: &str) -> String {
    let first = line.chars().next();
    if matches!(first, Some('#' | '>' | '-' | '+' | '=' | ':')) {
        return format!("\\{line}");
    }
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    if digits > 0 && matches!(line.as_bytes().get(digits), Some(b'.' | b')')) {
        return format!("{}\\{}", &line[..digits], &line[digits..]);
    }
    line.to_owned()
}

/// Escapes a trailing `#` run, which would otherwise close an ATX heading.
fn heading_text(text: &str) -> String {
    let trimmed = text.trim_end_matches('#');
    if trimmed.len() == text.len() || trimmed.ends_with('\\') {
        return text.to_owned();
    }
    let run = &text[trimmed.len()..];
    format!(
        "{trimmed}{}",
        run.chars().map(|_| "\\#").collect::<String>()
    )
}

/// A code span fenced with one more backtick than the longest run inside,
/// padded when the code starts or ends with a backtick or space.
fn code_span(code: &str) -> String {
    let code = code.replace(['\n', '\r'], " ");
    if code.is_empty() {
        return String::new();
    }
    let mut longest = 0;
    let mut run = 0;
    for c in code.chars() {
        run = if c == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    let fence = "`".repeat(longest + 1);
    let pad = code.starts_with(['`', ' ']) || code.ends_with(['`', ' ']);
    let pad = if pad { " " } else { "" };
    format!("{fence}{pad}{code}{pad}{fence}")
}

/// A link destination with spaces, angle brackets, parentheses, quotes,
/// backslashes and control characters percent-encoded.
fn link_destination(url: &str) -> String {
    let mut out = String::with_capacity(url.len());
    for c in url.chars() {
        if c.is_control() || c.is_whitespace() || matches!(c, '<' | '>' | '(' | ')' | '"' | '\\') {
            let mut buf = [0; 4];
            for b in c.encode_utf8(&mut buf).bytes() {
                out.push_str(&format!("%{b:02X}"));
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// A link title for a `"..."` title.
fn link_title(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    for c in single_line(title).chars() {
        if matches!(c, '"' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Prefixes every line with `> ` (`>` for blank lines).
fn quote(text: &str) -> String {
    text.lines()
        .map(|line| {
            if line.is_empty() {
                ">".to_owned()
            } else {
                format!("> {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::ast::parse_blocks;

    #[test]
    fn accents() {
        assert_eq!(parse_accent("#abc"), Ok([0xaa, 0xbb, 0xcc]));
        assert_eq!(parse_accent("#A1B2C3"), Ok([0xa1, 0xb2, 0xc3]));
        for bad in [
            "red",
            "abc",
            "#abcd",
            "#ggg",
            "#fff;}</style>",
            "#+1+2+3",
            "",
        ] {
            assert_eq!(
                parse_accent(bad),
                Err(BrandingError::Accent(bad.into())),
                "{bad}"
            );
        }
        let branding = Branding {
            accent: Some("#C0F".into()),
            logo: None,
        };
        assert_eq!(
            branding.check().unwrap().accent_hex().as_deref(),
            Some("#cc00ff")
        );
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(width, height, image::Rgba([10, 20, 30, 255]));
        let mut out = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
            .unwrap();
        out
    }

    fn check(bytes: &[u8]) -> Result<Logo, BrandingError> {
        let branding = Branding {
            logo: Some(bytes.to_vec()),
            accent: None,
        };
        branding.check().map(|b| b.logo.unwrap())
    }

    fn jpeg(width: u32, height: u32) -> Vec<u8> {
        let mut jpeg = Vec::new();
        image::RgbImage::from_pixel(width, height, image::Rgb([200, 0, 0]))
            .write_to(&mut Cursor::new(&mut jpeg), ImageFormat::Jpeg)
            .unwrap();
        jpeg
    }

    /// `jpeg` with an EXIF APP1 segment right after SOI, holding only an
    /// orientation tag.
    fn with_exif_orientation(jpeg: &[u8], orientation: u8) -> Vec<u8> {
        let mut app1 = vec![0xff, 0xe1, 0x00, 34];
        app1.extend_from_slice(b"Exif\0\0MM\0\x2a\0\0\0\x08");
        app1.extend_from_slice(&[0, 1, 0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, orientation, 0, 0]);
        app1.extend_from_slice(&[0, 0, 0, 0]);
        let mut out = jpeg[..2].to_vec();
        out.extend_from_slice(&app1);
        out.extend_from_slice(&jpeg[2..]);
        out
    }

    #[test]
    fn logos_become_png() {
        let logo = check(&png(40, 20)).unwrap();
        assert!(logo.png().starts_with(b"\x89PNG"));
        assert_eq!((logo.width(), logo.height()), (40, 20));
        let decoded = image::load_from_memory(logo.png()).unwrap();
        assert_eq!(decoded.to_rgba8().get_pixel(3, 3).0, [10, 20, 30, 255]);

        let logo = check(&jpeg(8, 4)).unwrap();
        assert!(logo.png().starts_with(b"\x89PNG"));
        assert_eq!((logo.width(), logo.height()), (8, 4));
    }

    #[test]
    fn exif_orientation_is_applied_and_metadata_dropped() {
        // 6: rotate 90° clockwise to display.
        let rotated = with_exif_orientation(&jpeg(8, 4), 6);
        assert!(rotated.windows(4).any(|w| w == b"Exif"));
        let logo = check(&rotated).unwrap();
        assert_eq!((logo.width(), logo.height()), (4, 8));
        let decoded = image::load_from_memory(logo.png()).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (4, 8));
        assert!(!logo.png().windows(4).any(|w| w == b"Exif" || w == b"eXIf"));
    }

    #[test]
    fn bad_logos_are_refused() {
        assert_eq!(
            check(b"\xef\xbb\xbf  <svg xmlns='http://www.w3.org/2000/svg'/>"),
            Err(BrandingError::SvgLogo)
        );
        assert_eq!(
            check(b"<?xml version='1.0'?><svg/>"),
            Err(BrandingError::SvgLogo)
        );
        assert_eq!(check(b"GIF89a"), Err(BrandingError::LogoFormat));
        assert_eq!(check(b""), Err(BrandingError::LogoFormat));
        assert_eq!(
            check(&png(2001, 10)),
            Err(BrandingError::LogoDimensions {
                width: 2001,
                height: 10
            })
        );
        assert!(check(&png(2000, 2000)).is_ok());
        let mut big = png(4, 4);
        big.resize(MAX_LOGO_BYTES + 1, 0);
        assert_eq!(
            check(&big),
            Err(BrandingError::LogoTooLarge(MAX_LOGO_BYTES + 1))
        );
        let mut truncated = png(40, 20);
        truncated.truncate(40);
        assert!(matches!(
            check(&truncated),
            Err(BrandingError::LogoUnreadable(_))
        ));
    }

    #[test]
    fn steps_of_annotation_guides_are_pages() {
        let en = Labels::for_language("en-GB");
        assert_eq!(en.step_kind(GuideKind::Walkthrough), "Step");
        assert_eq!(en.step_kind(GuideKind::Annotated), "Page");
        let de = Labels::for_language("de");
        assert_eq!(de.step_kind(GuideKind::Walkthrough), "Schritt");
        assert_eq!(de.step_kind(GuideKind::Annotated), "Seite");
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("Click **Save** in Finder"), "click-save-in-finder");
        assert_eq!(
            slug("Papier ins Fach legen – Größe"),
            "papier-ins-fach-legen-groesse"
        );
        assert_eq!(slug("…"), "step");
        assert_eq!(slug("日本 x"), "x");
        assert!(slug(&"word ".repeat(30)).len() <= 48);
        assert!(!slug(&"word ".repeat(30)).ends_with('-'));
    }

    #[test]
    fn export_image_names() {
        assert!(is_export_image_name("01-click-save.png"));
        assert!(is_export_image_name("100-step.png"));
        assert!(!is_export_image_name("1-a.png"));
        assert!(!is_export_image_name("01-.png"));
        assert!(!is_export_image_name("01-Mine.png"));
        assert!(!is_export_image_name("logo.png"));
    }

    #[test]
    fn escapes_line_starts() {
        assert_eq!(escape_line("# no heading"), "\\# no heading");
        assert_eq!(escape_line("1. no list"), "1\\. no list");
        assert_eq!(escape_line("12) no list"), "12\\) no list");
        assert_eq!(escape_line("- no bullet"), "\\- no bullet");
        assert_eq!(escape_line("> no quote"), "\\> no quote");
        assert_eq!(escape_line("a <b> & *c*"), "a \\<b\\> \\& \\*c\\*");
        assert_eq!(heading_text("Press #"), "Press \\#");
        assert_eq!(escape_line("costs $5"), "costs \\$5");
        assert_eq!(heading_text("C\\#"), "C\\#");
    }

    #[test]
    fn code_spans() {
        assert_eq!(code_span("ls"), "`ls`");
        assert_eq!(code_span("a`b"), "``a`b``");
        assert_eq!(code_span("`x"), "`` `x ``");
        assert_eq!(code_span(""), "");
    }

    #[test]
    fn lists_and_paragraphs() {
        let md = blocks(&parse_blocks(
            "Before:\n\n1. One\n2. Two\n   - nested\n\nLine a\\\nline b",
        ));
        assert_eq!(
            md,
            "Before:\n\n1. One\n2. Two\n   - nested\n\nLine a\\\nline b"
        );
        assert_eq!(
            parse_blocks(&md),
            parse_blocks("Before:\n\n1. One\n2. Two\n   - nested\n\nLine a\\\nline b")
        );
    }
}
