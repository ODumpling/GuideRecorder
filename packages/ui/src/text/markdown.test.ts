import { getSchema, type JSONContent } from "@tiptap/core";
import { describe, expect, it } from "vitest";
import { textExtensions } from "./extensions.ts";
import golden from "../../../../crates/guiderecorder-core/tests/golden/markdown_normalize.golden?raw";
import { isAllowedUrl, linkFromInput, normalizeMarkdown, parseMarkdown, serializeMarkdown } from "./markdown.ts";

const multiLine = getSchema(textExtensions());
const singleLine = getSchema(textExtensions({ singleLine: true }));

/** Markdown → editor document (checked against the schema) → Markdown. */
function roundTrip(input: string, single = false): string {
  const schema = single ? singleLine : multiLine;
  const doc = schema.nodeFromJSON(parseMarkdown(input, { singleLine: single }));
  doc.check();
  return serializeMarkdown(doc.toJSON());
}

/** Text the editor writes is a fixed point of core's normalize. */
function serialize(content: JSONContent[]): string {
  const doc = multiLine.nodeFromJSON({ type: "doc", content });
  doc.check();
  const text = serializeMarkdown(doc.toJSON());
  expect(normalizeMarkdown(text), `not normalized: ${JSON.stringify(text)}`).toBe(text);
  return text;
}

/** The texts in the golden file, by name. */
function goldenTexts(): { name: string; text: string }[] {
  return golden
    .split(/^=== (.*)\n/m)
    .slice(1)
    .reduce<{ name: string; text: string }[]>((entries, part, i, parts) => {
      if (i % 2 === 0) entries.push({ name: part, text: parts[i + 1].replace(/\n$/, "") });
      return entries;
    }, []);
}

const text = (value: string, marks?: JSONContent["marks"]): JSONContent =>
  marks ? { type: "text", text: value, marks } : { type: "text", text: value };
const p = (...content: JSONContent[]): JSONContent => ({ type: "paragraph", content });
const bold = { type: "bold" };
const italic = { type: "italic" };
const code = { type: "code" };
const link = (href: string) => ({ type: "link", attrs: { href } });

