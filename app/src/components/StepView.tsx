import { ImageOff } from "lucide-react";
import { Input } from "@/components/ui/input";
import { useStepImage } from "@/components/useStepImage";
import { setItemTitle } from "@/editor/commands";
import { editorStore, useEditor, useSelectedItem } from "@/editor/editor";
import type { Step } from "@/lib/types";
import { cn } from "@/lib/utils";

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

function TitleInput({ id, value, readOnly, large }: { id: string; value: string; readOnly: boolean; large?: boolean }) {
  return (
    <Input
      aria-label="Title"
      value={value}
      disabled={readOnly}
      className={cn(large ? "h-10 text-lg font-semibold" : "font-medium")}
      onChange={(e) =>
        editorStore.getState().dispatch(setItemTitle(editorStore.getState().doc!.guide, id, e.target.value))
      }
    />
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
          <TitleInput id={item.id} value={item.title} readOnly={doc.readOnly} />
          {/* Rich text editing comes with TipTap (#362). */}
          <p className="text-sm whitespace-pre-wrap text-muted-foreground">
            {item.description || "No description"}
          </p>
        </>
      )}
      {item.type === "section" && <TitleInput id={item.id} value={item.title} readOnly={doc.readOnly} large />}
      {item.type === "note" && (
        <div className="rounded-lg border border-border bg-card p-4">
          <p className="mb-1 text-xs font-semibold tracking-wide text-muted-foreground uppercase">{item.kind}</p>
          <p className="text-sm whitespace-pre-wrap">{item.body}</p>
        </div>
      )}
    </div>
  );
}
