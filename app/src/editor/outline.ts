// The editor outline's keyboard, collapsed sections, ranges and
// screen-reader announcements (#41), apart from the DOM.

import type { Item } from "@guiderecorder/guide-schema";
import { markdownToPlainText } from "@guiderecorder/ui/markdown";

/**
 * The section heading `id` is under (null: before any section, or `id` is
 * itself a section).
 */
export function sectionOf(items: Item[], id: string): string | null {
  const index = items.findIndex((item) => item.id === id);
  if (index < 0 || items[index].type === "section") return null;
  for (let i = index - 1; i >= 0; i--) {
    if (items[i].type === "section") return items[i].id;
  }
  return null;
}

/** The items after section `id`, up to the next section. */
function sectionBody(items: Item[], id: string): Item[] {
  const start = items.findIndex((item) => item.id === id);
  if (start < 0 || items[start].type !== "section") return [];
  const next = items.findIndex((item, i) => i > start && item.type === "section");
  return items.slice(start + 1, next < 0 ? items.length : next);
}

/** The items shown: a collapsed section hides everything up to the next section. */
export function visibleItems(items: Item[], collapsed: ReadonlySet<string>): Item[] {
  let hiding = false;
  return items.filter((item) => {
    if (item.type === "section") {
      hiding = collapsed.has(item.id);
      return true;
    }
    return !hiding;
  });
}

/** How many steps a section holds. */
export function sectionStepCount(items: Item[], id: string): number {
  return sectionBody(items, id).filter((item) => item.type === "step").length;
}

/**
 * `ids` with the items of collapsed sections among them: a folded section
 * moves as one with what it hides. In guide order.
 */
export function withCollapsed(items: Item[], ids: string[], collapsed: ReadonlySet<string>): string[] {
  const all = new Set(ids);
  for (const id of ids) {
    if (collapsed.has(id)) for (const item of sectionBody(items, id)) all.add(item.id);
  }
  return items.filter((item) => all.has(item.id)).map((item) => item.id);
}

/** The ids from `anchor` to `focus`, both included, in guide order ([] if either is gone). */
export function rangeIds(items: Item[], anchor: string, focus: string): string[] {
  const a = items.findIndex((item) => item.id === anchor);
  const b = items.findIndex((item) => item.id === focus);
  if (a < 0 || b < 0) return [];
  return items.slice(Math.min(a, b), Math.max(a, b) + 1).map((item) => item.id);
}

/**
 * The `to` for `moveItems` that moves the consecutive items `ids` past the
 * shown item above (-1) or below (1) them; null at the start or end. Up, a
 * collapsed section is passed as a whole; down, the items go into it. A
 * whole section (a heading with everything up to the next one) passes
 * whole sections instead, so the items of other sections stay in them.
 */
export function shiftTarget(items: Item[], ids: string[], by: -1 | 1, collapsed: ReadonlySet<string> = new Set()): number | null {
  const start = items.findIndex((item) => ids.includes(item.id));
  if (start < 0) return null;
  const end = start + ids.length - 1;
  if (items[start].type === "section" && (end + 1 === items.length || items[end + 1].type === "section")) {
    if (by > 0) {
      const next = items[end + 1];
      return next ? blockDropTarget(items, ids, end + 2 + sectionBody(items, next.id).length) : null;
    }
    // Before the heading above, or past one item outside any section.
    const heading = items.slice(0, start).map((item) => item.type).lastIndexOf("section");
    const before = heading >= 0 ? heading : start - 1;
    return before < 0 ? null : blockDropTarget(items, ids, before);
  }
  const visible = visibleItems(items, collapsed);
  const first = visible.findIndex((item) => ids.includes(item.id));
  if (first < 0) return null;
  let last = first;
  while (last + 1 < visible.length && ids.includes(visible[last + 1].id)) last++;
  const past = visible[by < 0 ? first - 1 : last + 1];
  if (!past) return null;
  const at = items.indexOf(past);
  return blockDropTarget(items, ids, by < 0 ? at : at + 1);
}

/**
 * The `to` for `moveItems` when the items `ids` are dropped before position
 * `before` (0..items.length, where items.length means the end).
 */
export function blockDropTarget(items: Item[], ids: string[], before: number): number {
  return items.slice(0, before).filter((item) => !ids.includes(item.id)).length;
}

/** Whether `ids` are two or more steps in a row, which merge into one. */
export function canMergeRange(items: Item[], ids: string[]): boolean {
  if (ids.length < 2) return false;
  const first = items.findIndex((item) => item.id === ids[0]);
  return ids.every((id, i) => items[first + i]?.id === id && items[first + i].type === "step");
}

export type OutlineKey =
  | "previous"
  | "next"
  | "first"
  | "last"
  | "extendUp"
  | "extendDown"
  | "moveUp"
  | "moveDown"
  | "collapse"
  | "expand"
  | "edit"
  | "delete";

/**
 * Keys on a focused outline item: arrows select (Shift: a range), Option
 * with arrows moves it, Home and End go to the ends, Left and Right fold
 * sections, Enter edits and Delete deletes.
 */
export function outlineKeyFor(e: Pick<KeyboardEvent, "key" | "metaKey" | "ctrlKey" | "shiftKey" | "altKey">): OutlineKey | null {
  if (e.metaKey || e.ctrlKey) return null;
  const up = e.key === "ArrowUp";
  if (up || e.key === "ArrowDown") {
    if (e.altKey) return e.shiftKey ? null : up ? "moveUp" : "moveDown";
    if (e.shiftKey) return up ? "extendUp" : "extendDown";
    return up ? "previous" : "next";
  }
  if (e.altKey || e.shiftKey) return null;
  switch (e.key) {
    case "Home":
      return "first";
    case "End":
      return "last";
    case "ArrowLeft":
      return "collapse";
    case "ArrowRight":
      return "expand";
    case "Enter":
      return "edit";
    case "Delete":
    case "Backspace":
      return "delete";
    default:
      return null;
  }
}

/** What a screen reader hears when the selection changes: "Step 4 of 7 selected". */
export function selectionAnnouncement(items: Item[], selectedId: string | null, rangeSize: number, annotated: boolean): string {
  if (rangeSize > 1) return `${rangeSize} items selected`;
  if (selectedId === null) return "Guide details selected";
  const item = items.find((other) => other.id === selectedId);
  if (!item) return "";
  if (item.type === "step") {
    const steps = items.filter((other) => other.type === "step");
    return `${annotated ? "Page" : "Step"} ${steps.indexOf(item) + 1} of ${steps.length} selected`;
  }
  if (item.type === "section") {
    const title = markdownToPlainText(item.title).trim();
    return `Section ${title ? `"${title}" ` : ""}selected`;
  }
  return `${item.kind[0].toUpperCase()}${item.kind.slice(1)} selected`;
}

/** What a screen reader hears after a move: "Moved to position 3 of 9". */
export function moveAnnouncement(items: Item[], ids: string[]): string {
  const at = items.findIndex((item) => ids.includes(item.id));
  if (at < 0) return "";
  const what = ids.length === 1 ? "Moved" : `Moved ${ids.length} items`;
  return `${what} to position ${at + 1} of ${items.length}`;
}
