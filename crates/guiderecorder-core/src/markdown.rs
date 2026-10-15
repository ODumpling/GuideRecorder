//! The Markdown subset used for guide text. See design scratchpad #444 ("Text format").
//!
//! Supported: paragraphs, bold, italic, inline code, links (http, https,
//! mailto and relative), bullet and numbered lists, hard line breaks.
//! Everything else degrades to plain text: headings and code blocks become
//! paragraphs, block quotes are unwrapped, images become their alt text, raw
//! HTML becomes literal text, rules are dropped.

use pulldown_cmark::{Event, LinkType, Options, Parser, Tag, TagEnd};

/// Parses `input` and re-serializes it in normalized form.
///
/// The output contains only supported constructs and is a fixed point:
/// `normalize(&normalize(s)) == normalize(s)`.
pub fn normalize(input: &str) -> String {
    let mut out = String::new();
    render_blocks(&parse(input), &mut out);
    out
}

/// Text content with all markup removed, e.g. for search indexing.
/// Blocks and list items are separated by newlines.
pub fn to_plain_text(input: &str) -> String {
    let mut lines = Vec::new();
    plain_blocks(&parse(input), &mut lines);
    lines.join("\n")
}

#[derive(Debug, Clone, PartialEq)]
enum Block {
    Paragraph(Vec<Inline>),
    List {
        /// Start number for numbered lists.
        start: Option<u64>,
        items: Vec<Vec<Block>>,
    },
}

#[derive(Debug, Clone, PartialEq)]
enum Inline {
    Text(String),
    Code(String),
    Strong(Vec<Inline>),
    Emphasis(Vec<Inline>),
    Link {
        url: String,
        title: String,
        children: Vec<Inline>,
    },
    Break,
}

enum Frame {
    /// Root or list item. `inlines` collects text of tight list items.
    Blocks {
        blocks: Vec<Block>,
        inlines: Vec<Inline>,
    },
    List {
        start: Option<u64>,
        items: Vec<Vec<Block>>,
    },
    Paragraph(Vec<Inline>),
    Strong(Vec<Inline>),
    Emphasis(Vec<Inline>),
    Link {
        url: Option<String>,
        title: String,
        children: Vec<Inline>,
    },
    /// Children become alt text in the parent.
    Image(Vec<Inline>),
}

struct Builder {
    stack: Vec<Frame>,
    /// Inside a code or HTML block, where text keeps its line structure.
    literal: bool,
    /// The previous literal text ended a line.
    literal_newline: bool,
}

fn parse(input: &str) -> Vec<Block> {
    let mut builder = Builder {
        stack: vec![Frame::Blocks {
            blocks: Vec::new(),
            inlines: Vec::new(),
        }],
        literal: false,
        literal_newline: false,
    };
    for event in Parser::new_ext(input, Options::empty()) {
        builder.event(event);
    }
    match builder.stack.pop() {
        Some(Frame::Blocks {
            mut blocks,
            inlines,
        }) if builder.stack.is_empty() => {
            push_paragraph(&mut blocks, inlines);
            blocks
        }
        _ => unreachable!("unbalanced markdown events"),
    }
}

