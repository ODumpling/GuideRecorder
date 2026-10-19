import type { ReactNode } from "react";
import { hotspots } from "@/annotations/geometry";
import { HotspotBadge } from "@/components/annotations/HotspotPopover";
import { annotationSelection, useSelectedAnnotation } from "@/components/annotations/selection";
import { Button } from "@/components/ui/button";
import { IMAGE_SHOW_HINTS, setImageShow } from "@/editor/commands";
import { editorStore, stepNoun, useAnnotated, useEditor, useSelectedItem } from "@/editor/editor";
import type { AiState, ImageShow, Step, StepKind } from "@/lib/types";
import { cn } from "@/lib/utils";

export const KIND_LABELS: Record<StepKind, string> = {
  click: "Click",
  doubleClick: "Double click",
  rightClick: "Right click",
  type: "Type",
  shortcut: "Shortcut",
  drag: "Drag",
  manual: "Manual",
};

const AI_LABELS: Record<AiState["status"], string> = {
  template: "Template",
  generating: "Generating…",
  ai: "AI",
  edited: "Edited",
};

function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <>
      <dt className="text-muted-foreground">{label}</dt>
      <dd className="min-w-0 break-words">{children}</dd>
    </>
  );
}

const HEADING = "text-[11px] font-semibold tracking-wide text-muted-foreground uppercase";
const FIELDS = "grid grid-cols-[5rem_1fr] gap-x-3 gap-y-2 text-[13px]";

/** The step's hotspots by number; a click selects one in the annotation editor. */
function HotspotList({ step, readOnly }: { step: Step; readOnly: boolean }) {
  const selectedId = useSelectedAnnotation(step.id);
  const list = hotspots(step.annotations);
  return (
    <section className="flex flex-col gap-2">
      <h2 className={HEADING}>Hotspots</h2>
      {list.length === 0 ? (
        <p className="text-sm text-muted-foreground">
          {readOnly ? "No hotspots" : "Drag over the image to add a hotspot the reader can point at."}
        </p>
      ) : (
        <ol className="-mx-2 flex flex-col gap-0.5" aria-label="Hotspots">
          {list.map((hotspot, i) => {
            const { title, description } = hotspot.geometry;
            return (
              <li key={hotspot.id}>
                <button
                  type="button"
                  aria-current={hotspot.id === selectedId || undefined}
                  onClick={() => annotationSelection.getState().select(step.id, hotspot.id)}
                  className={cn(
                    "flex w-full items-start gap-2 rounded-md px-2 py-1.5 text-left text-sm hover:bg-accent",
                    hotspot.id === selectedId && "bg-accent text-accent-foreground",
                  )}
                >
                  <HotspotBadge number={i + 1} style={hotspot.style} />
                  <span className="flex min-w-0 flex-col">
                    <span className={cn("truncate", !title.trim() && "text-muted-foreground italic")}>
                      {title.trim() || "Untitled hotspot"}
                    </span>
                    {description.trim() && (
                      <span className="truncate text-xs text-muted-foreground">{description.trim()}</span>
                    )}
                  </span>
                </button>
              </li>
            );
          })}
        </ol>
      )}
    </section>
  );
}

/** The right pane: details of the selected step, or the hotspots of a page. */
export function Inspector() {
  const readOnly = useEditor((s) => s.doc?.readOnly ?? true);
  const annotated = useAnnotated();
  const item = useSelectedItem();
  if (!item || item.type !== "step") {
    return <p className="p-4 text-sm text-muted-foreground">No {stepNoun(annotated)} selected</p>;
  }
  if (annotated) {
    // A page has no click, crop or AI text to show.
    return (
      <div className="flex flex-col gap-5 p-4">
        <HotspotList step={item} readOnly={readOnly} />
      </div>
    );
  }
  const { element, image } = item;
  const show = (value: ImageShow) =>
    editorStore.getState().dispatch(setImageShow(editorStore.getState().doc!.guide, item.id, value));

  return (
    <div className="flex flex-col gap-6 p-4 pt-5">
      <section className="flex flex-col gap-2">
        <h2 className={HEADING}>Image</h2>
        <div className="flex gap-0.5 rounded-lg bg-foreground/[0.06] p-0.5" role="group" aria-label="Image">
          {(["crop", "full"] as const).map((value) => (
            <Button
              key={value}
              size="sm"
              variant="ghost"
              aria-pressed={image.show === value}
              className={cn(
                "h-7 flex-1 rounded-md",
                image.show === value && "bg-card text-card-foreground shadow-sm hover:bg-card",
              )}
              disabled={readOnly || (value === "crop" && !image.crop)}
              title={IMAGE_SHOW_HINTS[value]}
              onClick={() => show(value)}
            >
              {value === "crop" ? "Click crop" : "Full screenshot"}
            </Button>
          ))}
        </div>
      </section>
      <section className="flex flex-col gap-2">
        <h2 className={HEADING}>Step</h2>
        <dl className={FIELDS}>
          <Field label="Action">{KIND_LABELS[item.kind]}</Field>
          {item.keystrokes && (
            <Field label="Keystrokes">
              <code className="rounded bg-muted px-1.5 py-0.5 font-mono text-xs">{item.keystrokes}</code>
            </Field>
          )}
          {item.point && (
            <Field label="Point">
              {Math.round(item.point.x)}, {Math.round(item.point.y)}
              {item.endPoint && ` → ${Math.round(item.endPoint.x)}, ${Math.round(item.endPoint.y)}`}
            </Field>
          )}
          <Field label="Text">
            <span className={cn(item.aiState.status === "ai" && "font-medium text-ai")}>{AI_LABELS[item.aiState.status]}</span>
          </Field>
        </dl>
      </section>
      {item.annotations.some((a) => a.type === "hotspot") && <HotspotList step={item} readOnly={readOnly} />}
      {element && (
        <section className="flex flex-col gap-2">
          <h2 className={HEADING}>Element</h2>
          <dl className={FIELDS}>
            {element.label && <Field label="Label">{element.label}</Field>}
            {element.role && <Field label="Role">{element.role}</Field>}
            {element.app && <Field label="App">{element.app}</Field>}
            {element.window && <Field label="Window">{element.window}</Field>}
            {element.bundleId && <Field label="Bundle ID">{element.bundleId}</Field>}
          </dl>
        </section>
      )}
    </div>
  );
}
