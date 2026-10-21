//! Interactive walkthrough mode of the HTML writer
//! ([`HtmlMode::Walkthrough`](super::html::HtmlMode)).
//!
//! The same one self-contained `guide.html`, shown one step at a time: a
//! pulsing target sits on the step's click point, and clicking it, or
//! pressing → or Enter, moves on; ← goes back. A progress bar, a step list
//! (with the guide's sections as group labels), Back, Next and Restart
//! complete it. Images, blur, hotspots, escaping and branding are the
//! document mode's (see [`html`](super::html)); only the layout differs.
//!
//! # Slides
//!
//! Each step is a slide. The intro and prerequisites, if any, make a first
//! slide, and an end slide closes the walkthrough. Notes and section titles
//! show on the slide of the next step, in reading order: a step carries the
//! title of the section it is in, and notes before it show above its image.
//! Notes after the last step go on the end slide.
//!
//! # Click target
//!
//! The target is placed from [`RenderedImage::point`](super::RenderedImage)
//! (the stored click point, through the crop and scaling;
//! [`ImageOptions::locate_point`] is forced on). Guides store no element
//! frame, so the target is a fixed-size ring centred on the point. A step
//! without a point in view has no target; Next (and Enter) still move on.
//!
//! # Script and CSP
//!
//! The page has one inline script, [`SCRIPT`], which is static: all guide
//! data is in the markup, so the script never holds guide text. The CSP
//! ([`csp`]) allows exactly that script by its SHA-256 hash and nothing
//! else beyond the document mode's `data:` images and inline style.
//! Without script the page still works as a document: every slide shows,
//! and the targets and step list are plain in-page links.
//!
//! # Accessibility
//!
//! Every slide's status ("Step 2 of 5: …") is announced through a polite
//! live region. Moving on keeps focus on Back or Next when they were used;
//! otherwise (a target, the step list, Restart, or focus on a slide that
//! was hidden) focus moves to the new slide's heading. Back and Next are
//! `aria-disabled` at the ends rather than disabled, so they keep focus.
//! The current step-list entry has `aria-current="step"`. The pulse stops
//! under `prefers-reduced-motion`.
//!
//! # Embedding
//!
//! A comment at the top of the page holds an `<iframe>` snippet for
//! embedding it in a help centre or wiki.
//!
//! A SCORM package ([`scorm`](super::scorm)) adds a second inline script
//! (also allowed by its hash), some style, and markup on the end slide; see
//! [`Extra`].

use minijinja::{context, Value};
use sha2::{Digest, Sha256};

use super::ast::plain_text;
use super::html::{
    base64_into, blocks_html, data_uri, environment, escape, hotspot_context, inlines_html,
    language_tag, note_kind_class, og_image, percent, render_template, stylesheet, HtmlOptions,
};
use super::md::{Labels, Result};
use super::{render_step_image, ExportReport, ImageOptions, RenderGuide, RenderItem, RenderNote};
use crate::library::GuideHandle;
use crate::model::Point;

const TEMPLATE: &str = include_str!("templates/walkthrough.html.j2");
const STYLE: &str = include_str!("templates/walkthrough.css");

/// The page's only script, allowed by its hash in [`csp`].
pub const SCRIPT: &str = include_str!("templates/walkthrough.js");

/// The Content-Security-Policy of a walkthrough: the document mode's
/// [`CSP`](super::html::CSP) plus [`SCRIPT`] by its SHA-256 hash.
pub fn csp() -> String {
    csp_with(None)
}

/// [`csp`], also allowing `extra` (a second inline script) by its hash.
pub(super) fn csp_with(extra: Option<&str>) -> String {
    let mut hashes = String::new();
    for script in std::iter::once(SCRIPT).chain(extra) {
        if !hashes.is_empty() {
            hashes.push(' ');
        }
        hashes.push_str("'sha256-");
        base64_into(&Sha256::digest(script.as_bytes()), &mut hashes);
        hashes.push('\'');
    }
    format!(
        "default-src 'none'; img-src data:; style-src 'unsafe-inline'; \
         script-src {hashes}; base-uri 'none'; form-action 'none'"
    )
}

