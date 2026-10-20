// The inspector's open tab and the blur suggestion pointed at in it,
// shared with the annotation editor (which highlights it) and the export
// dialog (which opens the privacy review). View-only, never part of the
// guide.

import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";
import type { SuggestionRef } from "@/annotations/commands";
import { hotspots } from "@/annotations/geometry";
import { pendingOf } from "@/editor/privacy";
import type { Step } from "@/lib/types";

export type InspectorTab = "step" | "privacy" | "hotspots";

interface InspectorState {
  tab: InspectorTab;
  /** The suggestion pointed at or focused in the privacy review. */
  focused: SuggestionRef | null;
  setTab(tab: InspectorTab): void;
  focus(ref: SuggestionRef | null): void;
}

export const inspectorStore = createStore<InspectorState>()((set) => ({
  tab: "step",
  focused: null,
  setTab: (tab) => set({ tab }),
  focus: (focused) => set({ focused }),
}));

export function useInspector<T>(selector: (state: InspectorState) => T): T {
  return useStore(inspectorStore, selector);
}

/** The index of the focused suggestion on step `stepId`, if any. */
export function useFocusedSuggestion(stepId: string): number | null {
  return useInspector((s) => (s.focused?.stepId === stepId ? s.focused.index : null));
}

/** What needs attention on each tab: suggestions to review, hotspots without a title. */
export function tabCounts(step: Step): Record<InspectorTab, number> {
  return {
    step: 0,
    privacy: pendingOf(step).length,
    hotspots: hotspots(step.annotations).filter((h) => !h.geometry.title.trim()).length,
  };
}
