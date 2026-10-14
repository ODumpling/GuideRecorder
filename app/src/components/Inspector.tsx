import type { ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { setImageShow } from "@/editor/commands";
import { editorStore, useEditor, useSelectedItem } from "@/editor/editor";
import type { AiState, ImageShow, StepKind } from "@/lib/types";

const KIND_LABELS: Record<StepKind, string> = {
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
    <div className="flex flex-col gap-0.5">
      <dt className="text-xs text-muted-foreground">{label}</dt>
      <dd className="text-sm break-words">{children}</dd>
    </div>
  );
}

/** The right pane: details of the selected step. */
export function Inspector() {
  const readOnly = useEditor((s) => s.doc?.readOnly ?? true);
  const item = useSelectedItem();
  if (!item || item.type !== "step") {
    return <p className="p-4 text-sm text-muted-foreground">No step selected</p>;
  }
  const { element, image } = item;
  const show = (value: ImageShow) =>
    editorStore.getState().dispatch(setImageShow(editorStore.getState().doc!.guide, item.id, value));

  return (
    <div className="flex flex-col gap-5 p-4">
      <section className="flex flex-col gap-2">
        <h2 className="text-xs font-semibold tracking-wide text-muted-foreground uppercase">Image</h2>
        <div className="flex gap-1" role="group" aria-label="Image">
          {(["crop", "full"] as const).map((value) => (
            <Button
              key={value}
              size="sm"
              variant={image.show === value ? "default" : "outline"}
              disabled={readOnly || (value === "crop" && !image.crop)}
              onClick={() => show(value)}
            >
              {value === "crop" ? "Click crop" : "Full screenshot"}
            </Button>
          ))}
        </div>
      </section>
      <section className="flex flex-col gap-2">
        <h2 className="text-xs font-semibold tracking-wide text-muted-foreground uppercase">Step</h2>
        <dl className="flex flex-col gap-2">
          <Field label="Action">{KIND_LABELS[item.kind]}</Field>
          {item.keystrokes && (
            <Field label="Keystrokes">
              <code className="rounded bg-muted px-1 py-0.5 font-mono text-xs">{item.keystrokes}</code>
            </Field>
          )}
          {item.point && (
            <Field label="Point">
              {Math.round(item.point.x)}, {Math.round(item.point.y)}
              {item.endPoint && ` → ${Math.round(item.endPoint.x)}, ${Math.round(item.endPoint.y)}`}
            </Field>
          )}
          <Field label="Text">{AI_LABELS[item.aiState.status]}</Field>
        </dl>
      </section>
      {element && (
        <section className="flex flex-col gap-2">
          <h2 className="text-xs font-semibold tracking-wide text-muted-foreground uppercase">Element</h2>
          <dl className="flex flex-col gap-2">
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
