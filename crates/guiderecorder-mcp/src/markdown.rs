//! Guides as Markdown for MCP clients.
//!
//! Text is written from the export AST ([`guiderecorder_core::export::ast`]),
//! escaping every string, so guide text can never inject headings, step-id
//! anchors or links of its own. Items carry stable anchors: steps are
//! `### 3. Click **Save** {#step-<ulid>}`, sections `## Setup
//! {#section-<ulid>}` and notes `> **Tip** {#note-<ulid>}`.
//!
//! A step's hotspots follow its description as a numbered list (the
//! numbers drawn on its image), each with its bold title and description.
//! In an annotation guide the steps are uploaded pages: the guide says so
//! up front, and pages have no recorded action details.

use guiderecorder_core::export::ast::{Block, Inline};
use guiderecorder_core::export::{RenderGuide, RenderItem, RenderStep};
use guiderecorder_core::model::{ElementInfo, GuideKind, NoteKind, StepKind};

/// Extra facts about the guide that are not in [`RenderGuide`].
pub struct GuideMeta<'a> {
    pub tags: &'a [String],
    pub updated_at: String,
    pub read_only: bool,
}

/// The whole guide.
pub fn guide(render: &RenderGuide, meta: &GuideMeta<'_>) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# {} {{#guide-{}}}\n\n",
        escape_line(&render.title),
        render.id
    ));
    if !render.summary.trim().is_empty() {
        out.push_str(&format!("{}\n\n", escape_line(&render.summary)));
    }
    let mut facts = vec![
        format!("Guide id: {}", code(&render.id.to_string())),
        format!("Language: {}", escape_line(&render.language)),
        format!("Updated: {}", escape_line(&meta.updated_at)),
    ];
    if !meta.tags.is_empty() {
        let tags: Vec<String> = meta.tags.iter().map(|t| escape_line(t)).collect();
        facts.insert(1, format!("Tags: {}", tags.join(", ")));
    }
    if render.kind == GuideKind::Annotated {
        facts.insert(
            1,
            "Kind: annotation guide (each step is an uploaded page; numbered hotspots on its \
             image explain it)"
                .to_owned(),
        );
    }
    out.push_str(&facts.join("  \n"));
    out.push_str("\n\n");
    if meta.read_only {
        out.push_str(
            "*Saved by a newer GuideRecorder: shown read-only; fields this version does not know \
             are missing.*\n\n",
        );
    }
    if !render.intro.is_empty() {
        out.push_str("## Introduction\n\n");
        out.push_str(&blocks(&render.intro));
        out.push('\n');
    }
    if !render.prerequisites.is_empty() {
        out.push_str("## Prerequisites\n\n");
        out.push_str(&blocks(&render.prerequisites));
        out.push('\n');
    }
    for item in &render.items {
        match item {
            RenderItem::Step(step) => out.push_str(&self::step(step, render.kind)),
            RenderItem::Section(section) => out.push_str(&format!(
                "## {} {{#section-{}}}\n\n",
                escape_line(&section.title),
                section.id
            )),
            RenderItem::Note(note) => {
                let label = match note.kind {
                    NoteKind::Tip => "Tip",
                    NoteKind::Warning => "Warning",
                    NoteKind::Note => "Note",
                };
                out.push_str(&format!("> **{label}** {{#note-{}}}\n", note.id));
                let body = blocks(&note.body);
                if !body.is_empty() {
                    out.push_str(">\n");
                    out.push_str(&prefix_lines(body.trim_end(), "> ", ">"));
                    out.push('\n');
                }
                out.push('\n');
            }
        }
    }
    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}

/// One step: heading, description, hotspots and its recorded details (a
/// page of an annotation guide has none).
pub fn step(step: &RenderStep, kind: GuideKind) -> String {
    let mut out = format!(
        "### {}. {} {{#step-{}}}\n\n",
        step.number,
        inlines_line(&step.title),
        step.step.id
    );
    if !step.description.is_empty() {
        out.push_str(&blocks(&step.description));
        out.push('\n');
    }
    let hotspots = hotspots(step);
    if !hotspots.is_empty() {
        out.push_str(&hotspots);
        out.push_str("\n\n");
    }
    if kind == GuideKind::Annotated {
        return out;
    }
    let mut details = vec![format!("Action: {}", kind_name(step.step.kind))];
    if let Some(element) = step.step.element.as_ref().and_then(element_line) {
        details.push(format!("Element: {element}"));
    }
    if let Some(keys) = step.step.keystrokes.as_deref().filter(|k| !k.is_empty()) {
        details.push(format!("Keys: {}", code(keys)));
    }
    out.push_str(&details.join("  \n"));
    out.push_str("\n\n");
    out
}

