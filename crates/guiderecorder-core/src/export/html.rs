//! Self-contained HTML writer over [`RenderGuide`] (minijinja).
//!
//! The output is one file with no external requests: step images are
//! embedded as PNG data URIs, the stylesheet is inline, there is no script,
//! and a Content-Security-Policy meta tag ([`CSP`]) blocks anything else
//! (only `data:` images and the inline style may load). Link hrefs are the
//! only URLs; they are followed only when clicked.
//!
//! Decisions are boxes with the question and an option per line, whose
//! "Go to Step N" links to that step's `#step-N` article; a section tagged
//! with audiences has a "For: Admin" line under its heading.
//!
//! A collection ([`crate::collection`]) adds a `nav.contents` list linking
//! to its chapters (`#chapter-N`), which are H2s with everything in them one
//! level deeper; in print, each chapter (or the text leading into it) starts
//! a page. Step ids use [`RenderStep::anchor`](super::RenderStep::anchor),
//! unique even when numbers restart per chapter. Its extra styles are added
//! only to collections, so a guide's page is unchanged.
//!
//! That is the default [`HtmlMode::Document`]. [`HtmlMode::Walkthrough`]
//! writes an interactive click-through of the same guide instead, with one
//! inline script allowed by its hash; see [`walkthrough`](super::walkthrough).
//!
//! # Escaping
//!
//! The template is rendered with HTML auto-escaping on, so every plain
//! value (title, summary, section titles, labels, alt text, hotspot titles)
//! is escaped.
//! The guide's Markdown [`ast`](super::ast) is rendered to HTML here, with
//! every `Text`, `Code`, URL and title escaped, and passed to the template
//! as a safe string. The only other safe strings are the stylesheet, the
//! data URIs (base64 this module produced), hotspot descriptions (escaped
//! here, line by line, joined with `<br>`) and hotspot `style` attributes
//! (percentages formatted here, and a colour the renderer validated as a
//! hex colour or CSS colour name).
//!
//! # Hotspots
//!
//! Step images are rendered without hotspot markers
//! ([`ImageOptions::hotspot_markers`] is forced off). Instead each hotspot
//! in view is an absolutely positioned element over the image, placed in
//! percentages of it ([`RenderedImage::hotspots`](super::RenderedImage)),
//! with an outline (dashed while untitled, as in the editor) and a number
//! badge inside its top-left corner, drawn by CSS in the editor's hotspot
//! blue unless the annotation has its own colour. Its popover (title and
//! description) shows on `:hover` and on `:focus` / `:focus-within`; the
//! element has `tabindex="0"`, so a click or tap focuses it (Safari focuses
//! such elements on click, though not `<button>`s), and so does the
//! keyboard. All of it is CSS, so the page still needs no script. The
//! popover opens below the hotspot, or above it when the hotspot's centre
//! is in the lower half of the image, and is aligned to the hotspot's left
//! edge, centred on it, or aligned to its right edge by which third of the
//! image the centre is in, so it stays on the page; these classes are
//! chosen here. A numbered legend under the image lists every hotspot
//! (including any outside the view) for print, touch and screen readers;
//! popovers are hidden in print.
//!
//! # Cover
//!
//! When the author chose a cover ([`RenderGuide::cover`]), both page kinds
//! carry it as an `og:image` meta tag: a PNG data URI at most
//! [`OG_IMAGE_WIDTH`] wide, so the file still makes no requests. Without
//! one there is no tag.
//!
//! # Branding
//!
//! [`CheckedBranding`] (a logo and an accent colour, shared by every writer)
//! is off by default. The logo is the checked, re-encoded PNG, embedded as a
//! data URI above the title. The accent was parsed to RGB and is written
//! back as `#rrggbb`, so it can't break out of the stylesheet.

use std::fmt::Write as _;
use std::path::Path;

use minijinja::{context, AutoEscape, Environment, Value};

use super::ast::{plain_text, Block, Inline};
use super::md::{hex, prepare_out_dir, ExportError, Labels, Result};
pub use super::md::{Branding, CheckedBranding};
use super::preset::{Font, FontKind, Layout};
use super::{
    render_step_image, ExportReport, ImageOptions, PlacedHotspot, RenderDecision, RenderGuide,
    RenderItem, RenderTarget,
};
use crate::library::{write_atomic, Durability, GuideHandle};
use crate::model::{GuideKind, NoteKind, Rect};

