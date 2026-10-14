import { describe, expect, it, vi } from "vitest";
import {
  canMergeSelected,
  deleteSelected,
  insertImages,
  insertNote,
  insertSection,
  mergeSelected,
  splitSelected,
} from "@/editor/actions";
import { createEditorStore } from "@/editor/store";
import { guide } from "@/editor/test-fixtures";
import type { EditorApi } from "@/lib/ipc";

const images = vi.hoisted(() => ({ insertImageSteps: vi.fn() }));
vi.mock("@/editor/images", async (importOriginal) => ({ ...(await importOriginal<object>()), ...images }));

async function store(readOnly = false) {
  const api = {
    openGuide: async () => ({ guide: guide(), path: "", readOnly, migratedFrom: null, migrationError: null }),
  } as unknown as EditorApi;
  const s = createEditorStore({ api });
  await s.getState().openGuide("G1");
  return s;
}

const ids = (s: Awaited<ReturnType<typeof store>>) => s.getState().doc!.guide.items.map((item) => item.id);

describe("actions", () => {
  it("insert around the selected item", async () => {
    const s = await store();
    s.getState().select("B");
    expect(insertSection(s, "S2")).toBe(true);
    expect(ids(s)).toEqual(["S1", "A", "S2", "B", "N1", "C"]);
    expect(insertNote(s, "warning", "W1")).toBe(true); // below the new section
    expect(ids(s)).toEqual(["S1", "A", "S2", "W1", "B", "N1", "C"]);
    s.getState().select(null);
    insertNote(s, "tip", "T1");
    expect(ids(s).at(-1)).toBe("T1");
    s.getState().select(null);
    insertSection(s, "S3");
    expect(ids(s)[0]).toBe("S3");
    // The new item is selected, so the next one goes next to it.
    insertNote(s, "note", "N2");
    expect(ids(s).slice(0, 2)).toEqual(["S3", "N2"]);
  });

  it("delete, merge and split the selection", async () => {
    const s = await store();
    s.getState().select("A");
    expect(canMergeSelected(s)).toBe(true);
    expect(mergeSelected(s)).toBe(true);
    expect(ids(s)).toEqual(["S1", "A", "N1", "C"]);
    expect(canMergeSelected(s)).toBe(false); // a note follows
    expect(mergeSelected(s)).toBe(false);

    expect(splitSelected(s, ["", ""], "A2")).toBe(true);
    expect(ids(s)).toEqual(["S1", "A", "A2", "N1", "C"]);
    expect(s.getState().selectedId).toBe("A2");

    expect(deleteSelected(s)).toBe(true);
    expect(ids(s)).toEqual(["S1", "A", "N1", "C"]);
    expect(s.getState().selectedId).toBe("N1");
    expect(splitSelected(s, ["", ""], "X")).toBe(false); // a note is selected

    s.getState().undo();
    s.getState().undo();
    s.getState().undo();
    expect(ids(s)).toEqual(guide().items.map((item) => item.id));
  });

  it("do nothing in read-only guides", async () => {
    const s = await store(true);
    s.getState().select("A");
    expect(insertSection(s)).toBe(false);
    expect(insertNote(s, "tip")).toBe(false);
    expect(deleteSelected(s)).toBe(false);
    expect(mergeSelected(s)).toBe(false);
    await insertImages(s, [new Blob(["x"])]);
    expect(images.insertImageSteps).not.toHaveBeenCalled();
    expect(ids(s)).toEqual(guide().items.map((item) => item.id));
  });

  it("report image failures as a notice", async () => {
    const s = await store();
    s.getState().select("C");
    images.insertImageSteps.mockRejectedValueOnce(new Error("bad file"));
    await insertImages(s, [new Blob(["x"])]);
    expect(images.insertImageSteps).toHaveBeenCalledWith(s, expect.any(Array), "C");
    expect(s.getState().notice).toBe("Could not add the image: bad file");
    images.insertImageSteps.mockResolvedValueOnce([]);
    await insertImages(s, [new Blob(["y"])]);
    expect(s.getState().notice).toBeNull();
  });
});
