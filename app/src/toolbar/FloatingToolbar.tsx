import { useEffect, useState } from "react";
import { useStore } from "zustand";
import { Check, LoaderCircle, Pause, Play, Plus, Square, TriangleAlert } from "lucide-react";
import { Button } from "@/components/ui/button";
import { elapsedMs, formatElapsed, type RecordingStore } from "@/recording/store";
import { toolbarView, type ManualStepStore } from "@/toolbar/toolbar";

/** Recorded time, ticking while recording and frozen while paused. */
function Elapsed({ recording }: { recording: RecordingStore }) {
  const state = useStore(recording, (s) => s.status.state);
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    if (state !== "recording") return;
    setNow(Date.now());
    const timer = setInterval(() => setNow(Date.now()), 500);
    return () => clearInterval(timer);
  }, [state]);
  const ms = useStore(recording, (s) => elapsedMs(s.status, now));
  return (
    <span aria-label="Recording time" className="font-mono text-xs tabular-nums">
      {formatElapsed(ms)}
    </span>
  );
}

/** The floating toolbar: state, clock, step count, and Pause/Resume, Manual step, Stop. */
export function FloatingToolbar({ recording, manual }: { recording: RecordingStore; manual: ManualStepStore }) {
  const status = useStore(recording, (s) => s.status);
  const pending = useStore(recording, (s) => s.pending);
  const snapshots = useStore(recording, (s) => s.snapshots);
  const phase = useStore(manual, (s) => s.phase);
  const manualError = useStore(manual, (s) => s.error);
  const view = toolbarView({ status, pending, snapshots }, phase === "adding");
  const { pause, resume, stop } = recording.getState();

  return (
    <div
      data-tauri-drag-region
      role="toolbar"
      aria-label="Recording toolbar"
      className="flex h-full w-full items-center gap-1.5 rounded-xl border border-border bg-background/95 px-2 text-foreground shadow-lg select-none"
    >
      <span
        data-tauri-drag-region
        aria-hidden
        className={`size-2 shrink-0 rounded-full ${view.excluded || view.lost ? "bg-muted-foreground" : "bg-[var(--status-danger)]"} ${
          view.paused || view.excluded || view.lost ? "opacity-40" : "animate-pulse"
        }`}
      />
      <span
        data-tauri-drag-region
        role="status"
        className="min-w-0 flex-1 truncate text-xs font-medium"
        title={
          view.lost ??
          (view.excluded ? "Nothing is captured while an excluded app is in front (Settings → Recording)." : view.label)
        }
      >
        {view.label}
      </span>
      <Elapsed recording={recording} />
      <span
        data-tauri-drag-region
        aria-label="Screenshots"
        title="Screenshots taken so far"
        className={`rounded px-1 font-mono text-xs tabular-nums transition-colors ${
          phase === "added" ? "bg-accent text-accent-foreground" : "text-muted-foreground"
        }`}
      >
        {view.steps}
      </span>
      {view.paused ? (
        <Button variant="ghost" size="icon" aria-label="Resume recording" title="Resume" disabled={!view.canResume} onClick={() => void resume()}>
          <Play />
        </Button>
      ) : (
        <Button variant="ghost" size="icon" aria-label="Pause recording" title="Pause" disabled={!view.canPause} onClick={() => void pause()}>
          <Pause />
        </Button>
      )}
      <Button
        variant="ghost"
        size="icon"
        aria-label="Add manual step"
        title={
          manualError
            ? `Couldn't add a step: ${manualError}`
            : phase === "added"
              ? "Step added"
              : view.lost
                ? "Capture stopped: no step can be added"
                : "Add a step from the screen now"
        }
        disabled={!view.canAddStep}
        onClick={() => void manual.getState().add()}
      >
        {phase === "adding" ? (
          <LoaderCircle className="animate-spin" />
        ) : phase === "added" ? (
          <Check className="text-[var(--status-info)]" />
        ) : phase === "failed" ? (
          <TriangleAlert className="text-[var(--status-danger)]" />
        ) : (
          <Plus />
        )}
      </Button>
      <span className="sr-only" role="alert">
        {phase === "added" ? "Step added" : phase === "failed" ? `Couldn't add a step: ${manualError}` : ""}
      </span>
      <Button variant="ghost" size="icon" aria-label="Stop recording" title="Stop and save" disabled={!view.canStop} onClick={() => void stop()}>
        <Square className="fill-current" />
      </Button>
    </div>
  );
}
