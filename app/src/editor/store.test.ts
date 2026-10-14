import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { EditorApi } from "@/lib/ipc";
import type { Guide, OpenedGuide } from "@/lib/types";
import { moveItem, renameGuide, setItemTitle } from "@/editor/commands";
import { createEditorStore } from "@/editor/store";
import { guide } from "@/editor/test-fixtures";

function fakeApi(opened: Partial<OpenedGuide> = {}) {
  const saved: Guide[] = [];
  let release: (() => void) | null = null;
  let blockSaves = false;
  const api: EditorApi = {
    listGuides: vi.fn(async (query?: string) =>
      query === "none"
        ? []
        : [
            {
              id: "G1",
              path: "/lib/Guide.guidesya",
              title: "Guide",
              summary: "",
              tags: [],
              stepCount: 3,
              updatedAt: "",
              readOnly: false,
              duplicateOf: null,
            },
          ],
    ),
    openGuide: vi.fn(async () => ({
      guide: guide(),
      path: "/lib/Guide.guidesya",
      readOnly: false,
      migratedFrom: null,
      migrationError: null,
      ...opened,
    })),
    saveGuide: vi.fn(async (g: Guide) => {
      if (blockSaves) await new Promise<void>((resolve) => (release = resolve));
      saved.push(g);
    }),
    stepImage: vi.fn(async () => new ArrayBuffer(0)),
    quit: vi.fn(async () => {}),
  };
  return {
    api,
    saved,
    block: () => (blockSaves = true),
    unblock: () => {
      blockSaves = false;
      release?.();
    },
  };
}

let clock = 0;
const now = () => clock;

async function opened(options: Partial<OpenedGuide> = {}) {
  const fake = fakeApi(options);
  const store = createEditorStore({ api: fake.api, now });
  await store.getState().openGuide("G1");
  return { ...fake, store, s: () => store.getState() };
}

beforeEach(() => {
  clock = 1_000_000;
  vi.useFakeTimers();
});
afterEach(() => vi.useRealTimers());

describe("library", () => {
  it("lists guides with the current query", async () => {
    const { api } = fakeApi();
    const store = createEditorStore({ api });
    await store.getState().loadGuides();
    expect(store.getState().guides).toHaveLength(1);
    await store.getState().setQuery("none");
    expect(api.listGuides).toHaveBeenLastCalledWith("none");
    expect(store.getState().guides).toEqual([]);
  });
});

describe("opening", () => {
  it("selects the first step and starts clean", async () => {
    const { s } = await opened();
    expect(s().doc?.guide.id).toBe("G1");
    expect(s().selectedId).toBe("A");
    expect(s().saveStatus).toBe("saved");
    expect(s().undoStack).toEqual([]);
  });

  it("reports open errors", async () => {
    const { api } = fakeApi();
    api.openGuide = vi.fn(async () => Promise.reject({ kind: "notFound", message: "gone" }));
    const store = createEditorStore({ api });
    await store.getState().openGuide("X");
    expect(store.getState().openError).toBe("gone");
    expect(store.getState().doc).toBeNull();
  });
});

describe("undo/redo", () => {
  it("undoes and redoes commands in order", async () => {
    const { s } = await opened();
    s().dispatch(renameGuide(s().doc!.guide, "Renamed"));
    clock += 5000;
    s().dispatch(moveItem(s().doc!.guide, "C", 1));
    const order = () => s().doc!.guide.items.map((item) => item.id);
    expect(order()).toEqual(["S1", "C", "A", "B", "N1"]);

    s().undo();
    expect(order()).toEqual(["S1", "A", "B", "N1", "C"]);
    expect(s().doc!.guide.title).toBe("Renamed");
    s().undo();
    expect(s().doc!.guide).toEqual(guide());
    s().undo(); // nothing left: no-op
    expect(s().redoStack).toHaveLength(2);

    s().redo();
    s().redo();
    expect(s().doc!.guide.title).toBe("Renamed");
    expect(order()).toEqual(["S1", "C", "A", "B", "N1"]);
  });

  it("a new command clears the redo stack", async () => {
    const { s } = await opened();
    s().dispatch(renameGuide(s().doc!.guide, "One"));
    s().undo();
    expect(s().redoStack).toHaveLength(1);
    s().dispatch(renameGuide(s().doc!.guide, "Two"));
    expect(s().redoStack).toHaveLength(0);
  });

  it("merges quick typing into one step, but not slow typing", async () => {
    const { s } = await opened();
    for (const title of ["C", "Cl", "Cli", "Click"]) {
      s().dispatch(setItemTitle(s().doc!.guide, "A", title));
      clock += 200;
    }
    expect(s().undoStack).toHaveLength(1);
    clock += 2000;
    s().dispatch(setItemTitle(s().doc!.guide, "A", "Click OK"));
    expect(s().undoStack).toHaveLength(2);

    s().undo();
    expect(s().doc!.guide.items[1]).toMatchObject({ title: "Click" });
    s().undo();
    expect(s().doc!.guide.items[1]).toMatchObject({ title: "Step A" });
  });

  it("does not merge into a redone step", async () => {
    const { s } = await opened();
    s().dispatch(setItemTitle(s().doc!.guide, "A", "x"));
    s().undo();
    s().redo();
    s().dispatch(setItemTitle(s().doc!.guide, "A", "xy"));
    expect(s().undoStack).toHaveLength(2);
  });

  it("drops no-op commands and edits that end where they started", async () => {
    const { s } = await opened();
    expect(s().dispatch(renameGuide(s().doc!.guide, "Guide"))).toBe(false);
    s().dispatch(renameGuide(s().doc!.guide, "Guidex"));
    s().dispatch(renameGuide(s().doc!.guide, "Guide"));
    expect(s().undoStack).toHaveLength(0);
  });

  it("refuses commands that do not apply", async () => {
    const { s } = await opened();
    const stale = moveItem(s().doc!.guide, "A", 2);
    stale.forward = [{ op: "moveItem", id: "gone", to: 0 }];
    vi.spyOn(console, "error").mockImplementation(() => {});
    expect(s().dispatch(stale)).toBe(false);
    expect(s().revision).toBe(0);
  });
});

