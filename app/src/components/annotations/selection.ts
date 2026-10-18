// The selected annotation, shared by the annotation editor and the
// inspector's hotspot list. View-only, never part of the guide.

import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

interface AnnotationSelection {
  /** The step the selection is on. */
  stepId: string | null;
  id: string | null;
  select(stepId: string, id: string | null): void;
}

export const annotationSelection = createStore<AnnotationSelection>()((set) => ({
  stepId: null,
  id: null,
  select: (stepId, id) => set({ stepId, id }),
}));

/** The selected annotation's id on step `stepId`, if any. */
export function useSelectedAnnotation(stepId: string): string | null {
  return useStore(annotationSelection, (s) => (s.stepId === stepId ? s.id : null));
}
