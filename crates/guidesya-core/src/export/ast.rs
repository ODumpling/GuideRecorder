//! Owned AST for the guide Markdown subset, walked by the format writers.
//!
//! Text is first passed through [`crate::markdown::normalize`], so the
//! degradation rules (headings and code blocks become paragraphs, unsafe link
//! schemes become plain text, ...) are exactly the ones the editor and the
//! index use. The normalized text contains only the subset, which is then
//! parsed here with pulldown-cmark into [`Block`]s and [`Inline`]s.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

use crate::markdown;

/// A block of guide text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Paragraph(Vec<Inline>),
    List {
        /// Start number for numbered lists; `None` for bullet lists.
        start: Option<u64>,
        /// Each item is a sequence of blocks (paragraphs and nested lists).
        items: Vec<Vec<Block>>,
    },
}

/// Inline content of a paragraph.
///
/// All strings are raw text, not markup: raw HTML in the source is kept as
/// literal text (e.g. `"x <b>raw</b>"`). Writers must escape `Text`, `Code`,
/// `Link::url` and `Link::title` for their output format (HTML, XML in DOCX,
/// Typst markup, Markdown).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inline {
    Text(String),
    Code(String),
    Strong(Vec<Inline>),
    Emphasis(Vec<Inline>),
    /// `url` is http, https, mailto or relative; anything else was already
    /// reduced to its text by normalization.
    Link {
        url: String,
        title: String,
        children: Vec<Inline>,
    },
    /// Hard line break.
    Break,
}

/// Parses guide Markdown into blocks.
pub fn parse_blocks(markdown: &str) -> Vec<Block> {
    let normalized = markdown::normalize(markdown);
    let mut builder = Builder {
        stack: vec![Frame::Blocks(Vec::new(), Vec::new())],
    };
    for event in Parser::new_ext(&normalized, Options::empty()) {
        builder.event(event);
    }
    match builder.stack.pop() {
        Some(Frame::Blocks(mut blocks, inlines)) if builder.stack.is_empty() => {
            push_paragraph(&mut blocks, inlines);
            blocks
        }
        _ => unreachable!("unbalanced markdown events"),
    }
}

/// Parses single-line guide Markdown (e.g. a step title) into inlines.
/// Paragraphs are joined with [`Inline::Break`]; list items are flattened
/// into their text, separated by breaks.
pub fn parse_inlines(markdown: &str) -> Vec<Inline> {
    let mut out = Vec::new();
    flatten(parse_blocks(markdown), &mut out);
    out
}

fn flatten(blocks: Vec<Block>, out: &mut Vec<Inline>) {
    for block in blocks {
        match block {
            Block::Paragraph(inlines) => {
                if !out.is_empty() {
                    out.push(Inline::Break);
                }
                out.extend(inlines);
            }
            Block::List { items, .. } => items.into_iter().for_each(|item| flatten(item, out)),
        }
    }
}

/// Text content of `inlines` with markup removed; breaks become `\n`.
pub fn plain_text(inlines: &[Inline]) -> String {
    let mut out = String::new();
    push_plain(inlines, &mut out);
    out
}

fn push_plain(inlines: &[Inline], out: &mut String) {
    for inline in inlines {
        match inline {
            Inline::Text(text) | Inline::Code(text) => out.push_str(text),
            Inline::Strong(children)
            | Inline::Emphasis(children)
            | Inline::Link { children, .. } => push_plain(children, out),
            Inline::Break => out.push('\n'),
        }
    }
}

enum Frame {
    /// Root or list item: finished blocks plus text of a tight item.
    Blocks(Vec<Block>, Vec<Inline>),
    List(Option<u64>, Vec<Vec<Block>>),
    Paragraph(Vec<Inline>),
    Strong(Vec<Inline>),
    Emphasis(Vec<Inline>),
    Link(String, String, Vec<Inline>),
}

struct Builder {
    stack: Vec<Frame>,
}