describe("read-only guides", () => {
  it("refuse edits and never save", async () => {
    const { s, api } = await opened({ readOnly: true });
    expect(s().dispatch(renameGuide(s().doc!.guide, "x"))).toBe(false);
    expect(s().doc!.guide.title).toBe("Guide");
    await vi.advanceTimersByTimeAsync(1000);
    await s().flush();
    expect(api.saveGuide).not.toHaveBeenCalled();
  });
});

describe("autosave", () => {
  it("saves 500 ms after the last change", async () => {
    const { s, saved } = await opened();
    s().dispatch(renameGuide(s().doc!.guide, "A"));
    expect(s().saveStatus).toBe("pending");
    await vi.advanceTimersByTimeAsync(300);
    s().dispatch(renameGuide(s().doc!.guide, "AB"));
    await vi.advanceTimersByTimeAsync(300);
    expect(saved).toHaveLength(0);
    await vi.advanceTimersByTimeAsync(200);
    expect(saved).toHaveLength(1);
    expect(saved[0].title).toBe("AB");
    expect(saved[0].updatedAt).toBe(new Date(clock).toISOString());
    expect(s().saveStatus).toBe("saved");
    expect(s().savedRevision).toBe(s().revision);
  });

  it("undo back to the saved state still saves", async () => {
    const { s, saved } = await opened();
    s().dispatch(renameGuide(s().doc!.guide, "X"));
    await vi.advanceTimersByTimeAsync(500);
    s().undo();
    await vi.advanceTimersByTimeAsync(500);
    expect(saved.map((g) => g.title)).toEqual(["X", "Guide"]);
  });

  it("saves changes made during a save afterwards", async () => {
    const { s, saved, block, unblock } = await opened();
    block();
    s().dispatch(renameGuide(s().doc!.guide, "One"));
    await vi.advanceTimersByTimeAsync(500);
    expect(s().saveStatus).toBe("saving");
    clock += 5000;
    s().dispatch(renameGuide(s().doc!.guide, "Two"));
    unblock();
    await vi.advanceTimersByTimeAsync(0);
    expect(saved.map((g) => g.title)).toEqual(["One"]);
    expect(s().saveStatus).toBe("pending");
    await vi.advanceTimersByTimeAsync(500);
    expect(saved.map((g) => g.title)).toEqual(["One", "Two"]);
    expect(s().saveStatus).toBe("saved");
  });

  it("shows save errors and retries on the next change", async () => {
    const { s, api, saved } = await opened();
    const ok = api.saveGuide;
    api.saveGuide = vi.fn(async () => Promise.reject({ kind: "io", message: "disk full" }));
    s().dispatch(renameGuide(s().doc!.guide, "X"));
    await vi.advanceTimersByTimeAsync(500);
    expect(s().saveStatus).toBe("error");
    expect(s().saveError).toBe("disk full");
    api.saveGuide = ok;
    clock += 5000;
    s().dispatch(renameGuide(s().doc!.guide, "Y"));
    await vi.advanceTimersByTimeAsync(500);
    expect(saved.map((g) => g.title)).toEqual(["Y"]);
    expect(s().saveStatus).toBe("saved");
  });

  it("recovers after a save that throws synchronously", async () => {
    const { s, api, saved } = await opened();
    const ok = api.saveGuide;
    api.saveGuide = vi.fn(() => {
      throw { kind: "io", message: "boom" };
    });
    s().dispatch(renameGuide(s().doc!.guide, "X"));
    await vi.advanceTimersByTimeAsync(500);
    expect(s().saveStatus).toBe("error");
    api.saveGuide = ok;
    expect(await s().flush()).toBe(true);
    expect(saved.map((g) => g.title)).toEqual(["X"]);
  });

  it("flushes pending changes before opening another guide", async () => {
    const { s, saved } = await opened();
    s().dispatch(renameGuide(s().doc!.guide, "Before switch"));
    await s().closeGuide();
    expect(saved.map((g) => g.title)).toEqual(["Before switch"]);
    await vi.advanceTimersByTimeAsync(1000);
    expect(saved).toHaveLength(1);
  });
});

