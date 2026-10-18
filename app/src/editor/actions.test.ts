import { describe, expect, it, vi } from "vitest";
import {
  canMergeSelected,
  deleteSelected,
  insertImages,
  insertNote,
  insertPages,
  insertSection,
  mergeSelected,
  splitSelected,
} from "@/editor/actions";
import { createEditorStore } from "@/editor/store";
import { guide, step } from "@/editor/test-fixtures";
import type { EditorApi } from "@/lib/ipc";
import type { Step, StepImage } from "@/lib/types";

const images = vi.hoisted(() => ({ insertImageSteps: vi.fn() }));
vi.mock("@/editor/images", async (importOriginal) => ({ ...(await importOriginal<object>()), ...images }));
const ipc = vi.hoisted(() => ({
  addAnnotatedPages: vi.fn(async (_guide: string): Promise<Step[]> => []),
  copyStepImages: vi.fn(async (_guide: string, image: StepImage, id: string): Promise<StepImage> => ({
    ...image,
    full: `steps/${id}.webp`,
    crop: image.crop && `steps/${id}.crop.webp`,
  })),
}));
vi.mock("@/lib/ipc", async (importOriginal) => ({ ...(await importOriginal<object>()), ...ipc }));

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

    expect(await splitSelected(s, ["", ""], "A2")).toBe(true);
    expect(ids(s)).toEqual(["S1", "A", "A2", "N1", "C"]);
    expect(s.getState().selectedId).toBe("A2");

    expect(deleteSelected(s)).toBe(true);
    expect(ids(s)).toEqual(["S1", "A", "N1", "C"]);
    expect(s.getState().selectedId).toBe("N1");
    expect(await splitSelected(s, ["", ""], "X")).toBe(false); // a note is selected

    s.getState().undo();
    s.getState().undo();
    s.getState().undo();
    expect(ids(s)).toEqual(guide().items.map((item) => item.id));
  });

  it("split gives the new step its own copy of the screenshot", async () => {
    const s = await store();
    s.getState().select("A");
    expect(await splitSelected(s, ["", "Later."], "A2")).toBe(true);
    const original = guide().items[1] as Step;
    expect(ipc.copyStepImages).toHaveBeenLastCalledWith("G1", original.image, "A2");
    const added = s.getState().doc!.guide.items[2] as Step;
    expect(added.image).toEqual({ ...original.image, full: "steps/A2.webp", crop: "steps/A2.crop.webp" });
    expect(s.getState().doc!.guide.items[1]).toEqual(original);
  });

  it("split reports a failed copy and changes nothing", async () => {
    const s = await store();
    s.getState().select("A");
    ipc.copyStepImages.mockRejectedValueOnce({ kind: "io", message: "disk full" });
    expect(await splitSelected(s, ["", ""], "A2")).toBe(false);
    expect(s.getState().notice).toBe("Could not split the step: disk full");
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

  it("add pages below the selection as one undo step", async () => {
    const s = await store();
    s.getState().select("B");
    // As from Rust: a bare step, without its `type` tag.
    const page = (id: string) => {
      const { type: _type, ...rest } = { ...step(id), kind: "manual" as const, image: { full: `steps/${id}.webp`, show: "full" as const } };
      return rest as unknown as Step;
    };
    ipc.addAnnotatedPages.mockResolvedValueOnce([page("P1"), page("P2")]);
    expect(await insertPages(s)).toBe(2);
    expect(ipc.addAnnotatedPages).toHaveBeenLastCalledWith("G1");
    expect(ids(s)).toEqual(["S1", "A", "B", "P1", "P2", "N1", "C"]);
    expect(s.getState().doc!.guide.items[3]).toMatchObject({ type: "step", id: "P1", kind: "manual" });
    expect(s.getState().selectedId).toBe("P1");
    expect(s.getState().undoStack.at(-1)?.command.label).toBe("Add 2 pages");
    s.getState().undo();
    expect(ids(s)).toEqual(guide().items.map((item) => item.id));
    expect(s.getState().selectedId).toBe("B");

    // Cancelled: nothing.
    ipc.addAnnotatedPages.mockResolvedValueOnce([]);
    expect(await insertPages(s)).toBe(0);
    expect(s.getState().undoStack).toHaveLength(0);
  });

  it("report failed pages as a notice", async () => {
    const s = await store();
    ipc.addAnnotatedPages.mockRejectedValueOnce({ kind: "invalid", message: "not a PDF" });
    expect(await insertPages(s)).toBe(0);
    expect(s.getState().notice).toBe("Could not add the pages: not a PDF");
    const readOnly = await store(true);
    ipc.addAnnotatedPages.mockClear();
    expect(await insertPages(readOnly)).toBe(0);
    expect(ipc.addAnnotatedPages).not.toHaveBeenCalled();
  });
});
