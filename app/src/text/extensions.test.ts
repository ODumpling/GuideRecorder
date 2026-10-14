import { getSchema } from "@tiptap/core";
import type { TagParseRule } from "@tiptap/pm/model";
import { describe, expect, it } from "vitest";
import { textExtensions } from "@/text/extensions";

const schema = getSchema(textExtensions());
const linkRule = schema.marks.link.spec.parseDOM![0] as TagParseRule;

/** Just enough of an `<a>` element for the parse rule. */
function anchor(attrs: Record<string, string>): HTMLElement {
  return { getAttribute: (name: string) => attrs[name] ?? null } as unknown as HTMLElement;
}

describe("link mark", () => {
  it("parses only href and title from pasted HTML", () => {
    const attrs = linkRule.getAttrs!(
      anchor({ href: "https://e.x", title: "T", target: "_blank", rel: "opener", class: "evil" }),
    );
    expect(attrs).toEqual({ href: "https://e.x", title: "T" });
  });

  it("refuses pasted links that are not http, https or mailto", () => {
    for (const href of ["javascript:alert(1)", "file:///etc/passwd", "../a.md", "data:text/html,x"]) {
      expect(linkRule.getAttrs!(anchor({ href })), href).toBe(false);
    }
  });

  it("never renders target, rel or class", () => {
    const mark = schema.marks.link.create({ href: "https://e.x", target: "_blank", rel: "opener", class: "x" });
    const [tag, attrs] = schema.marks.link.spec.toDOM!(mark, true) as [string, Record<string, unknown>];
    expect(tag).toBe("a");
    // ProseMirror does not set null attributes on the element.
    expect(attrs.target ?? null).toBeNull();
    expect(attrs.rel ?? null).toBeNull();
    expect(attrs.class ?? null).toBeNull();
    expect(attrs.href).toBe("https://e.x");
  });
});

describe("schema", () => {
  it("holds only the Markdown subset", () => {
    expect(Object.keys(schema.marks).sort()).toEqual(["bold", "code", "italic", "link"]);
    expect(Object.keys(schema.nodes).sort()).toEqual(
      ["bulletList", "doc", "hardBreak", "listItem", "orderedList", "paragraph", "text"].sort(),
    );
    const title = getSchema(textExtensions({ singleLine: true }));
    expect(Object.keys(title.nodes).sort()).toEqual(["doc", "paragraph", "text"]);
  });
});
