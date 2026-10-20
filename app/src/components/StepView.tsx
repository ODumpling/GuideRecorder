import { lazy, Suspense, useRef, useState, type Ref } from "react";
import {
  ChevronLeft,
  ChevronRight,
  GraduationCap,
  ImageOff,
  Merge,
  Minimize2,
  PenLine,
  RefreshCw,
  Rocket,
  Scissors,
  Trash2,
} from "lucide-react";
import { aiStore, useAi } from "@/ai/ai";
import { BUSY_HINT } from "@/ai/controller";
import { CustomRewrite } from "@/ai/CustomRewrite";
import { useAiBlocked } from "@/ai/WriteWithAi";
import { KIND_LABELS } from "@/components/Inspector";
import { GuideDetails } from "@/components/GuideDetails";
import type { MarkdownEditorHandle } from "@guiderecorder/ui/markdown-editor";
import { Button } from "@guiderecorder/ui/button";
import { useStepImage } from "@/components/useStepImage";
import { canMergeSelected, deleteSelected, mergeSelected, splitSelected } from "@/editor/actions";
import { setItemText, type TextField } from "@/editor/commands";
import { editorStore, stepNoun, stepNumbers, useAnnotated, useEditor, useSelectedItem } from "@/editor/editor";
import type { Item, Step } from "@guiderecorder/guide-schema";
import { ItemView } from "@guiderecorder/ui/step-view";
import type { RewriteStyleArg } from "@/lib/types";
import { cn } from "@guiderecorder/ui/utils";
import { markdownToPlainText } from "@guiderecorder/ui/markdown";

// TipTap is most of the bundle; load it with the first opened guide.
const MarkdownEditor = lazy(() => import("@guiderecorder/ui/markdown-editor"));
// The annotation layer, likewise; the plain image shows until it loads.
const AnnotationEditor = lazy(() => import("@/components/annotations/AnnotationEditor"));
// The step's ⋯ menu (Radix and its positioning), likewise.
const StepMenu = lazy(() => import("@/components/StepMenu"));

function StepImage({ guideId, step }: { guideId: string; step: Step }) {
  const path = step.image.show === "crop" && step.image.crop ? step.image.crop : step.image.full;
  const { url, error } = useStepImage(guideId, path);
  return (
    <div className="flex min-h-48 items-center justify-center overflow-hidden rounded-xl border border-border bg-muted shadow-lg">
      {url ? (
        <img src={url} alt="" className="max-h-[60vh] max-w-full object-contain" />
      ) : (
        <span className="flex items-center gap-2 p-6 text-sm text-muted-foreground">
          {error ? (
            <>
              <ImageOff className="size-4" /> {error}
            </>
          ) : (
            "Loading image…"
          )}
        </span>
      )}
    </div>
  );
}

/** A Markdown field of an item, edited through store commands. */
function TextEditor({
  handle,
  id,
  field,
  value,
  readOnly,
  label,
  placeholder,
  singleLine,
  className,
}: {
  handle?: Ref<MarkdownEditorHandle>;
  id: string;
  field: TextField;
  value: string;
  readOnly: boolean;
  label: string;
  placeholder: string;
  singleLine?: boolean;
  className?: string;
}) {
  const fallback = (
    <div
      aria-label={label}
      className={cn(
        "-mx-2.5 rounded-md border border-transparent px-2.5 py-1.5 whitespace-pre-wrap",
        !singleLine && "min-h-16",
        !value && "text-muted-foreground",
        className,
      )}
    >
      {markdownToPlainText(value) || placeholder}
    </div>
  );
  return (
    <Suspense fallback={fallback}>
      <MarkdownEditor
        // A fresh editor per item, so the cursor never carries over.
        key={`${id}.${field}`}
        handle={handle}
        value={value}
        readOnly={readOnly}
        label={label}
        placeholder={placeholder}
        singleLine={singleLine}
        className={className}
        bare
        onChange={(markdown) => {
          const { doc, dispatch } = editorStore.getState();
          return !!doc && dispatch(setItemText(doc.guide, id, field, markdown));
        }}
        onUndo={() => editorStore.getState().undo()}
        onRedo={() => editorStore.getState().redo()}
      />
    </Suspense>
  );
}

