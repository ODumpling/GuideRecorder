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

/** Sets an item's title (steps and sections). */
export function setItemTitle(guide: Guide, id: string, title: string): Command {
  const item = findItem(guide, id);
  if (item.type === "note") throw new PatchError(`note ${id} has no title`);
  return {
    label: item.type === "step" ? "Edit step title" : "Edit section title",
    forward: [{ op: "setItem", id, key: "title", value: title }],
    inverse: [{ op: "setItem", id, key: "title", value: item.title }],
    coalesceKey: `item.${id}.title`,
  };
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
