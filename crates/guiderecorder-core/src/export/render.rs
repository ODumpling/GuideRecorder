//! Render model: a [`Guide`] resolved for the format writers.
//!
//! Text fields are parsed into the [`ast`](super::ast); steps are numbered
//! 1, 2, 3, ... in item order, with sections, notes and decisions taking no
//! number. Decision options are resolved to the step number they lead to.
//! Step images are rendered on demand with [`RenderGuide::step_image`], so a
//! writer controls size and only pays for the images it emits.

use ulid::Ulid;

use super::ast::{parse_blocks, parse_inlines, Block, Inline};
use super::raster::{render_cover_image, render_step_image, ImageOptions, RenderedImage};
use crate::library::GuideHandle;
use crate::model::{
    BranchTarget, CoverOptions, Guide, GuideKind, HotspotGeometry, Item, NoteKind, ReviewInfo,
    Shape, Step,
};
use crate::web;

/// A guide ready for export.
#[derive(Debug, Clone)]
pub struct RenderGuide {
    /// Bundle the step images are read from.
    pub handle: GuideHandle,
    pub id: Ulid,
    /// Plain text.
    pub title: String,
    /// Plain text.
    pub summary: String,
    pub language: String,
    /// A walkthrough, or an annotation guide whose steps are pages
    /// explained by hotspots (writers call them "Page", see
    /// [`Labels::step_kind`](super::md::Labels::step_kind)).
    pub kind: GuideKind,
    pub intro: Vec<Block>,
    pub prerequisites: Vec<Block>,
    pub items: Vec<RenderItem>,
    /// Owner, status, version and review schedule, as stored. Writers show
    /// some of it only when asked (their `review_details` option).
    pub review: ReviewInfo,
    /// The cover the author chose ([`Guide::chosen_cover_step`]); `None`
    /// if they chose none, and writers then show no cover image.
    pub cover: Option<RenderCover>,
    /// When the guide was last changed; a cover shows its day.
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// A guide's chosen cover: its step and how to draw it.
#[derive(Debug, Clone)]
pub struct RenderCover {
    pub step: Step,
    pub options: CoverOptions,
}

impl RenderCover {
    /// Renders the cover from the bundle at `handle` (see
    /// [`render_cover_image`]). Never fails, like step images.
    pub fn image(&self, handle: &GuideHandle, opts: &ImageOptions) -> RenderedImage {
        render_cover_image(handle, &self.step, &self.options, opts)
    }
}

/// One resolved entry of [`Guide::items`].
// Like `model::Item`: most items are steps.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum RenderItem {
    Step(RenderStep),
    Section(RenderSection),
    Note(RenderNote),
    Decision(RenderDecision),
}

#[derive(Debug, Clone)]
pub struct RenderStep {
    /// 1-based step (or page) number; sections and notes are skipped.
    pub number: u32,
    pub title: Vec<Inline>,
    pub description: Vec<Block>,
    /// The step as stored, for kind, keystrokes, element and images.
    pub step: Step,
}

#[derive(Debug, Clone)]
pub struct RenderSection {
    pub id: Ulid,
    /// Plain text.
    pub title: String,
    /// Who the section is for (see [`crate::branch`]), trimmed, without
    /// empty entries. Empty: everyone.
    pub audiences: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct RenderNote {
    pub id: Ulid,
    pub kind: NoteKind,
    pub body: Vec<Block>,
}

/// A question with its options (see [`crate::model::Decision`]).
#[derive(Debug, Clone)]
pub struct RenderDecision {
    pub id: Ulid,
    pub question: Vec<Inline>,
    pub options: Vec<RenderOption>,
}

#[derive(Debug, Clone)]
pub struct RenderOption {
    /// Plain text on one line.
    pub label: String,
    pub target: RenderTarget,
}

/// Where an option leads, resolved against the exported items.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderTarget {
    /// A step, by number: the step chosen, the first step of the section
    /// chosen, or the step after the decision. `None` past the last step
    /// (the end of the guide).
    Step {
        number: Option<u32>,
        /// The section's plain-text title when a section was chosen.
        section: Option<String>,
    },
    /// Another guide; `title` as stored, on one line (may be empty).
    Guide { id: Ulid, title: String },
}

impl RenderStep {
    /// The step's hotspots with their numbers (1, 2, 3, ... in annotation
    /// order), as [`RenderedImage::hotspots`] numbers them, whether or not
    /// they are in view. Writers list them as the image's legend.
    pub fn hotspots(&self) -> impl Iterator<Item = (u32, &HotspotGeometry)> {
        self.step
            .annotations
            .iter()
            .filter_map(|a| match &a.shape {
                Shape::Hotspot(hotspot) => Some(hotspot),
                _ => None,
            })
            .zip(1..)
            .map(|(hotspot, number)| (number, hotspot))
    }
}

