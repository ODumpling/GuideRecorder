import { listen } from "@tauri-apps/api/event";
import { useStore } from "zustand";
import { createAiStore, type AiRunState } from "@/ai/controller";
import { editorStore } from "@/editor/editor";
import { tauriAiApi } from "@/lib/ipc";

/** The app's AI run controller. */
export const aiStore = createAiStore({
  api: tauriAiApi,
  editor: editorStore,
  listen: (event, handler) => listen(event, (e) => handler(e.payload as never)),
});

export function useAi<T>(selector: (state: AiRunState) => T): T {
  return useStore(aiStore, selector);
}
