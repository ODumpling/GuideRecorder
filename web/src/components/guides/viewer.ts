import type { Guide } from "@guiderecorder/guide-schema";

// The reading model behind <GuideViewer>: a guide's items resolved the way
// the exporter resolves them (core `export/render.rs` and `raster.rs`), so
// the web shows what a PDF or the HTML walkthrough would. Coordinates are
// full-screenshot pixels, the model's convention. Nothing here knows about
// workspaces: images are bundle-relative paths, turned into URLs by the
// page that shows the guide.

export type Point = { x: number; y: number };
export type Rect = { x: number; y: number; width: number; height: number };

export interface AnnotationStyle {
  color?: string | null;
  fill?: string | null;
  strokeWidth?: number | null;
  opacity?: number | null;
  fontSize?: number | null;
  blurMode?: "blur" | "pixelate" | null;
  blurAmount?: number | null;
}

export type Annotation = { id: string; style?: AnnotationStyle } & (
  | { type: "arrow"; geometry: { from: Point; to: Point } }
  | { type: "box" | "highlight" | "blur" | "crop"; geometry: Rect }
  | { type: "callout"; geometry: Rect & { text: string; anchor?: Point | null } }
  | { type: "hotspot"; geometry: Rect & { title?: string; description?: string } }
);

export type Hotspot = Extract<Annotation, { type: "hotspot" }>;

export interface StepImage {
  full: string;
  crop?: string | null;
  cropRect?: Rect | null;
  show?: "crop" | "full";
}

export interface ViewerStep {
  type: "step";
  id: string;
  /** 1-based; sections and notes take no number. */
  number: number;
  /** Markdown. */
  title: string;
  /** Markdown. */
  description: string;
  kind: string;
  image: StepImage | null;
  annotations: Annotation[];
  point: Point | null;
  /** The title of the section the step is in. */
  section: string | null;
}

export type ViewerNote = { type: "note"; id: string; kind: "tip" | "warning" | "note"; body: string };

/** Where a decision option leads, resolved as core `export/render.rs` does. */
export type ViewerTarget =
  /** Step `number` (null: past the last step), and the section chosen, if one was. */
  | { kind: "step"; number: number | null; section: string | null }
  | { kind: "guide"; id: string; title: string };

export type ViewerDecision = {
  type: "decision";
  id: string;
  /** Markdown. */
  question: string;
  options: { id: string; label: string; target: ViewerTarget }[];
};

export type ViewerItem = ViewerStep | { type: "section"; id: string; title: string; audiences: string[] } | ViewerNote | ViewerDecision;

type RawItem = {
  type: string;
  id?: string;
  title?: string;
  description?: string;
  body?: string;
  kind?: string;
  image?: StepImage;
  annotations?: Annotation[];
  point?: Point | null;
  audiences?: unknown;
  question?: string;
  options?: { id?: string; label?: string; target?: { kind?: string; id?: string; title?: string } }[];
};

/** The guide's items with steps numbered and placed in their sections. */
export function viewerItems(guide: Guide): ViewerItem[] {
  let number = 0;
  let section: string | null = null;
  const raw = (guide.items ?? []) as RawItem[];
  return raw.flatMap((item, index): ViewerItem[] => {
    const id = item.id ?? String(index);
    if (item.type === "section") {
      section = item.title ?? "";
      const audiences = Array.isArray(item.audiences) ? item.audiences.filter((a): a is string => typeof a === "string" && a.trim() !== "").map((a) => a.trim()) : [];
      return [{ type: "section", id, title: section, audiences }];
    }
    if (item.type === "decision") {
      const options = (Array.isArray(item.options) ? item.options : []).map((option, i) => ({
        id: option.id ?? String(i),
        label: (option.label ?? "").replace(/\s+/g, " ").trim(),
        target: resolveTarget(raw, index, option.target),
      }));
      return [{ type: "decision", id, question: item.question ?? "", options }];
    }
    if (item.type === "note") {
      const kind = item.kind === "tip" || item.kind === "warning" ? item.kind : "note";
      return [{ type: "note", id, kind, body: item.body ?? "" }];
    }
    if (item.type !== "step") return [];
    number += 1;
    return [
      {
        type: "step",
        id,
        number,
        title: item.title ?? "",
        description: item.description ?? "",
        kind: item.kind ?? "manual",
        image: item.image?.full ? item.image : null,
        annotations: Array.isArray(item.annotations) ? item.annotations : [],
        point: validPoint(item.point) ? item.point : null,
        section,
      },
    ];
  });
}