/// File name of the HTML document inside the output folder.
pub const GUIDE_FILE: &str = "guide.html";

/// The Content-Security-Policy of every exported page.
pub const CSP: &str = "default-src 'none'; img-src data:; style-src 'unsafe-inline'; \
                       base-uri 'none'; form-action 'none'";

/// Widest `og:image` cover, in pixels: what link previews show.
pub const OG_IMAGE_WIDTH: u32 = 1200;

const TEMPLATE: &str = include_str!("templates/guide.html.j2");
const STYLE: &str = include_str!("templates/style.css");

/// What kind of page [`export`] writes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum HtmlMode {
    /// Every step on one page, with no script.
    #[default]
    Document,
    /// One step at a time with a pulsing click target, a progress bar and a
    /// step list, driven by a small inline script; see
    /// [`walkthrough`](super::walkthrough).
    Walkthrough,
}

/// Options for [`export`] and [`render_string`].
#[derive(Debug, Clone, PartialEq)]
pub struct HtmlOptions {
    pub mode: HtmlMode,
    /// How step images are rendered. The default caps the width at 1600 px
    /// (about 2× the content column), which keeps Retina sharpness and the
    /// file size reasonable. `hotspot_markers` is ignored: the page draws
    /// its own, see the module docs.
    pub image: ImageOptions,
    /// Write into a non-empty `out_dir`, replacing `guide.html`.
    pub overwrite: bool,
    /// Fixed labels; `None` picks them from the guide language.
    pub labels: Option<Labels>,
    pub branding: CheckedBranding,
    /// Show the version, owner and last review day under the summary (see
    /// [`Labels::review_details`]); off by default.
    pub review_details: bool,
}

