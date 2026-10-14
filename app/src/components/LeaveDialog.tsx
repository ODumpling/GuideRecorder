import { AlertDialog } from "radix-ui";
import { Button } from "@/components/ui/button";
import { editorStore, useEditor } from "@/editor/editor";
import type { LeaveChoice, LeaveIntent } from "@/editor/store";

const ACTIONS: Record<LeaveIntent["kind"], string> = {
  close: "closing the guide",
  open: "opening another guide",
  quit: "quitting",
};

/** Asks what to do when changes could not be saved before leaving a guide. */
export function LeaveDialog() {
  const leave = useEditor((s) => s.leave);
  const error = useEditor((s) => s.saveError);
  const busy = useEditor((s) => s.busy);
  const choose = (choice: LeaveChoice) => void editorStore.getState().resolveLeave(choice);

  return (
    <AlertDialog.Root open={leave !== null} onOpenChange={(open) => !open && choose("cancel")}>
      <AlertDialog.Portal>
        <AlertDialog.Overlay className="fixed inset-0 z-50 bg-black/40" />
        <AlertDialog.Content className="fixed top-1/2 left-1/2 z-50 flex w-[26rem] -translate-x-1/2 -translate-y-1/2 flex-col gap-3 rounded-lg border border-border bg-background p-5 text-foreground shadow-lg">
          <AlertDialog.Title className="text-base font-semibold">Changes not saved</AlertDialog.Title>
          <AlertDialog.Description className="text-sm text-muted-foreground">
            Your latest changes could not be saved before {leave ? ACTIONS[leave.kind] : "leaving"}
            {error ? `: ${error}` : "."}
          </AlertDialog.Description>
          <div className="mt-2 flex justify-end gap-2">
            <AlertDialog.Cancel asChild>
              <Button variant="outline" disabled={busy}>
                Keep editing
              </Button>
            </AlertDialog.Cancel>
            <Button variant="outline" disabled={busy} onClick={() => choose("discard")}>
              Discard changes
            </Button>
            <Button disabled={busy} onClick={() => choose("retry")}>
              Try again
            </Button>
          </div>
        </AlertDialog.Content>
      </AlertDialog.Portal>
    </AlertDialog.Root>
  );
}
