import { lazy, Suspense } from "react";
import { ImageOff } from "lucide-react";
import { useStepImage } from "@/components/useStepImage";
import { setItemText, type TextField } from "@/editor/commands";
import { editorStore, useEditor, useSelectedItem } from "@/editor/editor";
import type { Step } from "@/lib/types";
import { cn } from "@/lib/utils";
import { markdownToPlainText } from "@/text/markdown";

// TipTap is most of the bundle; load it with the first opened guide.
const MarkdownEditor = lazy(() => import("@/components/MarkdownEditor"));

function StepImage({ guideId, step }: { guideId: string; step: Step }) {
  const variant = step.image.show === "crop" && step.image.crop ? "crop" : "full";
  const { url, error } = useStepImage(guideId, step.id, variant);
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
  id,
  field,
  value,
  readOnly,
  label,
  placeholder,
  singleLine,
  className,
}: {
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

/** The centre pane: the selected step, section or note. */
export function StepView() {
  const doc = useEditor((s) => s.doc);
  const item = useSelectedItem();
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
  return (
    <div className="mx-auto flex w-full max-w-4xl flex-col gap-4 p-6">
      {item.type === "step" && (
        <>
          <StepImage guideId={doc.guide.id} step={item} />
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