/**
 * Delete, merge and split for the selected item. A page of an annotation
 * guide is only deleted: merging would drop the next page's image.
 * A step's ⋯ menu sets it as the guide's cover.
 */
function ItemActions({ item, onSplit }: { item: Item; onSplit(): void }) {
  // Re-render when the guide changes, so merge availability stays current.
  const canMerge = useEditor(() => canMergeSelected(editorStore));
  const annotated = useAnnotated();
  const name = item.type === "note" ? item.kind : item.type === "step" ? stepNoun(annotated) : item.type;
  return (
    <div className="flex items-center gap-0.5" role="toolbar" aria-label={`${name} actions`}>
      {item.type === "step" && !annotated && (
        <>
          <Button
            variant="ghost"
            size="sm"
            title="Split this step at the cursor in its description"
            // Keep the description's cursor.
            onMouseDown={(e) => e.preventDefault()}
            onClick={onSplit}
          >
            <Scissors /> Split
          </Button>
          <Button
            variant="ghost"
            size="sm"
            title="Merge the next step into this one: its title, description and keystrokes are added here; its screenshot and annotations are removed"
            disabled={!canMerge}
            onClick={() => mergeSelected(editorStore)}
          >
            <Merge /> Merge with next
          </Button>
        </>
      )}
      <Button variant="ghost" size="sm" title={`Delete this ${name}`} onClick={() => deleteSelected(editorStore)}>
        <Trash2 /> Delete
      </Button>
      {item.type === "step" && (
        <Suspense fallback={null}>
          <StepMenu step={item} />
        </Suspense>
      )}
    </div>
  );
}

/** The rewrite styles under a step, also in the command palette. */
export const REWRITES: { style: RewriteStyleArg; label: string; icon: typeof Minimize2 }[] = [
  { style: { kind: "shorter" }, label: "Shorter", icon: Minimize2 },
  { style: { kind: "beginner" }, label: "For beginners", icon: GraduationCap },
  { style: { kind: "expert" }, label: "For experts", icon: Rocket },
];

const CHIP =
  "inline-flex h-7 items-center gap-1.5 rounded-full border border-border bg-card px-3 text-xs text-foreground/80 transition-colors outline-none hover:border-ai hover:text-ai focus-visible:ring-2 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50 [&_svg]:size-3.5";

/**
 * The step's AI actions under its text: Regenerate, and Rewrite in a style.
 * Each replaces the step's text (edited or not) as one undoable change.
 */
function AiActions({ stepId, onCustom }: { stepId: string; onCustom(): void }) {
  const { busy, background } = useAiBlocked();
  const writing = useAi((s) => s.generating.has(stepId));
  return (
    <div role="toolbar" aria-label="AI actions" className="flex flex-wrap items-center gap-1.5">
      <button
        type="button"
        disabled={busy}
        title={writing ? "AI is writing this step" : "Write this step again with AI"}
        onClick={() => void aiStore.getState().regenerateStep(stepId)}
        className={cn(CHIP, "border-transparent bg-ai/12 font-medium text-ai hover:bg-ai/18")}
      >
        <RefreshCw className={cn(writing && "animate-spin")} /> Regenerate
      </button>
      {REWRITES.map(({ style, label, icon: Icon }) => (
        <button
          key={style.kind}
          type="button"
          disabled={busy}
          title={`Rewrite this step ${label.toLowerCase()}`}
          onClick={() => void aiStore.getState().rewriteStep(stepId, style)}
          className={CHIP}
        >
          <Icon /> {label}
        </button>
      ))}
      <button type="button" disabled={busy} title="Rewrite this step your way" onClick={onCustom} className={CHIP}>
        <PenLine /> Custom…
      </button>
      {background && <span className="text-xs text-muted-foreground">{BUSY_HINT}</span>}
    </div>
  );
}