impl Builder {
    fn event(&mut self, event: Event) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) | Event::Html(text) if self.literal => {
                if std::mem::replace(&mut self.literal_newline, text.ends_with('\n')) {
                    self.inline(Inline::Break);
                }
                for (i, line) in text.trim_end_matches('\n').split('\n').enumerate() {
                    if i > 0 {
                        self.inline(Inline::Break);
                    }
                    if !line.trim().is_empty() {
                        self.inline(Inline::Text(line.to_string()));
                    }
                }
            }
            Event::Text(text)
            | Event::InlineHtml(text)
            | Event::InlineMath(text)
            | Event::DisplayMath(text)
            | Event::FootnoteReference(text) => self.inline(Inline::Text(text.into_string())),
            Event::Html(text) => self.inline(Inline::Text(text.into_string())),
            Event::Code(code) => self.inline(Inline::Code(code.into_string())),
            Event::SoftBreak => self.inline(Inline::Text(" ".into())),
            Event::HardBreak => self.inline(Inline::Break),
            Event::Rule | Event::TaskListMarker(_) => {}
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph | Tag::Heading { .. } | Tag::HtmlBlock | Tag::CodeBlock(_) => {
                self.literal = matches!(tag, Tag::HtmlBlock | Tag::CodeBlock(_));
                self.literal_newline = false;
                self.flush_inlines();
                self.stack.push(Frame::Paragraph(Vec::new()));
            }
            Tag::List(start) => {
                self.flush_inlines();
                self.stack.push(Frame::List {
                    start,
                    items: Vec::new(),
                });
            }
            Tag::Item => self.stack.push(Frame::Blocks {
                blocks: Vec::new(),
                inlines: Vec::new(),
            }),
            Tag::BlockQuote(_) => self.flush_inlines(),
            Tag::Emphasis => self.stack.push(Frame::Emphasis(Vec::new())),
            Tag::Strong => self.stack.push(Frame::Strong(Vec::new())),
            Tag::Link {
                link_type,
                dest_url,
                title,
                ..
            } => {
                let url = match link_type {
                    LinkType::Email => format!("mailto:{dest_url}"),
                    _ => dest_url.into_string(),
                };
                let url = url.trim_matches(' ');
                self.stack.push(Frame::Link {
                    url: is_allowed_url(url).then(|| url.to_string()),
                    title: title.into_string(),
                    children: Vec::new(),
                });
            }
            Tag::Image { .. } => self.stack.push(Frame::Image(Vec::new())),
            // Not produced with `Options::empty()`; their contents pass through.
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::HtmlBlock | TagEnd::CodeBlock => {
                self.literal = false;
                let Some(Frame::Paragraph(inlines)) = self.stack.pop() else {
                    unreachable!("paragraph frame")
                };
                self.flush_inlines();
                push_paragraph(self.blocks_mut(), inlines);
            }
            TagEnd::List(_) => {
                let Some(Frame::List { start, items }) = self.stack.pop() else {
                    unreachable!("list frame")
                };
                self.flush_inlines();
                self.blocks_mut().push(Block::List { start, items });
            }
            TagEnd::Item => {
                let Some(Frame::Blocks {
                    mut blocks,
                    inlines,
                }) = self.stack.pop()
                else {
                    unreachable!("item frame")
                };
                push_paragraph(&mut blocks, inlines);
                let Some(Frame::List { items, .. }) = self.stack.last_mut() else {
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
                let Some(Frame::Link {
                    url,
                    title,
                    children,
                }) = self.stack.pop()
                else {
                    unreachable!("link frame")
                };
                match url {
                    Some(url) => self.inline(Inline::Link {
                        url,
                        title,
                        children,
                    }),
                    None => children.into_iter().for_each(|child| self.inline(child)),
                }
            }
            TagEnd::Image => {
                let Some(Frame::Image(children)) = self.stack.pop() else {
                    unreachable!("image frame")
                };
                children.into_iter().for_each(|child| self.inline(child));
            }
            _ => {}
        }
    }

    fn inline(&mut self, inline: Inline) {
        match self.stack.last_mut() {
            Some(
                Frame::Paragraph(children)
                | Frame::Strong(children)
                | Frame::Emphasis(children)
                | Frame::Link { children, .. }
                | Frame::Image(children)
                | Frame::Blocks {
                    inlines: children, ..
                },
            ) => children.push(inline),
            Some(Frame::List { .. }) | None => unreachable!("inline outside a block"),
        }
    }

    /// Turns pending tight-item text into a paragraph before a block starts.
    fn flush_inlines(&mut self) {
        if let Some(Frame::Blocks { blocks, inlines }) = self.stack.last_mut() {
            let inlines = std::mem::take(inlines);
            push_paragraph(blocks, inlines);
        }
    }

    fn blocks_mut(&mut self) -> &mut Vec<Block> {
        match self.stack.last_mut() {
            Some(Frame::Blocks { blocks, .. }) => blocks,
            _ => unreachable!("block outside a container"),
        }
    }
}

fn push_paragraph(blocks: &mut Vec<Block>, inlines: Vec<Inline>) {
    if inlines.iter().any(has_content) {
        blocks.push(Block::Paragraph(merge_adjacent(inlines)));
    }
}