describe("round trip through the editor", () => {
  const texts = goldenTexts();

  it("reads core's golden file", () => {
    expect(texts.length).toBeGreaterThan(20);
  });

  it.each(texts)("keeps core-normalized text: $name", ({ text }) => {
    expect(roundTrip(text)).toBe(text);
  });

  // From core's markdown tests (supported_constructs_round_trip and others).
  it.each([
    "",
    "Plain paragraph.",
    "Click **Save** in the toolbar.",
    "An *italic* word and `inline code`.",
    "***both***",
    'See [the docs](https://example.com/docs "Docs").',
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
    "[x](<a(b).png>)",
    "Use ``a`b`` here.",
    "``` `` ```",
    "Intro:\n\n3. a\n4. b",
    "Intro:\n\n-\n- b",
    "- item\n\n  2. sub",
    "Wow\\![link](u)",
    String.raw`not \*bold\*`,
    String.raw`\# not a heading`,
    String.raw`2024\. was a year`,
    String.raw`a\\b and \[x\]`,
    "line\\\n\\- dash",
    // Spans that open together nest by how far they run.
    "Click **[Settings](https://x.y)**",
    "*[Docs](https://x.y) and more*",
    "[Docs *and more*](https://x.y)",
    "**[a](https://x.y) b** c",
    "Open *[the **menu**](https://x.y) first*",
  ])("keeps %j", (input) => {
    expect(roundTrip(input)).toBe(input);
  });

  // Core's normalizes_equivalent_syntax and unsupported_constructs_degrade_to_text.
  it.each([
    ["Click __Save__ or _Cancel_.", "Click **Save** or *Cancel*."],
    ["* a\n* b", "- a\n- b"],
    ["+ a\n\n+ b", "- a\n- b"],
    ["soft\nbreak", "soft break"],
    ["trailing  \nspaces", "trailing\\\nspaces"],
    ["<https://example.com>", "[https://example.com](https://example.com)"],
    ["<me@example.com>", "[me@example.com](mailto:me@example.com)"],
    ["[ref]\n\n[ref]: https://example.com", "[ref](https://example.com)"],
    ["**outer **inner** outer**", "**outer inner outer**"],
    ["Use `` a`b `` here.", "Use ``a`b`` here."],
    ["# Heading\n\nBody", "Heading\n\nBody"],
    ["> quoted **text**", "quoted **text**"],
    ["```\nlet x = 1;\nlet y = 2;\n```", "let x = 1;\\\nlet y = 2;"],
    ["![alt text](image.png)", "alt text"],
    ["before\n\n---\n\nafter", "before\n\nafter"],
    ["a <span>b</span>", "a \\<span\\>b\\</span\\>"],
    ["<div>\nhi\n</div>", "\\<div\\>\\\nhi\\\n\\</div\\>"],
    ["[bad](javascript:alert(1))", "bad"],
    ["~~strike~~ | table |", "\\~\\~strike\\~\\~ | table |"],
    ["&amp; &lt;", "\\& \\<"],
    ["```\na\n\nb\n```", "a\\\nb"],
    ["Wow!<https://e.x>", "Wow\\![https://e.x](https://e.x)"],
    ["Wow![link](u)", "Wowlink"],
    ["*a*_b_", "*ab*"],
    ["**a *x***__*y* b__", "**a *xy* b**"],
    ["[x](< https://example.com >)", "[x](https://example.com)"],
  ])("normalizes %j like core", (input, expected) => {
    expect(roundTrip(input)).toBe(expected);
    expect(normalizeMarkdown(input)).toBe(expected);
  });

  it.each([
    "[x](java&#9;script:alert(1))",
    "[x](JavaScript:alert(1))",
    "[x](%6Aavascript:alert(1))",
    "<javascript:alert(1)>",
    "[x](//evil.example/share)",
    String.raw`[x](/\\evil.example)`,
  ])("drops unsafe link %j", (input) => {
    expect(roundTrip(input)).not.toContain("](");
  });

  // ProseMirror stores marks flat, so these core spellings cannot survive.
  it.each([
    // Adjacent links with the same target are one link in the editor.
    ["[Save](https://x.y)[ now](https://x.y)", "[Save now](https://x.y)"],
    // A link around exactly the same bold text: bold goes outside.
    ["[**Settings**](https://x.y)", "**[Settings](https://x.y)**"],
  ])("known limit: %j becomes %j", (input, expected) => {
    expect(normalizeMarkdown(input)).toBe(input);
    expect(roundTrip(input)).toBe(expected);
  });

  it("reads CR LF and lone CR as line endings", () => {
    expect(roundTrip("a\r\nb\r\n\r\nc")).toBe("a b\n\nc");
    expect(roundTrip("a\rb")).toBe("a b");
  });

  it("keeps Unicode separators as text, like core, but shows them as spaces", () => {
    expect(normalizeMarkdown("a\u2028b\u2029c")).toBe("a\u2028b\u2029c");
    expect(roundTrip("a\u2028b\u2029c")).toBe("a b c");
    // Core's structure: a separator never starts a block quote or a list.
    expect(normalizeMarkdown("a\u2028> b")).toBe("a\u2028\\> b");
    expect(roundTrip("a\u2028> b")).toBe("a \\> b");
    expect(roundTrip("\u2028- x")).toBe("\\- x");
    // Typed or pasted separators are text too; core keeps them.
    expect(serialize([p(text("a\u2028b"))])).toBe("a\u2028b");
  });

  it("flattens titles to one line", () => {
    expect(roundTrip("Click **Save**", true)).toBe("Click **Save**");
    expect(roundTrip("one\\\ntwo\n\n- three", true)).toBe("one two three");
    expect(singleLine.nodeFromJSON(parseMarkdown("")).toJSON()).toEqual({
      type: "doc",
      content: [{ type: "paragraph" }],
    });
  });
});

