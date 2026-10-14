//! Self-contained HTML writer over [`RenderGuide`] (minijinja).
//!
//! The output is one file with no external requests: step images are
//! embedded as PNG data URIs, the stylesheet is inline, there is no script,
//! and a Content-Security-Policy meta tag ([`CSP`]) blocks anything else
//! (only `data:` images and the inline style may load). Link hrefs are the
//! only URLs; they are followed only when clicked.
//!
//! # Escaping
//!
//! The template is rendered with HTML auto-escaping on, so every plain
//! value (title, summary, section titles, labels, alt text) is escaped.
//! The guide's Markdown [`ast`](super::ast) is rendered to HTML here, with
//! every `Text`, `Code`, URL and title escaped, and passed to the template
//! as a safe string. The only other safe strings are the stylesheet, the
//! data URIs (base64 this module produced) and nothing else.
//!
//! # Branding
//!
//! [`Branding`] (a logo and an accent colour) is off by default. The accent
//! must be a hex colour, so it can't break out of the stylesheet.

use std::fmt::Write as _;
use std::path::Path;

use minijinja::{context, AutoEscape, Environment, Value};

use super::ast::{plain_text, Block, Inline};
use super::md::{prepare_out_dir, ExportError, Labels, Result};
use super::{render_step_image, ExportReport, ImageOptions, RenderGuide, RenderItem};
use crate::library::{write_atomic, Durability, GuideHandle};
use crate::model::NoteKind;

/// File name of the HTML document inside the output folder.
pub const GUIDE_FILE: &str = "guide.html";

/// The Content-Security-Policy of every exported page.
pub const CSP: &str = "default-src 'none'; img-src data:; style-src 'unsafe-inline'; \
                       base-uri 'none'; form-action 'none'";

const TEMPLATE: &str = include_str!("templates/guide.html.j2");
const STYLE: &str = include_str!("templates/style.css");

/// Options for [`export`] and [`render_string`].
#[derive(Debug, Clone, PartialEq)]
pub struct HtmlOptions {
    /// How step images are rendered. The default caps the width at 1600 px
    /// (about 2× the content column), which keeps Retina sharpness and the
    /// file size reasonable.
    pub image: ImageOptions,
    /// Write into a non-empty `out_dir`, replacing `guide.html`.
    pub overwrite: bool,
    /// Fixed labels; `None` picks them from the guide language.
    pub labels: Option<Labels>,
    pub branding: Branding,
}

impl Default for HtmlOptions {
    fn default() -> Self {
        Self {
            image: ImageOptions {
                max_width: Some(1600),
                ..ImageOptions::default()
            },
            overwrite: false,
            labels: None,
            branding: Branding::default(),
        }
    }
}

/// Optional branding; everything off by default.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Branding {
    /// Shown above the title.
    pub logo: Option<Logo>,
    /// Hex colour (`#rgb` or `#rrggbb`) for step numbers, links and the
    /// header rule.
    pub accent: Option<String>,
}

/// A logo image to embed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Logo {
    pub mime: LogoMime,
    pub bytes: Vec<u8>,
}

/// Raster formats a logo may have. SVG is left out on purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogoMime {
    Png,
    Jpeg,
    Webp,
}

impl LogoMime {
    fn as_str(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Webp => "image/webp",
        }
    }
}

/// Writes `<out_dir>/guide.html`, with step images read from the bundle at
/// `handle` (normally `guide.handle`). Refuses a non-empty `out_dir` unless
/// `opts.overwrite`.
pub fn export(
    guide: &RenderGuide,
    handle: &GuideHandle,
    out_dir: impl AsRef<Path>,
    opts: &HtmlOptions,
) -> Result<ExportReport> {
    let out_dir = out_dir.as_ref();
    // Validate branding before touching the file system.
    let css = stylesheet(&opts.branding)?;
    prepare_out_dir(out_dir, opts.overwrite)?;
    let (html, mut report) = render_with_css(guide, handle, opts, css)?;
    let path = out_dir.join(GUIDE_FILE);
    write_atomic(&path, html.as_bytes(), Durability::RenameOnly)?;
    report.files.push(path);
    Ok(report)
}

/// The HTML document as a string; the report has the image warnings and no
/// files.
pub fn render_string(
    guide: &RenderGuide,
    handle: &GuideHandle,
    opts: &HtmlOptions,
) -> Result<(String, ExportReport)> {
    let css = stylesheet(&opts.branding)?;
    render_with_css(guide, handle, opts, css)
}