/// What another format adds to the page: a SCORM package's runtime and
/// quiz.
pub(super) struct Extra<'a> {
    /// A static inline script, run before [`SCRIPT`]; it must not contain
    /// `</script`.
    pub script: &'static str,
    /// Appended to the stylesheet.
    pub style: &'static str,
    /// Safe HTML on the end slide, above Restart.
    pub end: &'a str,
}

/// The walkthrough's own words, by guide language like [`Labels`].
struct Words {
    start: &'static str,
    begin: &'static str,
    back: &'static str,
    next: &'static str,
    restart: &'static str,
    steps: &'static str,
    of: &'static str,
    done: &'static str,
    done_text: &'static str,
    target: &'static str,
    hint: &'static str,
}

impl Words {
    fn for_language(language: &str) -> Self {
        let primary = language.split(['-', '_']).next().unwrap_or_default();
        if primary.eq_ignore_ascii_case("de") {
            Self {
                start: "Bevor Sie beginnen",
                begin: "Los geht’s",
                back: "Zurück",
                next: "Weiter",
                restart: "Neu starten",
                steps: "Schritte",
                of: "von",
                done: "Fertig",
                done_text: "Sie haben das Ende dieser Anleitung erreicht.",
                target: "Hier klicken, um fortzufahren",
                hint: "Klicken Sie auf die markierte Stelle oder drücken Sie → oder die \
                       Eingabetaste, um fortzufahren; ← geht zurück.",
            }
        } else {
            Self {
                start: "Before you start",
                begin: "Start",
                back: "Back",
                next: "Next",
                restart: "Restart",
                steps: "Steps",
                of: "of",
                done: "Done",
                done_text: "You have reached the end of this guide.",
                target: "Click here to continue",
                hint: "Click the highlighted spot, or press → or Enter, to continue; ← goes back.",
            }
        }
    }
}

/// The walkthrough page as a string; the report has the image warnings and
/// no files. Called by [`html::render_string`](super::html::render_string).
pub(super) fn render_string(
    guide: &RenderGuide,
    handle: &GuideHandle,
    opts: &HtmlOptions,
) -> Result<(String, ExportReport)> {
    render_with(guide, handle, opts, None)
}

