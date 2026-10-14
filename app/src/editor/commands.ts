import type { Guide, ImageShow, Item, Step } from "@/lib/types";

/**
 * A minimal change to a guide. Items are addressed by id, so a patch stays
 * valid when other items move. Every patch has an exact inverse.
 */
export type Patch =
  | { op: "setGuide"; key: GuideField; value: unknown }
  | { op: "setItem"; id: string; key: string; value: unknown }
  | { op: "moveItem"; id: string; to: number }
  | { op: "insertItem"; index: number; item: Item }
  | { op: "removeItem"; id: string };

/** Guide fields that commands may set (not `items`, which has item patches). */
export type GuideField = Exclude<keyof Guide, "items" | "id" | "schemaVersion">;

/** An undoable mutation: `forward` applies it, `inverse` reverts it. */
export interface Command {
  label: string;
  forward: Patch[];
  inverse: Patch[];
  /**
   * Consecutive commands with the same key (e.g. typing into one field)
   * merge into one undo step.
   */
  coalesceKey?: string;
}

export class PatchError extends Error {}

function indexOf(guide: Guide, id: string): number {
  const index = guide.items.findIndex((item) => item.id === id);
  if (index < 0) throw new PatchError(`item ${id} not found`);
  return index;
}

/** Applies one patch, returning a new guide (the input is not modified). */
export function applyPatch(guide: Guide, patch: Patch): Guide {
  switch (patch.op) {
    case "setGuide":
      return { ...guide, [patch.key]: patch.value };
    case "setItem": {
      const index = indexOf(guide, patch.id);
      const items = guide.items.slice();
      items[index] = { ...items[index], [patch.key]: patch.value } as Item;
      return { ...guide, items };
    }
    case "moveItem": {
      const from = indexOf(guide, patch.id);
      if (patch.to < 0 || patch.to >= guide.items.length) {
        throw new PatchError(`move target ${patch.to} out of range`);
      }
      const items = guide.items.slice();
      const [item] = items.splice(from, 1);
      items.splice(patch.to, 0, item);
      return { ...guide, items };
    }
    case "insertItem": {
      if (patch.index < 0 || patch.index > guide.items.length) {
        throw new PatchError(`insert index ${patch.index} out of range`);
      }
      const items = guide.items.slice();
      items.splice(patch.index, 0, patch.item);
      return { ...guide, items };
    }
    case "removeItem": {
      const index = indexOf(guide, patch.id);
      const items = guide.items.slice();
      items.splice(index, 1);
      return { ...guide, items };
    }
  }
}

export function applyPatches(guide: Guide, patches: Patch[]): Guide {
  return patches.reduce(applyPatch, guide);
}

// ---------------------------------------------------------------------------
// Command builders. Each reads the current guide to record the inverse.

function findItem(guide: Guide, id: string): Item {
  return guide.items[indexOf(guide, id)];
}

function findStep(guide: Guide, id: string): Step {
  const item = findItem(guide, id);
  if (item.type !== "step") throw new PatchError(`item ${id} is not a step`);
  return item;
}

export function renameGuide(guide: Guide, title: string): Command {
  return {
    label: "Rename guide",
    forward: [{ op: "setGuide", key: "title", value: title }],
    inverse: [{ op: "setGuide", key: "title", value: guide.title }],
    coalesceKey: "guide.title",
  };
}

/** The Markdown text fields of items. */
export type TextField = "title" | "description" | "body";

const TEXT_LABELS: Record<Item["type"], Partial<Record<TextField, string>>> = {
  step: { title: "Edit step title", description: "Edit step description" },
  section: { title: "Edit section title" },
  note: { body: "Edit note" },
};

/**
 * Sets a text field (Markdown): a step's title or description, a section's
 * title or a note's body. Editing a step's text marks it `edited`, so AI
 * passes no longer overwrite it.
 */
export function setItemText(guide: Guide, id: string, field: TextField, value: string): Command {
  const item = findItem(guide, id);
  const label = TEXT_LABELS[item.type][field];
  if (!label) throw new PatchError(`${item.type} ${id} has no ${field}`);
  const forward: Patch[] = [{ op: "setItem", id, key: field, value }];
  const inverse: Patch[] = [{ op: "setItem", id, key: field, value: (item as unknown as Record<TextField, string>)[field] }];
  if (item.type === "step") {
    // Always in `forward`: a coalesced command keeps only the last forward.
    forward.push({ op: "setItem", id, key: "aiState", value: { ...item.aiState, status: "edited" } });
    inverse.push({ op: "setItem", id, key: "aiState", value: item.aiState });
  }
  return { label, forward, inverse, coalesceKey: `item.${id}.${field}` };
}

/** Sets an item's title (steps and sections). */
export function setItemTitle(guide: Guide, id: string, title: string): Command {
  return setItemText(guide, id, "title", title);
}

/** Moves an item to `to` (its index after the move). */
export function moveItem(guide: Guide, id: string, to: number): Command {
  const from = indexOf(guide, id);
  return {
    label: "Move item",
    forward: [{ op: "moveItem", id, to }],
    inverse: [{ op: "moveItem", id, to: from }],
  };
}

/**
 * The `to` for {@link moveItem} when the item at `from` is dropped before
 * position `before` (0..items.length, where items.length means the end).
 */
export function dropTarget(from: number, before: number): number {
  return from < before ? before - 1 : before;
}

export function setImageShow(guide: Guide, id: string, show: ImageShow): Command {
  const step = findStep(guide, id);
  return {
    label: show === "crop" ? "Show click crop" : "Show full screenshot",
    forward: [{ op: "setItem", id, key: "image", value: { ...step.image, show } }],
    inverse: [{ op: "setItem", id, key: "image", value: step.image }],
  };
}

/**
 * Merges `next` into `prev` when they edit the same thing: the result goes
 * from before `prev` to after `next`.
 */
export function coalesce(prev: Command, next: Command): Command | null {
  if (!prev.coalesceKey || prev.coalesceKey !== next.coalesceKey) return null;
  return { ...next, inverse: prev.inverse };
}