/** "Step 3 of 7" over the item, with what kind of item it is, and the previous and next items. */
function ItemHeader({ item }: { item: Item }) {
  const items = useEditor((s) => s.doc!.guide.items);
  const annotated = useAnnotated();
  const numbers = stepNumbers(items);
  const index = items.findIndex((i) => i.id === item.id);
  const select = (i: number) => editorStore.getState().select(items[i].id);
  return (
    <div className="flex min-h-8 items-center gap-2">
      <span className="text-[11px] font-semibold tracking-[0.08em] text-brand uppercase">
        {item.type === "step" ? (
          <>
            {stepNoun(annotated)} {numbers.get(item.id)} <span className="text-muted-foreground">of {numbers.size}</span>
          </>
        ) : item.type === "section" ? (
          "Section"
        ) : (
          item.kind
        )}
      </span>
      {item.type === "step" && !annotated && (
        <span className="rounded-full bg-foreground/[0.06] px-2 py-0.5 text-[11px] text-foreground/75">
          {KIND_LABELS[item.kind]}
        </span>
      )}
      {item.type === "step" && item.element?.app && (
        <span className="max-w-48 truncate rounded-full bg-foreground/[0.06] px-2 py-0.5 text-[11px] text-foreground/75">
          {item.element.app}
        </span>
      )}
      <span className="ml-auto flex items-center">
        <Button variant="ghost" size="icon" aria-label="Previous item" title="Previous (↑ in the outline)" disabled={index <= 0} onClick={() => select(index - 1)}>
          <ChevronLeft />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          aria-label="Next item"
          title="Next (↓ in the outline)"
          disabled={index < 0 || index >= items.length - 1}
          onClick={() => select(index + 1)}
        >
          <ChevronRight />
        </Button>
      </span>
    </div>
  );
}

/** The centre pane: the selected step, section or note, or else the guide's details. */
export function StepView() {
  const doc = useEditor((s) => s.doc);
  const item = useSelectedItem();
  const annotated = useAnnotated();
  const Noun = annotated ? "Page" : "Step";
  const description = useRef<MarkdownEditorHandle>(null);
  // The step whose custom rewrite form is open.
  const [customFor, setCustomFor] = useState<string | null>(null);
  if (!doc) {
    return (
      <div className="flex flex-1 items-center justify-center text-sm text-muted-foreground">
        Open a guide from the library
      </div>
    );
  }
  if (!item) return <GuideDetails />;
  const split = () => {
    if (item.type !== "step") return;
    // Until the editor has loaded, the whole description stays.
    void splitSelected(editorStore, description.current?.splitAtCursor() ?? [item.description, ""]);
  };
  return (
    <div className="mx-auto flex w-full max-w-[52rem] flex-col gap-3 px-10 pt-6 pb-10">
      <ItemHeader item={item} />
      {!doc.readOnly && customFor === item.id && <CustomRewrite stepId={item.id} onClose={() => setCustomFor(null)} />}
      <ItemView
        item={item}
        noun={Noun}
        image={
          item.type === "step" && (
            <Suspense fallback={<StepImage guideId={doc.guide.id} step={item} />}>
              {/* Fresh tools and selection per step. */}
              <AnnotationEditor key={item.id} guideId={doc.guide.id} step={item} readOnly={doc.readOnly} />
            </Suspense>
          )
        }
        text={(props) => (
          <TextEditor {...props} handle={props.field === "description" ? description : undefined} id={item.id} readOnly={doc.readOnly} />
        )}
      >
        {item.type === "step" && item.aiState.error && (
          <p className="text-xs text-muted-foreground" data-testid="ai-error">
            AI could not write this step: {item.aiState.error}
          </p>
        )}
        {item.type === "step" && !doc.readOnly && !annotated && <AiActions stepId={item.id} onCustom={() => setCustomFor(item.id)} />}
      </ItemView>
      {!doc.readOnly && (
        <div className="mt-2 flex justify-end border-t border-border pt-2">
          <ItemActions item={item} onSplit={split} />
        </div>
      )}
    </div>
  );
}