/**
 * Where an option of the decision at `from` leads: the first step at or
 * after the item it names (after the decision for `next`). A step or
 * section that is gone reads as `next`.
 */
function resolveTarget(items: RawItem[], from: number, target: { kind?: string; id?: string; title?: string } | undefined): ViewerTarget {
  if (target?.kind === "guide") return { kind: "guide", id: target.id ?? "", title: (target.title ?? "").trim() };
  const named = target?.kind === "step" || target?.kind === "section" ? items.findIndex((item) => item.id === target.id) : -1;
  const at = named >= 0 ? named : from;
  const section = named >= 0 && target?.kind === "section" ? (items[named]!.title ?? "").trim() : null;
  let number = 0;
  for (const [i, item] of items.entries()) {
    if (item.type !== "step") continue;
    number += 1;
    if (i >= at) return { kind: "step", number, section };
  }
  return { kind: "step", number: null, section };
}

const finite = (...values: unknown[]) => values.every((v) => typeof v === "number" && Number.isFinite(v));
const validPoint = (p: Point | null | undefined): p is Point => Boolean(p) && finite(p!.x, p!.y);
export const validRect = (r: Rect | null | undefined): r is Rect =>
  Boolean(r) && finite(r!.x, r!.y, r!.width, r!.height) && r!.width > 0 && r!.height > 0;

export type Slide =
  | { kind: "intro" }
  | { kind: "step"; step: ViewerStep; notes: SlideNote[] }
  | { kind: "done"; notes: SlideNote[] };

/** What shows above a slide's step: the notes and decisions before it. */
export type SlideNote = ViewerNote | ViewerDecision;

/**
 * The guide as slides, like the walkthrough export: an intro slide if
 * there's an intro or prerequisites, one per step (with the notes and
 * decisions before it), and an end slide with any after the last step.
 */
export function slides(guide: Guide, items: ViewerItem[]): Slide[] {
  const out: Slide[] = [];
  if (guide.intro?.trim() || guide.prerequisites?.trim()) out.push({ kind: "intro" });
  let notes: SlideNote[] = [];
  for (const item of items) {
    if (item.type === "note" || item.type === "decision") notes.push(item);
    else if (item.type === "step") {
      out.push({ kind: "step", step: item, notes });
      notes = [];
    }
  }
  out.push({ kind: "done", notes });
  return out;
}

/** Which image a step shows, and where it sits in the full screenshot (null: the whole of it, size unknown). */
export interface ImageChoice {
  path: string;
  rect: Rect | null;
}

/**
 * The step's image: the click crop when asked for and placed by
 * `cropRect`, else the full screenshot (so annotations stay where they
 * belong, as the exporter does for a crop it can't place).
 * `cropRejected`: the crop image turned out not to fit `cropRect`.
 */
export function chooseImage(image: StepImage, cropRejected = false): ImageChoice {
  if ((image.show ?? "crop") === "crop" && image.crop && validRect(image.cropRect) && !cropRejected) {
    return { path: image.crop, rect: image.cropRect };
  }
  return { path: image.full, rect: null };
}

/** Whether a loaded crop image fits its `cropRect`, give or take a pixel (the exporter's tolerance). */
export function cropFits(rect: Rect, size: { width: number; height: number }) {
  return Math.abs(rect.width - size.width) <= 1 && Math.abs(rect.height - size.height) <= 1;
}

/** The part of `image` in view: cut down by the last crop annotation, if it overlaps. */
export function viewRect(image: Rect, annotations: Annotation[]): Rect {
  const crop = annotations.findLast((a) => a.type === "crop");
  if (!crop || !validRect(crop.geometry as Rect)) return image;
  const g = crop.geometry as Rect;
  const x = Math.max(image.x, g.x);
  const y = Math.max(image.y, g.y);
  const right = Math.min(image.x + image.width, g.x + g.width);
  const bottom = Math.min(image.y + image.height, g.y + g.height);
  return right > x && bottom > y ? { x, y, width: right - x, height: bottom - y } : image;
}

