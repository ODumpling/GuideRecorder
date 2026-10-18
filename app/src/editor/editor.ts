import { useStore } from "zustand";
import { tauriApi } from "@/lib/ipc";
import type { Item, Step } from "@/lib/types";
import { createEditorStore, type EditorState } from "@/editor/store";

/** The app's single editor store. */
export const editorStore = createEditorStore({ api: tauriApi });

export function useEditor<T>(selector: (state: EditorState) => T): T {
  return useStore(editorStore, selector);
}

export function useSelectedItem(): Item | null {
  return useEditor((s) => s.doc?.guide.items.find((item) => item.id === s.selectedId) ?? null);
}

/** Whether the open guide is an annotation guide: pages (uploaded images) explained by hotspots. */
export function useAnnotated(): boolean {
  return useEditor((s) => s.doc?.guide.kind === "annotated");
}

/** What a guide calls its steps: an annotation guide has pages. */
export function stepNoun(annotated: boolean): "page" | "step" {
  return annotated ? "page" : "step";
}

/** 1-based step numbers by item id (sections and notes are not numbered). */
export function stepNumbers(items: Item[]): Map<string, number> {
  const numbers = new Map<string, number>();
  items.filter((item): item is Step => item.type === "step").forEach((step, i) => {
    numbers.set(step.id, i + 1);
  });
  return numbers;
}
