// Item operations as the UI triggers them: each builds a command against
// the current guide and selection and dispatches it (#351).

import {
  deleteItem,
  insertItemAfter,
  insertItemBefore,
  mergeWithNext,
  newNote,
  newSection,
  nextStep,
  splitStep,
} from "@/editor/commands";
import { insertImageSteps } from "@/editor/images";
import type { EditorStore } from "@/editor/store";
import { copyStepImages, errorMessage } from "@/lib/ipc";
import type { NoteKind, StepImage } from "@/lib/types";
import { ulid } from "@/lib/ulid";

function current(store: EditorStore) {
  const { doc, selectedId, dispatch } = store.getState();
  if (!doc || doc.readOnly) return null;
  const selected = doc.guide.items.find((item) => item.id === selectedId) ?? null;
  return { guide: doc.guide, selected, dispatch };
}

/** A section heading above the selected item (or at the start). */
export function insertSection(store: EditorStore, id = ulid()): boolean {
  const state = current(store);
  return !!state && state.dispatch(insertItemBefore(state.guide, newSection(id), state.selected?.id ?? null));
}

/** A tip, warning or note below the selected item (or at the end). */
export function insertNote(store: EditorStore, kind: NoteKind, id = ulid()): boolean {
  const state = current(store);
  return !!state && state.dispatch(insertItemAfter(state.guide, newNote(id, kind), state.selected?.id ?? null));
}

export function deleteSelected(store: EditorStore): boolean {
  const state = current(store);
  return !!state?.selected && state.dispatch(deleteItem(state.guide, state.selected.id));
}

/** Whether the selected step has a step right after it to merge with. */
export function canMergeSelected(store: EditorStore): boolean {
  const state = current(store);
  return state?.selected?.type === "step" && nextStep(state.guide, state.selected.id) !== null;
}

export function mergeSelected(store: EditorStore): boolean {
  const state = current(store);
  if (!state?.selected || !canMergeSelected(store)) return false;
  return state.dispatch(mergeWithNext(state.guide, state.selected.id));
}

/**
 * Splits the selected step; `split` gives its description before and after
 * the cut. The new step gets its own copy of the screenshot files first, so
 * the two steps never share files. Failures end up in the store's notice.
 */
export async function splitSelected(store: EditorStore, split: [string, string], id = ulid()): Promise<boolean> {
  const state = current(store);
  if (state?.selected?.type !== "step") return false;
  const { guide, selected } = state;
  let image: StepImage;
  try {
    image = await copyStepImages(guide.id, selected.image, id);
  } catch (error) {
    store.getState().notify(`Could not split the step: ${errorMessage(error)}`);
    return false;
  }
  // The guide may have changed meanwhile; split what is there now.
  const now = current(store);
  const step = now?.guide.items.find((item) => item.id === selected.id);
  if (!now || now.guide.id !== guide.id || step?.type !== "step") return false;
  return now.dispatch(splitStep(now.guide, step.id, id, split[0], split[1], image));
}

/**
 * Manual steps from images, below the selected item. Failures (an
 * unreadable file, a read-only guide) end up in the store's notice.
 */
export async function insertImages(
  store: EditorStore,
  images: (Blob | (() => Promise<Blob>))[],
): Promise<void> {
  const state = current(store);
  if (!state || images.length === 0) return;
  store.getState().notify(null);
  try {
    await insertImageSteps(store, images, state.selected?.id ?? null);
  } catch (error) {
    store.getState().notify(`Could not add the image: ${errorMessage(error)}`);
  }
}
