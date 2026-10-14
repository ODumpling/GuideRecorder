import { lazy, Suspense, useRef, type Ref } from "react";
import { ImageOff, Merge, Scissors, Trash2 } from "lucide-react";
import type { MarkdownEditorHandle } from "@/components/MarkdownEditor";
import { Button } from "@/components/ui/button";
import { useStepImage } from "@/components/useStepImage";
import { canMergeSelected, deleteSelected, mergeSelected, splitSelected } from "@/editor/actions";
import { setItemText, type TextField } from "@/editor/commands";
import { editorStore, useEditor, useSelectedItem } from "@/editor/editor";
import type { Item, Step } from "@/lib/types";
import { cn } from "@/lib/utils";
import { markdownToPlainText } from "@/text/markdown";

// TipTap is most of the bundle; load it with the first opened guide.
const MarkdownEditor = lazy(() => import("@/components/MarkdownEditor"));
// The annotation layer, likewise; the plain image shows until it loads.
const AnnotationEditor = lazy(() => import("@/components/annotations/AnnotationEditor"));

function StepImage({ guideId, step }: { guideId: string; step: Step }) {
  const path = step.image.show === "crop" && step.image.crop ? step.image.crop : step.image.full;
  const { url, error } = useStepImage(guideId, path);
  return (
    <div className="flex min-h-48 items-center justify-center overflow-hidden rounded-lg border border-border bg-muted">
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
        "rounded-md border border-input px-2.5 py-1.5 whitespace-pre-wrap",
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

const ITEM_NAMES: Record<Item["type"], string> = { step: "step", section: "section", note: "note" };

/** Delete, merge and split for the selected item. */
function ItemActions({ item, onSplit }: { item: Item; onSplit(): void }) {
  // Re-render when the guide changes, so merge availability stays current.
  const canMerge = useEditor(() => canMergeSelected(editorStore));
  const name = item.type === "note" ? item.kind : ITEM_NAMES[item.type];
  return (
    <div className="flex items-center gap-1" role="toolbar" aria-label={`${name} actions`}>
      {item.type === "step" && (
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
    </div>
  );
}

/** The centre pane: the selected step, section or note. */
export function StepView() {
  const doc = useEditor((s) => s.doc);
  const item = useSelectedItem();
  const description = useRef<MarkdownEditorHandle>(null);
  if (!doc) {
    return (
      <div className="flex flex-1 items-center justify-center text-sm text-muted-foreground">
        Open a guide from the library
      </div>
    );
  }
  if (!item) {
    return (
      <div className="flex flex-1 items-center justify-center text-sm text-muted-foreground">
        {doc.guide.items.length ? "Select a step" : "This guide has no steps"}
      </div>
    );
  }
  const split = () => {
    if (item.type !== "step") return;
    // Until the editor has loaded, the whole description stays.
    void splitSelected(editorStore, description.current?.splitAtCursor() ?? [item.description, ""]);
  };
  return (
    <div className="mx-auto flex w-full max-w-4xl flex-col gap-4 p-6">
      {!doc.readOnly && (
        <div className="-my-2 flex justify-end">
          <ItemActions item={item} onSplit={split} />
        </div>
      )}
      {item.type === "step" && (
        <>
          <Suspense fallback={<StepImage guideId={doc.guide.id} step={item} />}>
            {/* Fresh tools and selection per step. */}
            <AnnotationEditor key={item.id} guideId={doc.guide.id} step={item} readOnly={doc.readOnly} />
          </Suspense>
          <TextEditor
            id={item.id}
            field="title"
            value={item.title}
            readOnly={doc.readOnly}
            label="Step title"
            placeholder="Step title"
            singleLine
            className="font-medium"
          />
          <TextEditor
            handle={description}
            id={item.id}
            field="description"
            value={item.description}
            readOnly={doc.readOnly}
            label="Step description"
            placeholder="Add a description"
            className="text-sm"
          />
        </>
      )}
      {item.type === "section" && (
        <TextEditor
          id={item.id}
          field="title"
          value={item.title}
          readOnly={doc.readOnly}
          label="Section title"
          placeholder="Section title"
          singleLine
          className="text-lg font-semibold"
        />
      )}
      {item.type === "note" && (
        <div className="rounded-lg border border-border bg-card p-4">
          <p className="mb-1 text-xs font-semibold tracking-wide text-muted-foreground uppercase">{item.kind}</p>
          <TextEditor
            id={item.id}
            field="body"
            value={item.body}
            readOnly={doc.readOnly}
            label={`${item.kind} text`}
            placeholder="Write the note"
            className="text-sm"
          />
        </div>
      )}
    </div>
  );
}
