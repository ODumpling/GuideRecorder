import type { Node as PMNode } from "@tiptap/pm/model";
import { serializeMarkdown } from "./markdown.ts";

/**
 * Splits an editor document at a position into the Markdown before and
 * after it. Blocks cut in the middle (a paragraph, a list) continue on both
 * sides, so a list split between items stays a list in both halves. A cut
 * at the start or end of a block moves out to the block boundary, so
 * neither half gets an empty paragraph or list item.
 */
export function splitMarkdown(doc: PMNode, pos: number): [string, string] {
  let at = Math.max(0, Math.min(pos, doc.content.size));
  let $at = doc.resolve(at);
  while ($at.depth > 0 && $at.parentOffset === 0) {
    at = $at.before();
    $at = doc.resolve(at);
  }
  while ($at.depth > 0 && $at.parentOffset === $at.parent.content.size) {
    at = $at.after();
    $at = doc.resolve(at);
  }
  return [serializeMarkdown(doc.cut(0, at).toJSON()), serializeMarkdown(doc.cut(at).toJSON())];
}
