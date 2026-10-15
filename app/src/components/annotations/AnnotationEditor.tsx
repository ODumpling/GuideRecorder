import { useCallback, useEffect, useLayoutEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import {
  Crop,
  Droplets,
  Grid3x3,
  Highlighter,
  ImageOff,
  Maximize2,
  MessageSquareText,
  MousePointer2,
  MoveUpRight,
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
  imagePixelsPerScreenPixel,
  imageView,
  RECT_HANDLES,
  type DragPart,
  type Tool,
  type View,
} from "@/annotations/geometry";
import { CropMask, Shape, type Backdrop } from "@/components/annotations/Shapes";
import { StyleBar } from "@/components/annotations/StyleBar";
import { Button } from "@/components/ui/button";
import { useStepImage } from "@/components/useStepImage";
import { setImageShow, type Command } from "@/editor/commands";
import { editorStore } from "@/editor/editor";
import { verifyCrop } from "@/lib/ipc";
import type { Annotation, Guide, Point, RedactionSuggestion, Step, StepImage } from "@/lib/types";
import { ulid } from "@/lib/ulid";
import { cn } from "@/lib/utils";

const TOOLS: { tool: Tool | "select"; label: string; icon: typeof Square }[] = [
  { tool: "select", label: "Select", icon: MousePointer2 },
  { tool: "arrow", label: "Arrow", icon: MoveUpRight },
  { tool: "box", label: "Box", icon: Square },
  { tool: "highlight", label: "Highlight", icon: Highlighter },
  { tool: "callout", label: "Callout", icon: MessageSquareText },
  { tool: "blur", label: "Blur", icon: Droplets },
  { tool: "pixelate", label: "Pixelate", icon: Grid3x3 },
  { tool: "crop", label: "Crop", icon: Crop },
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

/** The size of a full screenshot, once loaded (the crop's size comes from `cropRect`). */
function useNaturalSize(url: string | null): { width: number; height: number } | null {
  const [size, setSize] = useState<{ url: string; width: number; height: number } | null>(null);
  useEffect(() => {
    if (!url) return;
    let cancelled = false;
    const image = new Image();
    image.onload = () => {
      if (!cancelled) setSize({ url, width: image.naturalWidth, height: image.naturalHeight });
    };
    image.src = url;
    return () => {
      cancelled = true;
    };
  }, [url]);
  return size && size.url === url ? size : null;
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

function Layer({
  guideId,
  step,
  readOnly,
  tool,
  onToolDone,
  selectedId,
  select,
  focusedSuggestion,
}: {
  guideId: string;
  step: Step;
  readOnly: boolean;
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
  // by the image paths and rect, so a changed crop is checked afresh.
  const verified = useCropCheck(guideId, step.image);
  const [rejectedCrop, setRejectedCrop] = useState<string | null>(null);
  const cropKey = cropCheckKey(step.image);
  const rejected = (rejectedCrop !== null && rejectedCrop === cropKey) || verified === false;
  const shown = imageView(step.image, rejected);
  const { url, error } = useStepImage(guideId, shown.path);
  const natural = useNaturalSize(url);
  const cropMismatch = !!shown.rect && !!natural && !cropFits(shown.rect, natural);
  useEffect(() => {
    if (cropMismatch) setRejectedCrop(cropKey);
  }, [cropMismatch, cropKey]);
  // Nothing is drawn until the crop is known to fit.
  const view: View | null = shown.rect
    ? natural && !cropMismatch && verified === true
      ? shown.rect
      : null
    : natural && { x: 0, y: 0, width: natural.width, height: natural.height };

  const svg = useRef<SVGSVGElement>(null);
  const drag = useRef<Drag | null>(null);
  const [draft, setDraft] = useState<Annotation | null>(null);
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
  }, [view?.x, view?.y, view?.width, view?.height]);

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

  const onShapeDown = (e: ReactPointerEvent, annotation: Annotation, part: DragPart) => {
    if (e.button !== 0 || tool !== "select") return;
    takeFocus();
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
      if (readOnly) return;
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
      if (!view) return;
      const moved = dragAnnotation(annotation, { kind: "body" }, { x: 0, y: 0 }, delta, view);
      run((guide) => updateAnnotation(guide, step.id, moved, "nudge"));
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [selectedId, readOnly, step, select, cancel, view]);

  if (!url || !view) {
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

  return (
    <div className="flex flex-col gap-2">
      {shown.cropUnknown && (
        <p className="text-xs text-muted-foreground">
          {shown.cropUnknown === "missing"
            ? "This step doesn't record where its click crop sits, so the full screenshot is shown for editing."
            : "This step's click crop doesn't match its recorded position, so the full screenshot is shown for editing. The export ignores the recorded position too."}
        </p>
      )}
    <div className="flex items-center justify-center overflow-hidden rounded-lg border border-border bg-muted">
      <svg
        ref={svg}
        role="img"
        aria-label="Step screenshot with annotations"
        viewBox={`${view.x} ${view.y} ${view.width} ${view.height}`}
        className="block max-h-[60vh] w-full touch-none select-none"
        style={{ aspectRatio: `${view.width} / ${view.height}`, cursor: tool === "select" ? "default" : "crosshair" }}
        onPointerDown={onBackgroundDown}
        onPointerMove={onMove}
        onPointerUp={finish}
        onPointerCancel={cancel}
      >
        <image href={url} x={view.x} y={view.y} width={view.width} height={view.height} preserveAspectRatio="none" />
        {/* The exporter's order: redactions last, over everything. */}
        {drawOrder(annotations).map((annotation) => (
          <Shape key={annotation.id} annotation={annotation} backdrop={backdrop} />
        ))}
        {crop?.type === "crop" && <CropMask view={view} rect={crop.geometry} />}
        <Suggestions suggestions={step.aiState.redactionSuggestions ?? []} unit={unit} focused={focusedSuggestion} />

        {/* Hit targets, in drawing order so the topmost shape wins. */}
        <g pointerEvents={tool === "select" ? "auto" : "none"}>
          {hitOrder(annotations).map((annotation) => {
            const down = (e: ReactPointerEvent) => onShapeDown(e, annotation, { kind: "body" });
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
                x={r.x}
                y={r.y}
                width={r.width}
                height={r.height}
                fill={edgeOnly ? "none" : "transparent"}
                stroke="transparent"
                strokeWidth={hitWidth}
                pointerEvents={edgeOnly ? "stroke" : "all"}
                style={{ cursor: "move" }}
                onPointerDown={down}
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
 * annotation's style below. All edits are undoable commands.
 */
export default function AnnotationEditor({ guideId, step, readOnly }: { guideId: string; step: Step; readOnly: boolean }) {
  const [tool, setTool] = useState<Tool | "select">("select");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const selected = step.annotations.find((a) => a.id === selectedId) ?? null;
  const select = useCallback((id: string | null) => setSelectedId(id), []);
  const [focusedSuggestion, setFocusedSuggestion] = useState<number | null>(null);

  return (
    <div className="flex flex-col gap-2">
      {!readOnly && (
        <div className="flex flex-wrap items-center gap-1" role="toolbar" aria-label="Annotation tools">
          {TOOLS.map(({ tool: value, label, icon: Icon }) => (
            <Button
              key={value}
              variant="ghost"
              size="icon"
              aria-label={label}
              title={label}
              aria-pressed={tool === value}
              className={cn(tool === value && "bg-accent text-accent-foreground")}
              onClick={() => {
                setTool(value);
                if (value !== "select") setSelectedId(null);
              }}
            >
              <Icon />
            </Button>
          ))}
          <div className="ml-auto flex items-center gap-1" role="group" aria-label="Image">
            {(["crop", "full"] as const).map((show) => (
              <Button
                key={show}
                size="sm"
                variant={step.image.show === show ? "default" : "outline"}
                disabled={show === "crop" && !step.image.crop}
                onClick={() => run((guide) => setImageShow(guide, step.id, show))}
              >
                {show === "crop" ? <Crop /> : <Maximize2 />}
                {show === "crop" ? "Click crop" : "Full"}
              </Button>
            ))}
          </div>
        </div>
      )}
      <Layer
        guideId={guideId}
        step={step}
        readOnly={readOnly}
        tool={readOnly ? "select" : tool}
        onToolDone={() => setTool("select")}
        selectedId={selected?.id ?? null}
        select={select}
        focusedSuggestion={focusedSuggestion}
      />
      <SuggestionList step={step} readOnly={readOnly} onFocus={setFocusedSuggestion} />
      {selected && !readOnly && (
        <StyleBar stepId={step.id} annotation={selected} run={run} onDeleted={() => setSelectedId(null)} />
      )}
    </div>
  );
}
