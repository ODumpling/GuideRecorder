import { describe, expect, it } from "vitest";
import {
  activeCrop,
  clampPoint,
  cropFits,
  drawOrder,
  arrowShape,
  blurAmount,
  calloutEdge,
  clientToImage,
  createAnnotation,
  dragAnnotation,
  handlePoint,
  imagePixelsPerScreenPixel,
  imageView,
  maskOutside,
  MIN_SIZE,
  resizeRect,
  strokeWidth,
} from "@/annotations/geometry";
import type { Annotation, StepImage } from "@/lib/types";

const crop: StepImage = {
  full: "steps/A.webp",
  crop: "steps/A.crop.webp",
  cropRect: { x: 400, y: 300, width: 320, height: 200 },
  show: "crop",
};

describe("image view", () => {
  it("shows the crop where it sits in the full screenshot", () => {
    expect(imageView(crop)).toEqual({ path: "steps/A.crop.webp", rect: crop.cropRect, cropUnknown: false });
  });

  it("falls back to the full screenshot when the crop's place is unknown", () => {
    expect(imageView({ ...crop, cropRect: undefined })).toEqual({
      path: "steps/A.webp",
      rect: null,
      cropUnknown: "missing",
    });
    expect(imageView({ ...crop, show: "full" })).toEqual({ path: "steps/A.webp", rect: null, cropUnknown: false });
    expect(imageView({ full: "steps/M.webp", show: "crop" }).cropUnknown).toBe(false);
    expect(imageView({ ...crop, cropRect: undefined }).cropUnknown).toBe("missing");
  });

  it("checks the recorded crop position against the crop image (±1 px, like the exporter)", () => {
    expect(cropFits(crop.cropRect!, { width: 400, height: 250 })).toBe(false);
    expect(cropFits(crop.cropRect!, { width: 320, height: 200 })).toBe(true);
    expect(cropFits(crop.cropRect!, { width: 321, height: 199 })).toBe(true);
    expect(cropFits(crop.cropRect!, { width: 322, height: 200 })).toBe(false);
    // A crop that doesn't fit is set aside for the full screenshot.
    expect(imageView(crop, true)).toEqual({ path: "steps/A.webp", rect: null, cropUnknown: "mismatch" });
  });
});

describe("pointer mapping", () => {
  const view = crop.cropRect!;

  it("maps client pixels into full-screenshot pixels through the crop", () => {
    // Shown at 2× in a box of the same shape.
    const box = { left: 100, top: 50, width: 640, height: 400 };
    expect(clientToImage({ x: 100, y: 50 }, box, view)).toEqual({ x: 400, y: 300 });
    expect(clientToImage({ x: 740, y: 450 }, box, view)).toEqual({ x: 720, y: 500 });
    expect(clientToImage({ x: 420, y: 250 }, box, view)).toEqual({ x: 560, y: 400 });
    expect(imagePixelsPerScreenPixel(box, view)).toBe(0.5);
  });

  it("accounts for letterboxing (xMidYMid meet)", () => {
    // A box wider than the view: 100 px bars left and right at scale 1.
    const box = { left: 0, top: 0, width: 520, height: 200 };
    expect(clientToImage({ x: 100, y: 0 }, box, view)).toEqual({ x: 400, y: 300 });
    expect(clientToImage({ x: 420, y: 200 }, box, view)).toEqual({ x: 720, y: 500 });
  });

  it("is the identity on an unscaled full screenshot", () => {
    const full = { x: 0, y: 0, width: 1440, height: 900 };
    const box = { left: 0, top: 0, width: 1440, height: 900 };
    expect(clientToImage({ x: 12.5, y: 7 }, box, full)).toEqual({ x: 12.5, y: 7 });
  });
});

describe("rectangles", () => {
  const rect = { x: 10, y: 20, width: 100, height: 50 };

  it("resize from any handle, keeping the opposite edges", () => {
    expect(resizeRect(rect, "se", { x: 150, y: 90 })).toEqual({ x: 10, y: 20, width: 140, height: 70 });
    expect(resizeRect(rect, "nw", { x: 0, y: 0 })).toEqual({ x: 0, y: 0, width: 110, height: 70 });
    expect(resizeRect(rect, "e", { x: 60, y: 999 })).toEqual({ x: 10, y: 20, width: 50, height: 50 });
    expect(resizeRect(rect, "n", { x: 999, y: 30 })).toEqual({ x: 10, y: 30, width: 100, height: 40 });
  });

  it("never invert or collapse", () => {
    expect(resizeRect(rect, "w", { x: 500, y: 0 })).toEqual({ x: 110 - MIN_SIZE, y: 20, width: MIN_SIZE, height: 50 });
    expect(resizeRect(rect, "s", { x: 0, y: -100 }).height).toBe(MIN_SIZE);
  });

  it("place handles on corners and edge middles", () => {
    expect(handlePoint(rect, "nw")).toEqual({ x: 10, y: 20 });
    expect(handlePoint(rect, "s")).toEqual({ x: 60, y: 70 });
    expect(handlePoint(rect, "e")).toEqual({ x: 110, y: 45 });
  });
});