/// [`render_string`] with what another format adds to the page.
pub(super) fn render_with(
    guide: &RenderGuide,
    handle: &GuideHandle,
    opts: &HtmlOptions,
    extra: Option<&Extra>,
) -> Result<(String, ExportReport)> {
    let branding = &opts.branding;
    let labels = opts
        .labels
        .clone()
        .unwrap_or_else(|| Labels::for_language(&guide.language));
    let words = Words::for_language(&guide.language);
    let step_label = labels.step_kind(guide.kind).to_owned();
    let image_opts = ImageOptions {
        hotspot_markers: false,
        locate_point: true,
        ..opts.image.clone()
    };
    let total = guide.steps().count();
    let intro = blocks_html(&guide.intro);
    let prerequisites = blocks_html(&guide.prerequisites);

    let mut report = ExportReport::default();
    let mut slides = Vec::new();
    // Step list entries: slides, and section titles above their first step.
    let mut list = Vec::new();
    if !intro.is_empty() || !prerequisites.is_empty() {
        list.push(context! { kind => "slide", index => 0, title => words.start });
        slides.push(context! { kind => "intro", index => 0, status => words.start });
    }
    let mut section: Option<&str> = None;
    let mut section_listed = true;
    let mut notes = Vec::new();
    for item in &guide.items {
        match item {
            RenderItem::Section(s) => {
                section = Some(&s.title);
                section_listed = false;
            }
            RenderItem::Note(note) => notes.push(note_context(note, &labels)),
            RenderItem::Step(step) => {
                let index = slides.len();
                if !section_listed {
                    list.push(context! { kind => "section", title => section });
                    section_listed = true;
                }
                let image = render_step_image(handle, &step.step, &image_opts);
                report.add_warnings(step.step.id, image.warnings.iter().cloned());
                let plain = plain_text(&step.title).replace('\n', " ");
                let title = inlines_html(&step.title);
                let title = if title.is_empty() {
                    escape(&step_label)
                } else {
                    title
                };
                let position = format!("{step_label} {} {} {total}", step.number, words.of);
                let status = if plain.trim().is_empty() {
                    position
                } else {
                    format!("{position}: {}", plain.trim())
                };
                let hotspots: Vec<Value> = image
                    .hotspots
                    .iter()
                    .map(|h| hotspot_context(step.number, h))
                    .collect();
                list.push(context! {
                    kind => "slide",
                    index => index,
                    number => step.number,
                    title => if plain.trim().is_empty() { step_label.as_str() } else { plain.trim() },
                });
                slides.push(context! {
                    kind => "step",
                    index => index,
                    number => step.number,
                    status => status,
                    section => section,
                    notes => std::mem::take(&mut notes),
                    title => Value::from_safe_string(title),
                    description => Value::from_safe_string(blocks_html(&step.description)),
                    image => context! {
                        src => Value::from_safe_string(data_uri("image/png", &image.png)),
                        width => image.width,
                        height => image.height,
                        alt => plain,
                    },
                    target => image.point.map(|p| Value::from_safe_string(target_style(p))),
                    hotspots => hotspots,
                });
            }
        }
    }
    let last = slides.len();
    list.push(context! { kind => "slide", index => last, title => words.done });
    slides.push(context! { kind => "done", index => last, status => words.done, notes => notes });

    let logo = branding
        .logo()
        .map(|logo| Value::from_safe_string(data_uri("image/png", logo.png())));
    let ctx = context! {
        language => language_tag(&guide.language),
        csp => csp_with(extra.map(|e| e.script)),
        title => &guide.title,
        og_image => og_image(guide, handle, &opts.image),
        summary => &guide.summary,
        details => opts.review_details.then(|| labels.review_details(&guide.review)).flatten(),
        css => Value::from_safe_string(format!(
            "{}{STYLE}{}",
            stylesheet(branding),
            extra.map_or("", |e| e.style)
        )),
        script => Value::from_safe_string(SCRIPT.to_owned()),
        extra_script => extra.map(|e| Value::from_safe_string(e.script.to_owned())),
        extra_end => extra.map(|e| Value::from_safe_string(e.end.to_owned())),
        logo => logo,
        labels => context! {
            prerequisites => labels.prerequisites,
            step => step_label,
            start => words.start,
            begin => words.begin,
            back => words.back,
            next => words.next,
            restart => words.restart,
            steps => words.steps,
            done => words.done,
            done_text => words.done_text,
            target => words.target,
            hint => words.hint,
        },
        intro => Value::from_safe_string(intro),
        prerequisites => Value::from_safe_string(prerequisites),
        list => list,
        slides => slides,
    };
    let env = environment("walkthrough.html", TEMPLATE)?;
    Ok((render_template(&env, "walkthrough.html", ctx)?, report))
}

fn note_context(note: &RenderNote, labels: &Labels) -> Value {
    context! {
        note_kind => note_kind_class(note.kind),
        label => labels.note_kind(note.kind),
        body => Value::from_safe_string(blocks_html(&note.body)),
    }
}

/// The target's `style`: its centre, as percentages of the image.
fn target_style(p: Point) -> String {
    format!("left:{};top:{}", percent(p.x), percent(p.y))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csp_hashes_the_script() {
        let csp = csp();
        assert!(csp.starts_with("default-src 'none'; img-src data:;"));
        let hash = csp
            .split("script-src 'sha256-")
            .nth(1)
            .and_then(|rest| rest.split('\'').next())
            .unwrap();
        // 32 bytes of base64 with one `=` of padding.
        assert_eq!(hash.len(), 44);
        assert!(hash.ends_with('='));
        assert!(!SCRIPT.contains("</script"));
    }

    #[test]
    fn targets_are_placed_in_percent() {
        assert_eq!(
            target_style(Point {
                x: 0.25,
                y: 1.0 / 3.0
            }),
            "left:25%;top:33.333%"
        );
    }

    #[test]
    fn words_follow_the_language() {
        assert_eq!(Words::for_language("de-AT").next, "Weiter");
        assert_eq!(Words::for_language("DE").next, "Weiter");
        assert_eq!(Words::for_language("en").next, "Next");
        assert_eq!(Words::for_language("").next, "Next");
    }
}