impl Builder {
    fn event(&mut self, event: Event) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.text(&text),
            Event::Code(code) => self.inline(Inline::Code(code.into_string())),
            Event::SoftBreak => self.text(" "),
            Event::HardBreak => self.inline(Inline::Break),
            // Normalized text has none of these; keep any text they carry.
            Event::Html(text)
            | Event::InlineHtml(text)
            | Event::InlineMath(text)
            | Event::DisplayMath(text)
            | Event::FootnoteReference(text) => self.text(&text),
            Event::Rule | Event::TaskListMarker(_) => {}
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph | Tag::Heading { .. } | Tag::CodeBlock(_) | Tag::HtmlBlock => {
                self.flush_inlines();
                self.stack.push(Frame::Paragraph(Vec::new()));
            }
            Tag::List(start) => {
                self.flush_inlines();
                self.stack.push(Frame::List(start, Vec::new()));
            }
            Tag::Item => self.stack.push(Frame::Blocks(Vec::new(), Vec::new())),
            Tag::Emphasis => self.stack.push(Frame::Emphasis(Vec::new())),
            Tag::Strong => self.stack.push(Frame::Strong(Vec::new())),
            Tag::Link {
                dest_url, title, ..
            } => self.stack.push(Frame::Link(
                dest_url.into_string(),
                title.into_string(),
                Vec::new(),
            )),
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::CodeBlock | TagEnd::HtmlBlock => {
                let Some(Frame::Paragraph(inlines)) = self.stack.pop() else {
                    unreachable!("paragraph frame")
                };
                self.flush_inlines();
                push_paragraph(self.blocks_mut(), inlines);
            }
            TagEnd::List(_) => {
                let Some(Frame::List(start, items)) = self.stack.pop() else {
                    unreachable!("list frame")
                };
                self.flush_inlines();
                self.blocks_mut().push(Block::List { start, items });
            }
            TagEnd::Item => {
                let Some(Frame::Blocks(mut blocks, inlines)) = self.stack.pop() else {
                    unreachable!("item frame")
                };
                push_paragraph(&mut blocks, inlines);
                let Some(Frame::List(_, items)) = self.stack.last_mut() else {
                    unreachable!("item outside list")
                };
                items.push(blocks);
            }
            TagEnd::Emphasis => {
                let Some(Frame::Emphasis(children)) = self.stack.pop() else {
                    unreachable!("emphasis frame")
                };
                self.inline(Inline::Emphasis(children));
            }
            TagEnd::Strong => {
                let Some(Frame::Strong(children)) = self.stack.pop() else {
                    unreachable!("strong frame")
                };
                self.inline(Inline::Strong(children));
            }
            TagEnd::Link => {
                let Some(Frame::Link(url, title, children)) = self.stack.pop() else {
                    unreachable!("link frame")
                };
                self.inline(Inline::Link {
                    url,
                    title,
                    children,
                });
            }
            _ => {}
        }
    }

    /// Appends text, merging with a preceding text node (pulldown-cmark
    /// splits text at escapes and entities).
    fn text(&mut self, text: &str) {
        if let Some(Inline::Text(prev)) = self.inlines_mut().last_mut() {
            prev.push_str(text);
        } else {
            self.inline(Inline::Text(text.to_owned()));
        }
    }

    fn inline(&mut self, inline: Inline) {
        self.inlines_mut().push(inline);
    }

    fn inlines_mut(&mut self) -> &mut Vec<Inline> {
        match self.stack.last_mut() {
            Some(
                Frame::Paragraph(children)
                | Frame::Strong(children)
                | Frame::Emphasis(children)
                | Frame::Link(_, _, children)
                | Frame::Blocks(_, children),
            ) => children,
            Some(Frame::List(..)) | None => unreachable!("inline outside a block"),
        }
    }

    /// Turns pending tight-item text into a paragraph before a block starts.
    fn flush_inlines(&mut self) {
        if let Some(Frame::Blocks(blocks, inlines)) = self.stack.last_mut() {
            let inlines = std::mem::take(inlines);
            push_paragraph(blocks, inlines);
        }
    }

    fn blocks_mut(&mut self) -> &mut Vec<Block> {
        match self.stack.last_mut() {
            Some(Frame::Blocks(blocks, _)) => blocks,
            _ => unreachable!("block outside a container"),
        }
    }
}

fn push_paragraph(blocks: &mut Vec<Block>, inlines: Vec<Inline>) {
    if !inlines.is_empty() {
        blocks.push(Block::Paragraph(inlines));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Inline {
        Inline::Text(s.into())
    }

    #[test]
    fn parses_inline_markup() {
        assert_eq!(
            parse_blocks(
                "Click **Save** in *Finder*, then run `ls` — see [docs](https://e.x/a \"T\")."
            ),
            vec![Block::Paragraph(vec![
                text("Click "),
                Inline::Strong(vec![text("Save")]),
                text(" in "),
                Inline::Emphasis(vec![text("Finder")]),
                text(", then run "),
                Inline::Code("ls".into()),
                text(" — see "),
                Inline::Link {
                    url: "https://e.x/a".into(),
                    title: "T".into(),
                    children: vec![text("docs")],
                },
                text("."),
            ])]
        );
    }

    #[test]
    fn parses_lists_and_paragraphs() {
        let blocks = parse_blocks("Before:\n\n1. One\n2. Two\n   - nested\n\nAfter");
        assert_eq!(
            blocks,
            vec![
                Block::Paragraph(vec![text("Before:")]),
                Block::List {
                    start: Some(1),
                    items: vec![
                        vec![Block::Paragraph(vec![text("One")])],
                        vec![
                            Block::Paragraph(vec![text("Two")]),
                            Block::List {
                                start: None,
                                items: vec![vec![Block::Paragraph(vec![text("nested")])]],
                            },
                        ],
                    ],
                },
                Block::Paragraph(vec![text("After")]),
            ]
        );
    }

    #[test]
    fn degrades_like_markdown_normalize() {
        assert_eq!(
            parse_blocks("# Title\n\n[x](javascript:alert(1)) <b>raw</b>"),
            vec![
                Block::Paragraph(vec![text("Title")]),
                Block::Paragraph(vec![text("x <b>raw</b>")]),
            ]
        );
        assert_eq!(parse_blocks("  \n"), Vec::<Block>::new());
    }

    #[test]
    fn hard_breaks_and_escapes() {
        assert_eq!(
            parse_blocks("a\\\nb \\*c\\*"),
            vec![Block::Paragraph(vec![
                text("a"),
                Inline::Break,
                text("b *c*")
            ])]
        );
    }

    #[test]
    fn inlines_flatten_blocks() {
        let inlines = parse_inlines("Open **A**\n\n- x\n- y");
        assert_eq!(
            inlines,
            vec![
                text("Open "),
                Inline::Strong(vec![text("A")]),
                Inline::Break,
                text("x"),
                Inline::Break,
                text("y"),
            ]
        );
        assert_eq!(plain_text(&inlines), "Open A\nx\ny");
    }
}