const BIG = { x: -1000, y: -1000, width: 5000, height: 5000 };

describe("dragging", () => {
  const arrow: Annotation = { id: "1", type: "arrow", geometry: { from: { x: 0, y: 0 }, to: { x: 10, y: 10 } } };
  const callout: Annotation = {
    id: "2",
    type: "callout",
    geometry: { x: 0, y: 0, width: 100, height: 40, anchor: { x: 200, y: 200 }, text: "Hi" },
  };

  it("moves whole shapes by the drag", () => {
    expect(dragAnnotation(arrow, { kind: "body" }, { x: 0, y: 0 }, { x: 5, y: -5 }, BIG).geometry).toEqual({
      from: { x: 5, y: -5 },
      to: { x: 15, y: 5 },
    });
    // A callout's pointer stays on its target.
    expect(dragAnnotation(callout, { kind: "body" }, { x: 0, y: 0 }, { x: 5, y: 5 }, BIG).geometry).toEqual({
      ...callout.geometry,
      x: 5,
      y: 5,
    });
  });

  it("moves arrow ends, callout anchors and rect handles to the pointer", () => {
    expect(dragAnnotation(arrow, { kind: "to" }, { x: 30.04, y: 40 }, { x: 0, y: 0 }, BIG).geometry).toEqual({
      from: { x: 0, y: 0 },
      to: { x: 30, y: 40 },
    });
    const anchored = dragAnnotation(callout, { kind: "anchor" }, { x: 1, y: 2 }, { x: 0, y: 0 }, BIG);
    expect(anchored.type === "callout" && anchored.geometry.anchor).toEqual({ x: 1, y: 2 });
    const resized = dragAnnotation(callout, { kind: "rect", handle: "se" }, { x: 50, y: 50 }, { x: 0, y: 0 }, BIG);
    expect(resized.geometry).toEqual({ ...callout.geometry, width: 50, height: 50 });
  });
});

describe("staying on the image", () => {
  const view = { x: 100, y: 50, width: 400, height: 300 };
  const box: Annotation = { id: "1", type: "box", geometry: { x: 150, y: 100, width: 100, height: 50 } };

  it("clamps points to the view", () => {
    expect(clampPoint({ x: -5, y: 999 }, view)).toEqual({ x: 100, y: 350 });
    expect(clampPoint({ x: 200, y: 60 }, view)).toEqual({ x: 200, y: 60 });
  });

  it("stops a moved shape at the edge", () => {
    expect(dragAnnotation(box, { kind: "body" }, { x: 0, y: 0 }, { x: -500, y: 1000 }, view).geometry).toEqual({
      x: 100,
      y: 300,
      width: 100,
      height: 50,
    });
    const arrow: Annotation = { id: "2", type: "arrow", geometry: { from: { x: 120, y: 60 }, to: { x: 200, y: 100 } } };
    expect(dragAnnotation(arrow, { kind: "body" }, { x: 0, y: 0 }, { x: -50, y: -50 }, view).geometry).toEqual({
      from: { x: 100, y: 50 },
      to: { x: 180, y: 90 },
    });
  });

  it("lets a shape already outside move back in, never further out", () => {
    const out: Annotation = { ...box, geometry: { x: 450, y: 100, width: 100, height: 50 } };
    expect(dragAnnotation(out, { kind: "body" }, { x: 0, y: 0 }, { x: 30, y: 0 }, view).geometry).toMatchObject({ x: 450 });
    expect(dragAnnotation(out, { kind: "body" }, { x: 0, y: 0 }, { x: -60, y: 0 }, view).geometry).toMatchObject({ x: 390 });
  });

  it("clamps handles, arrow ends, anchors and new shapes", () => {
    expect(dragAnnotation(box, { kind: "rect", handle: "se" }, { x: 900, y: 900 }, { x: 0, y: 0 }, view).geometry).toEqual({
      x: 150,
      y: 100,
      width: 350,
      height: 250,
    });
    const arrow: Annotation = { id: "2", type: "arrow", geometry: { from: { x: 120, y: 60 }, to: { x: 200, y: 100 } } };
    expect(dragAnnotation(arrow, { kind: "to" }, { x: -9, y: -9 }, { x: 0, y: 0 }, view).geometry).toMatchObject({
      to: { x: 100, y: 50 },
    });
    expect(createAnnotation("box", "3", { x: 450, y: 300 }, { x: 2000, y: 2000 }, view).geometry).toEqual({
      x: 450,
      y: 300,
      width: 50,
      height: 50,
    });
    expect(createAnnotation("arrow", "4", { x: 0, y: 0 }, { x: 300, y: 200 }, view).geometry).toEqual({
      from: { x: 100, y: 50 },
      to: { x: 300, y: 200 },
    });
  });
});

