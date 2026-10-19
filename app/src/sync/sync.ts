import { useStore } from "zustand";
import { editorStore } from "@/editor/editor";
import { tauriSyncApi, tauriSyncBackend } from "@/lib/ipc";
import { createSyncStore, type SyncStoreState } from "@/sync/store";

/** The app's sync store. */
export const syncStore = createSyncStore({
  api: tauriSyncApi,
  backend: tauriSyncBackend,
  onGuidesWritten: () => void editorStore.getState().loadGuides(),
});

export function useSync<T>(selector: (state: SyncStoreState) => T): T {
  return useStore(syncStore, selector);
}

const INTERVAL_MS = 30_000;

/**
 * Loads the sign-in and sync state, then syncs every half minute, when the
 * window gets focus, and soon after the library changes (an edit saved).
 * Returns a function that stops it.
 */
export function startSync(): () => void {
  void syncStore
    .getState()
    .load()
    .then(() => syncStore.getState().syncNow());
  const sync = () => void syncStore.getState().syncNow();
  const timer = setInterval(sync, INTERVAL_MS);
  window.addEventListener("focus", sync);
  let soon: ReturnType<typeof setTimeout> | undefined;
  const unsubscribe = editorStore.subscribe((state, previous) => {
    if (state.guides === previous.guides) return;
    clearTimeout(soon);
    soon = setTimeout(sync, 3000);
  });
  return () => {
    clearInterval(timer);
    clearTimeout(soon);
    window.removeEventListener("focus", sync);
    unsubscribe();
  };
}