/// Joins directly adjacent spans of the same kind (`*a*_b_` → `*ab*`), which
/// would otherwise serialize as one ambiguous delimiter run.
fn merge_adjacent(inlines: Vec<Inline>) -> Vec<Inline> {
    let mut merged: Vec<Inline> = Vec::new();
    for inline in inlines {
        match (merged.last_mut(), inline) {
            (Some(Inline::Strong(prev)), Inline::Strong(next))
            | (Some(Inline::Emphasis(prev)), Inline::Emphasis(next)) => prev.extend(next),
            (_, inline) => merged.push(inline),
        }
    }
    merged
        .into_iter()
        .map(|inline| match inline {
            Inline::Strong(children) => Inline::Strong(merge_adjacent(children)),
            Inline::Emphasis(children) => Inline::Emphasis(merge_adjacent(children)),
            Inline::Link {
                url,
                title,
                children,
            } => Inline::Link {
                url,
                title,
                children: merge_adjacent(children),
            },
            inline => inline,
        })
        .collect()
}

fn has_content(inline: &Inline) -> bool {
    match inline {
        Inline::Text(text) => !text.trim().is_empty(),
        Inline::Code(_) | Inline::Link { .. } => true,
        Inline::Strong(children) | Inline::Emphasis(children) => children.iter().any(has_content),
        Inline::Break => false,
    }
}

/// Allows http, https and mailto links and scheme-less (relative) links.
///
/// URLs containing whitespace or control characters are rejected outright:
/// browsers strip those, which could reveal a scheme such as `java\tscript:`.
/// Protocol-relative URLs (`//host`, and the `\\`, `/\`, `\/` spellings that
/// browsers treat the same) are rejected too: in an exported HTML file opened
/// from disk they resolve to `file://host/...`, a network share fetch on
/// Windows.
fn is_allowed_url(url: &str) -> bool {
    if url.is_empty()
        || url
            .chars()
            .any(|c| c.is_ascii_control() || c.is_whitespace())
    {
        return false;
    }
    if let [a, b, ..] = url.as_bytes() {
        if matches!(a, b'/' | b'\\') && matches!(b, b'/' | b'\\') {
            return false;
        }
    }
    // Anything before a ':' that precedes the path, query or fragment is a scheme.
    match url.find([':', '/', '?', '#']) {
        Some(i) if url[i..].starts_with(':') => ["http", "https", "mailto"]
            .iter()
            .any(|allowed| url[..i].eq_ignore_ascii_case(allowed)),
        _ => true,
    }
}

// ---------------------------------------------------------------------------
// Serializer

fn render_blocks(blocks: &[Block], out: &mut String) {
    // Which marker the previous list used, so adjacent lists stay separate.
    let mut prev_alt: Option<(bool, bool)> = None;
    for (i, block) in blocks.iter().enumerate() {
        if i > 0 {
            let tight = matches!(blocks[i - 1], Block::Paragraph(_))
                && matches!(block, Block::List { start, items }
                    if matches!(start, None | Some(1))
                        && items.first().is_some_and(|item| !item.is_empty()));
            out.push_str(if tight { "\n" } else { "\n\n" });
        }
        match block {
            Block::Paragraph(inlines) => {
                render_paragraph(inlines, out);
                prev_alt = None;
            }
            Block::List { start, items } => {
                let ordered = start.is_some();
                let alt = matches!(prev_alt, Some((o, a)) if o == ordered && !a);
                render_list(*start, items, alt, out);
                prev_alt = Some((ordered, alt));
            }
        }
    }
}

fn render_list(start: Option<u64>, items: &[Vec<Block>], alt: bool, out: &mut String) {
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let marker = match start {
            Some(start) => format!("{}{} ", start + i as u64, if alt { ')' } else { '.' }),
            None => format!("{} ", if alt { '*' } else { '-' }),
        };
        let mut body = String::new();
        render_blocks(item, &mut body);
        if body.is_empty() {
            out.push_str(marker.trim_end());
            continue;
        }
        let indent = " ".repeat(marker.len());
        for (j, line) in body.split('\n').enumerate() {
            if j == 0 {
                out.push_str(&marker);
            } else {
                out.push('\n');
                if !line.is_empty() {
                    out.push_str(&indent);
                }
            }
            out.push_str(line);
        }
    }
}