/// A paragraph linking to the web page a step is on, shown as
/// [`web::display_url`] says it. Through the guide Markdown, so an address
/// that is not http or https stays plain text.
fn page_link(url: &str) -> Block {
    let markdown = format!(
        "[{}](<{}>)",
        crate::steps::escape(&web::display_url(url)),
        url.replace(['<', '>'], "")
    );
    parse_blocks(&markdown)
        .into_iter()
        .next()
        .unwrap_or_else(|| Block::Paragraph(Vec::new()))
}

impl RenderGuide {
    /// Resolves `guide`, whose images live in the bundle at `handle`.
    ///
    /// A step on a web page ([`Step::url`]) links to it at the end of its
    /// description when the page differs from the previous step's and its
    /// title does not link to it already (a "Go to" step).
    pub fn new(guide: &Guide, handle: &GuideHandle) -> Self {
        let mut number = 0;
        let mut page: Option<&str> = None;
        let items = guide
            .items
            .iter()
            .map(|item| match item {
                Item::Step(step) => {
                    number += 1;
                    let mut description = parse_blocks(&step.description);
                    let url = step.url.as_deref();
                    if let Some(url) = url.filter(|url| page != Some(*url)) {
                        if !step.title.contains(url) {
                            description.push(page_link(url));
                        }
                    }
                    page = url.or(page);
                    RenderItem::Step(RenderStep {
                        number,
                        title: parse_inlines(&step.title),
                        description,
                        step: step.clone(),
                    })
                }
                Item::Section(section) => RenderItem::Section(RenderSection {
                    id: section.id,
                    title: section.title.clone(),
                    audiences: section
                        .audiences
                        .iter()
                        .map(|a| a.trim())
                        .filter(|a| !a.is_empty())
                        .map(str::to_owned)
                        .collect(),
                }),
                Item::Note(note) => RenderItem::Note(RenderNote {
                    id: note.id,
                    kind: note.kind,
                    body: parse_blocks(&note.body),
                }),
                Item::Decision(decision) => RenderItem::Decision(RenderDecision {
                    id: decision.id,
                    question: parse_inlines(&decision.question),
                    options: decision
                        .options
                        .iter()
                        .map(|option| RenderOption {
                            label: one_line(&option.label),
                            target: resolve(guide, decision.id, &option.target),
                        })
                        .collect(),
                }),
            })
            .collect();
        Self {
            handle: handle.clone(),
            id: guide.id,
            title: guide.title.clone(),
            summary: guide.summary.clone(),
            language: guide.language.clone(),
            kind: guide.kind,
            intro: parse_blocks(&guide.intro),
            prerequisites: parse_blocks(&guide.prerequisites),
            items,
            review: guide.review.clone(),
            updated_at: guide.updated_at,
            cover: guide.chosen_cover_step().map(|step| RenderCover {
                step: step.clone(),
                options: guide.cover_options,
            }),
        }
    }

    /// Steps in order.
    pub fn steps(&self) -> impl Iterator<Item = &RenderStep> {
        self.items.iter().filter_map(|item| match item {
            RenderItem::Step(step) => Some(step),
            _ => None,
        })
    }

    /// Decisions in order.
    pub fn decisions(&self) -> impl Iterator<Item = &RenderDecision> {
        self.items.iter().filter_map(|item| match item {
            RenderItem::Decision(decision) => Some(decision),
            _ => None,
        })
    }

