import { useState, type ReactNode } from "react";
import { ArrowDown, ArrowUp, TriangleAlert } from "lucide-react";
import type { AiState, ImageShow, Step, StepKind } from "@guiderecorder/guide-schema";
import { Button } from "@guiderecorder/ui/button";
import { Input } from "@guiderecorder/ui/input";
import { hotspots } from "@guiderecorder/ui/geometry";
import { cn } from "@guiderecorder/ui/utils";
import { moveHotspot } from "@/annotations/commands";
import { HotspotBadge } from "@/components/annotations/HotspotPopover";
import { annotationSelection, useSelectedAnnotation } from "@/components/annotations/selection";
import { PrivacyPanel } from "@/components/PrivacyPanel";
import { IMAGE_SHOW_HINTS, setImageShow } from "@/editor/commands";
import { editorStore, stepNoun, useAnnotated, useEditor, useSelectedItem } from "@/editor/editor";
import { describeIssues, elementIssues, setElementLabel } from "@/editor/element";
import { inspectorStore, tabCounts, useInspector, type InspectorTab } from "@/editor/inspector";
import { displayUrl, isWebAddress } from "@/editor/web";
import { openStepUrl } from "@/lib/ipc";
import { AI_ENABLED } from "@/lib/features";

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

function move(stepId: string, id: string, by: number) {
  const doc = editorStore.getState().doc;
  if (doc) editorStore.getState().dispatch(moveHotspot(doc.guide, stepId, id, by));
}

/**
 * The step's hotspots by number; a click selects one in the annotation
 * editor, the arrows reorder them (their numbers follow).
 */
function HotspotList({ step, readOnly, emptyHint }: { step: Step; readOnly: boolean; emptyHint: string }) {
  const selectedId = useSelectedAnnotation(step.id);
  const list = hotspots(step.annotations);
  if (list.length === 0) {
    return <p className="text-sm text-muted-foreground">{readOnly ? "No hotspots" : emptyHint}</p>;
  }
  return (
    <ol className="-mx-2 flex flex-col gap-0.5" aria-label="Hotspots">
      {list.map((hotspot, i) => {
        const { title, description } = hotspot.geometry;
        const name = title.trim() || "Untitled hotspot";
        return (
          <li key={hotspot.id} className="group flex items-start gap-0.5">
            <button
              type="button"
              aria-current={hotspot.id === selectedId || undefined}
              onClick={() => annotationSelection.getState().select(step.id, hotspot.id)}
              className={cn(
                "flex min-w-0 flex-1 items-start gap-2 rounded-md px-2 py-1.5 text-left text-sm hover:bg-accent",
                hotspot.id === selectedId && "bg-accent text-accent-foreground",
              )}
            >
              <HotspotBadge number={i + 1} style={hotspot.style} />
              <span className="flex min-w-0 flex-col">
                <span className={cn("truncate", !title.trim() && "text-muted-foreground italic")}>{name}</span>
                {description.trim() && <span className="truncate text-xs text-muted-foreground">{description.trim()}</span>}
              </span>
            </button>
            {!readOnly && list.length > 1 && (
              <span className="flex shrink-0 opacity-0 group-focus-within:opacity-100 group-hover:opacity-100">
                <Button
                  size="icon"
                  variant="ghost"
                  aria-label={`Move hotspot ${i + 1} up`}
                  title="Move up"
                  disabled={i === 0}
                  onClick={() => move(step.id, hotspot.id, -1)}
                >
                  <ArrowUp />
                </Button>
                <Button
                  size="icon"
                  variant="ghost"
                  aria-label={`Move hotspot ${i + 1} down`}
                  title="Move down"
                  disabled={i === list.length - 1}
                  onClick={() => move(step.id, hotspot.id, 1)}
                >
                  <ArrowDown />
                </Button>
              </span>
            )}
          </li>
        );
      })}
    </ol>
  );
}

const TABS: { tab: InspectorTab; label: string }[] = [
  { tab: "step", label: "Step" },
  { tab: "privacy", label: "Privacy" },
  { tab: "hotspots", label: "Hotspots" },
];

function Tabs({ step, tab }: { step: Step; tab: InspectorTab }) {
  const badges = tabCounts(step);
  return (
    <div className="flex gap-0.5 border-b border-border px-3 pt-2.5" role="tablist" aria-label="Inspector">
      {TABS.map(({ tab: value, label }) => (
        <button
          key={value}
          type="button"
          role="tab"
          id={`inspector-tab-${value}`}
          aria-selected={tab === value}
          aria-controls="inspector-panel"
          onClick={() => inspectorStore.getState().setTab(value)}
          className={cn(
            "-mb-px flex items-center gap-1.5 border-b-2 border-transparent px-2.5 pt-2 pb-2.5 text-[13px] font-medium text-muted-foreground hover:text-foreground",
            tab === value && "border-foreground text-foreground",
          )}
        >
          {label}
          {badges[value] > 0 && (
            <span
              className="rounded-full bg-[var(--status-warning)] px-1.5 text-[10.5px] leading-4 font-semibold text-white tabular-nums"
              title={value === "privacy" ? "Suggested blurs to review" : "Hotspots without a title"}
            >
              {badges[value]}
            </span>
          )}
        </button>
      ))}
    </div>
  );
}

/**
 * The right pane: the selected step in tabs (Step, Privacy, Hotspots), or
 * the hotspots of a page. `onPrivacySettings` opens Settings → Privacy.
 */