describe("leaving a guide", () => {
  async function failing() {
    const ctx = await opened();
    const ok = ctx.api.saveGuide;
    ctx.api.saveGuide = vi.fn(async () => Promise.reject({ kind: "io", message: "disk full" }));
    ctx.s().dispatch(renameGuide(ctx.s().doc!.guide, "Unsaved"));
    return { ...ctx, restore: () => (ctx.api.saveGuide = ok) };
  }

  it("close after a failed save keeps the doc and the error", async () => {
    const { s } = await failing();
    expect(await s().flush()).toBe(false);
    expect(await s().closeGuide()).toBe(false);
    expect(s().doc?.guide.title).toBe("Unsaved");
    expect(s().saveStatus).toBe("error");
    expect(s().saveError).toBe("disk full");
    expect(s().leave).toEqual({ kind: "close" });
    expect(s().busy).toBe(false);
  });

  it("opening another guide after a failed save is refused too", async () => {
    const { s, api } = await failing();
    expect(await s().openGuide("G2")).toBe(false);
    expect(s().leave).toEqual({ kind: "open", id: "G2" });
    expect(api.openGuide).toHaveBeenCalledTimes(1);
    expect(s().doc?.guide.title).toBe("Unsaved");
  });

  it("retry saves and then leaves", async () => {
    const { s, saved, restore } = await failing();
    await s().closeGuide();
    restore();
    expect(await s().resolveLeave("retry")).toBe(true);
    expect(saved.map((g) => g.title)).toEqual(["Unsaved"]);
    expect(s().doc).toBeNull();
    expect(s().leave).toBeNull();
  });

  it("retry that fails again keeps asking", async () => {
    const { s } = await failing();
    await s().closeGuide();
    expect(await s().resolveLeave("retry")).toBe(false);
    expect(s().leave).toEqual({ kind: "close" });
    expect(s().doc).not.toBeNull();
  });

  it("discard leaves without saving; cancel stays", async () => {
    const { s, api } = await failing();
    await s().openGuide("G2");
    expect(await s().resolveLeave("cancel")).toBe(false);
    expect(s().leave).toBeNull();
    expect(s().doc?.guide.title).toBe("Unsaved");

    await s().openGuide("G2");
    const calls = vi.mocked(api.saveGuide).mock.calls.length;
    expect(await s().resolveLeave("discard")).toBe(true);
    expect(api.saveGuide).toHaveBeenCalledTimes(calls);
    expect(api.openGuide).toHaveBeenLastCalledWith("G2");
    expect(s().saveStatus).toBe("saved");
    expect(s().revision).toBe(s().savedRevision);
  });

  it("quit saves first, and only quits once saved", async () => {
    const { s, api, restore } = await failing();
    expect(await s().requestQuit()).toBe(false);
    expect(api.quit).not.toHaveBeenCalled();
    expect(s().leave).toEqual({ kind: "quit" });
    restore();
    expect(await s().resolveLeave("retry")).toBe(true);
    expect(api.quit).toHaveBeenCalledTimes(1);
  });

  it("refuses edits while saving before a close", async () => {
    const { s, saved, block, unblock } = await opened();
    s().dispatch(renameGuide(s().doc!.guide, "One"));
    block();
    const closing = s().closeGuide();
    await vi.advanceTimersByTimeAsync(0);
    expect(s().busy).toBe(true);
    expect(s().dispatch(renameGuide(s().doc!.guide, "Lost?"))).toBe(false);
    unblock();
    expect(await closing).toBe(true);
    expect(saved.map((g) => g.title)).toEqual(["One"]);
    expect(s().doc).toBeNull();
  });
});
