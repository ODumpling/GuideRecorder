// Collections of the library (#21): manuals made of several guides, shown
// in the sidebar and edited in the main pane. Edits apply here at once and
// are saved shortly after, like guides.

import { createStore, type StoreApi } from "zustand/vanilla";
import { createDebouncer, realTimers, type Timers } from "@/editor/debounce";
import { errorMessage, type CollectionApi } from "@/lib/ipc";
import type { Collection, CollectionEntry, CollectionView, GuideListItem } from "@/lib/types";
import { ulid } from "@/lib/ulid";

/** How long edits wait before they are saved. */
export const SAVE_DELAY_MS = 500;

export interface CollectionsState {
  collections: CollectionView[];
  listError: string | null;
  /** The collection shown in the main pane instead of the guides. */
  openId: string | null;
  /** Why the last save, create or delete failed. */
  error: string | null;
  load(): Promise<void>;
  /** Creates a collection and opens it; null if that failed. */
  create(title: string): Promise<Collection | null>;
  open(id: string | null): void;
  /** Changes a collection here at once and saves it shortly after. */
  edit(id: string, change: (collection: Collection) => Collection): void;
  /** Saves pending edits now; false if a save failed. */
  flush(): Promise<boolean>;
  remove(id: string): Promise<boolean>;
}

/** The collection `id`, if it is listed and readable. */
export function findCollection(state: Pick<CollectionsState, "collections">, id: string | null): Collection | null {
  return state.collections.find((c) => c.id === id)?.collection ?? null;
}

/** `entries` with `guideIds` added at the end, each as a new entry. */
export function withGuides(entries: CollectionEntry[], guideIds: string[], newId: () => string = ulid): CollectionEntry[] {
  return [...entries, ...guideIds.map((guideId) => ({ type: "guide" as const, id: newId(), guideId }))];
}

/** `entries` with a new, empty text at the end. */
export function withText(entries: CollectionEntry[], newId: () => string = ulid): CollectionEntry[] {
  return [...entries, { type: "text", id: newId(), body: "" }];
}

/** `entries` with entry `id` moved `delta` places (kept within the list). */
export function moved(entries: CollectionEntry[], id: string, delta: number): CollectionEntry[] {
  const from = entries.findIndex((e) => e.id === id);
  const to = Math.min(entries.length - 1, Math.max(0, from + delta));
  if (from < 0 || from === to) return entries;
  const next = [...entries];
  const [entry] = next.splice(from, 1);
  next.splice(to, 0, entry!);
  return next;
}

/** `entries` without entry `id`. */
export function without(entries: CollectionEntry[], id: string): CollectionEntry[] {
  return entries.filter((e) => e.id !== id);
}

/** The guides of a collection as the library lists them; null for one that is gone. */
export function collectionGuides(collection: Collection, guides: GuideListItem[]): (GuideListItem | null)[] {
  const byId = new Map(guides.map((g) => [g.id, g]));
  return collection.entries
    .filter((e) => e.type === "guide")
    .map((e) => byId.get(e.guideId) ?? null);
}

/** "3 guides · 24 steps", counting only guides still in the library. */
export function collectionSummary(collection: Collection, guides: GuideListItem[]): string {
  const found = collectionGuides(collection, guides).filter((g): g is GuideListItem => g !== null);
  const count = collection.entries.filter((e) => e.type === "guide").length;
  const steps = found.reduce((n, g) => n + g.stepCount, 0);
  return `${count} ${count === 1 ? "guide" : "guides"} · ${steps} ${steps === 1 ? "step" : "steps"}`;
}

export function createCollectionsStore({
  api,
  timers = realTimers,
}: {
  api: CollectionApi;
  timers?: Timers;
}): StoreApi<CollectionsState> {
  return createStore<CollectionsState>()((set, get) => {
    /** Collections edited since their last save. */
    const dirty = new Set<string>();
    let saving: Promise<boolean> = Promise.resolve(true);

    const saveDirty = (): Promise<boolean> => {
      const ids = [...dirty];
      dirty.clear();
      saving = saving.then(async () => {
        let ok = true;
        for (const id of ids) {
          const collection = findCollection(get(), id);
          if (!collection) continue;
          try {
            await api.saveCollection(collection);
          } catch (error) {
            ok = false;
            set({ error: errorMessage(error) });
          }
        }
        if (ok) set({ error: null });
        return ok;
      });
      return saving;
    };
    const debouncer = createDebouncer(() => void saveDirty(), SAVE_DELAY_MS, timers);

    return {
      collections: [],
      listError: null,
      openId: null,
      error: null,

      async load() {
        debouncer.flush();
        await saving;
        try {
          const collections = await api.listCollections();
          const openId = get().openId;
          set({
            collections,
            listError: null,
            openId: collections.some((c) => c.id === openId) ? openId : null,
          });
        } catch (error) {
          set({ listError: errorMessage(error) });
        }
      },

      async create(title) {
        try {
          const collection = await api.createCollection(title.trim());
          set((s) => ({
            collections: [...s.collections, { id: collection.id, collection, error: null }],
            openId: collection.id,
            error: null,
          }));
          return collection;
        } catch (error) {
          set({ error: errorMessage(error) });
          return null;
        }
      },

      open(openId) {
        if (openId !== get().openId) set({ openId });
      },

      edit(id, change) {
        const current = findCollection(get(), id);
        if (!current) return;
        const next = change(current);
        if (next === current) return;
        set((s) => ({
          collections: s.collections.map((c) => (c.id === id ? { ...c, collection: next } : c)),
        }));
        dirty.add(id);
        debouncer.schedule();
      },

      async flush() {
        debouncer.cancel();
        return dirty.size > 0 ? saveDirty() : saving;
      },

      async remove(id) {
        dirty.delete(id);
        try {
          await api.deleteCollection(id);
        } catch (error) {
          set({ error: errorMessage(error) });
          return false;
        }
        set((s) => ({
          collections: s.collections.filter((c) => c.id !== id),
          openId: s.openId === id ? null : s.openId,
          error: null,
        }));
        return true;
      },
    };
  });
}
