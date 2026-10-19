import { Extension, type Extensions } from "@tiptap/core";
import Bold from "@tiptap/extension-bold";
import Code from "@tiptap/extension-code";
import Document from "@tiptap/extension-document";
import HardBreak from "@tiptap/extension-hard-break";
import Italic from "@tiptap/extension-italic";
import Link from "@tiptap/extension-link";
import { BulletList, ListItem, ListKeymap, OrderedList } from "@tiptap/extension-list";
import Paragraph from "@tiptap/extension-paragraph";
import Text from "@tiptap/extension-text";
import { Placeholder } from "@tiptap/extensions";
import { Fragment, type Node as PMNode, Slice } from "@tiptap/pm/model";
import { Plugin } from "@tiptap/pm/state";
import { linkFromInput } from "./markdown.ts";

export interface TextExtensionOptions {
  /** One paragraph, no line breaks or lists (titles). */
  singleLine?: boolean;
  placeholder?: string;
}

/** Joins pasted blocks into one line of inline content. */
function flattenSlice(slice: Slice): Slice {
  const inline: PMNode[] = [];
  slice.content.descendants((node) => {
    if (!node.isTextblock) return true;
    if (inline.length && node.childCount) inline.push(node.type.schema.text(" "));
    node.forEach((child) => inline.push(child.type.name === "hardBreak" ? node.type.schema.text(" ") : child));
    return false;
  });
  // Already inline content (no text blocks).
  if (!inline.length) return slice;
  return new Slice(Fragment.fromArray(inline), 0, 0);
}

/** An attribute that pasted HTML cannot set and that is never rendered. */
const pinned = { default: null, parseHTML: () => null, renderHTML: () => ({}) };

/**
 * Links keep only their href and title: a pasted `target`, `rel` or `class`
 * is dropped, so no link in the editor opens a new window.
 */
const PinnedLink = Link.extend({
  addAttributes() {
    return { ...this.parent?.(), target: pinned, rel: pinned, class: pinned };
  },
});

const SingleLinePaste = Extension.create({
  name: "singleLinePaste",
  addProseMirrorPlugins() {
    return [new Plugin({ props: { transformPasted: flattenSlice } })];
  },
});

/**
 * The editor schema for guide text: core's Markdown subset (bold, italic,
 * code, links, lists, paragraphs, hard breaks) and nothing else, so pasted
 * content loses every other kind of formatting. There is no TipTap history:
 * undo and redo go through the editor store.
 */
export function textExtensions({ singleLine = false, placeholder }: TextExtensionOptions = {}): Extensions {
  const extensions: Extensions = [
    singleLine ? Document.extend({ content: "paragraph" }) : Document,
    Paragraph,
    Text,
    Bold,
    Italic,
    // Core allows code inside bold, italic and links.
    Code.extend({ excludes: "" }),
    PinnedLink.configure({
      openOnClick: false,
      autolink: false,
      linkOnPaste: true,
      // Pasted or inserted links: absolute http, https and mailto only.
      isAllowedUri: (url) => linkFromInput(url) === url,
      // Overrides TipTap's default target="_blank" and rel; null attributes
      // are not rendered.
      HTMLAttributes: { target: null, rel: null, class: null },
    }),
  ];
  if (singleLine) extensions.push(SingleLinePaste);
  else extensions.push(HardBreak, BulletList, OrderedList, ListItem, ListKeymap);
  if (placeholder) extensions.push(Placeholder.configure({ placeholder }));
  return extensions;
}
