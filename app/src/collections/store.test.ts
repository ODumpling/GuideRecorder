import { describe, expect, it, vi } from "vitest";
import type { CollectionApi } from "@/lib/ipc";
import type { Collection, CollectionEntry, GuideListItem } from "@/lib/types";
import {
  collectionGuides,
  collectionSummary,
  createCollectionsStore,
  findCollection,
  moved,
  SAVE_DELAY_MS,
  withGuides,
  withText,
  without,
} from "@/collections/store";
import type { Timers } from "@/editor/debounce";

function collection(id: string, entries: CollectionEntry[] = []): Collection {
  return { version: 1, id, title: id, intro: "", numbering: "continuous", entries, updatedAt: "2026-10-23T00:00:00Z" };
}

const guide = (id: string): CollectionEntry => ({ type: "guide", id: `e-${id}`, guideId: id });
const text = (id: string): CollectionEntry => ({ type: "text", id, body: "" });

function manualTimers() {
  let pending: (() => void) | null = null;
  const timers: Timers = {
    setTimeout: (fn, ms) => {
      expect(ms).toBe(SAVE_DELAY_MS);
      pending = fn;
      return 1;
    },
    clearTimeout: () => {
      pending = null;
    },
  };
  return { timers, fire: () => pending?.() };
}

function fakeApi(stored: Collection[] = []) {
  const api = {
    listCollections: vi.fn(async () => stored.map((c) => ({ id: c.id, collection: c, error: null }))),
    createCollection: vi.fn(async (title: string) => ({ ...collection("new"), title })),
    saveCollection: vi.fn(async (c: Collection) => c),
    deleteCollection: vi.fn(async () => {}),
    exportCollection: vi.fn(),
  } satisfies CollectionApi;
  return api;
}

describe("entry edits", () => {
  const entries = [guide("a"), text("t"), guide("b")];

  it("moves within the list", () => {
    expect(moved(entries, "t", -1).map((e) => e.id)).toEqual(["t", "e-a", "e-b"]);
    expect(moved(entries, "e-a", 5).map((e) => e.id)).toEqual(["t", "e-b", "e-a"]);
    expect(moved(entries, "e-a", -1)).toBe(entries);
    expect(moved(entries, "nope", 1)).toBe(entries);
  });

  it("adds and removes", () => {
    let n = 0;
    const id = () => `id${++n}`;
    expect(withGuides(entries, ["c", "a"], id).slice(3)).toEqual([
      { type: "guide", id: "id1", guideId: "c" },
      { type: "guide", id: "id2", guideId: "a" },
    ]);
    expect(withText([], id)).toEqual([{ type: "text", id: "id3", body: "" }]);
    expect(without(entries, "t").map((e) => e.id)).toEqual(["e-a", "e-b"]);
  });

  it("summarises the guides still in the library", () => {
    const listed = [{ id: "a", stepCount: 3 }, { id: "c", stepCount: 9 }] as GuideListItem[];
    const c = collection("x", entries);
    expect(collectionGuides(c, listed).map((g) => g?.id ?? null)).toEqual(["a", null]);
    expect(collectionSummary(c, listed)).toBe("2 guides · 3 steps");
    expect(collectionSummary(collection("y", [guide("a")]), [{ id: "a", stepCount: 1 } as GuideListItem])).toBe(
      "1 guide · 1 step",
    );
  });
});

describe("collections store", () => {
  it("creates and opens, edits locally and saves after a pause", async () => {
    const api = fakeApi();
    const { timers, fire } = manualTimers();
    const store = createCollectionsStore({ api, timers });
    const created = await store.getState().create("  Manual ");
    expect(api.createCollection).toHaveBeenCalledWith("Manual");
    expect(store.getState().openId).toBe(created!.id);

    store.getState().edit(created!.id, (c) => ({ ...c, entries: withGuides(c.entries, ["g1"]) }));
    store.getState().edit(created!.id, (c) => ({ ...c, intro: "Hi" }));
    expect(findCollection(store.getState(), created!.id)?.intro).toBe("Hi");
    expect(api.saveCollection).not.toHaveBeenCalled();
    fire();
    await store.getState().flush();
    expect(api.saveCollection).toHaveBeenCalledTimes(1);
    expect(api.saveCollection.mock.calls[0]![0]).toMatchObject({ intro: "Hi", entries: [{ guideId: "g1" }] });
  });

  it("flush saves pending edits at once and reports failures", async () => {
    const api = fakeApi([collection("c1")]);
    const { timers } = manualTimers();
    const store = createCollectionsStore({ api, timers });
    await store.getState().load();
    store.getState().edit("c1", (c) => ({ ...c, title: "New" }));
    api.saveCollection.mockRejectedValueOnce({ kind: "io", message: "disk full" });
    expect(await store.getState().flush()).toBe(false);
    expect(store.getState().error).toBe("disk full");
    store.getState().edit("c1", (c) => ({ ...c, title: "Newer" }));
    expect(await store.getState().flush()).toBe(true);
    expect(store.getState().error).toBeNull();
    // Nothing pending: nothing saved.
    expect(await store.getState().flush()).toBe(true);
    expect(api.saveCollection).toHaveBeenCalledTimes(2);
  });

  it("deleting closes the collection; loading forgets one that is gone", async () => {
    const stored = [collection("c1"), collection("c2")];
    const api = fakeApi(stored);
    const store = createCollectionsStore({ api, timers: manualTimers().timers });
    await store.getState().load();
    store.getState().open("c1");
    expect(await store.getState().remove("c1")).toBe(true);
    expect(store.getState().openId).toBeNull();
    expect(store.getState().collections.map((c) => c.id)).toEqual(["c2"]);

    store.getState().open("c2");
    stored.splice(1, 1);
    await store.getState().load();
    expect(store.getState().openId).toBeNull();
  });

  it("ignores edits that change nothing", () => {
    const api = fakeApi();
    const store = createCollectionsStore({ api, timers: manualTimers().timers });
    store.getState().edit("missing", (c) => ({ ...c, title: "x" }));
    expect(store.getState().collections).toEqual([]);
  });
});
