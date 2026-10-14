// Converts guide text (core's Markdown subset, see
// crates/guidesya-core/src/markdown.rs and design #444 "Text format") to and
// from TipTap documents.
//
// The serializer is a port of core's `normalize` renderer, so text written by
// the editor is already normalized: core leaves it unchanged. The parser uses
// markdown-it (CommonMark, like core's pulldown-cmark) and degrades
// unsupported constructs the way core does: headings and code blocks become
// paragraphs, block quotes are unwrapped, images become their alt text, raw
// HTML becomes literal text and rules are dropped.

import type { JSONContent } from "@tiptap/core";
import MarkdownIt, { type Token } from "markdown-it";

// ---------------------------------------------------------------------------
// The intermediate tree, mirroring core's `Block` and `Inline`.

type Block = { kind: "paragraph"; inlines: Inline[] } | { kind: "list"; start: number | null; items: Block[][] };

type Inline =
  | { kind: "text"; text: string }
  | { kind: "code"; text: string }
  | { kind: "strong"; children: Inline[] }
  | { kind: "emphasis"; children: Inline[] }
  | { kind: "link"; url: string; title: string; children: Inline[] }
  | { kind: "break" };

type Wrapper = Extract<Inline, { children: Inline[] }>;

const BREAK: Inline = { kind: "break" };

// ---------------------------------------------------------------------------
// URLs

/**
 * Core's rule for links kept in stored text: http, https, mailto and
 * scheme-less (relative) URLs, without whitespace, control characters or a
 * protocol-relative prefix.
 */
