// Annotation geometry for the editor's SVG layer. Everything is in
// full-screenshot pixels, the model's convention (core `model::Annotation`).
// The layer's SVG `viewBox` is the visible part of the screenshot (the click
// crop, at `StepImage.cropRect`, or the whole screenshot), so shapes draw in
// those coordinates as they are; only pointer positions need mapping.
//
// Drawing rules (sizes, arrow heads, defaults) follow the exporter in core
// `export/annotate.rs`, so the editor shows what the export will contain.

import type { Annotation, AnnotationType, ArrowGeometry, Point, Rect, StepImage } from "@guiderecorder/guide-schema";

/** The part of the full screenshot on screen, in its pixels. */
export type View = Rect;

/** What to show for a step and where it sits in the full screenshot. */
export interface ImageView {
  /** Bundle-relative image path. */
  path: string;
  /** The view; null for the full screenshot until its size is known. */
  rect: Rect | null;
  /**
   * The crop was asked for, but where it sits is not recorded (older
   * guides) or the record doesn't fit the crop image ("mismatch"), so the
   * full screenshot is shown to keep annotations in place. The exporter
   * also ignores a `cropRect` that doesn't fit its crop.
   */
  cropUnknown: false | "missing" | "mismatch";
}

type Size = { width: number; height: number };

/** Whether two image sizes match, give or take 1 px each way (the exporter's tolerance). */
export function sizesMatch(a: Size, b: Size): boolean {
  return Math.abs(a.width - b.width) <= 1 && Math.abs(a.height - b.height) <= 1;
}

/** Whether the crop image's size fits its `cropRect` (±1 px, like the exporter). */
export function cropFits(cropRect: Rect, size: Size): boolean {
  return sizesMatch(size, cropRect);
}

/**
 * The view for a step. `cropRejected`: the crop image was found not to fit
 * `cropRect` (see {@link cropFits}), so the full screenshot is used.
 */
export function imageView(image: StepImage, cropRejected = false): ImageView {
  if (image.show === "crop" && image.crop) {
    const rect = image.cropRect;
    if (!rect || !(rect.width > 0 && rect.height > 0)) return { path: image.full, rect: null, cropUnknown: "missing" };
    if (cropRejected) return { path: image.full, rect: null, cropUnknown: "mismatch" };
    return { path: image.crop, rect, cropUnknown: false };
  }
  return { path: image.full, rect: null, cropUnknown: false };
}

/** The on-screen box of an element (as from `getBoundingClientRect`). */
export interface ScreenBox {
  left: number;
  top: number;
  width: number;
  height: number;
}

/**
 * Where `view` is drawn inside `box` with the SVG default
 * `preserveAspectRatio="xMidYMid meet"`: scaled to fit, centred.
 */
function fitted(box: ScreenBox, view: View) {
  const scale = Math.min(box.width / view.width, box.height / view.height);
  return {
    scale,
    left: box.left + (box.width - view.width * scale) / 2,
    top: box.top + (box.height - view.height * scale) / 2,
  };
}

/** A pointer position (client pixels) in full-screenshot pixels. */
export function clientToImage(client: Point, box: ScreenBox, view: View): Point {
  const { scale, left, top } = fitted(box, view);
  return { x: view.x + (client.x - left) / scale, y: view.y + (client.y - top) / scale };
}

/** Where a point in full-screenshot pixels is on screen: the inverse of {@link clientToImage}. */
export function imageToClient(p: Point, box: ScreenBox, view: View): Point {
  const { scale, left, top } = fitted(box, view);
  return { x: left + (p.x - view.x) * scale, y: top + (p.y - view.y) * scale };
}

/** Image pixels per screen pixel, e.g. to size handles on screen. */
export function imagePixelsPerScreenPixel(box: ScreenBox, view: View): number {
  const { scale } = fitted(box, view);
  return scale > 0 ? 1 / scale : 1;
}

const round = (v: number) => Math.round(v * 10) / 10;
const roundPoint = (p: Point): Point => ({ x: round(p.x), y: round(p.y) });
const roundRect = (r: Rect): Rect => ({ x: round(r.x), y: round(r.y), width: round(r.width), height: round(r.height) });

export function rectFromCorners(a: Point, b: Point): Rect {
  return roundRect({
    x: Math.min(a.x, b.x),
    y: Math.min(a.y, b.y),
    width: Math.abs(b.x - a.x),
    height: Math.abs(b.y - a.y),
  });
}

