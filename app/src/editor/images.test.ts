import { afterEach, describe, expect, it, vi } from "vitest";
import { fitSize, insertImageSteps, isImagePath, MAX_PIXELS, MAX_SIDE, toPng } from "@/editor/images";
import { createEditorStore } from "@/editor/store";
import { guide } from "@/editor/test-fixtures";
import type { EditorApi } from "@/lib/ipc";
import type { Step } from "@guiderecorder/guide-schema";

const ipc = vi.hoisted(() => ({ addImageStep: vi.fn(), readImageFile: vi.fn() }));
vi.mock("@/lib/ipc", async (importOriginal) => ({ ...(await importOriginal<object>()), ...ipc }));

describe("image sizes", () => {
  it("keep small images and scale big ones down, keeping the shape", () => {
    expect(fitSize(1440, 900)).toEqual({ width: 1440, height: 900 });
    expect(fitSize(40000, 100)).toEqual({ width: MAX_SIDE, height: 40 });
    const big = fitSize(10000, 8000);
    expect(big.width * big.height).toBeLessThanOrEqual(MAX_PIXELS);
    expect(big.width / big.height).toBeCloseTo(1.25, 2);
    expect(fitSize(1, 100000)).toEqual({ width: 1, height: MAX_SIDE });
  });

  it("recognise image files by extension", () => {
    expect(isImagePath("/Users/a/Desktop/Screen Shot.PNG")).toBe(true);
    expect(isImagePath("C:\\\\shots\\\\a.heic")).toBe(true);
    expect(isImagePath("/tmp/notes.txt")).toBe(false);
    expect(isImagePath("/tmp/png")).toBe(false);
  });
});

describe("insertImageSteps", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    ipc.addImageStep.mockReset();
  });

  /** Stand-ins for the webview's image, bitmap and canvas; blobs say their size as "WxH". */
  function stubDecoding() {
    const bitmaps: { options: ImageBitmapOptions | undefined }[] = [];
    vi.stubGlobal(
      "Image",
      class {
        naturalWidth = 0;
        naturalHeight = 0;
        onload: (() => void) | null = null;
        onerror: (() => void) | null = null;
        set src(url: string) {
          void fetch(url)
            .then((r) => r.text())
            .then((text) => {
              const size = /^(\d+)x(\d+)$/.exec(text);
              if (!size) return this.onerror?.();
              this.naturalWidth = Number(size[1]);
              this.naturalHeight = Number(size[2]);
              this.onload?.();
            });
        }
      },
    );
    vi.stubGlobal("createImageBitmap", async (_blob: Blob, options?: ImageBitmapOptions) => {
      bitmaps.push({ options });
      return { width: options?.resizeWidth, height: options?.resizeHeight, close: vi.fn() };
    });
    vi.stubGlobal(
      "OffscreenCanvas",
      class {
        constructor(
          public width: number,
          public height: number,
        ) {}
        getContext() {
          return { drawImage: vi.fn() };
        }
        async convertToBlob(options: { type: string }) {
          return new Blob([`${options.type} ${this.width}x${this.height}`]);
        }
      },
    );
    return bitmaps;
  }

  const text = (bytes: Uint8Array) => new TextDecoder().decode(bytes);

  it("re-encode as PNG, scaled to fit, sizing before decoding", async () => {
    const bitmaps = stubDecoding();
    expect(text(await toPng(new Blob(["3x2"])))).toBe("image/png 3x2");
    const big = fitSize(20000, 9000);
    expect(text(await toPng(new Blob(["20000x9000"])))).toBe(`image/png ${big.width}x${big.height}`);
    expect(bitmaps.at(-1)?.options).toMatchObject({
      resizeWidth: big.width,
      resizeHeight: big.height,
      imageOrientation: "from-image",
    });
    // Too many pixels to decode at all: refused before any bitmap.
    await expect(toPng(new Blob(["16000x16000"]))).rejects.toThrow("too large");
    await expect(toPng(new Blob(["not an image"]))).rejects.toThrow("not an image");
    expect(bitmaps).toHaveLength(2);
  });

  async function store() {
    const api = {
      openGuide: async () => ({ guide: guide(), path: "", readOnly: false, migratedFrom: null, migrationError: null }),
    } as unknown as EditorApi;
    const s = createEditorStore({ api });
    await s.getState().openGuide("G1");
    return s;
  }

  const manualStep = (id: string): Omit<Step, "type"> => ({
    id,
    title: "",
    description: "",
    kind: "manual",
    image: { full: `steps/${id}.webp`, show: "full" },
    annotations: [],
    aiState: { status: "template" },
  });

  it("adds each image as a step after the previous one, undoably", async () => {
    stubDecoding();
    ipc.addImageStep.mockImplementation(async (_guide: string, png: Uint8Array) => {
      expect(text(png)).toBe("image/png 3x2");
      return manualStep(`M${ipc.addImageStep.mock.calls.length}`);
    });
    const s = await store();
    const added = await insertImageSteps(s, [new Blob(["3x2"]), async () => new Blob(["3x2"])], "A");
    expect(added.map((step) => step.id)).toEqual(["M1", "M2"]);
    expect(ipc.addImageStep).toHaveBeenCalledWith("G1", expect.anything());
    const ids = () => s.getState().doc!.guide.items.map((item) => item.id);
    expect(ids()).toEqual(["S1", "A", "M1", "M2", "B", "N1", "C"]);
    expect(s.getState().doc!.guide.items[2]).toMatchObject({ type: "step", kind: "manual" });
    expect(s.getState().selectedId).toBe("M2");
    s.getState().undo();
    s.getState().undo();
    expect(ids()).toEqual(["S1", "A", "B", "N1", "C"]);
  });

  it("stops at an unreadable image, keeping the steps added before it", async () => {
    stubDecoding();
    ipc.addImageStep.mockImplementation(async () => manualStep("M1"));
    const s = await store();
    await expect(insertImageSteps(s, [new Blob(["3x2"]), new Blob(["junk"])], null)).rejects.toThrow(
      "not an image the app can read",
    );
    expect(s.getState().doc!.guide.items.at(-1)?.id).toBe("M1");
  });
});