fn render_with_css(
    guide: &RenderGuide,
    handle: &GuideHandle,
    opts: &HtmlOptions,
    css: String,
) -> Result<(String, ExportReport)> {
    let labels = opts
        .labels
        .clone()
        .unwrap_or_else(|| Labels::for_language(&guide.language));
    let mut report = ExportReport::default();
    let mut items = Vec::with_capacity(guide.items.len());
    for item in &guide.items {
        items.push(match item {
            RenderItem::Section(section) => context! {
                kind => "section",
                title => &section.title,
            },
            RenderItem::Note(note) => context! {
                kind => "note",
                note_kind => note_kind_class(note.kind),
                label => labels.note_kind(note.kind),
                body => Value::from_safe_string(blocks_html(&note.body)),
            },
            RenderItem::Step(step) => {
                let image = render_step_image(handle, &step.step, &opts.image);
                report.add_warnings(step.step.id, image.warnings.iter().cloned());
                let title = inlines_html(&step.title);
                let title = if title.is_empty() {
                    escape(&labels.step)
                } else {
                    title
                };
                context! {
                    kind => "step",
                    number => step.number,
                    title => Value::from_safe_string(title),
                    description => Value::from_safe_string(blocks_html(&step.description)),
                    image => context! {
                        src => Value::from_safe_string(data_uri("image/png", &image.png)),
                        width => image.width,
                        height => image.height,
                        alt => plain_text(&step.title).replace('\n', " "),
                    },
                }
            }
        });
    }

    let logo = opts
        .branding
        .logo
        .as_ref()
        .map(|logo| Value::from_safe_string(data_uri(logo.mime.as_str(), &logo.bytes)));
    // Steps sit under section headings when there are any.
    let has_sections = guide
        .items
        .iter()
        .any(|item| matches!(item, RenderItem::Section(_)));
    let ctx = context! {
        step_level => if has_sections { 3 } else { 2 },
        language => language_tag(&guide.language),
        csp => CSP,
        title => &guide.title,
        summary => &guide.summary,
        css => Value::from_safe_string(css),
        logo => logo,
        labels => context! {
            prerequisites => labels.prerequisites,
            step => labels.step,
        },
        intro => Value::from_safe_string(blocks_html(&guide.intro)),
        prerequisites => Value::from_safe_string(blocks_html(&guide.prerequisites)),
        items => items,
    };

    let mut env = Environment::new();
    env.set_auto_escape_callback(|_| AutoEscape::Html);
    env.add_template("guide.html", TEMPLATE)
        .map_err(|e| ExportError::Html(e.to_string()))?;
    let mut html = env
        .get_template("guide.html")
        .and_then(|t| t.render(ctx))
        .map_err(|e| ExportError::Html(e.to_string()))?;
    // minijinja drops the template's final newline.
    html.push('\n');
    Ok((html, report))
}

/// The default stylesheet plus the accent override, if any.
fn stylesheet(branding: &Branding) -> Result<String> {
    let mut css = STYLE.to_owned();
    if let Some(accent) = &branding.accent {
        if !is_hex_colour(accent) {
            return Err(ExportError::Html(format!(
                "accent {accent:?} is not a hex colour (#rgb or #rrggbb)"
            )));
        }
        let _ = writeln!(css, ":root {{ --accent: {accent}; }}");
    }
    Ok(css)
}