impl Default for HtmlOptions {
    fn default() -> Self {
        Self {
            mode: HtmlMode::default(),
            image: ImageOptions {
                max_width: Some(1600),
                ..ImageOptions::default()
            },
            overwrite: false,
            labels: None,
            branding: CheckedBranding::default(),
            review_details: false,
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
    prepare_out_dir(out_dir, opts.overwrite)?;
    let (html, mut report) = render_string(guide, handle, opts)?;
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
    if opts.mode == HtmlMode::Walkthrough {
        return super::walkthrough::render_string(guide, handle, opts);
    }
    let branding = &opts.branding;
    let layout = branding.layout();
    let labels = opts
        .labels
        .clone()
        .unwrap_or_else(|| Labels::for_language(&guide.language));
    let step_label = labels.step_kind(guide.kind).to_owned();
    let image_opts = ImageOptions {
        hotspot_markers: false,
        ..opts.image.clone()
    };
    let mut report = ExportReport::default();
    let mut items = Vec::with_capacity(guide.items.len());
    for (item, page_break) in guide.items.iter().zip(guide.page_breaks()) {
        items.push(match item {
            RenderItem::Chapter(chapter) => context! {
                kind => "chapter",
                page_break => page_break,
                number => chapter.number,
                title => &chapter.title,
                summary => chapter.summary.trim(),
                intro => Value::from_safe_string(blocks_html(&chapter.intro)),
                prerequisites => Value::from_safe_string(blocks_html(&chapter.prerequisites)),
            },
            RenderItem::Text(text) => context! {
                kind => "text",
                page_break => page_break,
                body => Value::from_safe_string(blocks_html(&text.body)),
            },
            RenderItem::Section(section) => context! {
                kind => "section",
                title => &section.title,
                audiences => labels.audiences(&section.audiences),
            },
            RenderItem::Decision(decision) => {
                decision_context(decision, &labels, guide.kind, |target| match target {
                    RenderTarget::Step {
                        anchor: Some(anchor),
                        ..
                    } => Some(format!("#step-{anchor}")),
                    _ => None,
                })
            }
            RenderItem::Note(note) => context! {
                kind => "note",
                note_kind => note_kind_class(note.kind),
                label => labels.note_kind(note.kind),
                body => Value::from_safe_string(blocks_html(&note.body)),
            },
            RenderItem::Step(step) => {
                let title = inlines_html(&step.title);
                let title = if title.is_empty() {
                    escape(&step_label)
                } else {
                    title
                };
                // A checklist has no images, so they are not rendered.
                let (image, hotspots) = if layout.has_images() {
                    let image = render_step_image(&step.handle, &step.step, &image_opts);
                    report.add_warnings(step.step.id, image.warnings.iter().cloned());
                    let hotspots: Vec<Value> = image
                        .hotspots
                        .iter()
                        .map(|h| hotspot_context(step.anchor, h))
                        .collect();
                    let image = context! {
                        src => Value::from_safe_string(data_uri("image/png", &image.png)),
                        width => image.width,
                        height => image.height,
                        alt => plain_text(&step.title).replace('\n', " "),
                    };
                    (Some(image), hotspots)
                } else {
                    (None, Vec::new())
                };
                context! {
                    kind => "step",
                    number => step.number,
                    anchor => step.anchor,
                    title => Value::from_safe_string(title),
                    description => Value::from_safe_string(blocks_html(&step.description)),
                    image => image,
                    hotspots => hotspots,
                }
            }
        });
    }

    let logo = branding
        .logo()
        .map(|logo| Value::from_safe_string(data_uri("image/png", logo.png())));
    // Steps sit under section headings when there are any, and sections
    // under a collection's chapters.
    let depth = u8::from(guide.has_chapters());
    let contents: Vec<Value> = guide
        .chapters()
        .map(|chapter| context! { number => chapter.number, title => &chapter.title })
        .collect();
    let ctx = context! {
        step_level => if guide.has_sections() { 3 } else { 2 } + depth,
        section_level => 2 + depth,
        contents => contents,
        language => language_tag(&guide.language),
        csp => CSP,
        title => &guide.title,
        og_image => og_image(guide, handle, &opts.image),
        summary => &guide.summary,
        details => opts.review_details.then(|| labels.review_details(&guide.review)).flatten(),
        cover => cover_line(branding, guide, &labels),
        page_header => branding.header(),
        page_footer => branding.footer(),
        layout => layout_class(layout),
        checklist => layout == Layout::Checklist,
        css => Value::from_safe_string(document_stylesheet(branding, guide.has_chapters())),
        logo => logo,
        labels => context! {
            prerequisites => labels.prerequisites,
            step => step_label,
            contents => labels.contents,
        },
        intro => Value::from_safe_string(blocks_html(&guide.intro)),
        prerequisites => Value::from_safe_string(blocks_html(&guide.prerequisites)),
        items => items,
    };

    let env = environment("guide.html", TEMPLATE)?;
    Ok((render_template(&env, "guide.html", ctx)?, report))
}

/// A decision for the templates: `kind` `decision`, its label, the question
/// as safe HTML, and per option its label, where it leads (plain text) and
/// `href`, the in-page link `href` gives for the target, if any.
pub(super) fn decision_context(
    decision: &RenderDecision,
    labels: &Labels,
    kind: GuideKind,
    href: impl Fn(&RenderTarget) -> Option<String>,
) -> Value {
    let options: Vec<Value> = decision
        .options
        .iter()
        .map(|option| {
            context! {
                label => &option.label,
                text => labels.go_to(&option.target, kind),
                href => href(&option.target),
            }
        })
        .collect();
    context! {
        kind => "decision",
        label => &labels.decision,
        question => Value::from_safe_string(inlines_html(&decision.question)),
        options => options,
    }
}

/// The chosen cover ([`RenderGuide::cover`]) as a PNG data URI for the
/// `og:image` meta tag, at most [`OG_IMAGE_WIDTH`] wide; `None` without one.
/// Its warnings are left out: the step reports its own.
pub(super) fn og_image(
    guide: &RenderGuide,
    handle: &GuideHandle,
    image: &ImageOptions,
) -> Option<Value> {
    let cover = guide.cover.as_ref()?;
    let max_width = image
        .max_width
        .map_or(OG_IMAGE_WIDTH, |max| max.min(OG_IMAGE_WIDTH));
    let opts = ImageOptions {
        max_width: Some(max_width),
        ..image.clone()
    };
    let png = cover.image(handle, &opts).png;
    Some(Value::from_safe_string(data_uri("image/png", &png)))
}

/// A template environment with HTML auto-escaping and `template` added as
/// `name`.
pub(super) fn environment(
    name: &'static str,
    template: &'static str,
) -> Result<Environment<'static>> {
    let mut env = Environment::new();
    env.set_auto_escape_callback(|_| AutoEscape::Html);
    env.add_template(name, template)
        .map_err(|e| ExportError::Html(e.to_string()))?;
    Ok(env)
}

/// Renders the template `name` of `env` with `ctx`, ending in a newline.
pub(super) fn render_template(env: &Environment, name: &str, ctx: Value) -> Result<String> {
    let mut html = env
        .get_template(name)
        .and_then(|t| t.render(ctx))
        .map_err(|e| ExportError::Html(e.to_string()))?;
    // minijinja drops the template's final newline.
    html.push('\n');
    Ok(html)
}

/// Template values of one hotspot of step `step`. `style` (position and
/// colour) and `class` (popover side) are unset for a hotspot outside the
/// view, which then only shows in the legend.
pub(super) fn hotspot_context(step: u32, hotspot: &PlacedHotspot) -> Value {
    let title = hotspot.title.trim();
    let label = if title.is_empty() {
        hotspot.number.to_string()
    } else {
        format!("{}. {title}", hotspot.number)
    };
    let description = hotspot
        .description
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(escape)
        .collect::<Vec<_>>()
        .join("<br>");
    let color = hotspot
        .color
        .as_deref()
        .map(|c| format!("--hs:{c}"))
        .unwrap_or_default();
    let (style, class) = match hotspot.rect {
        Some(r) => {
            let mut style = format!(
                "left:{};top:{};width:{};height:{}",
                percent(r.x),
                percent(r.y),
                percent(r.width),
                percent(r.height)
            );
            if !color.is_empty() {
                style.push(';');
                style.push_str(&color);
            }
            let mut class = popover_class(r).to_owned();
            if title.is_empty() {
                class.push_str(" hotspot-untitled");
            }
            (Some(style), Some(class))
        }
        None => (None, None),
    };
    context! {
        id => format!("hotspot-{step}-{}", hotspot.number),
        number => hotspot.number,
        title => title,
        label => label,
        description => Value::from_safe_string(description),
        style => style.map(Value::from_safe_string),
        class => class,
        legend_style => (!color.is_empty()).then(|| Value::from_safe_string(color)),
    }
}

/// Where a hotspot's popover opens, from its centre: below it, or above it
/// in the lower half of the image; from its left edge, centred on it, or
/// towards the left from its right edge, by thirds of the width.
fn popover_class(r: Rect) -> &'static str {
    let (cx, cy) = (r.x + r.width / 2.0, r.y + r.height / 2.0);
    let horizontal = if cx < 1.0 / 3.0 {
        0
    } else if cx <= 2.0 / 3.0 {
        1
    } else {
        2
    };
    let above = cy > 0.5;
    [
        [
            "hotspot-below hotspot-start",
            "hotspot-below hotspot-center",
            "hotspot-below hotspot-end",
        ],
        [
            "hotspot-above hotspot-start",
            "hotspot-above hotspot-center",
            "hotspot-above hotspot-end",
        ],
    ][usize::from(above)][horizontal]
}