export function isAllowedUrl(url: string): boolean {
  if (url === "" || [...url].some((c) => isControl(c) || isWhitespace(c))) return false;
  if (/^[/\\][/\\]/.test(url)) return false;
  const i = url.search(/[:/?#]/);
  if (i >= 0 && url[i] === ":") {
    const scheme = url.slice(0, i).toLowerCase();
    return scheme === "http" || scheme === "https" || scheme === "mailto";
  }
  return true;
}

/**
 * Validates a URL typed into the link dialog: only absolute http, https and
 * mailto links. A bare address gets `mailto:` and a bare host gets
 * `https://`. Returns null if the URL is not allowed.
 */
export function linkFromInput(input: string): string | null {
  let url = input.trim();
  if (!url) return null;
  if (!/^[a-z][a-z0-9+.-]*:/i.test(url)) {
    if (/^[^\s@/]+@[^\s@/]+\.[^\s@/]+$/.test(url)) url = `mailto:${url}`;
    else if (/^[^\s/:?#]+\.[^\s/:?#]+/.test(url)) url = `https://${url}`;
    else return null;
  }
  const scheme = url.slice(0, url.indexOf(":")).toLowerCase();
  if (!["http", "https", "mailto"].includes(scheme) || !isAllowedUrl(url)) return null;
  if (scheme === "mailto" ? url.length <= "mailto:".length : !/^https?:\/\/[^/?#]+/i.test(url)) return null;
  return url;
}

// ---------------------------------------------------------------------------
// Character classes, matching Rust's `char` methods.

function isControl(c: string): boolean {
  return c.charCodeAt(0) < 0x20 || c === "\x7f";
}

/** Rust's `char::is_whitespace` (the Unicode White_Space property). */
function isWhitespace(c: string): boolean {
  return /[\t\n\v\f\r \u0085\u00a0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000]/.test(c);
}

function trimStartWith(s: string, pred: (c: string) => boolean): string {
  let i = 0;
  while (i < s.length && pred(s[i])) i++;
  return s.slice(i);
}

function trimEndWith(s: string, pred: (c: string) => boolean): string {
  let i = s.length;
  while (i > 0 && pred(s[i - 1])) i--;
  return s.slice(0, i);
}

const trim = (s: string) => trimEndWith(trimStartWith(s, isWhitespace), isWhitespace);

// ---------------------------------------------------------------------------
// Parsing Markdown (markdown-it tokens → tree)

const md = new MarkdownIt("commonmark", { html: true });
// Keep URLs as written: core does not percent-encode, and it filters schemes
// itself (markdown-it would leave a rejected link as literal text).
md.normalizeLink = (url) => url;
md.normalizeLinkText = (url) => url;
md.validateLink = () => true;

type Frame =
  | { kind: "blocks"; blocks: Block[]; inlines: Inline[] }
  | { kind: "list"; start: number | null; items: Block[][] }
  | { kind: "paragraph"; inlines: Inline[] }
  | { kind: "strong" | "emphasis" | "image"; children: Inline[] }
  | { kind: "link"; url: string | null; title: string; children: Inline[] };

class Builder {
  stack: Frame[] = [{ kind: "blocks", blocks: [], inlines: [] }];

  finish(): Block[] {
    const root = this.stack.pop();
    if (root?.kind !== "blocks" || this.stack.length) throw new Error("unbalanced markdown tokens");
    pushParagraph(root.blocks, root.inlines);
    return root.blocks;
  }

  block(token: Token) {
    switch (token.type) {
      case "paragraph_open":
      case "heading_open":
        this.flushInlines();
        this.stack.push({ kind: "paragraph", inlines: [] });
        break;
      case "paragraph_close":
      case "heading_close":
        this.endParagraph();
        break;
      case "inline":
        for (const child of token.children ?? []) this.inlineToken(child);
        break;
      case "code_block":
      case "fence":
      case "html_block":
        this.flushInlines();
        this.stack.push({ kind: "paragraph", inlines: [] });
        token.content
          .replace(/\n$/, "")
          .split("\n")
          .forEach((line, i) => {
            if (i > 0) this.inline(BREAK);
            if (trim(line)) this.inline({ kind: "text", text: line });
          });
        this.endParagraph();
        break;
      case "bullet_list_open":
      case "ordered_list_open": {
        this.flushInlines();
        const start = token.type === "ordered_list_open" ? Number(token.attrGet("start") ?? 1) : null;
        this.stack.push({ kind: "list", start, items: [] });
        break;
      }
      case "bullet_list_close":
      case "ordered_list_close": {
        const list = this.stack.pop();
        if (list?.kind !== "list") throw new Error("list frame");
        this.flushInlines();
        this.blocks().push({ kind: "list", start: list.start, items: list.items });
        break;
      }
      case "list_item_open":
        this.stack.push({ kind: "blocks", blocks: [], inlines: [] });
        break;
      case "list_item_close": {
        const item = this.stack.pop();
        const list = this.stack[this.stack.length - 1];
        if (item?.kind !== "blocks" || list?.kind !== "list") throw new Error("item frame");
        pushParagraph(item.blocks, item.inlines);
        list.items.push(item.blocks);
        break;
      }
      case "blockquote_open":
        this.flushInlines();
        break;
      // blockquote_close and hr produce nothing.
    }
  }

  inlineToken(token: Token) {
    switch (token.type) {
      case "text":
      case "html_inline":
        // markdown-it leaves empty text where delimiters were consumed.
        if (token.content) this.inline({ kind: "text", text: token.content });
        break;
      case "code_inline":
        this.inline({ kind: "code", text: token.content });
        break;
      case "softbreak":
        this.inline({ kind: "text", text: " " });
        break;
      case "hardbreak":
        this.inline(BREAK);
        break;
      case "strong_open":
      case "em_open":
        this.stack.push({ kind: token.type === "strong_open" ? "strong" : "emphasis", children: [] });
        break;
      case "strong_close":
      case "em_close": {
        const frame = this.stack.pop();
        if (frame?.kind !== "strong" && frame?.kind !== "emphasis") throw new Error("emphasis frame");
        this.inline({ kind: frame.kind, children: frame.children });
        break;
      }
      case "link_open": {
        const href = String(token.attrGet("href") ?? "");
        const url = trimStartWith(trimEndWith(href, (c) => c === " "), (c) => c === " ");
        this.stack.push({
          kind: "link",
          url: isAllowedUrl(url) ? url : null,
          title: String(token.attrGet("title") ?? ""),
          children: [],
        });
        break;
      }
      case "link_close": {
        const frame = this.stack.pop();
        if (frame?.kind !== "link") throw new Error("link frame");
        if (frame.url === null) frame.children.forEach((child) => this.inline(child));
        else this.inline({ kind: "link", url: frame.url, title: frame.title, children: frame.children });
        break;
      }
      case "image":
        // The alt text, with its markup.
        for (const child of token.children ?? []) this.inlineToken(child);
        break;
    }
  }

  inline(inline: Inline) {
    const top = this.stack[this.stack.length - 1];
    switch (top?.kind) {
      case "paragraph":
      case "blocks":
        top.inlines.push(inline);
        break;
      case "strong":
      case "emphasis":
      case "image":
      case "link":
        top.children.push(inline);
        break;
      default:
        throw new Error("inline outside a block");
    }
  }

  endParagraph() {
    const frame = this.stack.pop();
    if (frame?.kind !== "paragraph") throw new Error("paragraph frame");
    this.flushInlines();
    pushParagraph(this.blocks(), frame.inlines);
  }

  /** Turns pending tight-item text into a paragraph before a block starts. */
  flushInlines() {
    const top = this.stack[this.stack.length - 1];
    if (top?.kind === "blocks") pushParagraph(top.blocks, top.inlines.splice(0));
  }

  blocks(): Block[] {
    const top = this.stack[this.stack.length - 1];
    if (top?.kind !== "blocks") throw new Error("block outside a container");
    return top.blocks;
  }
}

/**
 * Line endings as core's parser sees them: CR LF and lone CR become LF.
 * Unicode line and paragraph separators become a line break and a blank
 * line, so they never reach the editor as invisible characters.
 */
function cleanLineEndings(input: string): string {
  return input.replace(/\r\n?/g, "\n").replace(/\u2028/g, "\n").replace(/\u2029/g, "\n\n");
}

function parseBlocks(input: string): Block[] {
  const builder = new Builder();
  for (const token of md.parse(cleanLineEndings(input), {})) builder.block(token);
  return builder.finish();
}

function pushParagraph(blocks: Block[], inlines: Inline[]) {
  if (inlines.some(hasContent)) blocks.push({ kind: "paragraph", inlines: mergeAdjacent(inlines) });
}

/** Joins directly adjacent spans of the same kind (`*a*_b_` → `*ab*`). */
function mergeAdjacent(inlines: Inline[]): Inline[] {
  const merged: Inline[] = [];
  for (const inline of inlines) {
    const last = merged[merged.length - 1];
    if (last && (inline.kind === "strong" || inline.kind === "emphasis") && last.kind === inline.kind) {
      merged[merged.length - 1] = { kind: last.kind, children: [...last.children, ...inline.children] };
    } else {
      merged.push(inline);
    }
  }
  return merged.map((inline) => ("children" in inline ? { ...inline, children: mergeAdjacent(inline.children) } : inline));
}

function hasContent(inline: Inline): boolean {
  switch (inline.kind) {
    case "text":
      return trim(inline.text) !== "";
    case "code":
    case "link":
      return true;
    case "strong":
    case "emphasis":
      return inline.children.some(hasContent);
    case "break":
      return false;
  }
}

// ---------------------------------------------------------------------------
// Tree ↔ TipTap JSON

type MarkJSON = NonNullable<JSONContent["marks"]>[number];

/** `marks` plus `mark`; nested bold in bold (which core flattens) adds nothing. */
function withMark(marks: MarkJSON[], mark: MarkJSON): MarkJSON[] {
  return marks.some((m) => m.type === mark.type) ? marks : [...marks, mark];
}

function inlinesToJSON(inlines: Inline[], marks: MarkJSON[], out: JSONContent[]) {
  for (const inline of inlines) {
    const withMarks = (node: JSONContent): JSONContent => (marks.length ? { ...node, marks } : node);
    switch (inline.kind) {
      case "text":
        if (inline.text) out.push(withMarks({ type: "text", text: inline.text }));
        break;
      case "code":
        if (inline.text) out.push({ type: "text", text: inline.text, marks: withMark(marks, { type: "code" }) });
        break;
      case "break":
        out.push(withMarks({ type: "hardBreak" }));
        break;
      case "strong":
        inlinesToJSON(inline.children, withMark(marks, { type: "bold" }), out);
        break;
      case "emphasis":
        inlinesToJSON(inline.children, withMark(marks, { type: "italic" }), out);
        break;
      case "link":
        // CommonMark has no links inside links, so this never replaces one.
        inlinesToJSON(
          inline.children,
          withMark(marks, { type: "link", attrs: { href: inline.url, title: inline.title || null } }),
          out,
        );
        break;
    }
  }
}

function paragraphJSON(inlines: Inline[]): JSONContent {
  const content: JSONContent[] = [];
  inlinesToJSON(trimBreaks(inlines), [], content);
  return content.length ? { type: "paragraph", content } : { type: "paragraph" };
}

function blocksToJSON(blocks: Block[]): JSONContent[] {
  return blocks.map((block) => {
    if (block.kind === "paragraph") return paragraphJSON(block.inlines);
    const items = block.items.map((item): JSONContent => {
      const content = blocksToJSON(item);
      // A list item starts with a paragraph in the editor schema.
      if (content[0]?.type !== "paragraph") content.unshift({ type: "paragraph" });
      return { type: "listItem", content };
    });
    return block.start === null
      ? { type: "bulletList", content: items }
      : { type: "orderedList", attrs: { start: block.start }, content: items };
  });
}

/** Text of a single-line field: paragraphs and list items joined by spaces. */
function flattenToLine(blocks: Block[]): Inline[] {
  const inlines: Inline[] = [];
  const add = (more: Inline[]) => {
    if (inlines.length && more.length) inlines.push({ kind: "text", text: " " });
    inlines.push(...more.map(function noBreaks(inline): Inline {
      if (inline.kind === "break") return { kind: "text", text: " " };
      return "children" in inline ? { ...inline, children: inline.children.map(noBreaks) } : inline;
    }));
  };
  const walk = (list: Block[]) =>
    list.forEach((block) => (block.kind === "paragraph" ? add(block.inlines) : block.items.forEach(walk)));
  walk(blocks);
  return inlines;
}

export interface ParseOptions {
  /** A single paragraph without line breaks (titles). */
  singleLine?: boolean;
}

/** Parses stored Markdown into a TipTap document. */
export function parseMarkdown(input: string, { singleLine = false }: ParseOptions = {}): JSONContent {
  const blocks = parseBlocks(input);
  const content = singleLine ? [paragraphJSON(flattenToLine(blocks))] : blocksToJSON(blocks);
  return { type: "doc", content: content.length ? content : [{ type: "paragraph" }] };
}

// Wrapper marks. Among marks that open and close together, the earlier one
// goes outside: `**[Settings](u)**` (core's usual spelling) stays as it is.
const MARK_RANK: Record<string, number> = { bold: 0, italic: 1, link: 2 };

function sameMark(a: MarkJSON, b: MarkJSON): boolean {
  return (
    a.type === b.type &&
    (a.type !== "link" || (a.attrs?.href === b.attrs?.href && (a.attrs?.title ?? null) === (b.attrs?.title ?? null)))
  );
}

function wrapperFor(mark: MarkJSON): Wrapper {
  switch (mark.type) {
    case "bold":
      return { kind: "strong", children: [] };
    case "italic":
      return { kind: "emphasis", children: [] };
    default:
      return { kind: "link", url: String(mark.attrs?.href ?? ""), title: String(mark.attrs?.title ?? ""), children: [] };
  }
}

/** Builds nested spans from ProseMirror's flat marked text. */
function inlinesFromJSON(nodes: JSONContent[]): Inline[] {
  const root: Inline[] = [];
  const open: { mark: MarkJSON; node: Wrapper }[] = [];
  const target = () => (open.length ? open[open.length - 1].node.children : root);
  const wrapperMarks = (node: JSONContent) => (node.marks ?? []).filter((m) => m.type in MARK_RANK);
  // How many nodes from `i` on carry `mark`.
  const extent = (mark: MarkJSON, i: number) => {
    let end = i;
    while (end < nodes.length && wrapperMarks(nodes[end]).some((m) => sameMark(m, mark))) end++;
    return end - i;
  };
  nodes.forEach((node, i) => {
    const marks = wrapperMarks(node);
    const isCode = node.marks?.some((m) => m.type === "code") ?? false;
    // Keep the open spans that still apply, close the rest, open new ones.
    let keep = 0;
    while (keep < open.length && marks.some((m) => sameMark(m, open[keep].mark))) keep++;
    open.length = keep;
    // A span that runs further goes outside: `*[Docs](u) and more*`.
    const added = marks
      .filter((m) => !open.some((o) => sameMark(o.mark, m)))
      .map((mark) => ({ mark, extent: extent(mark, i) }))
      .sort((a, b) => b.extent - a.extent || MARK_RANK[a.mark.type] - MARK_RANK[b.mark.type])
      .map(({ mark }) => mark);
    for (const mark of added) {
      const wrapper = wrapperFor(mark);
      target().push(wrapper);
      open.push({ mark, node: wrapper });
    }
    if (node.type === "hardBreak") target().push(BREAK);
    else if (node.type === "text" && node.text) target().push({ kind: isCode ? "code" : "text", text: node.text });
  });
  return unwrapEmpty(root);
}

/**
 * Unwraps bold or italic spans that hold only whitespace: the renderer drops
 * those, which would delete the space.
 */
function unwrapEmpty(inlines: Inline[]): Inline[] {
  return inlines.flatMap((inline): Inline[] => {
    if (!("children" in inline)) return [inline];
    const children = unwrapEmpty(inline.children);
    if (inline.kind !== "link" && !children.some(hasContent)) return children;
    return [{ ...inline, children }];
  });
}

function blocksFromJSON(nodes: JSONContent[]): Block[] {
  const blocks: Block[] = [];
  for (const node of nodes) {
    switch (node.type) {
      case "paragraph":
        pushParagraph(blocks, inlinesFromJSON(node.content ?? []));
        break;
      case "bulletList":
      case "orderedList":
        blocks.push({
          kind: "list",
          start: node.type === "orderedList" ? Number(node.attrs?.start ?? 1) : null,
          items: (node.content ?? []).map((item) => blocksFromJSON(item.content ?? [])),
        });
        break;
    }
  }
  return blocks;
}

// ---------------------------------------------------------------------------
// Rendering, a port of core's serializer.

class Out {
  s = "";
  push(text: string) {
    this.s += text;
  }
}

function renderBlocks(blocks: Block[], out: Out) {
  // Which marker the previous list used, so adjacent lists stay separate.
  let prevAlt: [boolean, boolean] | null = null;
  blocks.forEach((block, i) => {
    if (i > 0) {
      const tight =
        blocks[i - 1].kind === "paragraph" &&
        block.kind === "list" &&
        (block.start === null || block.start === 1) &&
        block.items.length > 0 &&
        block.items[0].length > 0;
      out.push(tight ? "\n" : "\n\n");
    }
    if (block.kind === "paragraph") {
      renderParagraph(block.inlines, out);
      prevAlt = null;
    } else {
      const ordered = block.start !== null;
      const alt = prevAlt !== null && prevAlt[0] === ordered && !prevAlt[1];
      renderList(block.start, block.items, alt, out);
      prevAlt = [ordered, alt];
    }
  });
}

function renderList(start: number | null, items: Block[][], alt: boolean, out: Out) {
  items.forEach((item, i) => {
    if (i > 0) out.push("\n");
    const marker = start === null ? `${alt ? "*" : "-"} ` : `${start + i}${alt ? ")" : "."} `;
    const body = new Out();
    renderBlocks(item, body);
    if (!body.s) {
      out.push(marker.trimEnd());
      return;
    }
    const indent = " ".repeat(marker.length);
    body.s.split("\n").forEach((line, j) => {
      if (j === 0) out.push(marker);
      else {
        out.push("\n");
        if (line) out.push(indent);
      }
      out.push(line);
    });
  });
}

function renderParagraph(inlines: Inline[], out: Out) {
  const text = new Out();
  renderInlines(trimBreaks(inlines), text, null);
  trim(text.s)
    .split("\\\n")
    .forEach((line, i) => {
      if (i > 0) out.push("\\\n");
      escapeLineStart(trimStartWith(line, isWhitespace), out);
    });
}

/** Drops leading, trailing and repeated hard breaks. */
function trimBreaks(inlines: Inline[]): Inline[] {
  const result: Inline[] = [];
  const dropEmptyTail = () => {
    while (result.length && !hasContent(result[result.length - 1])) result.pop();
  };
  for (const inline of inlines) {
    if (inline.kind === "break") {
      dropEmptyTail();
      if (!result.length) continue;
    }
    result.push(inline);
  }
  dropEmptyTail();
  return result;
}

/** Escapes text at the start of a line that would otherwise begin a block. */
function escapeLineStart(line: string, out: Out) {
  if (/^[#\-+=]/.test(line)) {
    out.push(`\\${line}`);
    return;
  }
  const digits = /^[0-9]+/.exec(line)?.[0].length ?? 0;
  if (digits > 0 && /^[.)]/.test(line.slice(digits))) {
    out.push(`${line.slice(0, digits)}\\${line.slice(digits)}`);
    return;
  }
  out.push(line);
}

/** `parent` is the enclosing emphasis delimiter; same-kind nesting is flattened. */
function renderInlines(inlines: Inline[], out: Out, parent: string | null) {
  for (const inline of inlines) {
    switch (inline.kind) {
      case "text":
        escapeText(inline.text, out);
        break;
      case "code":
        renderCode(inline.text, out);
        break;
      case "strong":
        renderDelimited(inline.children, "**", out, parent);
        break;
      case "emphasis":
        renderDelimited(inline.children, "*", out, parent);
        break;
      case "link":
        // `![` would start an image.
        if (out.s.endsWith("!")) out.s = `${out.s.slice(0, -1)}\\!`;
        out.push("[");
        if (inline.children.some(hasContent)) renderInlines(inline.children, out, parent);
        else escapeText(inline.url, out);
        out.push("](");
        renderUrl(inline.url, out);
        if (inline.title) out.push(` "${inline.title.replace(/["\\]/g, "\\$&")}"`);
        out.push(")");
        break;
      case "break":
        out.push("\\\n");
        break;
    }
  }
}

function renderDelimited(children: Inline[], delim: string, out: Out, parent: string | null) {
  if (!children.some(hasContent)) return;
  if (parent === delim) {
    renderInlines(children, out, parent);
    return;
  }
  const buffer = new Out();
  renderInlines(children, buffer, delim);
  const inner = buffer.s;
  // Delimiters next to whitespace or a line break don't parse as emphasis.
  const isSpace = (c: string) => isWhitespace(c) && c !== "\n";
  let core = inner;
  let lead = 0;
  for (;;) {
    let trimmed = trimStartWith(core, isSpace);
    if (trimmed.startsWith("\\\n")) trimmed = trimmed.slice(2);
    if (trimmed.length === core.length) break;
    lead += core.length - trimmed.length;
    core = trimmed;
  }
  let trail = core.length;
  for (;;) {
    let trimmed = trimEndWith(core.slice(0, trail), isSpace);
    if (trimmed.endsWith("\\\n")) trimmed = trimmed.slice(0, -2);
    if (trimmed.length === trail) break;
    trail = trimmed.length;
  }
  out.push(inner.slice(0, lead) + delim + core.slice(0, trail) + delim + core.slice(trail));
}

function renderCode(code: string, out: Out) {
  const longestRun = Math.max(0, ...code.split(/[^`]/).map((run) => run.length));
  const fence = "`".repeat(longestRun + 1);
  const pad = trim(code) !== "" && (/^[ `]/.test(code) || /[ `]$/.test(code));
  out.push(pad ? `${fence} ${code} ${fence}` : `${fence}${code}${fence}`);
}

function renderUrl(url: string, out: Out) {
  const angle = url === "" || /[ ()<>]/.test(url);
  const escaped = url.replace(angle ? /[\\<>]/g : /\\/g, "\\$&");
  out.push(angle ? `<${escaped}>` : escaped);
}

function escapeText(text: string, out: Out) {
  out.push(text.replace(/\n/g, " ").replace(/[\\`*_[\]<>&~]/g, "\\$&"));
}

// ---------------------------------------------------------------------------
// Public API

function render(blocks: Block[]): string {
  const out = new Out();
  renderBlocks(blocks, out);
  return out.s;
}

/**
 * Text with all markup removed (core's `to_plain_text`): blocks and list
 * items on separate lines.
 */
export function markdownToPlainText(input: string): string {
  const lines: string[] = [];
  const plain = (inlines: Inline[]): string =>
    inlines
      .map((inline) => {
        switch (inline.kind) {
          case "text":
          case "code":
            return inline.text;
          case "break":
            return "\n";
          default:
            return plain(inline.children);
        }
      })
      .join("");
  const walk = (blocks: Block[]) =>
    blocks.forEach((block) =>
      block.kind === "paragraph" ? lines.push(trim(plain(trimBreaks(block.inlines)))) : block.items.forEach(walk),
    );
  walk(parseBlocks(input));
  return lines.join("\n");
}

/** Core's `markdown::normalize`, for checks and tests. */
export function normalizeMarkdown(input: string): string {
  return render(parseBlocks(input));
}

/**
 * Serializes a TipTap document to normalized Markdown.
 *
 * A few span shapes that ProseMirror allows have no CommonMark spelling, for
 * example bold punctuation inside a word (`a**(b)**c` is not bold). Those
 * come out as core would store them after normalizing, so the result is
 * always a fixed point of `normalize`.
 */
export function serializeMarkdown(doc: JSONContent): string {
  let text = render(blocksFromJSON(doc.content ?? []));
  for (let i = 0; i < 3; i++) {
    const again = normalizeMarkdown(text);
    if (again === text) break;
    text = again;
  }
  return text;
}
