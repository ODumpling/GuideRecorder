import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { EditorApi } from "@/lib/ipc";
import type { Guide, OpenedGuide, Step } from "@/lib/types";
import {
  deleteItem,
  insertItemAfter,
  moveItem,
  newNote,
  renameGuide,
  setGuideField,
  setItemTitle,
} from "@/editor/commands";
import { createEditorStore, createQuitFlow, type QuitSteps } from "@/editor/store";
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
              path: "/lib/Guide.guiderecorder",
              title: "Guide",
              summary: "",
              tags: [],
              stepCount: 3,
              createdAt: "",
              updatedAt: "",
              folder: "",
              readOnly: false,
              duplicateOf: null,
            },
          ],
    ),
    openGuide: vi.fn(async (id: string) => ({
      guide: { ...guide(), id },
      path: "/lib/Guide.guiderecorder",
      readOnly: false,
      migratedFrom: null,
      migrationError: null,
      ...opened,
    })),
    saveGuide: vi.fn(async (g: Guide) => {
      if (blockSaves) await new Promise<void>((resolve) => (release = resolve));
      saved.push(g);
    }),
    closeGuide: vi.fn(async () => []),
    deleteGuide: vi.fn(async () => true),
    listFolders: vi.fn(async () => ["Work"]),
    createFolder: vi.fn(async (parent: string, name: string) => (parent ? `${parent}/${name}` : name)),
    duplicateGuide: vi.fn(async () => "G3"),
    renameGuide: vi.fn(async (_id: string, _path: string, title: string) => ({
      path: `/lib/${title}.guiderecorder`,
      title,
      updatedAt: "2026-10-19T00:00:00Z",
    })),
    moveGuide: vi.fn(async () => "/lib/Work/Guide.guiderecorder"),
    reveal: vi.fn(async () => {}),
    newAnnotatedGuide: vi.fn(async (): Promise<string | null> => "G2"),
    stepImage: vi.fn(async () => new ArrayBuffer(0)),
    quit: vi.fn(async () => {}),
    cancelQuit: vi.fn(async () => {}),
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

  it("lists folders too, keeping the last ones if that fails", async () => {
    const { api } = fakeApi();
    const store = createEditorStore({ api });
    await store.getState().loadGuides();
    expect(store.getState().folders).toEqual(["Work"]);
    api.listFolders = vi.fn(async () => Promise.reject({ kind: "io", message: "gone" }));
    vi.spyOn(console, "error").mockImplementation(() => {});
    await store.getState().loadGuides();
    expect(store.getState().folders).toEqual(["Work"]);
    expect(store.getState().listError).toBeNull();
  });

  it("makes folders and lists them again", async () => {
    const { api } = fakeApi();
    const store = createEditorStore({ api });
    expect(await store.getState().createFolder("Work", "Team")).toBe(true);
    expect(api.createFolder).toHaveBeenCalledWith("Work", "Team");
    expect(api.listFolders).toHaveBeenCalled();
    api.createFolder = vi.fn(async () => Promise.reject({ kind: "exists", message: "exists" }));
    expect(await store.getState().createFolder("", "Work")).toBe(false);
    expect(store.getState().notice).toBe("Could not make the folder: exists");
  });

  it("duplicates under a free copy title", async () => {
    const { api } = fakeApi();
    const store = createEditorStore({ api });
    await store.getState().loadGuides();
    expect(await store.getState().duplicateGuide(store.getState().guides[0])).toBe(true);
    expect(api.duplicateGuide).toHaveBeenCalledWith("G1", "/lib/Guide.guiderecorder", "Guide copy");
    expect(store.getState().busy).toBe(false);
  });

  it("saves the open guide before duplicating it", async () => {
    const { api, s, saved } = await opened();
    await s().loadGuides();
    s().dispatch(renameGuide(s().doc!.guide, "Edited"));
    expect(await s().duplicateGuide(s().guides[0])).toBe(true);
    expect(saved.at(-1)?.title).toBe("Edited");
    expect(vi.mocked(api.saveGuide).mock.invocationCallOrder[0]).toBeLessThan(
      vi.mocked(api.duplicateGuide).mock.invocationCallOrder[0],
    );
  });

  it("renames the open guide: saved first, then its title and path follow", async () => {
    const { api, s, saved } = await opened();
    await s().loadGuides();
    s().dispatch(renameGuide(s().doc!.guide, "Edited"));
    expect(await s().renameGuide(s().guides[0], "  New name ")).toBe(true);
    expect(saved.at(-1)?.title).toBe("Edited");
    expect(api.renameGuide).toHaveBeenCalledWith("G1", "/lib/Guide.guiderecorder", "New name");
    expect(s().doc?.guide.title).toBe("New name");
    expect(s().doc?.path).toBe("/lib/New name.guiderecorder");
    // Nothing left to save: the backend wrote the new title.
    expect(s().saveStatus).toBe("saved");
    await vi.advanceTimersByTimeAsync(1000);
    expect(saved.at(-1)?.title).toBe("Edited");
  });

  it("moves the open guide and keeps saving to its new place", async () => {
    const { api, s } = await opened();
    await s().loadGuides();
    expect(await s().moveGuide(s().guides[0], "Work")).toBe(true);
    expect(api.moveGuide).toHaveBeenCalledWith("G1", "/lib/Guide.guiderecorder", "Work");
    expect(s().doc?.path).toBe("/lib/Work/Guide.guiderecorder");
  });

  it("reports failed moves and leaves the open guide alone", async () => {
    const { api, s } = await opened();
    await s().loadGuides();
    api.moveGuide = vi.fn(async () => Promise.reject({ kind: "invalid", message: "no such folder" }));
    expect(await s().moveGuide(s().guides[0], "Nope")).toBe(false);
    expect(s().notice).toBe("Could not move the guide: no such folder");
    expect(s().doc?.path).toBe("/lib/Guide.guiderecorder");
    expect(s().busy).toBe(false);
  });

  it("does not organize while busy", async () => {
    const { api, s, block, unblock } = await opened();
    await s().loadGuides();
    s().dispatch(renameGuide(s().doc!.guide, "Edited"));
    block();
    const closing = s().closeGuide();
    expect(await s().moveGuide(s().guides[0], "Work")).toBe(false);
    unblock();
    await closing;
    expect(api.moveGuide).not.toHaveBeenCalled();
  });

  it("keeps the sort and tag filter", () => {
    const { api } = fakeApi();
    const store = createEditorStore({ api });
    expect(store.getState().sort).toBe("updated");
    store.getState().setSort("title");
    store.getState().setTag("security");
    expect(store.getState().sort).toBe("title");
    expect(store.getState().tag).toBe("security");
  });
});