describe("creating", () => {
  const view = { x: 0, y: 0, width: 1000, height: 600 };

  it("spans a drag, or places a default size around a click", () => {
    expect(createAnnotation("box", "1", { x: 50, y: 60 }, { x: 10, y: 20 }, view).geometry).toEqual({
      x: 10,
      y: 20,
      width: 40,
      height: 40,
    });
    expect(createAnnotation("box", "1", { x: 500, y: 300 }, { x: 501, y: 301 }, view).geometry).toEqual({
      x: 420,
      y: 255,
      width: 160,
      height: 90,
    });
    // Kept inside the view near an edge.
    expect(createAnnotation("box", "1", { x: 5, y: 5 }, { x: 5, y: 5 }, view).geometry).toMatchObject({ x: 0, y: 0 });
  });

  it("makes arrows point at the click, pixelate a blur, callouts empty", () => {
    expect(createAnnotation("arrow", "1", { x: 200, y: 100 }, { x: 200, y: 100 }, view).geometry).toEqual({
      from: { x: 110, y: 40 },
      to: { x: 200, y: 100 },
    });
    expect(createAnnotation("pixelate", "1", { x: 0, y: 0 }, { x: 50, y: 50 }, view)).toMatchObject({
      type: "blur",
      style: { blurMode: "pixelate" },
    });
    expect(createAnnotation("callout", "1", { x: 0, y: 0 }, { x: 50, y: 50 }, view).geometry).toMatchObject({
      text: "",
    });
  });
});

describe("drawing rules (as the exporter)", () => {
  it("arrow heads", () => {
    const shape = arrowShape({ from: { x: 0, y: 0 }, to: { x: 100, y: 0 } }, 4)!;
    // head = max(4·4, 10) = 16; half-width 7.2; shaft stops 9.6 short.
    expect(shape.shaftEnd).toEqual({ x: 90.4, y: 0 });
    expect(shape.head[0]).toEqual({ x: 100, y: 0 });
    expect(shape.head[1].x).toBeCloseTo(84);
    expect(shape.head[1].y).toBeCloseTo(7.2);
    expect(shape.head[2].y).toBeCloseTo(-7.2);
    expect(arrowShape({ from: { x: 0, y: 0 }, to: { x: 0.5, y: 0 } }, 4)).toBeNull();
    // The head never exceeds the arrow.
    expect(arrowShape({ from: { x: 0, y: 0 }, to: { x: 6, y: 0 } }, 4)!.head[1].x).toBeCloseTo(0);
  });

  it("stroke widths and blur amounts", () => {
    expect(strokeWidth(undefined, 4)).toBe(4);
    expect(strokeWidth(-1, 4)).toBe(4);
    expect(strokeWidth(1000, 4)).toBe(256);
    const blur: Annotation = { id: "1", type: "blur", geometry: { x: 0, y: 0, width: 1, height: 1 } };
    expect(blurAmount(blur)).toBe(12);
    expect(blurAmount({ ...blur, style: { blurMode: "pixelate" } })).toBe(10);
    expect(blurAmount({ ...blur, style: { blurAmount: 1 } })).toBe(4);
    // Rounded and capped like the exporter.
    expect(blurAmount({ ...blur, style: { blurAmount: 7.6 } })).toBe(8);
    expect(blurAmount({ ...blur, style: { blurAmount: 5000 } })).toBe(1024);
  });

  it("draws redactions last, over every other shape, and no crops", () => {
    const shape = (id: string, type: "box" | "blur" | "crop" | "highlight"): Annotation => ({
      id,
      type,
      geometry: { x: 0, y: 0, width: 1, height: 1 },
    });
    const order = drawOrder([shape("b1", "blur"), shape("x1", "box"), shape("k", "crop"), shape("b2", "blur"), shape("h", "highlight")]);
    expect(order.map((a) => a.id)).toEqual(["x1", "h", "b1", "b2"]);
  });

  it("callout pointers, the active crop and the crop mask", () => {
    expect(calloutEdge({ x: 0, y: 0, width: 10, height: 10 }, { x: 30, y: 5 })).toEqual({ x: 10, y: 5 });
    const crops: Annotation[] = [
      { id: "1", type: "crop", geometry: { x: 0, y: 0, width: 1, height: 1 } },
      { id: "2", type: "box", geometry: { x: 0, y: 0, width: 1, height: 1 } },
      { id: "3", type: "crop", geometry: { x: 0, y: 0, width: 2, height: 2 } },
    ];
    expect(activeCrop(crops)?.id).toBe("3");
    expect(activeCrop([])).toBeNull();
    expect(maskOutside({ x: 0, y: 0, width: 10, height: 10 }, { x: 2, y: 2, width: 4, height: 4 })).toBe(
      "M0 0h10v10h-10zM2 2h4v4h-4z",
    );
  });
});
