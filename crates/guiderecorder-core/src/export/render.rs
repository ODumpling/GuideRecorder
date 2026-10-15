//! Render model: a [`Guide`] resolved for the format writers.
//!
//! Text fields are parsed into the [`ast`](super::ast); steps are numbered
//! 1, 2, 3, ... in item order, with sections and notes taking no number.
//! Step images are rendered on demand with [`RenderGuide::step_image`], so a
//! writer controls size and only pays for the images it emits.

use ulid::Ulid;

use super::ast::{parse_blocks, parse_inlines, Block, Inline};
use super::raster::{render_step_image, ImageOptions, RenderedImage};
use crate::library::GuideHandle;
use crate::model::{Guide, Item, NoteKind, Step};

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
    pub intro: Vec<Block>,
    pub prerequisites: Vec<Block>,
    pub items: Vec<RenderItem>,
}

/// One resolved entry of [`Guide::items`].
// Like `model::Item`: most items are steps.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum RenderItem {
    Step(RenderStep),
    Section(RenderSection),
    Note(RenderNote),
}

#[derive(Debug, Clone)]
pub struct RenderStep {
    /// 1-based step number; sections and notes are skipped.
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
}

#[derive(Debug, Clone)]
pub struct RenderNote {
    pub id: Ulid,
    pub kind: NoteKind,
    pub body: Vec<Block>,
}

impl RenderGuide {
    /// Resolves `guide`, whose images live in the bundle at `handle`.
    pub fn new(guide: &Guide, handle: &GuideHandle) -> Self {
        let mut number = 0;
        let items = guide
            .items
            .iter()
            .map(|item| match item {
                Item::Step(step) => {
                    number += 1;
                    RenderItem::Step(RenderStep {
                        number,
                        title: parse_inlines(&step.title),
                        description: parse_blocks(&step.description),
                        step: step.clone(),
                    })
                }
                Item::Section(section) => RenderItem::Section(RenderSection {
                    id: section.id,
                    title: section.title.clone(),
                }),
                Item::Note(note) => RenderItem::Note(RenderNote {
                    id: note.id,
                    kind: note.kind,
                    body: parse_blocks(&note.body),
                }),
            })
            .collect();
        Self {
            handle: handle.clone(),
            id: guide.id,
            title: guide.title.clone(),
            summary: guide.summary.clone(),
            language: guide.language.clone(),
            intro: parse_blocks(&guide.intro),
            prerequisites: parse_blocks(&guide.prerequisites),
            items,
        }
    }

    /// Steps in order.
    pub fn steps(&self) -> impl Iterator<Item = &RenderStep> {
        self.items.iter().filter_map(|item| match item {
            RenderItem::Step(step) => Some(step),
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
            image: StepImage {
                full: "steps/x.webp".into(),
                crop: None,
                crop_rect: None,
                after: None,
                show: ImageShow::Full,
            },
            annotations: Vec::new(),
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
            }),
            step("Click C"),
        ];
        let handle = GuideHandle {
            id: guide.id,
            path: PathBuf::from("/nonexistent.guiderecorder"),
        };
        let render = RenderGuide::new(&guide, &handle);
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
}
