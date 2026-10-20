import { lazy, Suspense, useMemo, useState, type DragEvent } from "react";
import {
  ArrowDown,
  ArrowLeft,
  ArrowUp,
  FileText,
  Heading,
  ImageIcon,
  Info,
  Lightbulb,
  MousePointerClick,
  Plus,
  Sparkles,
  TriangleAlert,
} from "lucide-react";
import { aiStore, useAi } from "@/ai/ai";
import { BUSY_HINT } from "@/ai/controller";
import { StepStatusBadge, useAiBlocked } from "@/ai/WriteWithAi";
import { Button } from "@guiderecorder/ui/button";
import { deleteSelected } from "@/editor/actions";
import { Input } from "@guiderecorder/ui/input";
import { useStepThumbnail } from "@/components/useStepThumbnail";
import { dropTarget, moveItem, renameGuide } from "@/editor/commands";
import { editorStore, stepNumbers, useEditor } from "@/editor/editor";
import type { Item, Step } from "@guiderecorder/guide-schema";
import { cn } from "@guiderecorder/ui/utils";
import { markdownToPlainText } from "@guiderecorder/ui/markdown";

// Plain-text labels by Markdown source. The outline re-renders on every
// keystroke, but only the edited item's text changes.
const plainLabels = new Map<string, string>();

function plainLabel(markdown: string): string {
  let label = plainLabels.get(markdown);
  if (label === undefined) {
    if (plainLabels.size >= 1000) plainLabels.clear();
    label = markdownToPlainText(markdown).replace(/\n/g, " ");
    plainLabels.set(markdown, label);
  }
  return label;
}

// The menu (Radix and its positioning) loads with the first opened guide.
const InsertMenu = lazy(() => import("@/components/InsertMenu"));

export function itemLabel(item: Item, annotated: boolean): string {
  switch (item.type) {
    case "step":
      return plainLabel(item.title) || (annotated ? "Untitled page" : "Untitled step");
    case "section":
      return plainLabel(item.title) || "Untitled section";
    case "note":
      return plainLabel(item.body) || `Empty ${item.kind}`;
  }
}

function ItemIcon({ item }: { item: Item }) {
  if (item.type === "section") return <Heading className="size-3.5 text-muted-foreground" />;
  if (item.type === "note") {
    if (item.kind === "tip") return <Lightbulb className="size-3.5 text-[var(--status-success)]" />;
    if (item.kind === "warning") return <TriangleAlert className="size-3.5 text-[var(--status-warning)]" />;
    return <Info className="size-3.5 text-[var(--status-info)]" />;
  }
  if (item.kind === "manual") return <ImageIcon className="size-3.5 text-muted-foreground" />;
  return <MousePointerClick className="size-3.5 text-muted-foreground" />;
}

/** A step's screenshot, small: the close-up if it has one. */
function StepThumb({ guideId, step }: { guideId: string; step: Step }) {
  const url = useStepThumbnail(guideId, step.image.crop || step.image.full || null);
  return (
    <span className="flex h-8 w-12 shrink-0 items-center justify-center overflow-hidden rounded-[5px] border border-sidebar-border bg-background compact:h-6 compact:w-9">
      {url ? <img src={url} alt="" className="size-full object-cover" /> : <ItemIcon item={step} />}
    </span>
  );
}

/**
 * Regenerates the guide details (title, intro, prerequisites, summary,
 * tags), edited or not, as one undoable change.
 */
function RegenerateGuide({ readOnly }: { readOnly: boolean }) {
  const { busy, background } = useAiBlocked();
  const writing = useAi((s) => s.explicit?.kind === "guide");
  return (
    <Button
      variant="ghost"
      size="icon"
      className={cn(writing && "animate-pulse")}
      aria-label={writing ? "Writing the guide details" : "Regenerate guide details"}
      title={background ? BUSY_HINT : "Regenerate the title, introduction, prerequisites, summary and tags with AI"}
      disabled={readOnly || busy}
      onClick={() => void aiStore.getState().regenerateGuide()}
    >
      <Sparkles />
    </Button>
  );
}

