import { useId, useState, type ReactNode } from "react";
import { Dialog } from "radix-ui";
import { Circle, ImagePlus, LoaderCircle, Plus, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { editorStore, useEditor } from "@/editor/editor";
import { startFlow, useRecording } from "@/recording/recording";

/** One kind of guide to make: a large button with what it does underneath. */
function Choice({
  icon,
  title,
  description,
  disabled,
  onChoose,
}: {
  icon: ReactNode;
  title: string;
  description: string;
  disabled: boolean;
  onChoose(): void;
}) {
  const id = useId();
  return (
    <button
      type="button"
      aria-labelledby={`${id}-title`}
      aria-describedby={`${id}-description`}
      disabled={disabled}
      onClick={onChoose}
      className="flex items-start gap-3 rounded-md border border-border p-3 text-left transition-colors outline-none hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50"
    >
      <span className="mt-0.5 flex size-8 shrink-0 items-center justify-center rounded-md bg-muted [&_svg]:size-4">
        {icon}
      </span>
      <span className="flex flex-col gap-0.5">
        <span id={`${id}-title`} className="text-sm font-medium">
          {title}
        </span>
        <span id={`${id}-description`} className="text-xs text-muted-foreground">
          {description}
        </span>
      </span>
    </button>
  );
}

/**
 * The toolbar's New Guide button: asks which kind of guide to make, then
 * starts a recording or makes an annotation guide from images or a PDF.
 */
export function NewGuideButton() {
  const pending = useRecording((s) => s.pending);
  const creating = useEditor((s) => s.creating);
  const opening = useEditor((s) => s.opening);
  const busy = useEditor((s) => s.busy);
  const [open, setOpen] = useState(false);
  const starting = pending === "start";

  return (
    <Dialog.Root open={open} onOpenChange={setOpen}>
      <Dialog.Trigger asChild>
        <Button variant="outline" size="sm" className="ml-1" disabled={pending !== null || creating}>
          {starting || creating ? <LoaderCircle className="animate-spin" /> : <Plus />}
          {starting ? "Starting…" : creating ? "Creating…" : "New Guide"}
        </Button>
      </Dialog.Trigger>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-black/40" />
        <Dialog.Content className="fixed top-1/2 left-1/2 z-50 flex w-[28rem] -translate-x-1/2 -translate-y-1/2 flex-col gap-4 rounded-lg border border-border bg-background p-5 text-foreground shadow-lg">
          <div className="flex items-center justify-between">
            <Dialog.Title className="text-base font-semibold">New guide</Dialog.Title>
            <Dialog.Close asChild>
              <Button variant="ghost" size="icon" aria-label="Close">
                <X />
              </Button>
            </Dialog.Close>
          </div>
          <Dialog.Description className="text-sm text-muted-foreground">
            Choose the kind of guide to make.
          </Dialog.Description>
          <div className="flex flex-col gap-2">
            <Choice
              icon={<Circle className="text-[var(--status-danger)]" />}
              title="Step-by-step guide"
              description="Records your screen as you work. Each click becomes a step with a screenshot and a description you can edit afterwards."
              disabled={pending !== null}
              onChoose={() => {
                setOpen(false);
                void startFlow.getState().begin();
              }}
            />
            <Choice
              icon={<ImagePlus />}
              title="Annotation guide"
              description="Import images or a PDF, then add hotspots that explain each part of the picture."
              disabled={creating || opening || busy}
              onChoose={() => {
                setOpen(false);
                void editorStore.getState().createAnnotatedGuide();
              }}
            />
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