/// A fraction as a CSS percentage, rounded to 1/1000 %, e.g. `12.5%`.
pub(super) fn percent(fraction: f64) -> String {
    let value = (fraction.clamp(0.0, 1.0) * 100_000.0).round() / 1000.0;
    format!("{value}%")
}

/// The default stylesheet plus the branding's accent, colours and fonts,
/// if any (shared with the walkthrough). Colours are the checked RGB
/// values, written as `#rrggbb`; font families passed
/// [`is_font_family`](super::preset::is_font_family) and are quoted. The text
/// and heading colours apply in the light appearance only, where they were
/// chosen to be read.
pub(super) fn stylesheet(branding: &CheckedBranding) -> String {
    let mut css = STYLE.to_owned();
    if let Some(accent) = branding.accent_hex() {
        let _ = writeln!(css, ":root {{ --accent: {accent}; }}");
    }
    let fonts = branding.fonts();
    if fonts.body != Font::default() {
        let _ = writeln!(css, "body {{ font-family: {}; }}", font_stack(&fonts.body));
    }
    if fonts.heading != Font::default() {
        let _ = writeln!(
            css,
            "h1, h2, h3, .step-title {{ font-family: {}; }}",
            font_stack(&fonts.heading)
        );
    }
    let text = branding.text_color().map(hex);
    let heading = branding.heading_color().map(hex);
    if text.is_some() || heading.is_some() {
        css.push_str("@media (prefers-color-scheme: light) {\n");
        if let Some(text) = text {
            let _ = writeln!(css, "  :root {{ --text: {text}; }}");
        }
        if let Some(heading) = heading {
            let _ = writeln!(css, "  h1, h2, h3 {{ color: {heading}; }}");
        }
        css.push_str("}\n");
    }
    css
}

