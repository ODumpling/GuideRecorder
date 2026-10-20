// The library's selected guides (for bulk actions) and the dragged ones.

import type { DragEvent } from "react";
import { create } from "zustand";
import { NO_SELECTION, type Selection } from "@/editor/libraryList";
import type { GuideListItem } from "@/lib/types";

/** The guides selected in the library grid or list. */
export const selectionStore = create<Selection>(() => NO_SELECTION);

export function clearSelection() {
  selectionStore.setState(NO_SELECTION, true);
}

/** The listed guides at `paths`, in list order. */
export function guidesAt(guides: GuideListItem[], paths: string[]): GuideListItem[] {
  const wanted = new Set(paths);
  return guides.filter((g) => wanted.has(g.path));
}

/** The drag data type of guides dragged out of the grid or list: their paths, as JSON. */
const DRAG_TYPE = "application/x-guiderecorder-guides";

/** Starts dragging `guide`, with the rest of the selection if it is selected. */
export function startGuideDrag(e: DragEvent, guide: GuideListItem) {
  const { paths } = selectionStore.getState();
  const dragged = paths.includes(guide.path) ? paths : [guide.path];
  e.dataTransfer.setData(DRAG_TYPE, JSON.stringify(dragged));
  e.dataTransfer.effectAllowed = "move";
}

/** Whether a drag carries guides from the library. */
export function isGuideDrag(e: DragEvent): boolean {
  return e.dataTransfer.types.includes(DRAG_TYPE);
}

/** The paths of guides dropped from the library (none if it is something else). */
export function droppedGuides(e: DragEvent): string[] {
  try {
    const paths: unknown = JSON.parse(e.dataTransfer.getData(DRAG_TYPE) || "[]");
    return Array.isArray(paths) ? paths.filter((p): p is string => typeof p === "string") : [];
  } catch {
    return [];
  }
}