fn render_paragraph(inlines: &[Inline], out: &mut String) {
    let mut text = String::new();
    render_inlines(&trim_breaks(inlines), &mut text, None);
    let text = text.trim();
    for (i, line) in text.split("\\\n").enumerate() {
        if i > 0 {
            out.push_str("\\\n");
        }
        escape_line_start(line.trim_start(), out);
    }
}

/// Drops leading, trailing and repeated hard breaks, including breaks
/// separated only by whitespace or empty spans.
fn trim_breaks(inlines: &[Inline]) -> Vec<Inline> {
    let mut result: Vec<Inline> = Vec::new();
    for inline in inlines {
        if *inline == Inline::Break {
            while result.last().is_some_and(|last| !has_content(last)) {
                result.pop();
            }
            if result.is_empty() {
                continue;
            }
        }
        result.push(inline.clone());
    }
    while result.last().is_some_and(|last| !has_content(last)) {
        result.pop();
    }
    result
}

/// Escapes text at the start of a line that would otherwise begin a block.
fn escape_line_start(line: &str, out: &mut String) {
    if line.starts_with(['#', '-', '+', '=']) {
        out.push('\\');
        out.push_str(line);
        return;
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 && line[digits..].starts_with(['.', ')']) {
        out.push_str(&line[..digits]);
        out.push('\\');
        out.push_str(&line[digits..]);
        return;
    }
    out.push_str(line);
}

/// `parent` is the enclosing emphasis kind; same-kind nesting is flattened.
fn render_inlines(inlines: &[Inline], out: &mut String, parent: Option<&str>) {
    for inline in inlines {
        match inline {
            Inline::Text(text) => escape_text(text, out),
            Inline::Code(code) => render_code(code, out),
            Inline::Strong(children) => render_delimited(children, "**", out, parent),
            Inline::Emphasis(children) => render_delimited(children, "*", out, parent),
            Inline::Link {
                url,
                title,
                children,
            } => {
                // `![` would start an image.
                if out.ends_with('!') {
                    out.pop();
                    out.push_str("\\!");
                }
                out.push('[');
                if children.iter().any(has_content) {
                    render_inlines(children, out, parent);
                } else {
                    escape_text(url, out);
                }
                out.push_str("](");
                render_url(url, out);
                if !title.is_empty() {
                    out.push_str(" \"");
                    for c in title.chars() {
                        if matches!(c, '"' | '\\') {
                            out.push('\\');
                        }
                        out.push(c);
                    }
                    out.push('"');
                }
                out.push(')');
            }
            Inline::Break => out.push_str("\\\n"),
        }
    }
}

fn render_delimited(children: &[Inline], delim: &str, out: &mut String, parent: Option<&str>) {
    if !children.iter().any(has_content) {
        return;
    }
    if parent == Some(delim) {
        render_inlines(children, out, parent);
        return;
    }
    let mut inner = String::new();
    render_inlines(children, &mut inner, Some(delim));
    // Delimiters next to whitespace or a line break don't parse as emphasis.
    let is_space = |c: char| c.is_whitespace() && c != '\n';
    let mut core = inner.as_str();
    let mut lead = 0;
    loop {
        let trimmed = core.trim_start_matches(is_space);
        let trimmed = trimmed.strip_prefix("\\\n").unwrap_or(trimmed);
        if trimmed.len() == core.len() {
            break;
        }
        lead += core.len() - trimmed.len();
        core = trimmed;
    }
    let mut trail = core.len();
    loop {
        let trimmed = core[..trail].trim_end_matches(is_space);
        let trimmed = trimmed.strip_suffix("\\\n").unwrap_or(trimmed);
        if trimmed.len() == trail {
            break;
        }
        trail = trimmed.len();
    }
    out.push_str(&inner[..lead]);
    out.push_str(delim);
    out.push_str(&core[..trail]);
    out.push_str(delim);
    out.push_str(&core[trail..]);
}

fn render_code(code: &str, out: &mut String) {
    let longest_run = code.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest_run + 1);
    let pad =
        !code.trim().is_empty() && (code.starts_with([' ', '`']) || code.ends_with([' ', '`']));
    out.push_str(&fence);
    if pad {
        out.push(' ');
    }
    out.push_str(code);
    if pad {
        out.push(' ');
    }
    out.push_str(&fence);
}