fn is_hex_colour(value: &str) -> bool {
    value
        .strip_prefix('#')
        .is_some_and(|hex| matches!(hex.len(), 3 | 6) && hex.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// The language tag if it looks like BCP 47 (letters, digits, `-`), else
/// empty.
fn language_tag(language: &str) -> &str {
    let ok = !language.is_empty()
        && language.len() <= 35
        && language
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-');
    if ok {
        language
    } else {
        ""
    }
}

fn note_kind_class(kind: NoteKind) -> &'static str {
    match kind {
        NoteKind::Tip => "tip",
        NoteKind::Warning => "warning",
        NoteKind::Note => "note",
    }
}

// ---------------------------------------------------------------------------
// AST → HTML

/// Escapes text for HTML element content and quoted attribute values.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// Blocks as HTML, one block per line.
pub fn blocks_html(blocks: &[Block]) -> String {
    let mut out = String::new();
    push_blocks(blocks, &mut out);
    out.truncate(out.trim_end().len());
    out
}

fn push_blocks(blocks: &[Block], out: &mut String) {
    for block in blocks {
        match block {
            Block::Paragraph(inlines) => {
                out.push_str("<p>");
                push_inlines(inlines, out);
                out.push_str("</p>\n");
            }
            Block::List { start, items } => {
                match start {
                    Some(1) => out.push_str("<ol>\n"),
                    Some(start) => {
                        let _ = writeln!(out, "<ol start=\"{start}\">");
                    }
                    None => out.push_str("<ul>\n"),
                }
                for item in items {
                    out.push_str("<li>");
                    // A single paragraph renders tight, without <p>.
                    match item.as_slice() {
                        [Block::Paragraph(inlines)] => push_inlines(inlines, out),
                        [Block::Paragraph(inlines), rest @ ..]
                            if rest.iter().all(|b| matches!(b, Block::List { .. })) =>
                        {
                            push_inlines(inlines, out);
                            out.push('\n');
                            push_blocks(rest, out);
                        }
                        _ => {
                            out.push('\n');
                            push_blocks(item, out);
                        }
                    }
                    out.push_str("</li>\n");
                }
                out.push_str(if start.is_some() {
                    "</ol>\n"
                } else {
                    "</ul>\n"
                });
            }
        }
    }
}

/// Inlines as HTML.
pub fn inlines_html(inlines: &[Inline]) -> String {
    let mut out = String::new();
    push_inlines(inlines, &mut out);
    out
}

fn push_inlines(inlines: &[Inline], out: &mut String) {
    for inline in inlines {
        match inline {
            Inline::Text(text) => out.push_str(&escape(text)),
            Inline::Code(code) => {
                out.push_str("<code>");
                out.push_str(&escape(code));
                out.push_str("</code>");
            }
            Inline::Strong(children) => {
                out.push_str("<strong>");
                push_inlines(children, out);
                out.push_str("</strong>");
            }
            Inline::Emphasis(children) => {
                out.push_str("<em>");
                push_inlines(children, out);
                out.push_str("</em>");
            }
            Inline::Link {
                url,
                title,
                children,
            } => {
                out.push_str("<a href=\"");
                out.push_str(&escape(url));
                out.push('"');
                if !title.is_empty() {
                    out.push_str(" title=\"");
                    out.push_str(&escape(title));
                    out.push('"');
                }
                out.push_str(" rel=\"noopener noreferrer\">");
                push_inlines(children, out);
                out.push_str("</a>");
            }
            Inline::Break => out.push_str("<br>\n"),
        }
    }
}

// ---------------------------------------------------------------------------
// Data URIs

/// `data:<mime>;base64,<bytes>`.
fn data_uri(mime: &str, bytes: &[u8]) -> String {
    let mut out = format!("data:{mime};base64,");
    base64_into(bytes, &mut out);
    out
}

/// Standard base64 with padding.
fn base64_into(bytes: &[u8], out: &mut String) {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    out.reserve(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::ast::parse_blocks;

    fn b64(bytes: &[u8]) -> String {
        let mut out = String::new();
        base64_into(bytes, &mut out);
        out
    }

    #[test]
    fn base64_matches_rfc4648_vectors() {
        assert_eq!(b64(b""), "");
        assert_eq!(b64(b"f"), "Zg==");
        assert_eq!(b64(b"fo"), "Zm8=");
        assert_eq!(b64(b"foo"), "Zm9v");
        assert_eq!(b64(b"foob"), "Zm9vYg==");
        assert_eq!(b64(b"fooba"), "Zm9vYmE=");
        assert_eq!(b64(b"foobar"), "Zm9vYmFy");
        assert_eq!(b64(&[0xff, 0xfe, 0xfd]), "//79");
    }

    #[test]
    fn hex_colours() {
        assert!(is_hex_colour("#abc"));
        assert!(is_hex_colour("#A1B2C3"));
        assert!(!is_hex_colour("red"));
        assert!(!is_hex_colour("#abcd"));
        assert!(!is_hex_colour("#fff;}</style><script>"));
    }

    #[test]
    fn language_tags() {
        assert_eq!(language_tag("de-DE"), "de-DE");
        assert_eq!(language_tag("en\" onload=\"x"), "");
    }

    #[test]
    fn renders_blocks() {
        let html = blocks_html(&parse_blocks(
            "Hi **you** <b>x</b> & [d](https://e.x/?a=1&b=\"2\" \"T'\")\n\n3. a\n4. b\n   - c",
        ));
        assert_eq!(
            html,
            "<p>Hi <strong>you</strong> &lt;b&gt;x&lt;/b&gt; &amp; \
             <a href=\"https://e.x/?a=1&amp;b=&quot;2&quot;\" title=\"T&#39;\" \
             rel=\"noopener noreferrer\">d</a></p>\n\
             <ol start=\"3\">\n<li>a</li>\n<li>b\n<ul>\n<li>c</li>\n</ul>\n</li>\n</ol>"
        );
    }
}
