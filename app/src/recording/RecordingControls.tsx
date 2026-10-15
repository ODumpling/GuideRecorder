import { useEffect, useState } from "react";
import { AlertDialog } from "radix-ui";
import { Circle, Info, LoaderCircle, Pause, Play, ShieldCheck, Square, TriangleAlert, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { recordingStore, useRecording } from "@/recording/recording";
import { elapsedMs, formatElapsed } from "@/recording/store";

/** Recorded time, ticking while recording and frozen while paused. */
function Elapsed() {
  const state = useRecording((s) => s.status.state);
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    if (state !== "recording") return;
    // Not the tick from before a pause, which would undercount after a resume.
    setNow(Date.now());
    const timer = setInterval(() => setNow(Date.now()), 500);
    return () => clearInterval(timer);
  }, [state]);
  const ms = useRecording((s) => elapsedMs(s, now));
  return (
    <span aria-label="Recording time" className="font-mono text-xs tabular-nums">
      {formatElapsed(ms)}
    </span>
  );
}

function CancelButton({ disabled }: { disabled: boolean }) {
  const [confirming, setConfirming] = useState(false);
  return (
    <AlertDialog.Root open={confirming} onOpenChange={setConfirming}>
      <AlertDialog.Trigger asChild>
        <Button variant="ghost" size="icon" aria-label="Cancel recording" title="Cancel recording" disabled={disabled}>
          <X />
        </Button>
      </AlertDialog.Trigger>
      <AlertDialog.Portal>
        <AlertDialog.Overlay className="fixed inset-0 z-50 bg-black/40" />
        <AlertDialog.Content className="fixed top-1/2 left-1/2 z-50 flex w-[26rem] -translate-x-1/2 -translate-y-1/2 flex-col gap-3 rounded-lg border border-border bg-background p-5 text-foreground shadow-lg">
          <AlertDialog.Title className="text-base font-semibold">Discard this recording?</AlertDialog.Title>
          <AlertDialog.Description className="text-sm text-muted-foreground">
            The steps recorded so far are deleted and no guide is made.
          </AlertDialog.Description>
          <div className="mt-2 flex justify-end gap-2">
            <AlertDialog.Cancel asChild>
              <Button variant="outline">Keep recording</Button>
            </AlertDialog.Cancel>
            <AlertDialog.Action asChild>
              <Button onClick={() => void recordingStore.getState().cancel()}>Discard recording</Button>
            </AlertDialog.Action>
          </div>
        </AlertDialog.Content>
      </AlertDialog.Portal>
    </AlertDialog.Root>
  );
}

/** The toolbar's Record button, or the live recording's controls. */
export function RecordingControls() {
  const status = useRecording((s) => s.status);
  const pending = useRecording((s) => s.pending);
  const { start, pause, resume, stop } = recordingStore.getState();

  if (status.state === "idle") {
    return (
      <Button variant="outline" size="sm" className="ml-1" disabled={pending !== null} onClick={() => void start()}>
        {pending === "start" ? <LoaderCircle className="animate-spin" /> : <Circle className="text-[var(--status-danger)]" />}
        {pending === "start" ? "Starting…" : "Record"}
      </Button>
    );
  }

  if (status.state === "saving") {
    return (
      <span role="status" className="ml-1 flex items-center gap-1.5 text-xs text-muted-foreground">
        <LoaderCircle className="size-4 animate-spin" /> Saving recording…
      </span>
    );
  }

  const paused = status.state === "paused";
  const busy = pending !== null;
  return (
    <div role="group" aria-label="Recording" className="ml-1 flex items-center gap-1 rounded-md border border-border px-1.5">
      <span
        className={`size-2 rounded-full bg-[var(--status-danger)] ${paused ? "opacity-40" : "animate-pulse"}`}
        aria-hidden
      />
      <span className="text-xs font-medium">{paused ? "Paused" : "Recording"}</span>
      <Elapsed />
      {paused ? (
        <Button variant="ghost" size="icon" aria-label="Resume recording" title="Resume" disabled={busy} onClick={() => void resume()}>
          <Play />
        </Button>
      ) : (
        <Button variant="ghost" size="icon" aria-label="Pause recording" title="Pause" disabled={busy} onClick={() => void pause()}>
          <Pause />
        </Button>
      )}
      <Button variant="ghost" size="icon" aria-label="Stop recording" title="Stop and save" disabled={busy} onClick={() => void stop()}>
        <Square className="fill-current" />
      </Button>
      <CancelButton disabled={busy} />
    </div>
  );
}

/** Below the toolbar: a failed recording command, and the capture area notice. */
export function RecordingBanner({ onPermissions }: { onPermissions(): void }) {
  const error = useRecording((s) => s.error);
  const fallback = useRecording(
    (s) => (s.status.state === "recording" || s.status.state === "paused") && s.status.captureAreaFallback,
  );
  return (
    <>
      {error && (
        <div role="alert" className="flex items-center gap-2 border-b border-border bg-muted px-4 py-2 text-sm">
          <TriangleAlert className="size-4 shrink-0 text-[var(--status-danger)]" />
          <span className="flex-1">Recording: {error.message}</span>
          {error.permissions && (
            <Button
              variant="outline"
              size="sm"
              onClick={() => {
                recordingStore.getState().clearError();
                onPermissions();
              }}
            >
              <ShieldCheck /> Open Permissions
            </Button>
          )}
          <Button variant="ghost" size="icon" aria-label="Dismiss" onClick={() => recordingStore.getState().clearError()}>
            <X />
          </Button>
        </div>
      )}
      {fallback && (
        <div className="flex items-center gap-2 border-b border-border bg-muted px-4 py-2 text-sm">
          <Info className="size-4 shrink-0 text-[var(--status-info)]" />
          <span>Recording the whole display; window and region capture are not available yet.</span>
        </div>
      )}
    </>
  );
}
