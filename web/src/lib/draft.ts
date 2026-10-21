import type { Guide, GuideField, Item } from "@guiderecorder/guide-schema";
import { equal } from "@guiderecorder/sync";

// Edits the web editor makes to a guide, as the desktop editor makes them
// (app/src/editor/commands.ts): text the user writes is marked `edited`, so
// the app's AI passes leave it alone. v1 edits text only: no images,
// annotations or new steps.

/** The guide-level text fields the web editor edits. */
export type GuideTextField = Extract<GuideField, "title" | "intro" | "prerequisites">;

/** The item fields holding Markdown. */
export type ItemTextField = "title" | "description" | "body" | "question";

const GUIDE_FIELDS: readonly GuideField[] = ["title", "intro", "prerequisites", "summary", "tags"];

/**
 * Sets a guide-level field and marks it edited (in `editedFields`, and the
 * guide's `aiState` `edited`).
 */
export function setGuideText(guide: Guide, field: GuideTextField, value: string): Guide {
  if (guide[field] === value) return guide;
  // A guide saved before `editedFields` existed: an `edited` aiState covers every field.
  const before = guide.editedFields?.length ? guide.editedFields : guide.aiState?.status === "edited" ? GUIDE_FIELDS : [];
  const edited = new Set([...before, field]);
  return {
    ...guide,
    [field]: value,
    editedFields: GUIDE_FIELDS.filter((f) => edited.has(f)),
    aiState: { ...(guide.aiState ?? { status: "template" }), status: "edited" },
  };
}

/** Sets an item's Markdown field; a step's text is marked edited. */
export function setItemText(guide: Guide, id: string, field: ItemTextField, value: string): Guide {
  return {
    ...guide,
    items: guide.items.map((item): Item => {
      if (item.id !== id || !(field in item) || (item as unknown as Record<string, unknown>)[field] === value) return item;
      const changed = { ...item, [field]: value } as Item;
      return changed.type === "step" ? { ...changed, aiState: { ...changed.aiState, status: "edited" } } : changed;
    }),
  };
}

/** Moves an item up (-1) or down (+1). */
export function moveItem(guide: Guide, id: string, by: -1 | 1): Guide {
  const from = guide.items.findIndex((item) => item.id === id);
  const to = from + by;
  if (from < 0 || to < 0 || to >= guide.items.length) return guide;
  const items = [...guide.items];
  const [item] = items.splice(from, 1);
  items.splice(to, 0, item!);
  return { ...guide, items };
}

export function deleteItem(guide: Guide, id: string): Guide {
  return { ...guide, items: guide.items.filter((item) => item.id !== id) };
}

/** Item fields the web editor may change, by item type. */
const ITEM_TEXT: Record<Item["type"], string[]> = {
  step: ["title", "description", "aiState"],
  section: ["title"],
  note: ["body"],
  // The question only: options and their targets are edited in the app.
  decision: ["question"],
};

/** Guide fields the web editor may change. */
const GUIDE_TEXT = ["title", "intro", "prerequisites", "editedFields", "aiState", "updatedAt", "items"];

const without = (value: object, keys: string[]) => Object.fromEntries(Object.entries(value).filter(([k]) => !keys.includes(k)));

/**
 * Why `edited` isn't a text-only edit of `base`, or null if it is: only
 * text changed, and items were only reordered or deleted, never added. A
 * step's `aiState` may only change its status.
 */
export function textOnlyProblem(base: Guide, edited: Guide): string | null {
  if (!equal(without(base, GUIDE_TEXT), without(edited, GUIDE_TEXT))) return "Only the guide's title, introduction and prerequisites can change.";
  const before = new Map(base.items.map((item) => [item.id, item]));
  for (const item of edited.items) {
    const old = before.get(item.id);
    if (!old) return "Steps, sections and notes can't be added on the web.";
    if (old.type !== item.type || !equal(without(old, ITEM_TEXT[item.type]), without(item, ITEM_TEXT[item.type]))) {
      return "Only the text of steps, sections and notes can change.";
    }
    if (old.type === "step" && item.type === "step" && !equal(without(old.aiState, ["status"]), without(item.aiState, ["status"]))) {
      return "Only the text of steps, sections and notes can change.";
    }
  }
  return null;
}
