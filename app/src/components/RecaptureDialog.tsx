import { AlertDialog } from "radix-ui";
import { useStore } from "zustand";
import { Button } from "@guiderecorder/ui/button";
import { editorStore } from "@/editor/editor";
import { resolveRecapture } from "@/editor/recapture";
import type { AnnotationChoice } from "@/editor/commands";
import { recaptureStore } from "@/recording/recording";

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/**
 * After a re-capture of a step with annotations: move them by how far the
 * click moved, clear them, or keep them where they are. Closing the dialog
 * keeps them; either way the re-capture is one undoable change.
 */
export function RecaptureDialog() {
  const pending = useStore(recaptureStore, (s) => s.pending);
  const choose = (choice: AnnotationChoice) => resolveRecapture(editorStore, recaptureStore, choice);
  const count = pending ? plural(pending.annotations, "annotation", "annotations") : "";
  const offset = pending?.offset;

  return (
    <AlertDialog.Root open={pending !== null} onOpenChange={(open) => !open && choose("keep")}>
      <AlertDialog.Portal>
        <AlertDialog.Overlay className="fixed inset-0 z-50 bg-black/40 backdrop-blur-[2px]" />
        <AlertDialog.Content className="fixed top-1/2 left-1/2 z-50 flex w-[28rem] -translate-x-1/2 -translate-y-1/2 flex-col gap-3 rounded-2xl border border-border bg-card p-5 text-card-foreground shadow-2xl">
          <AlertDialog.Title className="text-base font-semibold">Step re-captured</AlertDialog.Title>
          <AlertDialog.Description className="text-sm text-muted-foreground">
            {offset
              ? `The click moved ${Math.round(offset.dx)}, ${Math.round(offset.dy)} pixels. Move the step's ${count} with it, or clear them?`
              : `The step has ${count}. Keep them on the new screenshot, or clear them?`}
          </AlertDialog.Description>
          <div className="mt-2 flex justify-end gap-2">
            <AlertDialog.Cancel asChild>
              <Button variant="outline">Keep as they are</Button>
            </AlertDialog.Cancel>
            <Button variant="outline" onClick={() => choose("clear")}>
              Clear annotations
            </Button>
            {offset && <Button onClick={() => choose("move")}>Move annotations</Button>}
          </div>
        </AlertDialog.Content>
      </AlertDialog.Portal>
    </AlertDialog.Root>
  );
}