describe("serializing editor documents", () => {
  it("writes marks as core does", () => {
    expect(serialize([p(text("Click "), text("Save", [bold]), text(" now"))])).toBe("Click **Save** now");
    expect(serialize([p(text("both", [bold, italic]))])).toBe("***both***");
    expect(serialize([p(text("a "), text("b", [italic]), text(" "), text("c", [code]))])).toBe("a *b* `c`");
    expect(serialize([p(text("docs", [link("https://example.com")]))])).toBe("[docs](https://example.com)");
  });

  it("keeps one link around differently marked text", () => {
    const href = link("https://e.x");
    expect(serialize([p(text("a ", [href]), text("b", [href, bold]), text(" c", [href]))])).toBe(
      "[a **b** c](https://e.x)",
    );
    expect(serialize([p(text("x", [bold]), text("y", [bold, href]))])).toBe("**x[y](https://e.x)**");
  });

  it("moves spaces out of marks", () => {
    expect(serialize([p(text("a"), text(" b ", [bold]), text("c"))])).toBe("a **b** c");
    // A bold space alone would be dropped by the renderer, losing the space.
    expect(serialize([p(text("a"), text(" ", [bold]), text("b"))])).toBe("a b");
  });

  it("escapes literal markup and line starts", () => {
    expect(serialize([p(text("2 * 3 = 6 and a_b [c] <d> & ~e~ `f`"))])).toBe(
      String.raw`2 \* 3 = 6 and a\_b \[c\] \<d\> \& \~e\~ \`f\``,
    );
    expect(serialize([p(text("# not a heading"))])).toBe(String.raw`\# not a heading`);
    expect(serialize([p(text("- not a list"))])).toBe(String.raw`\- not a list`);
    expect(serialize([p(text("1. not a list"))])).toBe(String.raw`1\. not a list`);
  });

  it("writes hard breaks, code with backticks and empty paragraphs", () => {
    expect(serialize([p(text("one"), { type: "hardBreak" }, text("two"))])).toBe("one\\\ntwo");
    expect(serialize([p(text("one"), { type: "hardBreak" })])).toBe("one");
    expect(serialize([p(text("a`b", [code]))])).toBe("``a`b``");
    expect(serialize([p(text("a")), { type: "paragraph" }, p(text("b"))])).toBe("a\n\nb");
    expect(serialize([{ type: "paragraph" }])).toBe("");
  });

  it("writes lists", () => {
    const item = (...content: JSONContent[]): JSONContent => ({ type: "listItem", content });
    expect(
      serialize([
        p(text("Intro:")),
        { type: "bulletList", content: [item(p(text("a"))), item(p(text("b")))] },
        { type: "orderedList", attrs: { start: 3 }, content: [item(p(text("c"))), item(p(text("d")))] },
      ]),
    ).toBe("Intro:\n- a\n- b\n\n3. c\n4. d");
    expect(
      serialize([
        {
          type: "orderedList",
          attrs: { start: 1 },
          content: [item(p(text("x")), { type: "bulletList", content: [item(p(text("y")))] }), item(p())],
        },
      ]),
    ).toBe("1. x\n   - y\n2.");
    expect(
      serialize([
        { type: "bulletList", content: [item(p(text("a")))] },
        { type: "bulletList", content: [item(p(text("b")))] },
      ]),
    ).toBe("- a\n\n* b");
  });

  it("stays normalized when a mark cannot be written", () => {
    // `a**(b)**c` does not parse as bold, so the stars become literal text.
    expect(serialize([p(text("a"), text("(b)", [bold]), text("c"))])).toBe(String.raw`a\*\*(b)\*\*c`);
  });
});

describe("links", () => {
  it("follow core's rule for stored text", () => {
    for (const url of ["https://e.x", "HTTP://e.x", "mailto:a@b.c", "../a.md", "/docs", "#top"]) {
      expect(isAllowedUrl(url), url).toBe(true);
    }
    for (const url of ["", "javascript:x", "java\tscript:x", "a b", "//host", "\\\\host", "file:///etc", "data:x"]) {
      expect(isAllowedUrl(url), url).toBe(false);
    }
  });

  it("accept only http, https and mailto when inserted", () => {
    expect(linkFromInput(" https://example.com/a?b#c ")).toBe("https://example.com/a?b#c");
    expect(linkFromInput("http://localhost:8080")).toBe("http://localhost:8080");
    expect(linkFromInput("mailto:help@example.com")).toBe("mailto:help@example.com");
    expect(linkFromInput("example.com/docs")).toBe("https://example.com/docs");
    expect(linkFromInput("help@example.com")).toBe("mailto:help@example.com");
    for (const input of ["", "javascript:alert(1)", "file:///etc/passwd", "ftp://x.y", "../a.md", "https://", "mailto:", "https://a b.c", "word"]) {
      expect(linkFromInput(input), input).toBeNull();
    }
  });
});
