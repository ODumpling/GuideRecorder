import { useEffect, useState } from "react";
import { Trash2 } from "lucide-react";
import { isValidColor } from "@/annotations/color";
import { removeAnnotation, setAnnotationStyle, setCalloutText, updateAnnotation } from "@/annotations/commands";
import {
  blurAmount,
  DEFAULT_CALLOUT_STROKE,
  DEFAULT_COLOR,
  DEFAULT_FONT_SIZE,
  DEFAULT_HIGHLIGHT,
  DEFAULT_HIGHLIGHT_OPACITY,
  DEFAULT_STROKE,
  MIN_BLUR_AMOUNT,
  strokeWidth,
} from "@/annotations/geometry";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import type { Command } from "@/editor/commands";
import type { Annotation, AnnotationStyle, Guide } from "@/lib/types";
import { cn } from "@/lib/utils";

const SWATCHES = ["#ff3b30", "#ff9500", "#ffcc00", "#34c759", "#007aff", "#af52de", "#1d1d1f", "#ffffff"];
const STROKES = [1, 2, 3, 4, 6, 8, 12];
const FONT_SIZES = [12, 14, 16, 20, 24, 32, 48];

function Label({ children }: { children: string }) {
  return <span className="text-xs text-muted-foreground">{children}</span>;
}

/** Colour swatches plus a free field that only applies valid colours. */
function ColorField({ value, onChange }: { value: string; onChange(color: string): void }) {
  const [text, setText] = useState(value);
  useEffect(() => setText(value), [value]);
  const valid = isValidColor(text);
  const apply = () => {
    if (valid && text.trim() !== value) onChange(text.trim());
  };
  return (
    <div className="flex items-center gap-1">
      <Label>Colour</Label>
      {SWATCHES.map((swatch) => (
        <button
          key={swatch}
          type="button"
          aria-label={`Colour ${swatch}`}
          title={swatch}
          aria-pressed={value.toLowerCase() === swatch}
          onClick={() => onChange(swatch)}
          className={cn(
            "size-5 rounded-full border border-border",
            value.toLowerCase() === swatch && "ring-2 ring-ring ring-offset-1",
          )}
          style={{ background: swatch }}
        />
      ))}
      <Input
        aria-label="Colour value"
        aria-invalid={!valid}
        title={valid ? "A hex colour or CSS colour name" : "Not a colour: use #rrggbb or a CSS colour name"}
        className={cn("h-7 w-24 font-mono text-xs", !valid && "border-destructive")}
        value={text}
        onChange={(e) => setText(e.target.value)}
        onBlur={apply}
        onKeyDown={(e) => {
          if (e.key === "Enter") apply();
          if (e.key === "Escape") setText(value);
        }}
      />
    </div>
  );
}

