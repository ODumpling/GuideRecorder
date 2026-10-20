import { Dialog } from "radix-ui";
import { X } from "lucide-react";
import type { Conflict } from "@guiderecorder/sync";
import { Button } from "@guiderecorder/ui/button";
import type { Resolution } from "@guiderecorder/sync/engine";
import { syncStore, useSync } from "@/sync/sync";

const FIELD_NAMES: Record<string, string> = {
  title: "title",
  description: "description",
  body: "text",
  intro: "introduction",
  prerequisites: "prerequisites",
  summary: "summary",
  tags: "tags",
  annotations: "annotations",
  image: "screenshot",
};

/** A short, readable form of a conflicting value. */
function preview(value: unknown): string {
  if (value === undefined || value === null || value === "") return "(empty)";
  const text = typeof value === "string" ? value : Array.isArray(value) && value.every((v) => typeof v === "string") ? value.join(", ") : "(changed)";
  return text.length > 140 ? `${text.slice(0, 140)}…` : text;
}

function describe(conflict: Conflict, stepNumbers: Map<string, number>): { what: string; mine: string; theirs: string } {
  if (conflict.kind === "order") return { what: "Order of the steps", mine: "Reordered here", theirs: "Reordered on the web" };
  const item = conflict.itemId ? (stepNumbers.has(conflict.itemId) ? `Step ${stepNumbers.get(conflict.itemId)}` : "An item") : "The guide";
  if (conflict.kind === "deleted") {
    return conflict.deletedBy === "ours"
      ? { what: item, mine: "Deleted here", theirs: "Changed on the web" }
      : { what: item, mine: "Changed here", theirs: "Deleted on the web" };
  }
  return {
    what: `${item}: ${FIELD_NAMES[conflict.field] ?? conflict.field}`,
    mine: preview(conflict.ours),
    theirs: preview(conflict.theirs),
  };
}

/** Asks which version to keep when a guide changed here and on the web in the same place. */
export function ConflictDialog() {
  const id = useSync((s) => s.resolving);
  const entry = useSync((s) => (s.resolving ? s.state.guides[s.resolving] : undefined));
  const running = useSync((s) => s.running);
  const open = Boolean(id && entry?.conflicts?.length && entry.head);
  const store = syncStore.getState();

  const items = (entry?.head?.guide.items ?? []) as { id: string; type: string }[];
  const stepNumbers = new Map(items.filter((i) => i.type === "step").map((i, n) => [i.id, n + 1]));
  const resolve = (resolution: Resolution) => void store.resolve(id!, resolution);

  return (
    <Dialog.Root open={open} onOpenChange={(o) => !o && store.openConflict(null)}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-black/30" />
        <Dialog.Content className="fixed top-1/2 left-1/2 z-50 flex max-h-[80vh] w-[560px] -translate-x-1/2 -translate-y-1/2 flex-col gap-4 rounded-xl border border-border bg-background p-5 shadow-xl">
          <div className="flex items-start justify-between gap-4">
            <div>
              <Dialog.Title className="text-base font-semibold">“{entry?.head?.guide.title}” changed in two places</Dialog.Title>
              <Dialog.Description className="mt-1 text-xs text-muted-foreground">
                These were changed both here and on the web. Every other change is already combined.
              </Dialog.Description>
            </div>
            <Dialog.Close asChild>
              <Button variant="ghost" size="icon" aria-label="Close">
                <X />
              </Button>
            </Dialog.Close>
          </div>
          <ul className="flex flex-col gap-3 overflow-y-auto text-sm">
            {entry?.conflicts?.map((conflict, n) => {
              const { what, mine, theirs } = describe(conflict, stepNumbers);
              return (
                <li key={n} className="rounded-lg border border-border p-3">
                  <p className="font-medium">{what}</p>
                  <dl className="mt-2 grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-xs">
                    <dt className="text-muted-foreground">This Mac</dt>
                    <dd className="break-words whitespace-pre-wrap">{mine}</dd>
                    <dt className="text-muted-foreground">Web</dt>
                    <dd className="break-words whitespace-pre-wrap">{theirs}</dd>
                  </dl>
                </li>
              );
            })}
          </ul>
          <div className="flex justify-end gap-2">
            <Button variant="outline" disabled={running} onClick={() => resolve("both")} title="Keep the web's version here, and yours as a separate guide">
              Keep both
            </Button>
            <Button variant="outline" disabled={running} onClick={() => resolve("theirs")}>
              Keep theirs
            </Button>
            <Button disabled={running} onClick={() => resolve("mine")}>
              Keep mine
            </Button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
