import type {
  Annotation,
  BranchTarget,
  CoverOptions,
  Decision,
  DecisionOption,
  Guide,
  GuideField as GuideTextField,
  ImageShow,
  Item,
  NoteKind,
  Section,
  Step,
  StepImage,
} from "@guiderecorder/guide-schema";
import type { ReviewFields } from "@/lib/types";
import { ulid } from "@/lib/ulid";
import { normalizeMarkdown } from "@guiderecorder/ui/markdown";

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

/** The owner, status, version and review fields (`ReviewFields`). */
export type ReviewField = keyof ReviewFields;

const REVIEW_LABELS: Record<ReviewField, string> = {
  owner: "Edit owner",
  status: "Change status",
  version: "Edit version",
  lastReviewed: "Edit review date",
  reviewIntervalMonths: "Edit review interval",
};

/**
 * Sets an owner, status, version or review field. These are the user's
 * alone (AI never writes them), so nothing is marked edited. An empty
 * value (`""`, null, a 0 interval) unsets the field.
 */
export function setReviewField<K extends ReviewField>(guide: Guide, key: K, value: ReviewFields[K]): Command {
  const unset = value === "" || value === null || value === undefined || (key === "reviewIntervalMonths" && value === 0);
  return {
    label: REVIEW_LABELS[key],
    forward: [{ op: "setGuide", key, value: unset ? undefined : value }],
    inverse: [{ op: "setGuide", key, value: guide[key] }],
    coalesceKey: `guide.${key}`,
  };
}

export function renameGuide(guide: Guide, title: string): Command {
  return setGuideField(guide, "title", title);
}

/** The Markdown text fields of items. */
export type TextField = "title" | "description" | "body" | "question";

const TEXT_LABELS: Record<Item["type"], Partial<Record<TextField, string>>> = {
  step: { title: "Edit step title", description: "Edit step description" },
  section: { title: "Edit section title" },
  note: { body: "Edit note" },
  decision: { question: "Edit decision question" },
};

/**
 * Sets a text field (Markdown): a step's title or description, a section's
 * title, a note's body or a decision's question. Editing a step's text
 * marks it `edited`, so AI passes no longer overwrite it.
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

/** `moveItem` patches that turn the order `from` into `to` (the same ids). */
function reorderPatches(from: string[], to: string[]): Patch[] {
  const current = from.slice();
  const patches: Patch[] = [];
  to.forEach((id, index) => {
    const at = current.indexOf(id);
    if (at === index) return;
    current.splice(at, 1);
    current.splice(index, 0, id);
    patches.push({ op: "moveItem", id, to: index });
  });
  return patches;
}

/**
 * Moves the items `ids` together, in guide order, so the first of them ends
 * up at `to` (its index after the move). One undo step.
 */
