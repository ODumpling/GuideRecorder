import { useStore } from "zustand";
import { editorStore } from "@/editor/editor";
import { tauriCollectionApi } from "@/lib/ipc";
import { createCollectionsStore, type CollectionsState } from "@/collections/store";

/** The app's single collections store. */
export const collectionsStore = createCollectionsStore({ api: tauriCollectionApi });

export function useCollections<T>(selector: (state: CollectionsState) => T): T {
  return useStore(collectionsStore, selector);
}

// A search is of guides, so it shows the guides again.
editorStore.subscribe((state, prev) => {
  if (state.query !== prev.query && state.query.trim()) collectionsStore.getState().open(null);
});

/** Shows the library's guides again (a sidebar filter was chosen). */
export function closeCollection() {
  collectionsStore.getState().open(null);
}

// A collection lists the whole library's guides, so it ends a search.
collectionsStore.subscribe((state, prev) => {
  if (state.openId && state.openId !== prev.openId && editorStore.getState().query) {
    void editorStore.getState().setQuery("");
  }
});