fn render_url(url: &str, out: &mut String) {
    let angle = url.is_empty() || url.contains([' ', '(', ')', '<', '>']);
    if angle {
        out.push('<');
    }
    for c in url.chars() {
        if c == '\\' || (angle && matches!(c, '<' | '>')) {
            out.push('\\');
        }
        out.push(c);
    }
    if angle {
        out.push('>');
    }
}

fn escape_text(text: &str, out: &mut String) {
    for c in text.chars() {
        if c == '\n' {
            out.push(' ');
            continue;
        }
        if matches!(
            c,
            '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '&' | '~'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
}

// ---------------------------------------------------------------------------
// Plain text

fn plain_blocks(blocks: &[Block], lines: &mut Vec<String>) {
    for block in blocks {
        match block {
            Block::Paragraph(inlines) => {
                let mut text = String::new();
                plain_inlines(&trim_breaks(inlines), &mut text);
                lines.push(text.trim().to_string());
            }
            Block::List { items, .. } => {
                for item in items {
                    plain_blocks(item, lines);
                }
            }
        }
    }
}

fn plain_inlines(inlines: &[Inline], out: &mut String) {
    for inline in inlines {
        match inline {
            Inline::Text(text) | Inline::Code(text) => out.push_str(text),
            Inline::Strong(children)
            | Inline::Emphasis(children)
            | Inline::Link { children, .. } => plain_inlines(children, out),
            Inline::Break => out.push('\n'),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[track_caller]
    fn check(input: &str, expected: &str) {
        let once = normalize(input);
        assert_eq!(once, expected, "normalize({input:?})");
        assert_eq!(normalize(&once), once, "not a fixed point: {once:?}");
    }

    #[test]
    fn supported_constructs_round_trip() {
        for input in [
            "",
            "Plain paragraph.",
            "Click **Save** in the toolbar.",
            "An *italic* word and `inline code`.",
            "***both***",
            "See [the docs](https://example.com/docs \"Docs\").",
            "Mail [us](mailto:help@example.com).",
            "Relative [link](../other.md).",
            "First paragraph.\n\nSecond paragraph.",
            "- one\n- two\n- three",
            "1. first\n2. second",
            "3. starts at three\n4. four",
            "- outer\n  - inner **bold**\n- next",
            "1. step\n   - detail\n2. step two",
            "line one\\\nline two",
            "Intro:\n- a\n- b",
            "- a\n\n* b",
            "1. a\n\n1) b",
        ] {
            check(input, input);
        }
    }

    #[test]
    fn normalizes_equivalent_syntax() {
        check("Click __Save__ or _Cancel_.", "Click **Save** or *Cancel*.");
        check("* a\n* b", "- a\n- b");
        check("+ a\n\n+ b", "- a\n- b");
        check("soft\nbreak", "soft break");
        check("trailing  \nspaces", "trailing\\\nspaces");
        check(
            "<https://example.com>",
            "[https://example.com](https://example.com)",
        );
        check(
            "<me@example.com>",
            "[me@example.com](mailto:me@example.com)",
        );
        check("[x](<a(b).png>)", "[x](<a(b).png>)");
        check(
            "[ref]\n\n[ref]: https://example.com",
            "[ref](https://example.com)",
        );
        check("**outer **inner** outer**", "**outer inner outer**");
        check("Use `` a`b `` here.", "Use ``a`b`` here.");
        check("` `` `", "``` `` ```");
    }

    #[test]
    fn unsupported_constructs_degrade_to_text() {
        check("# Heading\n\nBody", "Heading\n\nBody");
        check("> quoted **text**", "quoted **text**");
        check(
            "```\nlet x = 1;\nlet y = 2;\n```",
            "let x = 1;\\\nlet y = 2;",
        );
        check("![alt text](image.png)", "alt text");
        check("before\n\n---\n\nafter", "before\n\nafter");
        check("a <span>b</span>", "a \\<span\\>b\\</span\\>");
        check("<div>\nhi\n</div>", "\\<div\\>\\\nhi\\\n\\</div\\>");
        check("[bad](javascript:alert(1))", "bad");
        check("~~strike~~ | table |", "\\~\\~strike\\~\\~ | table |");
    }

    #[test]
    fn escapes_literal_markup() {
        check(r"not \*bold\*", r"not \*bold\*");
        check(r"\# not a heading", r"\# not a heading");
        check(r"2024\. was a year", r"2024\. was a year");
        check(r"a\\b and \[x\]", r"a\\b and \[x\]");
        check("&amp; &lt;", r"\& \<");
        check("line\\\n\\- dash", "line\\\n\\- dash");
    }

    #[test]
    fn plain_text_strips_markup() {
        assert_eq!(
            to_plain_text("Click **Save** in *the* [toolbar](https://x.y) with `⌘S`."),
            "Click Save in the toolbar with ⌘S."
        );
        assert_eq!(
            to_plain_text("# Title\n\n- a\n- b\n\n> c"),
            "Title\na\nb\nc"
        );
        assert_eq!(to_plain_text("one\\\ntwo"), "one\ntwo");
    }

    #[track_caller]
    fn stable(input: &str) -> String {
        let once = normalize(input);
        assert_eq!(normalize(&once), once, "not a fixed point for {input:?}");
        once
    }

    #[test]
    fn rejects_hidden_schemes() {
        for input in [
            "[x](java&#9;script:alert(1))",
            "[x](java&#10;script:alert(1))",
            "[x](&#1;javascript:alert(1))",
            "[x](&#0;javascript:alert(1))",
            "[x](<java\tscript:alert(1)>)",
            "[x](< javascript:alert(1)>)",
            "[x](JavaScript:alert(1))",
            "[x](%6Aavascript:alert(1))",
            "[x](<a b.png>)",
            "<javascript:alert(1)>",
        ] {
            let out = stable(input);
            assert!(!out.contains("]("), "link kept for {input:?}: {out:?}");
        }
        check("[x](< https://example.com >)", "[x](https://example.com)");
    }

    #[test]
    fn rejects_protocol_relative_urls() {
        for input in [
            "[x](//evil.example/share)",
            r"[x](\\\\evil.example\\share)",
            r"[x](/\\evil.example)",
            r"[x](\\/evil.example)",
            "<//evil.example>",
        ] {
            let out = stable(input);
            assert!(!out.contains("]("), "link kept for {input:?}: {out:?}");
        }
        check("[x](/docs/a)", "[x](/docs/a)");
        check("[x](docs//a)", "[x](docs//a)");
        check("[x](/)", "[x](/)");
    }

    #[test]
    fn list_after_paragraph_that_cannot_interrupt() {
        check("Intro:\n\n3. a\n4. b", "Intro:\n\n3. a\n4. b");
        check("Intro:\n\n-\n- b", "Intro:\n\n-\n- b");
        check("- item\n\n  2. sub", "- item\n\n  2. sub");
        check("Intro:\n1. a", "Intro:\n1. a");
    }

    #[test]
    fn blank_lines_in_literal_blocks() {
        check("```\na\n\nb\n```", "a\\\nb");
        check("```\na\n   \n\n b\n```", "a\\\nb");
        check("<div>\n\n</div>", "\\<div\\>\n\n\\</div\\>");
    }

    #[test]
    fn bang_before_link() {
        check("Wow\\![link](u)", "Wow\\![link](u)");
        check("Wow!<https://e.x>", "Wow\\![https://e.x](https://e.x)");
        check("Wow![link](u)", "Wowlink");
    }

    #[test]
    fn adjacent_and_padded_emphasis() {
        check("*a*_b_", "*ab*");
        check("**a**__b__", "**ab**");
        check("**a *x***__*y* b__", "**a *xy* b**");
        stable("*a*__b__");
        stable("**a**_b_");
        check("**![](a.png) b**", "**b**");
        check("x **b ![](a.png)** y", "x **b**  y");
        check("*![](a.png)*", "");
    }
}