describe("new annotation guides", () => {
  it("are listed and opened once made", async () => {
    const { api } = fakeApi();
    const store = createEditorStore({ api });
    expect(await store.getState().createAnnotatedGuide()).toBe(true);
    expect(api.newAnnotatedGuide).toHaveBeenCalledOnce();
    expect(api.listGuides).toHaveBeenCalled();
    expect(store.getState().doc?.guide.id).toBe("G2");
    expect(store.getState().creating).toBe(false);
  });

  it("cancelled: nothing opens", async () => {
    const { api } = fakeApi();
    api.newAnnotatedGuide = vi.fn(async () => null);
    const store = createEditorStore({ api });
    expect(await store.getState().createAnnotatedGuide()).toBe(false);
    expect(api.openGuide).not.toHaveBeenCalled();
    expect(store.getState().openError).toBeNull();
  });

  it("reports failures like open errors", async () => {
    const { api } = fakeApi();
    api.newAnnotatedGuide = vi.fn(async () => Promise.reject({ kind: "invalid", message: "not a PDF" }));
    const store = createEditorStore({ api });
    expect(await store.getState().createAnnotatedGuide()).toBe(false);
    expect(store.getState().openError).toBe("Could not make the annotation guide: not a PDF");
    expect(store.getState().creating).toBe(false);
    expect(store.getState().doc).toBeNull();
  });
});