/// `Hotspots:` and a numbered list: `1. **Title**`, then the description
/// lines as hard breaks. Empty if the step has none.
fn hotspots(step: &RenderStep) -> String {
    let items: Vec<String> = step
        .hotspots()
        .map(|(number, hotspot)| {
            let marker = format!("{number}. ");
            let mut lines = Vec::new();
            if !hotspot.title.trim().is_empty() {
                lines.push(format!("**{}**", escape_line(hotspot.title.trim())));
            }
            lines.extend(
                hotspot
                    .description
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(escape_line),
            );
            let indent = " ".repeat(marker.len());
            format!("{marker}{}", lines.join(&format!("\\\n{indent}")))
                .trim_end()
                .to_owned()
        })
        .collect();
    if items.is_empty() {
        return String::new();
    }
    format!("Hotspots:\n\n{}", items.join("\n"))
}

pub fn kind_name(kind: StepKind) -> &'static str {
    match kind {
        StepKind::Click => "click",
        StepKind::DoubleClick => "double click",
        StepKind::RightClick => "right click",
        StepKind::Type => "type",
        StepKind::Shortcut => "shortcut",
        StepKind::Drag => "drag",
        StepKind::Manual => "manual",
    }
}

/// `**Save** (AXButton) in Finder, window “Documents”`, or `None` if empty.
fn element_line(element: &ElementInfo) -> Option<String> {
    let nonempty = |s: &Option<String>| s.clone().filter(|s| !s.trim().is_empty());
    let mut parts = Vec::new();
    if let Some(label) = nonempty(&element.label) {
        parts.push(format!("**{}**", escape_line(&label)));
    }
    if let Some(role) = nonempty(&element.role) {
        parts.push(format!("({})", escape_line(&role)));
    }
    if let Some(app) = nonempty(&element.app) {
        parts.push(format!("in {}", escape_line(&app)));
    }
    if let Some(window) = nonempty(&element.window) {
        parts.push(format!("window “{}”", escape_line(&window)));
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

// ---------------------------------------------------------------------------
// AST writer

/// Blocks separated by blank lines, ending in a newline (empty if none).
fn blocks(blocks: &[Block]) -> String {
    let mut out = String::new();
    for block in blocks {
        out.push_str(&self::block(block));
        out.push_str("\n\n");
    }
    out.truncate(out.trim_end().len());
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

fn block(block: &Block) -> String {
    match block {
        Block::Paragraph(inlines) => {
            let mut writer = InlineWriter::new(false);
            writer.inlines(inlines);
            writer.out
        }
        Block::List { start, items } => {
            let mut lines = Vec::new();
            for (i, item) in items.iter().enumerate() {
                let marker = match start {
                    Some(n) => format!("{}. ", n.saturating_add(i as u64)),
                    None => "- ".to_owned(),
                };
                let body = blocks(item);
                let indent = " ".repeat(marker.len());
                let body = prefix_lines(body.trim_end(), &indent, "");
                // The first line takes the marker instead of the indent.
                let body = body.strip_prefix(&indent).unwrap_or(&body).to_owned();
                lines.push(format!("{marker}{body}"));
            }
            lines.join("\n")
        }
    }
}

/// Inlines on one line (for headings): breaks become spaces.
fn inlines_line(inlines: &[Inline]) -> String {
    let mut writer = InlineWriter::new(true);
    writer.inlines(inlines);
    writer.out
}

struct InlineWriter {
    out: String,
    /// Hard breaks become spaces.
    single_line: bool,
    /// The next text starts a line (block markers must be escaped).
    line_start: bool,
}

impl InlineWriter {
    fn new(single_line: bool) -> Self {
        Self {
            out: String::new(),
            single_line,
            line_start: true,
        }
    }

    fn inlines(&mut self, inlines: &[Inline]) {
        for inline in inlines {
            self.inline(inline);
        }
    }

    fn inline(&mut self, inline: &Inline) {
        match inline {
            Inline::Text(text) => {
                let text = escape(text, self.line_start, self.single_line);
                self.push(&text);
            }
            Inline::Code(text) => self.push(&code(text)),
            Inline::Strong(children) => self.wrap("**", children),
            Inline::Emphasis(children) => self.wrap("*", children),
            Inline::Link {
                url,
                title,
                children,
            } => {
                self.push("[");
                self.inlines(children);
                self.push("](<");
                self.push(&escape_chars(url, &['<', '>', '\\', '\n', '\r']));
                self.push(">");
                if !title.is_empty() {
                    self.push(" \"");
                    self.push(&escape_chars(title, &['"', '\\']));
                    self.push("\"");
                }
                self.push(")");
            }
            Inline::Break if self.single_line => self.push(" "),
            Inline::Break => {
                self.out.push_str("\\\n");
                self.line_start = true;
            }
        }
    }

    fn wrap(&mut self, marker: &str, children: &[Inline]) {
        self.push(marker);
        self.inlines(children);
        self.push(marker);
    }

    fn push(&mut self, text: &str) {
        if !text.is_empty() {
            self.out.push_str(text);
            self.line_start = false;
        }
    }
}

/// Plain text on one line (titles, labels), escaped.
pub fn escape_line(text: &str) -> String {
    escape(text, true, true)
}

/// Escapes Markdown punctuation in plain text. Newlines become spaces when
/// `single_line`, and hard breaks otherwise.
fn escape(text: &str, mut line_start: bool, single_line: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\r' || c == '\n' {
            if c == '\r' && chars.get(i + 1) == Some(&'\n') {
                i += 1;
            }
            if single_line {
                out.push(' ');
            } else {
                out.push_str("\\\n");
                line_start = true;
                i += 1;
                continue;
            }
            i += 1;
            continue;
        }
        if line_start {
            // Block markers only matter after leading spaces.
            if c == ' ' || c == '\t' {
                out.push(c);
                i += 1;
                continue;
            }
            if matches!(c, '-' | '+' | '=') {
                out.push('\\');
            } else if c.is_ascii_digit() {
                let digits = chars[i..].iter().take_while(|c| c.is_ascii_digit()).count();
                if matches!(chars.get(i + digits), Some('.' | ')')) {
                    out.extend(&chars[i..i + digits]);
                    out.push('\\');
                    out.push(chars[i + digits]);
                    i += digits + 1;
                    line_start = false;
                    continue;
                }
            }
            line_start = false;
        }
        if matches!(
            c,
            '\\' | '`'
                | '*'
                | '_'
                | '['
                | ']'
                | '<'
                | '>'
                | '{'
                | '}'
                | '|'
                | '#'
                | '~'
                | '&'
                | '!'
        ) {
            out.push('\\');
        }
        out.push(c);
        i += 1;
    }
    out
}

fn escape_chars(text: &str, special: &[char]) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\n' | '\r' if special.contains(&c) => out.push(' '),
            c if special.contains(&c) => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out
}