export type RectHandle = "nw" | "n" | "ne" | "e" | "se" | "s" | "sw" | "w";
export const RECT_HANDLES: RectHandle[] = ["nw", "n", "ne", "e", "se", "s", "sw", "w"];

/** A handle's position on `rect`. */
export function handlePoint(rect: Rect, handle: RectHandle): Point {
  const x = handle.includes("w") ? rect.x : handle.includes("e") ? rect.x + rect.width : rect.x + rect.width / 2;
  const y = handle.includes("n") ? rect.y : handle.includes("s") ? rect.y + rect.height : rect.y + rect.height / 2;
  return { x, y };
}

/** Smallest rectangle side a resize leaves. */
export const MIN_SIZE = 4;

/**
 * Moves the edges `handle` holds to `p`. The opposite edges stay put; an
 * edge dragged past its opposite stops at {@link MIN_SIZE} from it.
 */
export function resizeRect(rect: Rect, handle: RectHandle, p: Point): Rect {
  let left = rect.x;
  let top = rect.y;
  let right = rect.x + rect.width;
  let bottom = rect.y + rect.height;
  if (handle.includes("w")) left = Math.min(p.x, right - MIN_SIZE);
  if (handle.includes("e")) right = Math.max(p.x, left + MIN_SIZE);
  if (handle.includes("n")) top = Math.min(p.y, bottom - MIN_SIZE);
  if (handle.includes("s")) bottom = Math.max(p.y, top + MIN_SIZE);
  return roundRect({ x: left, y: top, width: right - left, height: bottom - top });
}

/** `p` moved into `view`. */
export function clampPoint(p: Point, view: View): Point {
  return {
    x: Math.min(Math.max(p.x, view.x), view.x + view.width),
    y: Math.min(Math.max(p.y, view.y), view.y + view.height),
  };
}

/**
 * `delta` limited so `box` doesn't move further out of `view`: a box inside
 * stays inside; one already sticking out may move back in, not further out.
 */
function clampDelta(box: Rect, delta: Point, view: View): Point {
  const axis = (d: number, lo: number, hi: number) => Math.min(Math.max(d, Math.min(lo, 0)), Math.max(hi, 0));
  return {
    x: axis(delta.x, view.x - box.x, view.x + view.width - (box.x + box.width)),
    y: axis(delta.y, view.y - box.y, view.y + view.height - (box.y + box.height)),
  };
}

/** The part of an annotation a drag holds. */
export type DragPart = { kind: "body" } | { kind: "rect"; handle: RectHandle } | { kind: "from" | "to" | "anchor" };

/**
 * `annotation` with the dragged part moved: by `delta` for the body, to `p`
 * otherwise. Everything stays within `view`: points are clamped to it, and
 * a moved shape stops at its edge.
 */
export function dragAnnotation(annotation: Annotation, part: DragPart, pointer: Point, rawDelta: Point, view: View): Annotation {
  const p = clampPoint(pointer, view);
  const delta = part.kind === "body" ? clampDelta(bounds(annotation), rawDelta, view) : rawDelta;
  const move = (q: Point): Point => roundPoint({ x: q.x + delta.x, y: q.y + delta.y });
  switch (annotation.type) {
    case "arrow": {
      const g = annotation.geometry;
      const geometry: ArrowGeometry =
        part.kind === "from"
          ? { ...g, from: roundPoint(p) }
          : part.kind === "to"
            ? { ...g, to: roundPoint(p) }
            : { from: move(g.from), to: move(g.to) };
      return { ...annotation, geometry };
    }
    case "callout": {
      const g = annotation.geometry;
      if (part.kind === "anchor") return { ...annotation, geometry: { ...g, anchor: roundPoint(p) } };
      if (part.kind === "rect") return { ...annotation, geometry: { ...g, ...resizeRect(g, part.handle, p) } };
      const moved = move(g);
      // The pointer stays on its target when the box moves.
      return { ...annotation, geometry: { ...g, x: moved.x, y: moved.y } };
    }
    case "hotspot": {
      const g = annotation.geometry;
      if (part.kind === "rect") return { ...annotation, geometry: { ...g, ...resizeRect(g, part.handle, p) } };
      const moved = move(g);
      return { ...annotation, geometry: { ...g, x: moved.x, y: moved.y } };
    }
    default: {
      const g = annotation.geometry;
      if (part.kind === "rect") return { ...annotation, geometry: resizeRect(g, part.handle, p) };
      const moved = move(g);
      return { ...annotation, geometry: { ...g, x: moved.x, y: moved.y } };
    }
  }
}

