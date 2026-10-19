import { useCallback, useEffect, useLayoutEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import {
  Crop,
  Droplets,
  Eye,
  Grid3x3,
  Highlighter,
  ImageOff,
  Maximize2,
  MessageSquareText,
  MousePointer2,
  MoveUpRight,
  SquareDashedMousePointer,
  Square,
} from "lucide-react";
import {
  acceptBlurSuggestion,
  addAnnotation,
  rejectBlurSuggestion,
  removeAnnotation,
  updateAnnotation,
} from "@/annotations/commands";
import {
  activeCrop,
  bounds,
  cropFits,
  drawOrder,
  hitOrder,
  clientToImage,
  createAnnotation,
  dragAnnotation,
  handlePoint,
  hotspotNumbers,
  imagePixelsPerScreenPixel,
  imageToClient,
  imageView,
  RECT_HANDLES,
  type DragPart,
  type Tool,
  type View,
} from "@/annotations/geometry";
import { afterFrame } from "@/components/annotations/afterFrame";
import { HotspotPopover, POPOVER_WIDTH, type PopoverAnchor } from "@/components/annotations/HotspotPopover";
import { annotationSelection, useSelectedAnnotation } from "@/components/annotations/selection";
import { CropMask, Shape, type Backdrop } from "@/components/annotations/Shapes";
import { StyleBar } from "@/components/annotations/StyleBar";
import { BeforeAfterSwitch, type Frame } from "@/components/BeforeAfterSwitch";
import { Button } from "@/components/ui/button";
import { useStepImage } from "@/components/useStepImage";
import { IMAGE_SHOW_HINTS, setImageShow, type Command } from "@/editor/commands";
import { editorStore, useAnnotated } from "@/editor/editor";
import { tauriApi, verifyCrop } from "@/lib/ipc";
import type { Annotation, Guide, Point, Rect, RedactionSuggestion, Step, StepImage } from "@/lib/types";
import { ulid } from "@/lib/ulid";
import { cn } from "@/lib/utils";

const TOOLS: { tool: Tool | "select"; label: string; hint: string; icon: typeof Square }[] = [
  { tool: "select", label: "Select", hint: "Select: click a shape to move, resize or style it", icon: MousePointer2 },
  { tool: "arrow", label: "Arrow", hint: "Arrow: drag from the tail to the tip, or click where it should point", icon: MoveUpRight },
  { tool: "box", label: "Box", hint: "Box: drag to outline an area", icon: Square },
  { tool: "highlight", label: "Highlight", hint: "Highlight: drag to tint an area with a see-through colour", icon: Highlighter },
  { tool: "callout", label: "Callout", hint: "Callout: drag to add a text box, with an optional pointer", icon: MessageSquareText },
  {
    tool: "hotspot",
    label: "Hotspot",
    hint: "Hotspot: drag to mark an area that shows a title and description when the reader points at it",
    icon: SquareDashedMousePointer,
  },
  { tool: "blur", label: "Blur", hint: "Blur: drag to hide sensitive details by blurring them", icon: Droplets },
  { tool: "pixelate", label: "Pixelate", hint: "Pixelate: drag to hide sensitive details behind large pixels", icon: Grid3x3 },
  { tool: "crop", label: "Crop", hint: "Crop: drag to choose the part of the image the export shows", icon: Crop },
];

/** Builds a command against the current guide and applies it. */
function run(build: (guide: Guide) => Command): boolean {
  const doc = editorStore.getState().doc;
  if (!doc) return false;
  try {
    return editorStore.getState().dispatch(build(doc.guide));
  } catch (error) {
    console.error("annotation edit failed:", error);
    return false;
  }
}

function isEditable(target: EventTarget | null): boolean {
  return (
    target instanceof HTMLElement &&
    (target.isContentEditable || !!target.closest("input, textarea, select, [contenteditable]"))
  );
}

type Drag =
  | { mode: "create"; tool: Tool; id: string; start: Point }
  | { mode: "move"; original: Annotation; part: DragPart; start: Point };

type Size = { width: number; height: number };

/**
 * The size of an image, once decoded; `failed` when it can't be decoded
 * (a damaged or unsupported file).
 */
function useImageSize(url: string | null): { size: Size | null; failed: boolean } {
  const [result, setResult] = useState<{ url: string; size: Size | null } | null>(null);
  useEffect(() => {
    if (!url) return;
    let cancelled = false;
    const image = new Image();
    image.onload = () => {
      if (!cancelled) setResult({ url, size: { width: image.naturalWidth, height: image.naturalHeight } });
    };
    image.onerror = () => {
      if (!cancelled) setResult({ url, size: null });
    };
    image.src = url;
    return () => {
      cancelled = true;
    };
  }, [url]);
  const current = result && result.url === url ? result : null;
  return { size: current?.size ?? null, failed: !!current && !current.size };
}

/**
 * Whether the step image at `path` can't be decoded: `true` when its bytes
 * aren't an image, `false` when it decodes or can't be read, `null` while
 * pending or with no `path`.
 */
function useUndecodable(guideId: string, path: string | null): boolean | null {
  const [result, setResult] = useState<{ key: string; undecodable: boolean } | null>(null);
  const key = path && `${guideId}|${path}`;
  useEffect(() => {
    if (!key || !path) return;
    let cancelled = false;
    let url: string | null = null;
    const done = (undecodable: boolean) => !cancelled && setResult({ key, undecodable });
    tauriApi.stepImage(guideId, path).then(
      (bytes) => {
        if (cancelled) return;
        url = URL.createObjectURL(new Blob([bytes], { type: "image/webp" }));
        const image = new Image();
        image.onload = () => done(false);
        image.onerror = () => done(true);
        image.src = url;
      },
      () => done(false),
    );
    return () => {
      cancelled = true;
      if (url) URL.revokeObjectURL(url);
    };
  }, [guideId, path, key]);
  return key && result?.key === key ? result.undecodable : null;
}

/** Identifies what `verify_crop` checked: the image paths and `cropRect`. */
function cropCheckKey(image: StepImage): string | null {
  if (image.show !== "crop" || !image.crop || !image.cropRect) return null;
  const { x, y, width, height } = image.cropRect;
  return `${image.full}|${image.crop}|${x},${y},${width},${height}`;
}

/**
 * The result of `verify_crop` for the step's crop view: `true` or `false`
 * once known, `null` while pending or when no crop is shown. A failed
 * check counts as `false`, so the full screenshot is shown.
 */
function useCropCheck(guideId: string, image: StepImage): boolean | null {
  const key = cropCheckKey(image);
  const [result, setResult] = useState<{ key: string; ok: boolean } | null>(null);
  useEffect(() => {
    if (!key) return;
    let cancelled = false;
    verifyCrop(guideId, image).then(
      (ok) => !cancelled && setResult({ key, ok }),
      (error) => {
        console.error("verify_crop failed:", error);
        if (!cancelled) setResult({ key, ok: false });
      },
    );
    return () => {
      cancelled = true;
    };
    // `key` covers the parts of `image` the check reads.
  }, [guideId, key]);
  return key && result?.key === key ? result.ok : null;
}

function ImageStatus({ error }: { error: string | null }) {
  return (
    <div className="flex min-h-48 items-center justify-center rounded-lg border border-border bg-muted">
      <span className="flex items-center gap-2 p-6 text-sm text-muted-foreground">
        {error ? (
          <>
            <ImageOff className="size-4" /> {error}
          </>
        ) : (
          "Loading image…"
        )}
      </span>
    </div>
  );
}

/**
 * The step's after frame, without annotations, in the Before view's
 * framing (see {@link afterFrame}). `crop`: the part of the full
 * screenshot the Before view shows (null: all of it, undefined: not known yet).
 */
function AfterImage({ guideId, image, crop }: { guideId: string; image: StepImage; crop: Rect | null | undefined }) {
  if (crop === undefined) return <ImageStatus error={null} />;
  if (crop) return <CroppedAfterImage guideId={guideId} image={image} crop={crop} />;
  return <AfterFrameImage guideId={guideId} path={image.after!} crop={null} full={null} />;
}

/** The after frame cropped like the full screenshot, whose size it checks first. */
function CroppedAfterImage({ guideId, image, crop }: { guideId: string; image: StepImage; crop: Rect }) {
  const full = useStepImage(guideId, image.full);
  const fullSize = useImageSize(full.url);
  // A full screenshot that can't be read or decoded: the whole frame.
  if (full.error || fullSize.failed) return <AfterFrameImage guideId={guideId} path={image.after!} crop={null} full={null} />;
  return <AfterFrameImage guideId={guideId} path={image.after!} crop={crop} full={fullSize.size} />;
}

function AfterFrameImage({
  guideId,
  path,
  crop,
  full,
}: {
  guideId: string;
  path: string;
  crop: Rect | null;
  full: Size | null;
}) {
  const { url, error } = useStepImage(guideId, path);
  const { size, failed } = useImageSize(url);
  const view = afterFrame(crop, size, full);
  if (!url || !size || !view) {
    return <ImageStatus error={error ?? (failed ? "The after screenshot can't be shown: the image is damaged." : null)} />;
  }
  return (
    <div className="flex items-center justify-center overflow-hidden rounded-lg border border-border bg-muted">
      <svg
        role="img"
        aria-label="Step screenshot after the action"
        viewBox={`${view.x} ${view.y} ${view.width} ${view.height}`}
        className="block max-h-[60vh] w-full select-none"
        style={{ aspectRatio: `${view.width} / ${view.height}` }}
      >
        <image href={url} x={0} y={0} width={size.width} height={size.height} preserveAspectRatio="none" />
      </svg>
    </div>
  );
}

function Layer({
  guideId,
  step,
  readOnly,
  annotated,
  preview,
  tool,
  onToolDone,
  selectedId,
  select,
  focusedSuggestion,
  frame,
}: {
  guideId: string;
  step: Step;
  readOnly: boolean;
  /** A page of an annotation guide. */
  annotated: boolean;
  /**
   * The step as the reader of the export sees it: no editing; pointing at
   * a hotspot shows its title and description, a click keeps them shown.
   */
  preview: boolean;
  /** "after": the after frame instead, without annotations. */
  frame: Frame;
  tool: Tool | "select";
  onToolDone(): void;
  selectedId: string | null;
  select(id: string | null): void;
  /** The blur suggestion whose row has the pointer or focus, drawn stronger. */
  focusedSuggestion: number | null;
}) {
  // A crop is shown only once the Rust side has checked its `cropRect`
  // the way the exporter does (size, and pixels against the full
  // screenshot); otherwise it is set aside for the full screenshot. Keyed
  // by the image paths and rect, so a changed crop is checked afresh. A
  // crop image that can't be decoded is set aside the same way.
  const verified = useCropCheck(guideId, step.image);
  const [rejectedCrop, setRejectedCrop] = useState<{ key: string | null; damaged: boolean } | null>(null);
  const cropKey = cropCheckKey(step.image);
  const rejectedHere = rejectedCrop !== null && rejectedCrop.key === cropKey ? rejectedCrop : null;
  const rejected = !!rejectedHere || verified === false;
  const shown = imageView(step.image, rejected);
  const { url, error } = useStepImage(guideId, shown.path);
  const { size: natural, failed } = useImageSize(url);
  const cropMismatch = !!shown.rect && !!natural && !cropFits(shown.rect, natural);
  const cropDamaged = !!shown.rect && failed;
  useEffect(() => {
    if (cropMismatch || cropDamaged) setRejectedCrop({ key: cropKey, damaged: cropDamaged });
  }, [cropMismatch, cropDamaged, cropKey]);
  // The full screenshot can't be decoded: nothing to edit on.
  const damaged = !shown.rect && failed;
  // When the Rust check sets the crop aside before its decode here fails,
  // the crop is decoded on its own, so a damaged crop reads as damaged
  // whichever check finishes first.
  const undecodable = useUndecodable(guideId, verified === false && !rejectedHere ? (step.image.crop ?? null) : null);
  const cropNote: "damaged" | "mismatch" | null =
    rejectedHere?.damaged || undecodable ? "damaged" : rejectedHere || undecodable === false ? "mismatch" : null;
  useEffect(() => {
    if (damaged) select(null);
  }, [damaged, select]);
  // Nothing is drawn until the crop is known to fit.
  const view: View | null = shown.rect
    ? natural && !cropMismatch && verified === true
      ? shown.rect
      : null
    : natural && { x: 0, y: 0, width: natural.width, height: natural.height };

  const svg = useRef<SVGSVGElement>(null);
  const box = useRef<HTMLDivElement>(null);
  const drag = useRef<Drag | null>(null);
  const [draft, setDraft] = useState<Annotation | null>(null);
  // The hotspot with the pointer, and in preview the one clicked open.
  const [hovered, setHovered] = useState<string | null>(null);
  const [pinned, setPinned] = useState<string | null>(null);
  const [anchor, setAnchor] = useState<PopoverAnchor | null>(null);
  // Image pixels per screen pixel, to keep handles a constant size on screen.
  const [unit, setUnit] = useState(1);

  useLayoutEffect(() => {
    const element = svg.current;
    if (!element || !view) return;
    const update = () => setUnit(imagePixelsPerScreenPixel(element.getBoundingClientRect(), view));
    update();
    const observer = new ResizeObserver(update);
    observer.observe(element);
    return () => observer.disconnect();
    // The SVG is replaced on returning from the After view.
  }, [frame, view?.x, view?.y, view?.width, view?.height]);

  const toImage = (e: { clientX: number; clientY: number }): Point =>
    clientToImage({ x: e.clientX, y: e.clientY }, svg.current!.getBoundingClientRect(), view!);

  // The layer takes the keyboard (Delete, arrows) from text fields.
  const takeFocus = () => {
    if (document.activeElement instanceof HTMLElement && isEditable(document.activeElement)) {
      document.activeElement.blur();
    }
  };

  const begin = (e: ReactPointerEvent, next: Drag) => {
    e.preventDefault();
    e.stopPropagation();
    svg.current?.setPointerCapture(e.pointerId);
    drag.current = next;
  };

  const onBackgroundDown = (e: ReactPointerEvent) => {
    if (e.button !== 0 || !view) return;
    if (preview) {
      setPinned(null);
      return;
    }
    takeFocus();
    if (readOnly || tool === "select") {
      select(null);
      return;
    }
    const p = toImage(e);
    const id = ulid();
    begin(e, { mode: "create", tool, id, start: p });
    setDraft(createAnnotation(tool, id, p, p, view));
  };

  /** Whether a press on `annotation` picks it: any with Select, a hotspot with the hotspot tool too. */
  const picks = (annotation: Annotation) =>
    !preview && (tool === "select" || (tool === "hotspot" && annotation.type === "hotspot"));

  const onShapeDown = (e: ReactPointerEvent, annotation: Annotation, part: DragPart) => {
    if (e.button !== 0 || !picks(annotation)) return;
    takeFocus();
    if (tool !== "select") onToolDone();
    select(annotation.id);
    if (readOnly) {
      e.stopPropagation();
      return;
    }
    begin(e, { mode: "move", original: annotation, part, start: toImage(e) });
  };

  /** The dragged annotation with the pointer at `e`. */
  const dragged = (current: Drag, e: { clientX: number; clientY: number }): Annotation => {
    const p = toImage(e);
    if (current.mode === "create") return createAnnotation(current.tool, current.id, current.start, p, view!);
    const delta = { x: p.x - current.start.x, y: p.y - current.start.y };
    return dragAnnotation(current.original, current.part, p, delta, view!);
  };

  const onMove = (e: ReactPointerEvent) => {
    if (drag.current && view) setDraft(dragged(drag.current, e));
  };

  // From the release point itself: the last move may not have rendered yet.
  const finish = (e: ReactPointerEvent) => {
    const current = drag.current;
    drag.current = null;
    setDraft(null);
    if (!current || !view) return;
    const result = dragged(current, e);
    if (current.mode === "create") {
      if (run((guide) => addAnnotation(guide, step.id, result))) select(result.id);
      onToolDone();
    } else if (JSON.stringify(result) !== JSON.stringify(current.original)) {
      run((guide) => updateAnnotation(guide, step.id, result));
    }
  };

  const cancel = useCallback(() => {
    drag.current = null;
    setDraft(null);
  }, []);

  // The hotspot whose title and description show: the one clicked open in
  // preview, else the one pointed at (unless selected: the panel below
  // shows it), never while drawing or dragging.
  const cardId = preview ? (pinned ?? hovered) : draft || hovered === selectedId ? null : hovered;
  const card = step.annotations.find((a) => a.id === cardId && a.type === "hotspot") ?? null;
  const cardRect = card && bounds(card);
  // Placed below the hotspot, or above it in the lower half, within the box.
  useLayoutEffect(() => {
    const element = svg.current;
    const outer = box.current;
    if (!cardRect || !element || !outer || !view) {
      setAnchor(null);
      return;
    }
    const screen = element.getBoundingClientRect();
    const area = outer.getBoundingClientRect();
    const topLeft = imageToClient(cardRect, screen, view);
    const bottomRight = imageToClient(
      { x: cardRect.x + cardRect.width, y: cardRect.y + cardRect.height },
      screen,
      view,
    );
    const below = cardRect.y + cardRect.height / 2 < view.y + view.height / 2;
    setAnchor({
      left: Math.max(0, Math.min(topLeft.x - area.left, area.width - POPOVER_WIDTH)),
      ...(below ? { top: bottomRight.y - area.top + 6 } : { bottom: area.bottom - topLeft.y + 6 }),
    });
    // The rect's fields, and `unit` for a resized layer.
  }, [cardRect?.x, cardRect?.y, cardRect?.width, cardRect?.height, unit, view?.x, view?.y, view?.width, view?.height]);

  // Leaving preview closes the popover, so it doesn't come back with it.
  useEffect(() => {
    if (!preview) setPinned(null);
  }, [preview]);

  useEffect(() => {
    if (!pinned) return;
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") setPinned(null);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [pinned]);

  // Keyboard: Delete removes, arrows nudge (Shift: 10 px), Escape deselects.
  useEffect(() => {
    if (!selectedId) return;
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.defaultPrevented || isEditable(e.target) || e.metaKey || e.ctrlKey || e.altKey) return;
      if (e.key === "Escape") {
        cancel();
        select(null);
        return;
      }
      // No edits without the image they apply to (the After view shows another).
      if (readOnly || !view || frame === "after") return;
      const annotation = step.annotations.find((a) => a.id === selectedId);
      if (!annotation) return;
      if (e.key === "Delete" || e.key === "Backspace") {
        e.preventDefault();
        if (run((guide) => removeAnnotation(guide, step.id, annotation.id))) select(null);
        return;
      }
      const steps: Record<string, Point> = {
        ArrowLeft: { x: -1, y: 0 },
        ArrowRight: { x: 1, y: 0 },
        ArrowUp: { x: 0, y: -1 },
        ArrowDown: { x: 0, y: 1 },
      };
      const direction = steps[e.key];
      if (!direction) return;
      e.preventDefault();
      const by = e.shiftKey ? 10 : 1;
      const delta = { x: direction.x * by, y: direction.y * by };
      const moved = dragAnnotation(annotation, { kind: "body" }, { x: 0, y: 0 }, delta, view);
      run((guide) => updateAnnotation(guide, step.id, moved, "nudge"));
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [selectedId, readOnly, step, select, cancel, view, frame]);

  if (frame === "after") {
    // The Before view's crop once it is known to fit; a crop image that
    // can't be read gives the whole frame.
    const crop = shown.rect ? (view ?? (error ? null : undefined)) : null;
    return <AfterImage guideId={guideId} image={step.image} crop={crop} />;
  }

  if (!url || !view) {
    return <ImageStatus error={error ?? (damaged ? "The screenshot can't be shown: the image is damaged." : null)} />;
  }

  // The draft replaces its annotation while dragging, or is added on top.
  const annotations = draft
    ? step.annotations.some((a) => a.id === draft.id)
      ? step.annotations.map((a) => (a.id === draft.id ? draft : a))
      : [...step.annotations, draft]
    : step.annotations;
  const crop = activeCrop(annotations);
  const backdrop: Backdrop = { url, rect: view };
  const selected = annotations.find((a) => a.id === selectedId) ?? null;
  const handle = 9 * unit;
  const hitWidth = 12 * unit;
  const numbers = hotspotNumbers(step.annotations);
  // A new hotspot is numbered after the others while drawn.
  const numberOf = (a: Annotation) => numbers.get(a.id) ?? numbers.size + 1;

  return (
    <div className="flex flex-col gap-2">
      {(shown.cropUnknown === "missing" || (shown.cropUnknown === "mismatch" && cropNote)) && (
        <p className="text-xs text-muted-foreground">
          {shown.cropUnknown === "missing"
            ? "This step doesn't record where its click crop sits, so the full screenshot is shown for editing."
            : cropNote === "damaged"
              ? "This step's click crop can't be shown: the image is damaged. The full screenshot is shown for editing."
              : "This step's click crop doesn't match its recorded position, so the full screenshot is shown for editing. The export ignores the recorded position too."}
        </p>
      )}
    <div ref={box} className="relative">
    <div className="flex items-center justify-center overflow-hidden rounded-lg border border-border bg-muted">
      <svg
        ref={svg}
        role="img"
        aria-label={preview ? "Page preview" : annotated ? "Page with annotations" : "Step screenshot with annotations"}
        viewBox={`${view.x} ${view.y} ${view.width} ${view.height}`}
        className="block max-h-[60vh] w-full touch-none select-none"
        style={{
          aspectRatio: `${view.width} / ${view.height}`,
          cursor: preview || tool === "select" ? "default" : "crosshair",
        }}
        onPointerDown={onBackgroundDown}
        onPointerMove={onMove}
        onPointerUp={finish}
        onPointerCancel={cancel}
      >
        <image href={url} x={view.x} y={view.y} width={view.width} height={view.height} preserveAspectRatio="none" />
        {/* The exporter's order: redactions last, over everything but hotspots. */}
        {drawOrder(annotations).map((annotation) => (
          <Shape key={annotation.id} annotation={annotation} backdrop={backdrop} number={numberOf(annotation)} unit={unit} />
        ))}
        {crop?.type === "crop" && <CropMask view={view} rect={crop.geometry} />}
        {!preview && !annotated && (
          <Suggestions suggestions={step.aiState.redactionSuggestions ?? []} unit={unit} focused={focusedSuggestion} />
        )}

        {/* Hit targets, in drawing order so the topmost shape wins. */}
        <g>
          {hitOrder(annotations).map((annotation) => {
            if (preview) {
              // Only hotspots answer the reader.
              if (annotation.type !== "hotspot") return null;
              const r = annotation.geometry;
              return (
                <rect
                  key={annotation.id}
                  data-hotspot-target={annotation.id}
                  x={r.x}
                  y={r.y}
                  width={r.width}
                  height={r.height}
                  fill="transparent"
                  style={{ cursor: "pointer" }}
                  onPointerEnter={() => setHovered(annotation.id)}
                  onPointerLeave={() => setHovered(null)}
                  onPointerDown={(e) => {
                    if (e.button !== 0) return;
                    e.stopPropagation();
                    setPinned((id) => (id === annotation.id ? null : annotation.id));
                  }}
                />
              );
            }
            const down = (e: ReactPointerEvent) => onShapeDown(e, annotation, { kind: "body" });
            const active = picks(annotation);
            const hover =
              annotation.type === "hotspot"
                ? { onPointerEnter: () => setHovered(annotation.id), onPointerLeave: () => setHovered(null) }
                : {};
            if (annotation.type === "arrow") {
              const { from, to } = annotation.geometry;
              return (
                <line
                  key={annotation.id}
                  x1={from.x}
                  y1={from.y}
                  x2={to.x}
                  y2={to.y}
                  stroke="transparent"
                  strokeWidth={Math.max(hitWidth, (annotation.style?.strokeWidth ?? 4) + 4 * unit)}
                  pointerEvents={active ? undefined : "none"}
                  style={{ cursor: "move" }}
                  onPointerDown={down}
                />
              );
            }
            const r = bounds(annotation);
            // A crop is picked by its edge, so the shapes inside stay reachable.
            const edgeOnly = annotation.type === "crop";
            return (
              <rect
                key={annotation.id}
                data-hotspot-target={annotation.type === "hotspot" ? annotation.id : undefined}
                x={r.x}
                y={r.y}
                width={r.width}
                height={r.height}
                fill={edgeOnly ? "none" : "transparent"}
                stroke="transparent"
                strokeWidth={hitWidth}
                pointerEvents={!active ? "none" : edgeOnly ? "stroke" : "all"}
                style={{ cursor: "move" }}
                onPointerDown={down}
                {...hover}
              />
            );
          })}
        </g>

        {selected && (
          <g>
            {(() => {
              const r = bounds(selected);
              return (
                <rect
                  x={r.x}
                  y={r.y}
                  width={r.width}
                  height={r.height}
                  fill="none"
                  stroke="var(--status-info)"
                  strokeWidth={1.5}
                  strokeDasharray="5 4"
                  vectorEffect="non-scaling-stroke"
                  pointerEvents="none"
                />
              );
            })()}
            {!readOnly && tool === "select" && (
              <Handles annotation={selected} size={handle} onDown={(e, part) => onShapeDown(e, selected, part)} />
            )}
          </g>
        )}
      </svg>
    </div>
      {card?.type === "hotspot" && anchor && (
        <HotspotPopover hotspot={card.geometry} number={numberOf(card)} style={card.style} anchor={anchor} />
      )}
    </div>
    </div>
  );
}

/**
 * AI blur suggestions as dashed, numbered proposals (full-screenshot pixels,
 * like annotations). Editor-only: they are in `aiState`, never exported.
 */
function Suggestions({
  suggestions,
  unit,
  focused,
}: {
  suggestions: RedactionSuggestion[];
  unit: number;
  focused: number | null;
}) {
  if (!suggestions.length) return null;
  return (
    <g pointerEvents="none" data-testid="blur-suggestions">
      {suggestions.map(({ rect }, i) => (
        <g key={i} data-suggestion={i}>
          <rect
            x={rect.x}
            y={rect.y}
            width={rect.width}
            height={rect.height}
            fill="var(--status-warning)"
            fillOpacity={focused === i ? 0.25 : 0.1}
            stroke="var(--status-warning)"
            strokeWidth={focused === i ? 2.5 : 1.5}
            strokeDasharray="6 4"
            vectorEffect="non-scaling-stroke"
          />
          <rect x={rect.x} y={rect.y} width={16 * unit} height={16 * unit} fill="var(--status-warning)" />
          <text
            x={rect.x + 8 * unit}
            y={rect.y + 12 * unit}
            fontSize={11 * unit}
            textAnchor="middle"
            fill="white"
            fontWeight={600}
          >
            {i + 1}
          </text>
        </g>
      ))}
    </g>
  );
}

/** Accept or reject each blur suggestion; both are undoable. */
function SuggestionList({
  step,
  readOnly,
  onFocus,
}: {
  step: Step;
  readOnly: boolean;
  onFocus(index: number | null): void;
}) {
  const suggestions = step.aiState.redactionSuggestions ?? [];
  if (!suggestions.length) return null;
  return (
    <ul className="flex flex-col gap-1 text-sm" aria-label="Blur suggestions">
      {suggestions.map((suggestion, i) => (
        <li
          key={`${i}:${suggestion.rect.x},${suggestion.rect.y}`}
          className="flex items-center gap-2 rounded-md border border-dashed border-[var(--status-warning)] px-2 py-1"
          onPointerEnter={() => onFocus(i)}
          onPointerLeave={() => onFocus(null)}
          onFocus={() => onFocus(i)}
          onBlur={() => onFocus(null)}
        >
          <span className="text-xs font-semibold text-[var(--status-warning)] tabular-nums">{i + 1}</span>
          <span className="min-w-0 flex-1 truncate" title={suggestion.reason}>
            Suggested blur: {suggestion.reason || "may be sensitive"}
          </span>
          {!readOnly && (
            <>
              <Button
                size="sm"
                variant="outline"
                aria-label={`Accept blur suggestion ${i + 1}`}
                title="Blur this area"
                onClick={() => {
                  onFocus(null);
                  run((guide) => acceptBlurSuggestion(guide, step.id, i, ulid()));
                }}
              >
                Accept
              </Button>
              <Button
                size="sm"
                variant="ghost"
                aria-label={`Reject blur suggestion ${i + 1}`}
                title="Dismiss this suggestion and leave the area as it is"
                onClick={() => {
                  onFocus(null);
                  run((guide) => rejectBlurSuggestion(guide, step.id, i));
                }}
              >
                Reject
              </Button>
            </>
          )}
        </li>
      ))}
    </ul>
  );
}

function Handles({
  annotation,
  size,
  onDown,
}: {
  annotation: Annotation;
  size: number;
  onDown(e: ReactPointerEvent, part: DragPart): void;
}) {
  const square = (key: string, p: Point, part: DragPart, cursor: string) => (
    <rect
      key={key}
      x={p.x - size / 2}
      y={p.y - size / 2}
      width={size}
      height={size}
      fill="white"
      stroke="var(--status-info)"
      strokeWidth={1.5}
      vectorEffect="non-scaling-stroke"
      style={{ cursor }}
      onPointerDown={(e) => onDown(e, part)}
    />
  );
  const dot = (key: string, p: Point, part: DragPart) => (
    <circle
      key={key}
      cx={p.x}
      cy={p.y}
      r={size * 0.6}
      fill="white"
      stroke="var(--status-info)"
      strokeWidth={1.5}
      vectorEffect="non-scaling-stroke"
      style={{ cursor: "crosshair" }}
      onPointerDown={(e) => onDown(e, part)}
    />
  );
  if (annotation.type === "arrow") {
    return (
      <>
        {dot("from", annotation.geometry.from, { kind: "from" })}
        {dot("to", annotation.geometry.to, { kind: "to" })}
      </>
    );
  }
  const rect = bounds(annotation);
  const cursors: Record<string, string> = { nw: "nwse", se: "nwse", ne: "nesw", sw: "nesw", n: "ns", s: "ns", e: "ew", w: "ew" };
  return (
    <>
      {RECT_HANDLES.map((h) => square(h, handlePoint(rect, h), { kind: "rect", handle: h }, `${cursors[h]}-resize`))}
      {annotation.type === "callout" &&
        annotation.geometry.anchor &&
        dot("anchor", annotation.geometry.anchor, { kind: "anchor" })}
    </>
  );
}

/**
 * The step image with its annotation layer: tools above it, the selected
 * annotation's style below. All edits are undoable commands. On a page of
 * an annotation guide the hotspot tool is ready, and walkthrough-only
 * controls (Before/After, click crop, blur suggestions) are left out.
 */
export default function AnnotationEditor({ guideId, step, readOnly }: { guideId: string; step: Step; readOnly: boolean }) {
  const annotated = useAnnotated();
  const [tool, setTool] = useState<Tool | "select">(annotated ? "hotspot" : "select");
  const [preview, setPreview] = useState(false);
  const selectedId = useSelectedAnnotation(step.id);
  const selected = step.annotations.find((a) => a.id === selectedId) ?? null;
  const select = useCallback((id: string | null) => annotationSelection.getState().select(step.id, id), [step.id]);
  // A selection made elsewhere (the inspector's hotspot list) is for editing.
  useEffect(() => {
    if (!selectedId) return;
    setTool("select");
    setPreview(false);
  }, [selectedId]);
  // Fresh selection per step (the editor is keyed by step).
  useEffect(
    () => () => {
      if (annotationSelection.getState().stepId === step.id) annotationSelection.getState().select(step.id, null);
    },
    [step.id],
  );
  const [focusedSuggestion, setFocusedSuggestion] = useState<number | null>(null);
  // View-only, never part of the guide; a new step starts at Before (the
  // editor is keyed by step).
  const [chosenFrame, setFrame] = useState<Frame>("before");
  const frame: Frame = step.image.after && !annotated ? chosenFrame : "before";
  const after = frame === "after";
  const canPreview = annotated || step.annotations.some((a) => a.type === "hotspot");
  const previewing = preview && canPreview;
  const chooseFrame = (next: Frame) => {
    setFrame(next);
    if (next === "after") {
      setTool("select");
      select(null);
      setFocusedSuggestion(null);
    }
  };
  const togglePreview = () => {
    setPreview(!previewing);
    select(null);
  };
  const frameSwitch = step.image.after && !annotated && <BeforeAfterSwitch value={frame} onChange={chooseFrame} />;
  const editing = !readOnly && !after && !previewing;

  return (
    <div className="flex flex-col gap-2">
      {frameSwitch && <div className="flex items-center justify-end">{frameSwitch}</div>}
      {(!readOnly || canPreview) && (
        <div className="flex flex-wrap items-center gap-1" role="toolbar" aria-label="Annotation tools">
          {editing &&
            TOOLS.map(({ tool: value, label, hint, icon: Icon }) => (
              <Button
                key={value}
                variant="ghost"
                size="icon"
                aria-label={label}
                title={hint}
                aria-pressed={tool === value}
                className={cn(tool === value && "bg-accent text-accent-foreground")}
                onClick={() => {
                  setTool(value);
                  if (value !== "select") select(null);
                }}
              >
                <Icon />
              </Button>
            ))}
          {previewing && (
            <span className="text-xs text-muted-foreground">
              As the reader sees it: point at a hotspot, or click it to keep it open.
            </span>
          )}
          <div className="ml-auto flex items-center gap-1">
            {canPreview && (
              <Button
                size="sm"
                variant={previewing ? "default" : "outline"}
                aria-pressed={previewing}
                title="Show the page as the reader of the export sees it"
                onClick={togglePreview}
              >
                <Eye /> Preview
              </Button>
            )}
            {!readOnly && !annotated && (
              <div className="flex items-center gap-1" role="group" aria-label="Image">
                {(["crop", "full"] as const).map((show) => (
                  <Button
                    key={show}
                    size="sm"
                    variant={step.image.show === show ? "default" : "outline"}
                    disabled={show === "crop" && !step.image.crop}
                    title={IMAGE_SHOW_HINTS[show]}
                    onClick={() => run((guide) => setImageShow(guide, step.id, show))}
                  >
                    {show === "crop" ? <Crop /> : <Maximize2 />}
                    {show === "crop" ? "Click crop" : "Full"}
                  </Button>
                ))}
              </div>
            )}
          </div>
        </div>
      )}
      {after && <p className="text-xs text-muted-foreground">Annotations apply to the Before screenshot.</p>}
      <Layer
        guideId={guideId}
        step={step}
        readOnly={readOnly}
        annotated={annotated}
        preview={previewing}
        tool={readOnly ? "select" : tool}
        onToolDone={() => setTool("select")}
        selectedId={previewing ? null : (selected?.id ?? null)}
        select={select}
        focusedSuggestion={focusedSuggestion}
        frame={frame}
      />
      {!after && !annotated && <SuggestionList step={step} readOnly={readOnly} onFocus={setFocusedSuggestion} />}
      {editing && selected && (
        <StyleBar
          key={selected.id}
          stepId={step.id}
          annotation={selected}
          number={hotspotNumbers(step.annotations).get(selected.id)}
          run={run}
          onDeleted={() => select(null)}
        />
      )}
    </div>
  );
}
