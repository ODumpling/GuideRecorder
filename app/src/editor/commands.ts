import type { Guide, GuideTextField, ImageShow, Item, NoteKind, Step, StepImage } from "@/lib/types";
import { ulid } from "@/lib/ulid";
import { normalizeMarkdown } from "@/text/markdown";

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
  /**
   * The item to select after `forward` and after `inverse` (null: none);
   * leaves the selection alone when absent.
   */
  select?: { forward?: string | null; inverse?: string | null };
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

export function findStep(guide: Guide, id: string): Step {
  const item = findItem(guide, id);
  if (item.type !== "step") throw new PatchError(`item ${id} is not a step`);
  return item;
}

export type { GuideTextField };

/** The guide-level fields the AI guide pass writes, in order. */
export const GUIDE_TEXT_FIELDS: readonly GuideTextField[] = ["title", "intro", "prerequisites", "summary", "tags"];

/**
 * The guide-level fields the user wrote by hand, which AI must not
 * overwrite. A guide saved before `editedFields` existed has only an
 * `edited` `aiState`, which covers every field.
 */
export function editedGuideFields(guide: Guide): GuideTextField[] {
  if (guide.editedFields?.length) return guide.editedFields;
  return guide.aiState?.status === "edited" ? [...GUIDE_TEXT_FIELDS] : [];
}

const GUIDE_LABELS: Record<GuideTextField, string> = {
  title: "Rename guide",
  intro: "Edit introduction",
  prerequisites: "Edit prerequisites",
  summary: "Edit summary",
  tags: "Edit tags",
};

/**
 * Sets a guide-level field. Like step text, this marks the field edited (in
 * `editedFields`, and the guide's `aiState` `edited`), so the AI guide pass
 * no longer overwrites it; it may still write the other fields.
 */
export function setGuideField<K extends GuideTextField>(guide: Guide, key: K, value: Guide[K]): Command {
  const field: Command = {
    label: GUIDE_LABELS[key],
    forward: [{ op: "setGuide", key, value }],
    inverse: [{ op: "setGuide", key, value: guide[key] }],
    coalesceKey: `guide.${key}`,
  };
  // Unchanged (e.g. retyping the title): a no-op the store refuses.
  if (JSON.stringify(value) === JSON.stringify(guide[key])) return field;
  const edited = new Set([...editedGuideFields(guide), key]);
  return {
    ...field,
    // Always in `forward`: a coalesced command keeps only the last forward.
    forward: [
      ...field.forward,
      { op: "setGuide", key: "editedFields", value: GUIDE_TEXT_FIELDS.filter((f) => edited.has(f)) },
      { op: "setGuide", key: "aiState", value: { ...(guide.aiState ?? { status: "template" }), status: "edited" } },
    ],
    inverse: [
      ...field.inverse,
      { op: "setGuide", key: "editedFields", value: guide.editedFields },
      { op: "setGuide", key: "aiState", value: guide.aiState },
    ],
  };
}

export function renameGuide(guide: Guide, title: string): Command {
  return setGuideField(guide, "title", title);
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

/** Tooltips for the step image choice. */
export const IMAGE_SHOW_HINTS: Record<ImageShow, string> = {
  crop: "Show only the area around the click in the guide",
  full: "Show the whole screenshot in the guide",
};

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
  const merged = { ...next, inverse: prev.inverse };
  // Typed, then deleted back: the text is as it was, so its AI state is too.
  const provenance = new Set(["aiState", "editedFields"]);
  const text = (patches: Patch[]) => JSON.stringify(patches.filter((p) => !("key" in p) || !provenance.has(p.key)));
  return text(merged.forward) === text(merged.inverse) ? { ...merged, forward: merged.inverse } : merged;
}

// ---------------------------------------------------------------------------
// Item operations (#351)

const ITEM_NAMES: Record<Item["type"], string> = { step: "step", section: "section", note: "note" };

/** Inserts `item` after the item `afterId`, or at the end when it is null. */
export function insertItemAfter(guide: Guide, item: Item, afterId: string | null): Command {
  const index = afterId === null ? guide.items.length : indexOf(guide, afterId) + 1;
  return insertItemAt(guide, item, index, afterId);
}

/** Inserts `item` before the item `beforeId`, or at the start when it is null. */
export function insertItemBefore(guide: Guide, item: Item, beforeId: string | null): Command {
  const index = beforeId === null ? 0 : indexOf(guide, beforeId);
  return insertItemAt(guide, item, index, beforeId);
}

/**
 * Inserts `items`, in order, after the item `afterId` (or at the end when
 * it is null) as one undo step; the first is selected.
 */
export function insertItemsAfter(guide: Guide, items: Item[], afterId: string | null, label: string): Command {
  if (items.length === 0) throw new PatchError("nothing to insert");
  const index = afterId === null ? guide.items.length : indexOf(guide, afterId) + 1;
  const ids = new Set(guide.items.map((item) => item.id));
  for (const item of items) {
    if (ids.has(item.id)) throw new PatchError(`item ${item.id} exists`);
    ids.add(item.id);
  }
  return {
    label,
    forward: items.map((item, i) => ({ op: "insertItem", index: index + i, item })),
    inverse: [...items].reverse().map((item) => ({ op: "removeItem", id: item.id })),
    select: { forward: items[0].id, inverse: afterId },
  };
}