/// [`stylesheet`] plus what only the document page has: its header and
/// footer lines, cover line, layout, and last the custom CSS.
fn document_stylesheet(branding: &CheckedBranding, chapters: bool) -> String {
    let mut css = stylesheet(branding);
    if chapters {
        css.push_str(
            ".contents { margin-bottom: 2rem; }\n\
             .contents ol { padding-left: 1.75rem; }\n\
             .contents li { margin: .2rem 0; }\n\
             .chapter { margin-top: 3.5rem; padding-bottom: .5rem; border-bottom: 1px solid var(--border); }\n\
             h3 { font-size: 1.2rem; line-height: 1.3; margin: 2rem 0 .75rem; overflow-wrap: anywhere; }\n\
             .chapter-summary { color: var(--muted); }\n\
             .text { margin: 1.5rem 0; }\n\
             @media print { .page-break { break-before: page; page-break-before: always; } }\n",
        );
    }
    if branding.header().is_some() || branding.footer().is_some() {
        css.push_str(
            ".page-header, .page-footer { color: var(--muted); font-size: .8rem; margin: 0; }\n\
             .page-header { margin-bottom: 1.5rem; }\n\
             .page-footer { margin-top: 2.5rem; border-top: 1px solid var(--border); padding-top: .75rem; }\n",
        );
    }
    if branding.cover().is_some() {
        css.push_str(
            ".cover-fields { color: var(--muted); font-size: .875rem; margin: .5rem 0 0; }\n",
        );
    }
    css.push_str(match branding.layout() {
        Layout::ImageThenText => "",
        Layout::SideBySide => {
            "@media (min-width: 40rem) {\n\
             \x20 .layout-side-by-side .step { display: grid; grid-template-columns: minmax(0, 3fr) minmax(0, 2fr); column-gap: 1.25rem; align-items: start; }\n\
             \x20 .layout-side-by-side .step-title { grid-column: 1 / -1; }\n\
             \x20 .layout-side-by-side .step-image { grid-column: 1; grid-row: 2; margin-bottom: 0; }\n\
             \x20 .layout-side-by-side .hotspot-legend { grid-column: 1; grid-row: 3; }\n\
             \x20 .layout-side-by-side .step-description { grid-column: 2; grid-row: 2 / span 2; }\n\
             }\n"
        }
        Layout::Compact => {
            ".layout-compact .step { padding: .9rem 1rem; margin-bottom: .75rem; }\n\
             .layout-compact .step-title { margin-bottom: .6rem; }\n\
             .layout-compact .step-image { max-width: 50%; }\n"
        }
        Layout::Checklist => {
            ".layout-checklist .step { padding: .75rem 1rem; margin-bottom: .5rem; }\n\
             .layout-checklist .step-title { align-items: center; margin-bottom: .25rem; }\n\
             .layout-checklist .step-check { flex: none; width: 1.25rem; height: 1.25rem; margin: 0; accent-color: var(--accent); }\n\
             .layout-checklist .step-description { padding-left: 2rem; }\n"
        }
    });
    if let Some(custom) = branding.css() {
        css.push_str("/* Custom CSS */\n");
        css.push_str(custom);
        if !custom.ends_with('\n') {
            css.push('\n');
        }
    }
    css
}

/// A CSS font list: the family (quoted) if any, then the kind's stack.
fn font_stack(font: &Font) -> String {
    let stack = match font.kind {
        FontKind::Sans => {
            r#"-apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif"#
        }
        FontKind::Serif => r#"Georgia, Cambria, "Times New Roman", Times, serif"#,
        FontKind::Mono => "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
    };
    match font.family() {
        Some(family) => format!("\"{family}\", {stack}"),
        None => stack.to_owned(),
    }
}

/// The class `<main>` gets for a layout; none for the default.
fn layout_class(layout: Layout) -> Option<String> {
    (layout != Layout::ImageThenText).then(|| format!("layout-{}", layout.name()))
}