/**
 * The exporter's drawing order: shapes, then redactions over them, then
 * hotspots on top. Crops aren't drawn.
 */
export function drawOrder(annotations: Annotation[]): Annotation[] {
  return [
    ...annotations.filter((a) => a.type !== "blur" && a.type !== "hotspot" && a.type !== "crop"),
    ...annotations.filter((a) => a.type === "blur"),
  ];
}

/** The step's hotspots, numbered 1, 2, 3, ... in annotation order. */
export function hotspots(annotations: Annotation[]): { number: number; hotspot: Hotspot }[] {
  return annotations
    .filter((a): a is Hotspot => a.type === "hotspot" && validRect(a.geometry))
    .map((hotspot, i) => ({ number: i + 1, hotspot }));
}

/** `rect` as CSS percentages of `view`. */
export function percentBox(rect: Rect, view: Rect) {
  const pct = (v: number) => `${(v * 100).toFixed(3)}%`;
  return {
    left: pct((rect.x - view.x) / view.width),
    top: pct((rect.y - view.y) / view.height),
    width: pct(rect.width / view.width),
    height: pct(rect.height / view.height),
  };
}

/** Whether `p` lies in `view`. */
export function inView(p: Point, view: Rect) {
  return p.x >= view.x && p.x <= view.x + view.width && p.y >= view.y && p.y <= view.y + view.height;
}

/** The text of a Markdown string without its markup, for labels and alt text. */
export function plainText(markdown: string): string {
  return markdown
    .replace(/!?\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/[*_`~]+/g, "")
    .replace(/\s+/g, " ")
    .trim();
}

// Drawing defaults, as in core `export/annotate.rs` (and the app's editor).
export const DEFAULT_COLOR = "#ff3b30";
export const DEFAULT_HIGHLIGHT = "#ffcc00";
export const DEFAULT_HIGHLIGHT_OPACITY = 0.35;
export const DEFAULT_STROKE = 4;
export const DEFAULT_CALLOUT_STROKE = 2;
export const DEFAULT_CALLOUT_FILL = "#ffffff";
export const CALLOUT_TEXT = "#1d1d1f";
export const DEFAULT_FONT_SIZE = 16;
export const DEFAULT_BLUR_RADIUS = 12;
export const DEFAULT_BLOCK_SIZE = 10;
export const DEFAULT_HOTSPOT_COLOR = "#007aff";

/** A hex or named CSS colour, else `fallback`; the exporter drops anything else too. */
export function colorOr(value: string | null | undefined, fallback: string): string {
  const v = value?.trim();
  if (!v) return fallback;
  if (/^#(?:[0-9a-f]{3,4}|[0-9a-f]{6}|[0-9a-f]{8})$/i.test(v) || /^[a-z]{3,20}$/i.test(v)) return v;
  return fallback;
}

/** The exporter's rule for `strokeWidth`. */
export function strokeWidth(value: number | null | undefined, fallback: number): number {
  return value != null && Number.isFinite(value) && value > 0 ? Math.min(value, 256) : fallback;
}

/** The exporter's rule for `blurAmount`, by mode. */
export function blurAmount(style: AnnotationStyle | undefined): number {
  const fallback = style?.blurMode === "pixelate" ? DEFAULT_BLOCK_SIZE : DEFAULT_BLUR_RADIUS;
  const value = style?.blurAmount;
  return value != null && Number.isFinite(value) && value > 0 ? Math.min(Math.max(Math.round(value), 4), 1024) : fallback;
}

/** The arrow's shaft end and head, as the exporter draws them; null when shorter than a pixel. */
export function arrowShape(g: { from: Point; to: Point }, sw: number): { shaftEnd: Point; head: [Point, Point, Point] } | null {
  const dx = g.to.x - g.from.x;
  const dy = g.to.y - g.from.y;
  const len = Math.hypot(dx, dy);
  if (!(len >= 1)) return null;
  const head = Math.min(Math.max(sw * 4, 10), len);
  const [ux, uy] = [dx / len, dy / len];
  const base = { x: g.to.x - ux * head, y: g.to.y - uy * head };
  const half = head * 0.45;
  const [px, py] = [-uy * half, ux * half];
  return {
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