    /// Renders the image of `step` with its annotations and burned-in blur.
    /// Never fails: a missing or broken image becomes a placeholder with a
    /// warning.
    pub fn step_image(&self, step: &RenderStep, opts: &ImageOptions) -> RenderedImage {
        render_step_image(&self.handle, &step.step, opts)
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Where `target`, an option of the decision `from`, leads: the first step
/// at or after the item it names (after the decision for `next`). A step
/// or section that is gone reads as `next`.
fn resolve(guide: &Guide, from: Ulid, target: &BranchTarget) -> RenderTarget {
    let (id, section) = match target {
        BranchTarget::Guide { id, title } => {
            return RenderTarget::Guide {
                id: *id,
                title: one_line(title),
            }
        }
        BranchTarget::Next => (from, None),
        BranchTarget::Step { id } => (*id, None),
        BranchTarget::Section { id } => (
            *id,
            guide.items.iter().find_map(|item| match item {
                Item::Section(s) if s.id == *id => Some(one_line(&s.title)),
                _ => None,
            }),
        ),
    };
    let (id, section) = if guide.items.iter().any(|item| item.id() == id) {
        (id, section)
    } else {
        (from, None)
    };
    let mut number = 0;
    let mut found = false;
    for item in &guide.items {
        found |= item.id() == id;
        if let Item::Step(_) = item {
            number += 1;
            if found {
                return RenderTarget::Step {
                    number: Some(number),
                    section,
                };
            }
        }
    }
    RenderTarget::Step {
        number: None,
        section,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ImageShow, Note, Section, StepImage, StepKind};
    use std::path::PathBuf;

    fn step(title: &str) -> Item {
        Item::Step(Step {
            id: Ulid::new(),
            title: title.into(),
            description: "Some *text*".into(),
            kind: StepKind::Click,
            point: None,
            end_point: None,
            element: None,
            keystrokes: None,
            url: None,
            image: StepImage {
                full: "steps/x.webp".into(),
                crop: None,
                crop_rect: None,
                after: None,
                show: ImageShow::Full,
            },
            annotations: Vec::new(),
            narration: String::new(),
            ai_state: Default::default(),
        })
    }

    #[test]
    fn numbers_steps_skipping_sections_and_notes() {
        let mut guide = Guide::new("T", "en");
        guide.intro = "Hello **you**".into();
        guide.items = vec![
            Item::Section(Section {
                id: Ulid::new(),
                title: "Setup".into(),
                audiences: Vec::new(),
            }),
            step("Click **A**"),
            Item::Note(Note {
                id: Ulid::new(),
                kind: NoteKind::Tip,
                body: "- a\n- b".into(),
            }),
            step("Click B"),
            Item::Section(Section {
                id: Ulid::new(),
                title: "Then".into(),
                audiences: Vec::new(),
            }),
            step("Click C"),
        ];
        let handle = GuideHandle {
            id: guide.id,
            path: PathBuf::from("/nonexistent.guiderecorder"),
        };
        let render = RenderGuide::new(&guide, &handle);
        assert_eq!(render.kind, GuideKind::Walkthrough);
        let numbers: Vec<u32> = render.steps().map(|s| s.number).collect();
        assert_eq!(numbers, [1, 2, 3]);
        assert_eq!(render.items.len(), 6);
        let first = render.steps().next().unwrap();
        assert_eq!(
            first.title,
            [
                Inline::Text("Click ".into()),
                Inline::Strong(vec![Inline::Text("A".into())])
            ]
        );
        assert_eq!(first.description.len(), 1);
        assert!(
            matches!(&render.items[2], RenderItem::Note(n) if matches!(n.body[0], Block::List { start: None, .. }))
        );
        assert_eq!(render.intro.len(), 1);
        assert!(render.prerequisites.is_empty());
    }

    #[test]
    fn steps_link_to_their_web_page_when_it_changes() {
        let on = |title: &str, url: Option<&str>| {
            let mut item = step(title);
            if let Item::Step(step) = &mut item {
                step.url = url.map(Into::into);
            }
            item
        };
        let mut guide = Guide::new("T", "en");
        guide.items = vec![
            on("Click **Sign in**", Some("https://www.example.com/")),
            on("Click **Settings**", Some("https://www.example.com/")),
            on(
                "Go to [example.com/keys](https://example.com/keys)",
                Some("https://example.com/keys"),
            ),
            on("Click **Rotate**", Some("https://example.com/keys")),
            on("Click **Save** in Finder", None),
            on("Click **Done**", Some("https://example.com/keys?done=1")),
            on("Click **Bad**", Some("javascript:alert(1)")),
        ];
        let handle = GuideHandle {
            id: guide.id,
            path: PathBuf::from("/nonexistent.guiderecorder"),
        };
        let render = RenderGuide::new(&guide, &handle);
        let links: Vec<Option<Block>> = render
            .steps()
            .map(|s| s.description.get(1).cloned())
            .collect();
        let link = |text: &str, url: &str| {
            Some(Block::Paragraph(vec![Inline::Link {
                url: url.into(),
                title: String::new(),
                children: vec![Inline::Text(text.into())],
            }]))
        };
        assert_eq!(
            links,
            [
                link("example.com", "https://www.example.com/"),
                // The same page; a "Go to" step links in its title.
                None,
                None,
                None,
                None,
                link("example.com/keys?done=1", "https://example.com/keys?done=1"),
                // Never a link to anything but http or https.
                Some(Block::Paragraph(vec![Inline::Text(
                    "javascript:alert(1)".into()
                )])),
            ]
        );
    }
}
