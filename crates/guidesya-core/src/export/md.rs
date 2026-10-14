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
//! Description
//!
//! > **Tip:** note body
//! ```
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
use std::io;
use std::path::{Path, PathBuf};

use super::ast::{plain_text, Block, Inline};
use super::{render_step_image, ExportReport, ImageOptions, RenderGuide, RenderItem};
use crate::library::{write_atomic, Durability, GuideHandle, LibraryError};
use crate::model::NoteKind;

/// File name of the Markdown document inside the output folder.
pub const GUIDE_FILE: &str = "guide.md";
/// Folder (inside the output folder) the step images are written to.
pub const IMAGES_DIR: &str = "images";

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
    /// Invalid HTML branding or a template failure.
    #[error("html: {0}")]
    Html(String),
}

pub type Result<T, E = ExportError> = std::result::Result<T, E>;

/// Fixed words a writer adds around the guide text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Labels {
    pub prerequisites: String,
    pub step: String,
    pub tip: String,
    pub warning: String,
    pub note: String,
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
        let [prerequisites, step, tip, warning, note] = match primary.as_str() {
            "de" => ["Voraussetzungen", "Schritt", "Tipp", "Warnung", "Hinweis"],
            _ => ["Prerequisites", "Step", "Tip", "Warning", "Note"],
        };
        Self {
            prerequisites: prerequisites.into(),
            step: step.into(),
            tip: tip.into(),
            warning: warning.into(),
            note: note.into(),
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

/// Writes `<out_dir>/guide.md` and `<out_dir>/images/NN-<slug>.png`, with
/// step images read from the bundle at `handle` (normally `guide.handle`).
///
/// Refuses a non-empty `out_dir` unless `opts.overwrite`; with it, earlier
/// `NN-*.png` images this export did not write are removed, and nothing
/// else is touched. A broken step image becomes a placeholder plus a
/// warning in the report.
pub fn export(
    guide: &RenderGuide,
    handle: &GuideHandle,
    out_dir: impl AsRef<Path>,
    opts: &MdOptions,
) -> Result<ExportReport> {
    let out_dir = out_dir.as_ref();
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

    let markdown = render(guide, &names, &labels);
    let doc = out_dir.join(GUIDE_FILE);
    write_atomic(&doc, markdown.as_bytes(), Durability::RenameOnly)?;
    report.files.push(doc);
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
    let mut out = Vec::new();
    let title = escape_line(&single_line(&guide.title));
    if !title.is_empty() {
        push_block(&mut out, &format!("# {}", heading_text(&title)));
    }
    let summary = single_line(&guide.summary);
    if !summary.is_empty() {
        push_block(&mut out, &escape_line(&summary));
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
                    escape_line(&labels.step)
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