export function moveItems(guide: Guide, ids: string[], to: number): Command {
  const moving = new Set(ids);
  const block = guide.items.filter((item) => moving.has(item.id));
  if (block.length === 0 || block.length !== moving.size) throw new PatchError("moving an item that is not in the guide");
  const rest = guide.items.filter((item) => !moving.has(item.id));
  if (to < 0 || to > rest.length) throw new PatchError(`move target ${to} out of range`);
  const before = guide.items.map((item) => item.id);
  const after = [...rest.slice(0, to), ...block, ...rest.slice(to)].map((item) => item.id);
  return {
    label: block.length === 1 ? "Move item" : `Move ${block.length} items`,
    forward: reorderPatches(before, after),
    inverse: reorderPatches(after, before),
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

// ---------------------------------------------------------------------------
// Cover (#44)

/** How a cover is drawn when `coverOptions` is absent (core `CoverOptions::default`). */
export const DEFAULT_COVER_OPTIONS: CoverOptions = { image: "full", annotations: false };

/** The step chosen as the cover, if it is still a step with a screenshot; else null (the first screenshot is the cover). */
export function coverStepId(guide: Guide): string | null {
  const id = guide.coverStepId;
  const step = id ? guide.items.find((item) => item.id === id) : undefined;
  return step?.type === "step" && step.image.full ? step.id : null;
}

export function coverOptions(guide: Guide): CoverOptions {
  return { ...DEFAULT_COVER_OPTIONS, ...guide.coverOptions };
}

/** Makes step `id` the guide's cover, or (null) goes back to the first screenshot. */
export function setCover(guide: Guide, id: string | null): Command {
  if (id !== null) findStep(guide, id);
  return {
    label: id === null ? "Remove cover" : "Set as cover",
    forward: [{ op: "setGuide", key: "coverStepId", value: id ?? undefined }],
    inverse: [{ op: "setGuide", key: "coverStepId", value: guide.coverStepId }],
  };
}

/** Changes how the cover is drawn: the close-up or the full screenshot, with or without annotations. */
export function setCoverOptions(guide: Guide, change: Partial<CoverOptions>): Command {
  return {
    label: "Change cover",
    forward: [{ op: "setGuide", key: "coverOptions", value: { ...coverOptions(guide), ...change } }],
    inverse: [{ op: "setGuide", key: "coverOptions", value: guide.coverOptions }],
  };
}

/** Patches that unset the cover when it is step `id`, which is going; empty otherwise. */
function dropCover(guide: Guide, id: string): { forward: Patch[]; inverse: Patch[] } {
  if (guide.coverStepId !== id) return { forward: [], inverse: [] };
  return {
    forward: [{ op: "setGuide", key: "coverStepId", value: undefined }],
    inverse: [{ op: "setGuide", key: "coverStepId", value: id }],
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

const ITEM_NAMES: Record<Item["type"], string> = { step: "step", section: "section", note: "note", decision: "decision" };

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

/** A new decision with a Yes and a No option, both going on to the next item. */
export function newDecision(id: string, optionIds: [string, string]): Item {
  return {
    type: "decision",
    id,
    question: "",
    options: [
      { id: optionIds[0], label: "Yes", target: { kind: "next" } },
      { id: optionIds[1], label: "No", target: { kind: "next" } },
    ],
  };
}

// ---------------------------------------------------------------------------
// Decisions and audiences (#22)

function findDecision(guide: Guide, id: string): Decision {
  const item = findItem(guide, id);
  if (item.type !== "decision") throw new PatchError(`item ${id} is not a decision`);
  return item;
}

function findSection(guide: Guide, id: string): Section {
  const item = findItem(guide, id);
  if (item.type !== "section") throw new PatchError(`item ${id} is not a section`);
  return item;
}

/** Replaces a decision's options. */
function setOptions(decision: Decision, options: DecisionOption[], label: string, coalesceKey?: string): Command {
  return {
    label,
    forward: [{ op: "setItem", id: decision.id, key: "options", value: options }],
    inverse: [{ op: "setItem", id: decision.id, key: "options", value: decision.options }],
    coalesceKey,
  };
}

/** Adds an option (going on to the next item) to the decision `id`. */
export function addDecisionOption(guide: Guide, id: string, optionId: string, label = ""): Command {
  const decision = findDecision(guide, id);
  return setOptions(decision, [...decision.options, { id: optionId, label, target: { kind: "next" } }], "Add option");
}

function checkOption(decision: Decision, optionId: string) {
  if (!decision.options.some((o) => o.id === optionId)) {
    throw new PatchError(`decision ${decision.id} has no option ${optionId}`);
  }
}

export function removeDecisionOption(guide: Guide, id: string, optionId: string): Command {
  const decision = findDecision(guide, id);
  checkOption(decision, optionId);
  return setOptions(decision, decision.options.filter((o) => o.id !== optionId), "Remove option");
}

function changeOption(
  guide: Guide,
  id: string,
  optionId: string,
  change: Partial<DecisionOption>,
  label: string,
  coalesceKey?: string,
): Command {
  const decision = findDecision(guide, id);
  checkOption(decision, optionId);
  const options = decision.options.map((o) => (o.id === optionId ? { ...o, ...change } : o));
  return setOptions(decision, options, label, coalesceKey);
}

/** Renames an option (plain text); typing into one option is one undo step. */
export function setOptionLabel(guide: Guide, id: string, optionId: string, label: string): Command {
  return changeOption(guide, id, optionId, { label }, "Rename option", `option.${optionId}.label`);
}

/** Sets where an option leads: the next item, a step, a section or another guide. */
export function setOptionTarget(guide: Guide, id: string, optionId: string, target: BranchTarget): Command {
  return changeOption(guide, id, optionId, { target }, "Change where an option goes");
}

/**
 * Sets who a section is for. Blank and repeated (ignoring case) names are
 * dropped; none left unsets the field, so the section is for everyone.
 */
export function setSectionAudiences(guide: Guide, id: string, audiences: string[]): Command {
  const section = findSection(guide, id);
  const seen = new Set<string>();
  const kept = audiences
    .map((a) => a.trim())
    .filter((a) => a && !seen.has(a.toLowerCase()) && seen.add(a.toLowerCase()));
  return {
    label: "Change section audience",
    forward: [{ op: "setItem", id, key: "audiences", value: kept.length ? kept : undefined }],
    inverse: [{ op: "setItem", id, key: "audiences", value: section.audiences }],
    coalesceKey: `item.${id}.audiences`,
  };
}

/**
 * The distinct audiences the guide's sections name, in order of first use
 * (core `branch::audiences`).
 */
export function guideAudiences(guide: Guide): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const item of guide.items) {
    if (item.type !== "section") continue;
    for (const raw of item.audiences ?? []) {
      const audience = raw.trim();
      if (audience && !seen.has(audience.toLowerCase())) {
        seen.add(audience.toLowerCase());
        out.push(audience);
      }
    }
  }
  return out;
}

/** Deletes an item; the next one (or else the previous one) is selected. */
export function deleteItem(guide: Guide, id: string): Command {
  const index = indexOf(guide, id);
  const item = guide.items[index];
  const neighbour = guide.items[index + 1] ?? guide.items[index - 1] ?? null;
  // A deleted cover step stops being the cover; undo makes it the cover again.
  const cover = dropCover(guide, id);
  return {
    label: `Delete ${item.type === "note" ? item.kind : ITEM_NAMES[item.type]}`,
    forward: [{ op: "removeItem", id }, ...cover.forward],
    inverse: [...cover.inverse, { op: "insertItem", index, item }],
    select: { forward: neighbour?.id ?? null, inverse: id },
  };
}

/**
 * Deletes several items as one undo step; the item after the last of them
 * (or else the one before the first) is selected.
 */
export function deleteItems(guide: Guide, ids: string[]): Command {
  if (ids.length === 1) return deleteItem(guide, ids[0]);
  const doomed = new Set(ids);
  const indices = guide.items.flatMap((item, index) => (doomed.has(item.id) ? [index] : []));
  if (indices.length === 0 || indices.length !== doomed.size) {
    throw new PatchError("deleting an item that is not in the guide");
  }
  const kept = (item: Item) => !doomed.has(item.id);
  const neighbour =
    guide.items.slice(indices[indices.length - 1] + 1).find(kept) ??
    guide.items.slice(0, indices[0]).filter(kept).at(-1) ??
    null;
  // A deleted cover step stops being the cover; undo makes it the cover again.
  const cover = guide.coverStepId && doomed.has(guide.coverStepId) ? dropCover(guide, guide.coverStepId) : null;
  return {
    label: `Delete ${indices.length} items`,
    forward: [
      ...indices.map((index): Patch => ({ op: "removeItem", id: guide.items[index].id })),
      ...(cover?.forward ?? []),
    ],
    // In guide order, so each goes back where it was.
    inverse: [
      ...(cover?.inverse ?? []),
      ...indices.map((index): Patch => ({ op: "insertItem", index, item: guide.items[index] })),
    ],
    select: { forward: neighbour?.id ?? null, inverse: guide.items[indices[0]].id },
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
  const cover = dropCover(guide, second.id);
  forward.push(...cover.forward);
  inverse.unshift(...cover.inverse);
  return { label: "Merge steps", forward, inverse, select: { forward: id, inverse: id } };
}

/**
 * Merges consecutive steps `ids` (in guide order) into the first, as
 * {@link mergeWithNext} would one after another, in one undo step.
 */
export function mergeSteps(guide: Guide, ids: string[]): Command {
  if (ids.length < 2) throw new PatchError("merging needs two steps or more");
  const [first, ...rest] = ids;
  let current = guide;
  const forward: Patch[] = [];
  const inverse: Patch[] = [];
  for (const id of rest) {
    if (nextStep(current, first)?.id !== id) throw new PatchError(`step ${id} does not follow the merged steps`);
    const command = mergeWithNext(current, first);
    forward.push(...command.forward);
    inverse.unshift(...command.inverse);
    current = applyPatches(current, command.forward);
  }
  return { label: `Merge ${ids.length} steps`, forward, inverse, select: { forward: first, inverse: first } };
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

// ---------------------------------------------------------------------------
// Re-capture (#12)

/** What a re-capture does with the step's annotations. */
export type AnnotationChoice = "keep" | "move" | "clear";

/**
 * How far the click moved from the step's point to the re-captured one, in
 * screenshot pixels; null unless both have a point and it moved.
 */
export function clickOffset(step: Step, captured: Pick<Step, "point">): { dx: number; dy: number } | null {
  if (!step.point || !captured.point) return null;
  const dx = captured.point.x - step.point.x;
  const dy = captured.point.y - step.point.y;
  return dx === 0 && dy === 0 ? null : { dx, dy };
}

/** `annotation` moved by (`dx`, `dy`) screenshot pixels, whatever its shape. */
export function moveAnnotation(annotation: Annotation, dx: number, dy: number): Annotation {
  const point = <P extends { x: number; y: number }>(p: P): P => ({ ...p, x: p.x + dx, y: p.y + dy });
  switch (annotation.type) {
    case "arrow":
      return { ...annotation, geometry: { from: point(annotation.geometry.from), to: point(annotation.geometry.to) } };
    case "callout": {
      const { anchor } = annotation.geometry;
      return { ...annotation, geometry: { ...point(annotation.geometry), ...(anchor ? { anchor: point(anchor) } : {}) } };
    }
    case "hotspot":
      return { ...annotation, geometry: point(annotation.geometry) };
    default:
      return { ...annotation, geometry: point(annotation.geometry) };
  }
}

/** The step fields a re-capture replaces; the rest (text, provenance, annotations) are the user's. */
const RECAPTURED = ["kind", "image", "point", "endPoint", "element", "keystrokes"] as const;

/**
 * Re-captures step `id` with `captured`, a step just recorded for it
 * (`RecordingSaved.recorded`): its kind, screenshots (and close-up, after
 * frame), click point, element and keystrokes replace the step's. The
 * title, description, narration and AI provenance stay; so does the choice of
 * close-up or full screenshot, while there is a close-up. Blur suggestions
 * were for the old screenshot, so the new one's replace them.
 *
 * The annotations are kept as they are, moved by the click offset
 * (`clickOffset`; kept where there is none) or cleared, as `annotations`
 * says. Annotations the recording added (blurs of sensitive text found on
 * the new screenshot) are added after them. One undo step.
 */
export function recaptureStep(
  guide: Guide,
  id: string,
  captured: Omit<Step, "type">,
  annotations: AnnotationChoice = "keep",
): Command {
  const step = findStep(guide, id);
  const next: Partial<Step> = {};
  for (const key of RECAPTURED) (next as Record<string, unknown>)[key] = captured[key] ?? undefined;
  next.image = { ...captured.image, show: captured.image.crop ? step.image.show : "full" };
  const offset = clickOffset(step, captured);
  const kept =
    annotations === "clear"
      ? []
      : annotations === "move" && offset
        ? step.annotations.map((a) => moveAnnotation(a, offset.dx, offset.dy))
        : step.annotations;
  next.annotations = [...kept, ...captured.annotations];
  const { redactionSuggestions: _old, scanned: _scanned, ...provenance } = step.aiState;
  const { redactionSuggestions, scanned } = captured.aiState;
  next.aiState = {
    ...provenance,
    ...(redactionSuggestions?.length ? { redactionSuggestions } : {}),
    ...(scanned ? { scanned } : {}),
  };
  const forward: Patch[] = [];
  const inverse: Patch[] = [];
  for (const [key, value] of Object.entries(next)) {
    const before = (step as unknown as Record<string, unknown>)[key];
    if (JSON.stringify(value) === JSON.stringify(before)) continue;
    forward.push({ op: "setItem", id, key, value });
    inverse.unshift({ op: "setItem", id, key, value: before });
  }
  return { label: "Re-capture step", forward, inverse, select: { forward: id, inverse: id } };
}
