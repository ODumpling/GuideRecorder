import { Dialog } from "radix-ui";
import { LoaderCircle, Monitor, TriangleAlert, X } from "lucide-react";
import { useStore } from "zustand";
import { Button } from "@/components/ui/button";
import type { CaptureDisplay } from "@/lib/types";
import type { StartFlowStore } from "@/recording/startFlow";
import type { RecordingStore } from "@/recording/store";

/** "Main display", else "Display n" by its place in the list. */
export function displayName(d: CaptureDisplay, index: number): string {
  return d.main ? "Main display" : `Display ${index + 1}`;
}

/** The displays' arrangement, `active` filled in. */
function Layout({ displays, active }: { displays: CaptureDisplay[]; active: number }) {
  const left = Math.min(...displays.map((d) => d.bounds.x));
  const top = Math.min(...displays.map((d) => d.bounds.y));
  const right = Math.max(...displays.map((d) => d.bounds.x + d.bounds.width));
  const bottom = Math.max(...displays.map((d) => d.bounds.y + d.bounds.height));
  const gap = (right - left) * 0.02;
  return (
    <svg
      aria-hidden
      viewBox={`${left - gap} ${top - gap} ${right - left + 2 * gap} ${bottom - top + 2 * gap}`}
      className="h-8 w-14 shrink-0"
    >
      {displays.map((d) => (
        <rect
          key={d.displayId}
          x={d.bounds.x + gap / 2}
          y={d.bounds.y + gap / 2}
          width={Math.max(d.bounds.width - gap, 1)}
          height={Math.max(d.bounds.height - gap, 1)}
          rx={gap}
          className={d.displayId === active ? "fill-primary" : "fill-muted-foreground/30"}
        />
      ))}
    </svg>
  );
}

/** Asks which display to record, when the capture area is One display. */
export function DisplayPicker({ flow, recording }: { flow: StartFlowStore; recording: RecordingStore }) {
  const picker = useStore(flow, (s) => s.displayPicker);
  const starting = useStore(recording, (s) => s.pending === "start");
  const { chooseDisplay, closeDisplayPicker } = flow.getState();
  const choose = (displayId: number | null) => {
    if (!starting) void chooseDisplay(displayId);
  };

  return (
    <Dialog.Root open={picker !== null} onOpenChange={(next) => !next && !starting && closeDisplayPicker()}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-black/40" />
        <Dialog.Content className="fixed top-1/2 left-1/2 z-50 flex max-h-[80vh] w-[28rem] -translate-x-1/2 -translate-y-1/2 flex-col gap-3 rounded-lg border border-border bg-background p-5 text-foreground shadow-lg">
          <div className="flex items-center justify-between">
            <Dialog.Title className="text-base font-semibold">Choose a display to record</Dialog.Title>
            <Dialog.Close asChild>
              <Button variant="ghost" size="icon" aria-label="Close" disabled={starting}>
                <X />
              </Button>
            </Dialog.Close>
          </div>
          <Dialog.Description className="sr-only">Pick the display whose screen each step captures.</Dialog.Description>

          {picker?.startError && (
            <p role="alert" className="flex items-center gap-2 text-sm">
              <TriangleAlert className="size-4 shrink-0 text-[var(--status-danger)]" />
              {picker.startError} Choose another display.
            </p>
          )}

          <ul aria-label="Displays" className="-mx-1 flex min-h-0 flex-col gap-0.5 overflow-y-auto">
            {picker?.displays.map((d, i) => (
              <li key={d.displayId}>
                <button
                  type="button"
                  disabled={starting}
                  className="flex w-full cursor-default items-center gap-3 rounded-md px-2 py-1.5 text-left text-sm hover:bg-muted focus-visible:bg-accent focus-visible:outline-none disabled:opacity-60"
                  onClick={() => choose(d.displayId)}
                >
                  <Layout displays={picker.displays} active={d.displayId} />
                  <span className="flex-1 font-medium">{displayName(d, i)}</span>
                  <span className="shrink-0 font-mono text-xs text-muted-foreground tabular-nums">
                    {Math.round(d.bounds.width)}×{Math.round(d.bounds.height)}
                  </span>
                </button>
              </li>
            ))}
          </ul>

          <div className="mt-1 flex items-center justify-between gap-2">
            {starting ? (
              <span role="status" className="flex items-center gap-1.5 text-xs text-muted-foreground">
                <LoaderCircle className="size-3.5 animate-spin" /> Starting…
              </span>
            ) : (
              <span />
            )}
            <div className="flex gap-2">
              <Button variant="outline" disabled={starting} onClick={() => choose(null)}>
                <Monitor /> All displays
              </Button>
              <Dialog.Close asChild>
                <Button variant="outline" disabled={starting}>
                  Cancel
                </Button>
              </Dialog.Close>
            </div>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