/** The open guide's title and its items, with selection and reordering. */
export function Outline() {
  const doc = useEditor((s) => s.doc)!;
  const selectedId = useEditor((s) => s.selectedId);
  const { guide, readOnly } = doc;
  // Annotation guides have pages, which AI doesn't write.
  const annotated = guide.kind === "annotated";
  const numbers = useMemo(() => stepNumbers(guide.items), [guide.items]);
  const [dragId, setDragId] = useState<string | null>(null);
  const [dropIndex, setDropIndex] = useState<number | null>(null);

  const { dispatch, select } = editorStore.getState();
  const move = (id: string, to: number) => dispatch(moveItem(editorStore.getState().doc!.guide, id, to));
  /** Drops the dragged item before position `before` (items.length: the end). */
  const drop = (before: number) => {
    const items = editorStore.getState().doc!.guide.items;
    const from = items.findIndex((item) => item.id === dragId);
    if (from >= 0) move(items[from].id, dropTarget(from, before));
    setDragId(null);
    setDropIndex(null);
  };
  const dropZone = (before: number) => ({
    onDragOver: (e: DragEvent) => {
      if (!dragId) return;
      e.preventDefault();
      setDropIndex(before);
    },
    onDrop: (e: DragEvent) => {
      e.preventDefault();
      drop(before);
    },
  });

  const back = async () => {
    if (await editorStore.getState().closeGuide()) await editorStore.getState().loadGuides();
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex flex-col gap-1.5 border-b border-sidebar-border p-2.5">
        <div className="flex items-center justify-between">
          <Button variant="ghost" size="sm" title="Close this guide and go back to the library" onClick={() => void back()}>
            <ArrowLeft /> Library
          </Button>
          <Suspense
            fallback={
              <Button variant="outline" size="sm" disabled>
                <Plus /> Insert
              </Button>
            }
          >
            <InsertMenu disabled={readOnly} />
          </Suspense>
        </div>
        <div className="flex items-center gap-1">
          <Input
            aria-label="Guide title"
            className="h-9 border-transparent bg-transparent px-2 text-[15px] font-semibold hover:border-sidebar-border focus-visible:bg-background"
            value={guide.title}
            disabled={readOnly}
            onChange={(e) => dispatch(renameGuide(editorStore.getState().doc!.guide, e.target.value))}
          />
          {!annotated && <RegenerateGuide readOnly={readOnly} />}
        </div>
        <button
          type="button"
          aria-current={selectedId === null || undefined}
          title="Edit the guide's introduction, prerequisites, summary and tags"
          onClick={() => select(null)}
          className={cn(
            "flex items-center gap-2 rounded-lg px-2 py-1.5 text-left text-[13px] hover:bg-sidebar-foreground/5",
            selectedId === null && "bg-sidebar-accent text-sidebar-accent-foreground shadow-sm hover:bg-sidebar-accent",
          )}
        >
          <FileText className="size-3.5 text-muted-foreground" /> Guide details
        </button>
      </div>
      <ol className="flex min-h-0 flex-1 flex-col gap-px overflow-y-auto p-2" onDragLeave={() => setDropIndex(null)}>
        {guide.items.map((item, index) => (
          <li
            key={item.id}
            draggable={!readOnly}
            onDragStart={(e) => {
              setDragId(item.id);
              e.dataTransfer.effectAllowed = "move";
            }}
            onDragEnd={() => {
              setDragId(null);
              setDropIndex(null);
            }}
            {...dropZone(index)}
            className={cn(
              "group flex items-center gap-0.5 rounded-lg border-t-2 border-transparent",
              item.type === "section" && "mt-2 first:mt-0",
              dropIndex === index && dragId !== item.id && "border-[var(--status-info)]",
              dragId === item.id && "opacity-50",
            )}
          >
            <button
              type="button"
              onClick={() => select(item.id)}
              onKeyDown={(e) => {
                if (readOnly || (e.key !== "Delete" && e.key !== "Backspace")) return;
                e.preventDefault();
                select(item.id);
                deleteSelected(editorStore);
                // Keep the keyboard in the outline, on the newly selected item.
                const next = editorStore.getState().selectedId;
                requestAnimationFrame(() =>
                  document.querySelector<HTMLElement>(`[data-outline-item="${next}"]`)?.focus(),
                );
              }}
              data-outline-item={item.id}
              className={cn(
                "flex min-w-0 flex-1 items-center gap-2 rounded-lg px-1.5 py-1.5 text-left text-[13px] compact:py-1 text-sidebar-foreground/85 outline-none hover:bg-sidebar-foreground/5 focus-visible:ring-2 focus-visible:ring-sidebar-ring",
                item.type === "section" && "py-1 compact:py-0.5 text-[11px] font-semibold tracking-wide text-muted-foreground uppercase",
                item.type === "note" && "bg-sidebar-foreground/[0.04] text-xs",
                item.id === selectedId &&
                  "bg-sidebar-accent text-sidebar-accent-foreground shadow-sm hover:bg-sidebar-accent",
              )}
            >
              {item.type === "step" ? (
                <>
                  <span
                    className={cn(
                      "w-4 shrink-0 text-right text-[11px] text-muted-foreground tabular-nums",
                      item.id === selectedId && "font-semibold text-brand",
                    )}
                  >
                    {numbers.get(item.id)}
                  </span>
                  <StepThumb guideId={guide.id} step={item} />
                </>
              ) : (
                <span className="flex w-4 shrink-0 justify-end">
                  <ItemIcon item={item} />
                </span>
              )}
              <span className={cn(item.type === "step" ? "line-clamp-2 leading-snug" : "truncate")}>
                {itemLabel(item, annotated)}
              </span>
              {item.type === "step" && !annotated && <StepStatusBadge stepId={item.id} aiState={item.aiState} />}
            </button>
            {!readOnly && (
              <span className="flex opacity-0 group-focus-within:opacity-100 group-hover:opacity-100">
                <Button
                  variant="ghost"
                  size="icon"
                  aria-label="Move up"
                  title="Move up"
                  disabled={index === 0}
                  onClick={() => move(item.id, index - 1)}
                >
                  <ArrowUp />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  aria-label="Move down"
                  title="Move down"
                  disabled={index === guide.items.length - 1}
                  onClick={() => move(item.id, index + 1)}
                >
                  <ArrowDown />
                </Button>
              </span>
            )}
          </li>
        ))}
        {dragId && (
          // Drop target after the last item.
          <li
            aria-hidden
            {...dropZone(guide.items.length)}
            className={cn(
              "h-8 rounded-md border-t-2 border-transparent",
              dropIndex === guide.items.length && "border-[var(--status-info)]",
            )}
          />
        )}
      </ol>
    </div>
  );
}