export function Inspector({ onPrivacySettings }: { onPrivacySettings?: () => void }) {
  const readOnly = useEditor((s) => s.doc?.readOnly ?? true);
  const annotated = useAnnotated();
  const item = useSelectedItem();
  const tab = useInspector((s) => s.tab);
  if (!item || item.type !== "step") {
    return <p className="p-4 text-sm text-muted-foreground">No {stepNoun(annotated)} selected</p>;
  }
  if (annotated) {
    // A page has no click, crop, AI text or scan to show.
    return (
      <div className="flex flex-col gap-5 p-4">
        <section className="flex flex-col gap-2">
          <h2 className={HEADING}>Hotspots</h2>
          <HotspotList
            step={item}
            readOnly={readOnly}
            emptyHint="Drag over the image to add a hotspot the reader can point at."
          />
        </section>
      </div>
    );
  }
  return (
    <div className="flex flex-col">
      <Tabs step={item} tab={tab} />
      <div id="inspector-panel" role="tabpanel" aria-labelledby={`inspector-tab-${tab}`} className="p-4 pt-5">
        {tab === "step" && <StepDetails step={item} readOnly={readOnly} />}
        {tab === "privacy" && <PrivacyPanel step={item} readOnly={readOnly} onSettings={onPrivacySettings} />}
        {tab === "hotspots" && (
          <HotspotList
            step={item}
            readOnly={readOnly}
            emptyHint="Pick the hotspot tool and drag over the image to add an area the reader can point at."
          />
        )}
      </div>
    </div>
  );
}

/**
 * A warning when the step's element is unclear (#15), so an agent skill or
 * the replay script may not find it, with a field to name it. The field
 * stays while the author types, even once the name makes the element clear.
 */
function ElementCheck({ step, readOnly }: { step: Step; readOnly: boolean }) {
  const issues = elementIssues(step);
  const [naming, setNaming] = useState<string | null>(null);
  const open = naming === step.id;
  if (issues.length === 0 && !open) return null;
  const canName = !readOnly && (open || issues.includes("noLabel") || issues.includes("noElement"));
  const rename = (label: string) => {
    setNaming(step.id);
    const doc = editorStore.getState().doc;
    if (doc) editorStore.getState().dispatch(setElementLabel(doc.guide, step.id, label));
  };
  return (
    <div className="flex flex-col gap-2">
      {issues.length > 0 && (
        <p
          role="status"
          className="flex items-start gap-2 rounded-md border border-[var(--status-warning)] bg-muted p-2.5 text-xs"
        >
          <TriangleAlert className="mt-0.5 size-3.5 shrink-0 text-[var(--status-warning)]" />
          <span>
            <span className="font-medium">Unclear element: </span>
            {describeIssues(issues)}. Agent skills and the replay script may not find it.
          </span>
        </p>
      )}
      {canName && (
        <Input
          aria-label="Element name"
          placeholder="Name it as it reads on screen, e.g. Save"
          value={step.element?.label ?? ""}
          onChange={(e) => rename(e.target.value)}
        />
      )}
    </div>
  );
}

/** The web page a step was recorded on, opened in the default browser. */
function PageLink({ url }: { url: string }) {
  return (
    <a
      href={url}
      title={url}
      className="text-primary underline-offset-2 hover:underline"
      onClick={(e) => {
        // Never in the app's own window.
        e.preventDefault();
        void openStepUrl(url).catch(() => {});
      }}
    >
      {displayUrl(url)}
    </a>
  );
}

/** The Step tab: close-up or full screenshot, the action, its element, its web page and the text's status. */
function StepDetails({ step: item, readOnly }: { step: Step; readOnly: boolean }) {
  const { element, image } = item;
  const show = (value: ImageShow) =>
    editorStore.getState().dispatch(setImageShow(editorStore.getState().doc!.guide, item.id, value));

  return (
    <div className="flex flex-col gap-6">
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
          {item.url && isWebAddress(item.url) && (
            <Field label="Web page">
              <PageLink url={item.url} />
            </Field>
          )}
          {item.point && (
            <Field label="Point">
              {Math.round(item.point.x)}, {Math.round(item.point.y)}
              {item.endPoint && ` → ${Math.round(item.endPoint.x)}, ${Math.round(item.endPoint.y)}`}
            </Field>
          )}
          {AI_ENABLED && (
            <Field label="Text">
              <span className={cn(item.aiState.status === "ai" && "font-medium text-ai")}>{AI_LABELS[item.aiState.status]}</span>
            </Field>
          )}
        </dl>
      </section>
      {(element || elementIssues(item).length > 0) && (
        <section className="flex flex-col gap-2">
          <h2 className={HEADING}>Element</h2>
          <ElementCheck step={item} readOnly={readOnly} />
          {element && (
            <dl className={FIELDS}>
              {element.label && <Field label="Label">{element.label}</Field>}
              {element.role && <Field label="Role">{element.role}</Field>}
              {element.app && <Field label="App">{element.app}</Field>}
              {element.window && <Field label="Window">{element.window}</Field>}
              {element.bundleId && (
                // Recorded on Windows, the id is the executable, e.g. 1password.exe.
                <Field label={/\.exe$/i.test(element.bundleId) ? "Executable" : "Bundle ID"}>{element.bundleId}</Field>
              )}
            </dl>
          )}
        </section>
      )}
    </div>
  );
}