/// The cover fields as one line, `Acme · Jo Bloggs · Version 2 · 2026-10-22`
/// (the day the guide was last changed); `None` without any.
pub(super) fn cover_line(
    branding: &CheckedBranding,
    guide: &RenderGuide,
    labels: &Labels,
) -> Option<String> {
    let date = guide.updated_at.format("%Y-%m-%d").to_string();
    let parts = branding.cover_parts(&labels.version, Some(&date));
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// The language tag if it looks like BCP 47 (letters, digits, `-`), else
/// empty.
pub(super) fn language_tag(language: &str) -> &str {
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

pub(super) fn note_kind_class(kind: NoteKind) -> &'static str {
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
pub fn data_uri(mime: &str, bytes: &[u8]) -> String {
    let mut out = format!("data:{mime};base64,");
    base64_into(bytes, &mut out);
    out
}

/// Standard base64 with padding.
pub(super) fn base64_into(bytes: &[u8], out: &mut String) {
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
    fn language_tags() {
        assert_eq!(language_tag("de-DE"), "de-DE");
        assert_eq!(language_tag("en\" onload=\"x"), "");
    }

    fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn popovers_open_away_from_the_edges() {
        // By the centre: thirds across, halves down.
        assert_eq!(
            popover_class(rect(0.05, 0.1, 0.2, 0.2)),
            "hotspot-below hotspot-start"
        );
        assert_eq!(
            popover_class(rect(0.4, 0.1, 0.2, 0.2)),
            "hotspot-below hotspot-center"
        );
        assert_eq!(
            popover_class(rect(0.8, 0.2, 0.2, 0.2)),
            "hotspot-below hotspot-end"
        );
        assert_eq!(
            popover_class(rect(0.0, 0.6, 0.2, 0.2)),
            "hotspot-above hotspot-start"
        );
        assert_eq!(
            popover_class(rect(0.7, 0.45, 0.25, 0.5)),
            "hotspot-above hotspot-end"
        );
        // A hotspot over the whole image opens below, centred.
        assert_eq!(
            popover_class(rect(0.0, 0.0, 1.0, 1.0)),
            "hotspot-below hotspot-center"
        );
    }

    #[test]
    fn percentages_are_compact() {
        assert_eq!(percent(0.0), "0%");
        assert_eq!(percent(0.125), "12.5%");
        assert_eq!(percent(1.0 / 3.0), "33.333%");
        assert_eq!(percent(1.5), "100%");
    }

    #[test]
    fn hotspot_text_is_escaped_and_placed() {
        let hotspot = PlacedHotspot {
            number: 2,
            id: ulid::Ulid::nil(),
            title: " <b>\"Fjord\" & co</b> ".into(),
            description: "Line <1>\n\n  Line 'two' & more\n".into(),
            rect: Some(rect(0.5, 0.25, 0.25, 0.125)),
            color: Some("#16a34a".into()),
        };
        let value = hotspot_context(3, &hotspot);
        let get = |key: &str| value.get_attr(key).unwrap();
        assert_eq!(get("id").as_str(), Some("hotspot-3-2"));
        // Plain values are escaped by the template; the description is
        // escaped here.
        assert_eq!(get("title").as_str(), Some("<b>\"Fjord\" & co</b>"));
        assert_eq!(get("label").as_str(), Some("2. <b>\"Fjord\" & co</b>"));
        assert_eq!(
            get("description").to_string(),
            "Line &lt;1&gt;<br>Line &#39;two&#39; &amp; more"
        );
        assert!(get("description").is_safe());
        assert_eq!(
            get("style").to_string(),
            "left:50%;top:25%;width:25%;height:12.5%;--hs:#16a34a"
        );
        assert_eq!(get("class").as_str(), Some("hotspot-below hotspot-center"));
        assert_eq!(get("legend_style").to_string(), "--hs:#16a34a");

        // Out of view: only in the legend; no colour, no style.
        let hidden = PlacedHotspot {
            rect: None,
            color: None,
            title: String::new(),
            ..hotspot
        };
        let value = hotspot_context(3, &hidden);
        assert!(value.get_attr("style").unwrap().is_none());
        assert!(value.get_attr("class").unwrap().is_none());
        let untitled = PlacedHotspot {
            rect: Some(rect(0.0, 0.0, 0.1, 0.1)),
            ..hidden.clone()
        };
        assert_eq!(
            hotspot_context(3, &untitled)
                .get_attr("class")
                .unwrap()
                .as_str(),
            Some("hotspot-below hotspot-start hotspot-untitled")
        );
        assert!(value.get_attr("legend_style").unwrap().is_none());
        assert_eq!(value.get_attr("label").unwrap().as_str(), Some("2"));
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
