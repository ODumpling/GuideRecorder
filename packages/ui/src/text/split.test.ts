import { getSchema } from "@tiptap/core";
import { describe, expect, it } from "vitest";
import { textExtensions } from "./extensions.ts";
import { parseMarkdown } from "./markdown.ts";
import { splitMarkdown } from "./split.ts";

const schema = getSchema(textExtensions());
const doc = (markdown: string) => schema.nodeFromJSON(parseMarkdown(markdown));

/** The position just before the first occurrence of `text`. */
function before(markdown: string, text: string): number {
  let found = -1;
  doc(markdown).descendants((node, pos) => {
    if (found < 0 && node.isText && node.text!.includes(text)) found = pos + node.text!.indexOf(text);
  });
  return found;
}

describe("splitMarkdown", () => {
  it("splits inside a paragraph, keeping marks on both sides", () => {
    const md = "Click **Save now** to finish.";
    expect(splitMarkdown(doc(md), before(md, "now"))).toEqual(["Click **Save**", "**now** to finish."]);
  });

  it("splits between paragraphs and inside lists", () => {
    const md = "One.\n\nTwo.";
    expect(splitMarkdown(doc(md), before(md, "Two"))).toEqual(["One.", "Two."]);
    const list = "- a\n- b\n- c";
    expect(splitMarkdown(doc(list), before(list, "b"))).toEqual(["- a", "- b\n- c"]);
    // Right after an item: the next item starts the second half.
    expect(splitMarkdown(doc(list), before(list, "b") - 3)).toEqual(["- a", "- b\n- c"]);
    const mixed = "Intro.\n\n1. one\n2. two";
    // A numbered list split in two restarts at 1 in the new step.
    expect(splitMarkdown(doc(mixed), before(mixed, "two"))).toEqual(["Intro.\n1. one", "1. two"]);
  });

  it("handles the ends and out-of-range positions", () => {
    const md = "Text";
    const d = doc(md);
    expect(splitMarkdown(d, 0)).toEqual(["", "Text"]);
    expect(splitMarkdown(d, d.content.size)).toEqual(["Text", ""]);
    expect(splitMarkdown(d, 999)).toEqual(["Text", ""]);
    expect(splitMarkdown(doc(""), 1)).toEqual(["", ""]);
  });
});