// ---------------------------------------------------------------------------
// Defaults and drawing rules, as in core `export/annotate.rs` and `blur.rs`.

export const DEFAULT_COLOR = "#ff3b30";
export const DEFAULT_HIGHLIGHT = "#ffcc00";
export const DEFAULT_HIGHLIGHT_OPACITY = 0.35;
export const DEFAULT_STROKE = 4;
export const DEFAULT_CALLOUT_STROKE = 2;
export const DEFAULT_CALLOUT_FILL = "#ffffff";
export const CALLOUT_TEXT = "#1d1d1f";
export const DEFAULT_FONT_SIZE = 16;
export const LINE_HEIGHT = 1.25;
export const DEFAULT_BLUR_RADIUS = 12;
export const DEFAULT_BLOCK_SIZE = 10;
/** Blur amounts are rounded and kept within these at export. */
export const MIN_BLUR_AMOUNT = 4;
export const MAX_BLUR_AMOUNT = 1024;

/** Hotspots without a colour of their own. */
export const DEFAULT_HOTSPOT_COLOR = "#007aff";

/** Redactions (blur and pixelate). */
export const isRedaction = (annotation: Annotation) => annotation.type === "blur";

/**
 * Hotspots are not burned into the image: the reader of an exported guide
 * hovers or clicks them, over the image and everything drawn on it.
 */
export const isHotspot = (annotation: Annotation) => annotation.type === "hotspot";

/**
 * The exporter's drawing order: every other shape in order, then the
 * redactions, burned in last so nothing inside them survives (shapes drawn
 * over a blur are blurred too). Crops are not drawn. Hotspots go on top,
 * as the reader sees them.
 */
export function drawOrder(annotations: Annotation[]): Annotation[] {
  return [
    ...annotations.filter((a) => !isRedaction(a) && !isHotspot(a) && a.type !== "crop"),
    ...annotations.filter(isRedaction),
    ...annotations.filter(isHotspot),
  ];
}

/** A step's hotspots, numbered 1, 2, 3, ... in annotation order. */
export function hotspots(annotations: Annotation[]): (Annotation & { type: "hotspot" })[] {
  return annotations.filter((a): a is Annotation & { type: "hotspot" } => a.type === "hotspot");
}

/** Each hotspot's number by annotation id. */
export function hotspotNumbers(annotations: Annotation[]): Map<string, number> {
  return new Map(hotspots(annotations).map((a, i) => [a.id, i + 1]));
}

/**
 * The order of the hit targets, bottom first: crops (picked by their edge)
 * under everything, then {@link drawOrder}, so a click picks the shape
 * drawn on top.
 */
export function hitOrder(annotations: Annotation[]): Annotation[] {
  return [...annotations.filter((a) => a.type === "crop"), ...drawOrder(annotations)];
}

/** The exporter's rule for `strokeWidth`. */
export function strokeWidth(value: number | null | undefined, fallback: number): number {
  return value != null && Number.isFinite(value) && value > 0 ? Math.min(value, 256) : fallback;
}

/** The exporter's rule for `blurAmount`. */
export function blurAmount(annotation: Annotation): number {
  const fallback = annotation.style?.blurMode === "pixelate" ? DEFAULT_BLOCK_SIZE : DEFAULT_BLUR_RADIUS;
  const value = annotation.style?.blurAmount;
  return value != null && Number.isFinite(value) && value > 0
    ? Math.min(Math.max(Math.round(value), MIN_BLUR_AMOUNT), MAX_BLUR_AMOUNT)
    : fallback;
}

/**
 * The arrow's shaft end and head triangle, as the exporter draws them, or
 * null when it is shorter than a pixel (the exporter skips it).
 */