function insertItemAt(guide: Guide, item: Item, index: number, previousSelection: string | null): Command {
  if (guide.items.some((other) => other.id === item.id)) throw new PatchError(`item ${item.id} exists`);
  return {
    label: `Insert ${item.type === "note" ? item.kind : ITEM_NAMES[item.type]}`,
    forward: [{ op: "insertItem", index, item }],
    inverse: [{ op: "removeItem", id: item.id }],
    select: { forward: item.id, inverse: previousSelection },
  };
}

export function newSection(id: string): Item {
  return { type: "section", id, title: "" };
}

export function newNote(id: string, kind: NoteKind): Item {
  return { type: "note", id, kind, body: "" };
}

/** Deletes an item; the next one (or else the previous one) is selected. */
export function deleteItem(guide: Guide, id: string): Command {
  const index = indexOf(guide, id);
  const item = guide.items[index];
  const neighbour = guide.items[index + 1] ?? guide.items[index - 1] ?? null;
  return {
    label: `Delete ${item.type === "note" ? item.kind : ITEM_NAMES[item.type]}`,
    forward: [{ op: "removeItem", id }],
    inverse: [{ op: "insertItem", index, item }],
    select: { forward: neighbour?.id ?? null, inverse: id },
  };
}

/** The step directly after `id`, if that item is a step. */
export function nextStep(guide: Guide, id: string): Step | null {
  const next = guide.items[indexOf(guide, id) + 1];
  return next?.type === "step" ? next : null;
}

/** Normalized Markdown blocks joined by blank lines, skipping empty ones. */
function joinBlocks(parts: string[]): string {
  return normalizeMarkdown(parts.filter((part) => part.trim()).join("\n\n"));
}

/**
 * Merges the step `id` with the step right after it:
 *
 * - the first step keeps its title, kind, element, point and end point,
 *   screenshot and annotations;
 * - the second step's title and description are appended to its
 *   description, as paragraphs;
 * - keystrokes are joined with a space (or whichever step has them);
 * - the text counts as `edited`;
 * - the second step goes, with its screenshot and annotations. Undo brings
 *   them back until the guide is closed; then its image files are deleted.
 */
export function mergeWithNext(guide: Guide, id: string): Command {
  const first = findStep(guide, id);
  const second = nextStep(guide, id);
  if (!second) throw new PatchError(`no step after ${id} to merge with`);
  const description = joinBlocks([first.description, second.title, second.description]);
  const keystrokes = [first.keystrokes, second.keystrokes].filter(Boolean).join(" ") || undefined;
  const forward: Patch[] = [
    { op: "setItem", id, key: "description", value: description },
    { op: "setItem", id, key: "aiState", value: { ...first.aiState, status: "edited" } },
  ];
  const inverse: Patch[] = [
    { op: "setItem", id, key: "aiState", value: first.aiState },
    { op: "setItem", id, key: "description", value: first.description },
  ];
  if (keystrokes !== first.keystrokes) {
    forward.push({ op: "setItem", id, key: "keystrokes", value: keystrokes });
    inverse.unshift({ op: "setItem", id, key: "keystrokes", value: first.keystrokes });
  }
  forward.push({ op: "removeItem", id: second.id });
  inverse.unshift({ op: "insertItem", index: indexOf(guide, second.id), item: second });
  return { label: "Merge steps", forward, inverse, select: { forward: id, inverse: id } };
}

/**
 * Splits a step in two at a point in its description: the step keeps its
 * title and `before`; a new manual step `newId` right after it gets `after`
 * and shows the same screenshot. Pass `image`, the step's images copied for
 * `newId` (`copyStepImages`), so the steps never share files; without it
 * they do.
 *
 * The new step gets a copy of every annotation (with new ids from
 * `makeId`): it shows the same screenshot, so it must keep the same
 * redactions and crop, or the export would reveal what was blurred.
 */
export function splitStep(
  guide: Guide,
  id: string,
  newId: string,
  before: string,
  after: string,
  image?: StepImage,
  makeId: () => string = ulid,
): Command {
  const step = findStep(guide, id);
  if (guide.items.some((item) => item.id === newId)) throw new PatchError(`item ${newId} exists`);
  const added: Step = {
    type: "step",
    id: newId,
    title: "",
    description: after,
    kind: "manual",
    image: image ?? { ...step.image },
    annotations: step.annotations.map((annotation) => ({ ...structuredClone(annotation), id: makeId() })),
    // Text the user wrote is theirs; an empty step is left to the AI.
    aiState: { status: after.trim() ? "edited" : "template" },
  };
  const forward: Patch[] = [{ op: "insertItem", index: indexOf(guide, id) + 1, item: added }];
  const inverse: Patch[] = [{ op: "removeItem", id: newId }];
  if (before !== step.description) {
    forward.unshift(
      { op: "setItem", id, key: "description", value: before },
      { op: "setItem", id, key: "aiState", value: { ...step.aiState, status: "edited" } },
    );
    inverse.push(
      { op: "setItem", id, key: "aiState", value: step.aiState },
      { op: "setItem", id, key: "description", value: step.description },
    );
  }
  return { label: "Split step", forward, inverse, select: { forward: newId, inverse: id } };
}