/// An inline code span holding `text` verbatim.
pub fn code(text: &str) -> String {
    let text = text.replace(['\r', '\n'], " ");
    let longest = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest + 1);
    let pad = text.starts_with('`')
        || text.ends_with('`')
        || (text.starts_with(' ') && text.ends_with(' ') && !text.trim().is_empty());
    if pad {
        format!("{fence} {text} {fence}")
    } else {
        format!("{fence}{text}{fence}")
    }
}

/// Prefixes every line; empty lines get `empty` instead.
fn prefix_lines(text: &str, prefix: &str, empty: &str) -> String {
    text.split('\n')
        .map(|line| {
            if line.is_empty() {
                empty.to_owned()
            } else {
                format!("{prefix}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use guiderecorder_core::export::ast::parse_blocks;

    fn md(source: &str) -> String {
        blocks(&parse_blocks(source))
    }

    #[test]
    fn escapes_text_that_looks_like_markup() {
        assert_eq!(
            escape_line("# Not *a* [heading]({#step-x})"),
            r"\# Not \*a\* \[heading\](\{\#step-x\})"
        );
        assert_eq!(escape_line("- item"), r"\- item");
        assert_eq!(escape_line("1. first"), r"1\. first");
        assert_eq!(escape_line("a\nb"), "a b");
        assert_eq!(escape_line("x <b>raw</b> & y"), r"x \<b\>raw\</b\> \& y");
    }

    #[test]
    fn writes_the_subset_back_as_markdown() {
        assert_eq!(md("Hello **you** and *me*"), "Hello **you** and *me*\n");
        assert_eq!(md("- a\n- b\n"), "- a\n- b\n");
        assert_eq!(md("3. a\n4. b\n"), "3. a\n4. b\n");
        assert_eq!(
            md("See [docs](https://example.com/a_b \"T\")"),
            "See [docs](<https://example.com/a_b> \"T\")\n"
        );
        assert_eq!(md("a  \nb"), "a\\\nb\n");
        assert_eq!(md("use `a``b`"), "use ```a``b```\n");
    }

    #[test]
    fn heading_text_stays_on_one_line() {
        let inlines = guiderecorder_core::export::ast::parse_inlines("Click **Save**\n\nthen # go");
        assert_eq!(inlines_line(&inlines), r"Click **Save** then \# go");
    }

    #[test]
    fn code_spans_survive_backticks() {
        assert_eq!(code("a"), "`a`");
        assert_eq!(code("`x`"), "`` `x` ``");
        assert_eq!(code("••••⏎"), "`••••⏎`");
    }
}