export function arrowShape(g: ArrowGeometry, sw: number): { shaftEnd: Point; head: [Point, Point, Point] } | null {
  const dx = g.to.x - g.from.x;
  const dy = g.to.y - g.from.y;
  const len = Math.hypot(dx, dy);
  if (len < 1) return null;
  const head = Math.min(Math.max(sw * 4, 10), len);
  const [ux, uy] = [dx / len, dy / len];
  const base = { x: g.to.x - ux * head, y: g.to.y - uy * head };
  const half = head * 0.45;
  const [px, py] = [-uy * half, ux * half];
  return {
    // Inside the head, so the round cap doesn't poke out.
    shaftEnd: { x: g.to.x - ux * head * 0.6, y: g.to.y - uy * head * 0.6 },
    head: [g.to, { x: base.x + px, y: base.y + py }, { x: base.x - px, y: base.y - py }],
  };
}

/** Where a callout's pointer leaves its box: the box point nearest the anchor. */
export function calloutEdge(rect: Rect, anchor: Point): Point {
  return {
    x: Math.min(Math.max(anchor.x, rect.x), rect.x + rect.width),
    y: Math.min(Math.max(anchor.y, rect.y), rect.y + rect.height),
  };
}

/** The crop that applies: the last crop annotation (the exporter's rule). */
export function activeCrop(annotations: Annotation[]): Annotation | null {
  for (let i = annotations.length - 1; i >= 0; i--) if (annotations[i].type === "crop") return annotations[i];
  return null;
}

/** An even-odd path covering `view` outside `rect`, to dim what a crop cuts away. */
export function maskOutside(view: View, rect: Rect): string {
  const outer = `M${view.x} ${view.y}h${view.width}v${view.height}h${-view.width}z`;
  const inner = `M${rect.x} ${rect.y}h${rect.width}v${rect.height}h${-rect.width}z`;
  return `${outer}${inner}`;
}

/** Below this drag distance a new shape gets its default size instead. */
export const CLICK_SLOP = 6;

/** Default sizes for shapes placed with a click, centred on it. */
const DEFAULT_SIZES: Record<Exclude<AnnotationType, "arrow">, [number, number]> = {
  box: [160, 90],
  highlight: [220, 40],
  blur: [180, 40],
  callout: [220, 70],
  crop: [480, 300],
  hotspot: [160, 90],
};

/** A drawing tool: an annotation type, or pixelate (a blur in pixelate mode). */
export type Tool = AnnotationType | "pixelate";

/**
 * A new annotation from a drag between `a` and `b` (or a click at `a`):
 * rectangles span the drag; an arrow points from `a` to `b`; a click places
 * a default-sized shape, or an arrow pointing at the click.
 */
export function createAnnotation(tool: Tool, id: string, start: Point, end: Point, view: View): Annotation {
  // Drawn within the image.
  const [a, b] = [clampPoint(start, view), clampPoint(end, view)];
  const dragged = Math.hypot(b.x - a.x, b.y - a.y) >= CLICK_SLOP;
  if (tool === "arrow") {
    const from = dragged ? a : { x: a.x - 90, y: a.y - 60 };
    const to = dragged ? b : a;
    return { id, type: "arrow", geometry: { from: roundPoint(from), to: roundPoint(to) } };
  }
  const type = tool === "pixelate" ? "blur" : tool;
  let rect: Rect;
  if (dragged) {
    rect = rectFromCorners(a, b);
  } else {
    const [w, h] = DEFAULT_SIZES[type].map((side, i) => Math.min(side, i === 0 ? view.width : view.height));
    const x = Math.min(Math.max(a.x - w / 2, view.x), view.x + view.width - w);
    const y = Math.min(Math.max(a.y - h / 2, view.y), view.y + view.height - h);
    rect = roundRect({ x, y, width: w, height: h });
  }
  rect = { ...rect, width: Math.max(rect.width, MIN_SIZE), height: Math.max(rect.height, MIN_SIZE) };
  switch (type) {
    case "callout":
      return { id, type, geometry: { ...rect, text: "" } };
    case "hotspot":
      return { id, type, geometry: { ...rect, title: "", description: "" } };
    case "blur":
      return { id, type, geometry: rect, style: { blurMode: tool === "pixelate" ? "pixelate" : "blur" } };
    default:
      return { id, type, geometry: rect };
  }
}

/** The bounding box of an annotation (for the selection outline). */
export function bounds(annotation: Annotation): Rect {
  if (annotation.type !== "arrow") {
    const g = annotation.geometry;
    return { x: g.x, y: g.y, width: g.width, height: g.height };
  }
  const { from, to } = annotation.geometry;
  return rectFromCorners(from, to);
}