/** Style controls for the selected annotation. */
export function StyleBar({
  stepId,
  annotation,
  run,
  onDeleted,
}: {
  stepId: string;
  annotation: Annotation;
  run(build: (guide: Guide) => Command): boolean;
  onDeleted(): void;
}) {
  const style = annotation.style ?? {};
  const set = (changes: Partial<AnnotationStyle>, field?: string) =>
    run((guide) => setAnnotationStyle(guide, stepId, annotation.id, changes, field));
  const type = annotation.type;
  const pixelate = type === "blur" && style.blurMode === "pixelate";

  return (
    <div
      className="flex flex-wrap items-center gap-x-4 gap-y-2 rounded-lg border border-border bg-card px-3 py-2"
      role="group"
      aria-label="Annotation style"
    >
      {(type === "arrow" || type === "box" || type === "callout") && (
        <ColorField value={style.color ?? DEFAULT_COLOR} onChange={(color) => set({ color })} />
      )}
      {type === "highlight" && (
        // The highlight's fill wins over its colour at export; edit the colour.
        <ColorField value={style.fill ?? style.color ?? DEFAULT_HIGHLIGHT} onChange={(color) => set({ color, fill: undefined }, "color")} />
      )}
      {(type === "arrow" || type === "box" || type === "callout") && (
        <label className="flex items-center gap-1">
          <Label>Stroke</Label>
          <select
            aria-label="Stroke width"
            className="h-7 rounded-md border border-input bg-transparent px-1 text-sm"
            value={strokeWidth(style.strokeWidth, type === "callout" ? DEFAULT_CALLOUT_STROKE : DEFAULT_STROKE)}
            onChange={(e) => set({ strokeWidth: Number(e.target.value) })}
          >
            {STROKES.map((w) => (
              <option key={w} value={w}>
                {w} px
              </option>
            ))}
          </select>
        </label>
      )}
      {type === "highlight" && (
        <label className="flex items-center gap-1">
          <Label>Opacity</Label>
          <input
            type="range"
            aria-label="Opacity"
            min={0.1}
            max={1}
            step={0.05}
            value={style.opacity ?? DEFAULT_HIGHLIGHT_OPACITY}
            onChange={(e) => set({ opacity: Number(e.target.value) })}
          />
        </label>
      )}
      {type === "blur" && (
        <>
          <div className="flex items-center gap-1" role="group" aria-label="Redaction">
            {(["blur", "pixelate"] as const).map((mode) => (
              <Button
                key={mode}
                size="sm"
                variant={(style.blurMode ?? "blur") === mode ? "default" : "outline"}
                // The amount means radius or block size: back to the mode's default.
                onClick={() => set({ blurMode: mode, blurAmount: undefined }, "mode")}
              >
                {mode === "blur" ? "Blur" : "Pixelate"}
              </Button>
            ))}
          </div>
          <label className="flex items-center gap-1">
            <Label>{pixelate ? "Block size" : "Strength"}</Label>
            <input
              type="range"
              aria-label={pixelate ? "Block size" : "Blur strength"}
              min={MIN_BLUR_AMOUNT}
              max={40}
              step={1}
              value={blurAmount(annotation)}
              onChange={(e) => set({ blurAmount: Number(e.target.value) })}
            />
          </label>
          <span className="text-xs text-muted-foreground">
            Preview only: the export burns the redaction into the image, over everything inside it, including other
            shapes and callout text.
          </span>
        </>
      )}
      {type === "callout" && (
        <>
          <label className="flex items-center gap-1">
            <Label>Text size</Label>
            <select
              aria-label="Text size"
              className="h-7 rounded-md border border-input bg-transparent px-1 text-sm"
              value={style.fontSize ?? DEFAULT_FONT_SIZE}
              onChange={(e) => set({ fontSize: Number(e.target.value) })}
            >
              {FONT_SIZES.map((size) => (
                <option key={size} value={size}>
                  {size}
                </option>
              ))}
            </select>
          </label>
          <Button
            size="sm"
            variant="outline"
            onClick={() => {
              const g = annotation.geometry;
              const geometry = { ...g };
              if (g.anchor) delete geometry.anchor;
              else geometry.anchor = { x: Math.round(g.x - 60), y: Math.round(g.y + g.height + 60) };
              run((guide) => updateAnnotation(guide, stepId, { ...annotation, geometry }));
            }}
          >
            {annotation.geometry.anchor ? "Remove pointer" : "Add pointer"}
          </Button>
          <textarea
            aria-label="Callout text"
            placeholder="Callout text"
            // A new, empty callout is waiting for its text.
            autoFocus={annotation.geometry.text === ""}
            rows={2}
            className="min-w-60 flex-1 rounded-md border border-input bg-transparent px-2 py-1 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring"
            value={annotation.geometry.text}
            onChange={(e) => run((guide) => setCalloutText(guide, stepId, annotation.id, e.target.value))}
          />
        </>
      )}
      {type === "crop" && (
        <span className="text-xs text-muted-foreground">The export shows only the area inside the crop.</span>
      )}
      <Button
        variant="ghost"
        size="sm"
        className="ml-auto"
        title="Delete (⌫)"
        onClick={() => {
          if (run((guide) => removeAnnotation(guide, stepId, annotation.id))) onDeleted();
        }}
      >
        <Trash2 /> Delete
      </Button>
    </div>
  );
}