describe("deleting", () => {
  it("deletes the open guide without saving it, and closes it", async () => {
    const { api, s } = await opened();
    await s().loadGuides();
    s().dispatch(renameGuide(s().doc!.guide, "Renamed"));
    expect(await s().deleteGuide(s().guides[0])).toBe(true);
    expect(api.deleteGuide).toHaveBeenCalledWith("G1", "/lib/Guide.guiderecorder", "Guide");
    expect(s().doc).toBeNull();
    await vi.advanceTimersByTimeAsync(1000);
    expect(api.saveGuide).not.toHaveBeenCalled();
  });

  it("cancelled keeps the guide open and its changes saving", async () => {
    const { api, s, saved } = await opened();
    await s().loadGuides();
    api.deleteGuide = vi.fn(async () => false);
    s().dispatch(renameGuide(s().doc!.guide, "Renamed"));
    expect(await s().deleteGuide(s().guides[0])).toBe(false);
    expect(s().doc?.guide.title).toBe("Renamed");
    expect(s().busy).toBe(false);
    await vi.advanceTimersByTimeAsync(1000);
    expect(saved.at(-1)?.title).toBe("Renamed");
  });

  it("reports failures", async () => {
    const { api, s } = await opened();
    await s().loadGuides();
    api.deleteGuide = vi.fn(async () => Promise.reject({ kind: "io", message: "disk full" }));
    expect(await s().deleteGuide(s().guides[0])).toBe(false);
    expect(s().notice).toBe("Could not delete the guide: disk full");
    expect(s().doc).not.toBeNull();
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
    expect(api.cancelQuit).not.toHaveBeenCalled();
  });

  it("staying in the leave dialog of a quit tells the backend at once", async () => {
    const { s, api } = await failing();
    expect(await s().requestQuit()).toBe(false);
    expect(api.cancelQuit).not.toHaveBeenCalled();
    expect(await s().resolveLeave("cancel")).toBe(false);
    expect(api.cancelQuit).toHaveBeenCalledTimes(1);
    expect(api.quit).not.toHaveBeenCalled();
  });

  it("staying when closing a guide is not a cancelled quit", async () => {
    const { s, api } = await failing();
    expect(await s().closeGuide()).toBe(false);
    expect(await s().resolveLeave("cancel")).toBe(false);
    expect(api.cancelQuit).not.toHaveBeenCalled();
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

describe("selection hints", () => {
  it("follow dispatch, undo and redo", async () => {
    const { s } = await opened();
    s().select("B");
    s().dispatch(deleteItem(s().doc!.guide, "B"));
    expect(s().selectedId).toBe("N1");
    s().undo();
    expect(s().selectedId).toBe("B");
    s().redo();
    expect(s().selectedId).toBe("N1");
    s().dispatch(insertItemAfter(s().doc!.guide, newNote("T1", "tip"), "A"));
    expect(s().selectedId).toBe("T1");
    // Commands without a hint keep the selection.
    s().dispatch(renameGuide(s().doc!.guide, "x"));
    expect(s().selectedId).toBe("T1");
  });
});

describe("closing in the backend", () => {
  it("happens after saving, on close, open and quit", async () => {
    const { s, api, saved } = await opened();
    s().dispatch(renameGuide(s().doc!.guide, "Saved first"));
    vi.mocked(api.closeGuide).mockImplementation(async () => {
      expect(saved.map((g) => g.title)).toEqual(["Saved first"]);
      return [];
    });
    expect(await s().openGuide("G2")).toBe(true);
    expect(api.closeGuide).toHaveBeenLastCalledWith("G1");
    expect(await s().closeGuide()).toBe(true);
    expect(api.closeGuide).toHaveBeenLastCalledWith("G2");
    await s().openGuide("G3");
    await s().requestQuit();
    expect(api.closeGuide).toHaveBeenLastCalledWith("G3");
    expect(vi.mocked(api.closeGuide).mock.invocationCallOrder.at(-1)!).toBeLessThan(
      vi.mocked(api.quit).mock.invocationCallOrder[0],
    );
  });

  it("not when leaving is refused or the new guide fails to open", async () => {
    const { s, api } = await opened();
    api.saveGuide = vi.fn(async () => Promise.reject({ kind: "io", message: "disk full" }));
    s().dispatch(renameGuide(s().doc!.guide, "Unsaved"));
    expect(await s().closeGuide()).toBe(false);
    expect(api.closeGuide).not.toHaveBeenCalled();

    await s().resolveLeave("discard");
    expect(api.closeGuide).toHaveBeenCalledWith("G1");
    vi.mocked(api.closeGuide).mockClear();
    await s().openGuide("G1");
    api.openGuide = vi.fn(async () => Promise.reject({ kind: "notFound", message: "gone" }));
    await s().openGuide("G9");
    expect(s().doc?.guide.id).toBe("G1");
    expect(api.closeGuide).not.toHaveBeenCalled();
  });

  it("errors do not stop leaving", async () => {
    const { s, api } = await opened();
    const log = vi.spyOn(console, "error").mockImplementation(() => {});
    api.closeGuide = vi.fn(async () => Promise.reject({ kind: "io", message: "busy" }));
    expect(await s().closeGuide()).toBe(true);
    expect(s().doc).toBeNull();
    expect(log).toHaveBeenCalled();
    log.mockRestore();
  });
});

describe("quitting", () => {
  it("waits while the editor is busy, then quits once", async () => {
    const { s, api, block, unblock } = await opened();
    s().dispatch(renameGuide(s().doc!.guide, "One"));
    block();
    const closing = s().closeGuide();
    await vi.advanceTimersByTimeAsync(0);
    const first = s().requestQuit();
    const second = s().requestQuit();
    expect(s().notice).toBe("Quitting once the guide is saved…");
    expect(api.quit).not.toHaveBeenCalled();
    unblock();
    expect(await closing).toBe(true);
    expect(await first).toBe(true);
    expect(await second).toBe(true);
    expect(api.quit).toHaveBeenCalledTimes(1);
    expect(s().notice).toBeNull();
  });

  it("a request during a quit joins it", async () => {
    const { s, api, block, unblock } = await opened();
    s().dispatch(renameGuide(s().doc!.guide, "One"));
    block();
    const first = s().requestQuit();
    await vi.advanceTimersByTimeAsync(0);
    const second = s().requestQuit();
    unblock();
    expect(await first).toBe(true);
    expect(await second).toBe(true);
    expect(api.quit).toHaveBeenCalledTimes(1);
  });

  it("a rejected quit shows an error and leaves the guide open and editable", async () => {
    const { s, api } = await opened();
    vi.mocked(api.quit).mockRejectedValueOnce({ kind: "internal", message: "no app" });
    expect(await s().requestQuit()).toBe(false);
    expect(s().notice).toBe("Could not quit: no app");
    expect(s().busy).toBe(false);
    expect(s().leave).toBeNull();
    // Closed in the backend, so opened again.
    expect(api.closeGuide).toHaveBeenCalledWith("G1");
    expect(api.openGuide).toHaveBeenCalledTimes(2);
    expect(s().doc?.guide.id).toBe("G1");
    expect(s().dispatch(renameGuide(s().doc!.guide, "Still editing"))).toBe(true);
    // And quitting again works.
    expect(await s().requestQuit()).toBe(true);
    expect(api.quit).toHaveBeenCalledTimes(2);
  });

  function steps(initial: string | null = null) {
    let reason = initial;
    const listeners = new Set<() => void>();
    const fake = {
      waitReason: vi.fn(() => reason),
      subscribe: vi.fn((listener: () => void) => {
        listeners.add(listener);
        return () => listeners.delete(listener);
      }),
      prepare: vi.fn(async () => true),
    } satisfies QuitSteps;
    return {
      fake,
      listeners,
      set(next: string | null) {
        reason = next;
        for (const listener of [...listeners]) listener();
      },
    };
  }

  it("the quit flow waits for its steps, shows why, and runs once", async () => {
    const { store, s, api } = await opened();
    const recording = steps("Quitting once the recording is saved…");
    const quit = createQuitFlow(store, recording.fake);
    const first = quit();
    const second = quit();
    expect(first).toBe(second);
    await vi.advanceTimersByTimeAsync(0);
    expect(s().notice).toBe("Quitting once the recording is saved…");
    expect(recording.fake.prepare).not.toHaveBeenCalled();
    recording.set("Quitting once the recording command finishes…");
    await vi.advanceTimersByTimeAsync(0);
    expect(recording.fake.prepare).not.toHaveBeenCalled();
    recording.set(null);
    await first;
    expect(recording.fake.prepare).toHaveBeenCalledTimes(1);
    expect(api.quit).toHaveBeenCalledTimes(1);
    expect(recording.listeners.size).toBe(0);
    expect(s().notice).toBeNull();
  });

  it("the quit flow follows the wait reason: pending, then saving, then idle", async () => {
    const { store, s, api } = await opened();
    const recording = steps("Quitting once the recording command finishes…");
    const quit = createQuitFlow(store, recording.fake);
    const done = quit();
    await vi.advanceTimersByTimeAsync(0);
    expect(s().notice).toBe("Quitting once the recording command finishes…");
    recording.set("Quitting once the recording is saved…");
    expect(s().notice).toBe("Quitting once the recording is saved…");
    expect(recording.fake.prepare).not.toHaveBeenCalled();
    recording.set(null);
    await done;
    expect(s().notice).toBeNull();
    expect(api.quit).toHaveBeenCalledTimes(1);
  });

  it("the quit flow leaves a notice it did not set", async () => {
    const { store, s } = await opened();
    const recording = steps("Quitting once the recording command finishes…");
    const quit = createQuitFlow(store, recording.fake);
    const done = quit();
    s().notify("Recording failed: disk full");
    recording.set("Quitting once the recording is saved…");
    expect(s().notice).toBe("Recording failed: disk full");
    recording.set(null);
    await done;
    expect(s().notice).toBe("Recording failed: disk full");
  });

  it("the quit flow stays open when preparing fails, and can quit again", async () => {
    const { store, api } = await opened();
    const recording = steps();
    recording.fake.prepare.mockResolvedValueOnce(false);
    const quit = createQuitFlow(store, recording.fake);
    await quit();
    expect(api.quit).not.toHaveBeenCalled();
    expect(api.cancelQuit).toHaveBeenCalledTimes(1);
    await quit();
    expect(api.quit).toHaveBeenCalledTimes(1);
    expect(api.cancelQuit).toHaveBeenCalledTimes(1);
  });

  it("the quit flow shows an error that escapes it and is not stuck", async () => {
    const { store, s, api } = await opened();
    const recording = steps();
    recording.fake.prepare.mockRejectedValueOnce(new Error("stop failed"));
    const quit = createQuitFlow(store, recording.fake);
    await quit();
    expect(s().notice).toBe("Could not quit: stop failed");
    expect(api.cancelQuit).toHaveBeenCalledTimes(1);
    await quit();
    expect(api.quit).toHaveBeenCalledTimes(1);
  });
});

describe("AI state of edits", () => {
  it("marks the guide edited on a guide-level edit, undoably", async () => {
    const { s } = await opened();
    s().dispatch(setGuideField(s().doc!.guide, "intro", "Hello"));
    expect(s().doc!.guide.aiState).toEqual({ status: "edited" });
    s().undo();
    expect(s().doc!.guide.aiState).toBeUndefined();
    expect(s().doc!.guide.intro).toBe("");
    s().redo();
    expect(s().doc!.guide).toMatchObject({ intro: "Hello", aiState: { status: "edited" } });
  });

  it("typing a step title and deleting it back leaves the step as it was", async () => {
    const { s } = await opened();
    s().dispatch(setItemTitle(s().doc!.guide, "A", "Step Ax"));
    s().dispatch(setItemTitle(s().doc!.guide, "A", "Step A"));
    expect(s().undoStack).toHaveLength(0);
    expect(s().doc!.guide.items[1]).toMatchObject({ aiState: { status: "template" } });
  });

  it("keeps the AI fields when saving", async () => {
    const { s, saved } = await opened();
    const aiState = { status: "ai" as const, error: "boom", redactionSuggestions: [{ rect: { x: 1, y: 2, width: 3, height: 4 }, reason: "email" }] };
    s().applyUntracked([
      { op: "setGuide", key: "aiState", value: { status: "ai", model: "m" } },
      { op: "setItem", id: "A", key: "aiState", value: aiState },
    ]);
    await s().flush();
    expect(saved.at(-1)!.aiState).toEqual({ status: "ai", model: "m" });
    expect(saved.at(-1)!.items[1]).toMatchObject({ aiState });
  });
});

describe("untracked changes", () => {
  it("autosave but stay out of the undo history", async () => {
    const { s, saved } = await opened();
    s().dispatch(renameGuide(s().doc!.guide, "Mine"));
    expect(s().applyUntracked([{ op: "setItem", id: "B", key: "title", value: "AI title" }])).toBe(true);
    expect(s().undoStack).toHaveLength(1);
    expect(s().saveStatus).toBe("pending");
    await vi.advanceTimersByTimeAsync(500);
    expect(saved.at(-1)!.items[2]).toMatchObject({ title: "AI title" });

    // Undo reverts the user's edit only.
    s().undo();
    expect(s().doc!.guide.title).toBe("Guide");
    expect(s().doc!.guide.items[2]).toMatchObject({ title: "AI title" });
  });

  it("are refused when they no longer apply or the guide is read-only", async () => {
    const { s } = await opened();
    expect(s().applyUntracked([{ op: "setItem", id: "gone", key: "title", value: "x" }])).toBe(false);
    expect(s().revision).toBe(0);
    const ro = await opened({ readOnly: true });
    expect(ro.s().applyUntracked([{ op: "setItem", id: "A", key: "title", value: "x" }])).toBe(false);
  });
});

describe("history after untracked changes", () => {
  it("keeps entries when only an AI error is recorded", async () => {
    const { s } = await opened();
    s().dispatch(setItemTitle(s().doc!.guide, "A", "Mine"));
    clock += 5000;
    s().dispatch(renameGuide(s().doc!.guide, "My guide"));
    const stepState = (s().doc!.guide.items[1] as Step).aiState;
    s().applyUntracked([{ op: "setItem", id: "A", key: "aiState", value: { ...stepState, error: "rate limited" } }]);
    s().applyUntracked([{ op: "setGuide", key: "aiState", value: { ...s().doc!.guide.aiState, error: "overloaded" } }]);
    expect(s().undoStack).toHaveLength(2);
    // A changed status is a write: it drops them.
    s().applyUntracked([{ op: "setItem", id: "A", key: "aiState", value: { status: "ai" } }]);
    expect(s().undoStack).toHaveLength(1);
  });

  it("drops entries whose stale guide field would come back over AI text", async () => {
    const { s } = await opened();
    s().dispatch(renameGuide(s().doc!.guide, "Mine"));
    s().undo();
    // The guide is no longer edited, so AI writes the title.
    s().applyUntracked([
      { op: "setGuide", key: "title", value: "AI title" },
      { op: "setGuide", key: "aiState", value: { status: "ai" } },
    ]);
    expect(s().redoStack).toHaveLength(0);
    s().redo();
    s().undo();
    expect(s().doc!.guide).toMatchObject({ title: "AI title", aiState: { status: "ai" } });
  });

  it("drops entries whose stale step text would come back over AI text", async () => {
    const { s } = await opened();
    s().dispatch(setItemTitle(s().doc!.guide, "A", "Mine"));
    clock += 5000;
    s().dispatch(moveItem(s().doc!.guide, "C", 1));
    clock += 5000;
    s().undo(); // the move
    s().undo(); // the title edit
    s().applyUntracked([
      { op: "setItem", id: "A", key: "title", value: "AI title" },
      { op: "setItem", id: "A", key: "aiState", value: { status: "ai" } },
    ]);
    // The move does not touch the step's text, so it stays.
    expect(s().redoStack.map((entry) => entry.command.label)).toEqual(["Move item"]);
    s().redo();
    s().undo();
    expect(s().doc!.guide.items[1]).toMatchObject({ id: "A", title: "AI title", aiState: { status: "ai" } });
  });

  it("drops an undoable delete of a step AI then wrote", async () => {
    const { s } = await opened();
    s().dispatch(deleteItem(s().doc!.guide, "B"));
    s().undo();
    s().applyUntracked([{ op: "setItem", id: "B", key: "title", value: "AI title" }]);
    expect(s().redoStack).toHaveLength(0);
  });

  it("keeps entries for other fields and items", async () => {
    const { s } = await opened();
    s().dispatch(setItemTitle(s().doc!.guide, "B", "Mine"));
    s().applyUntracked([{ op: "setItem", id: "A", key: "title", value: "AI title" }]);
    expect(s().undoStack).toHaveLength(1);
  });
});
