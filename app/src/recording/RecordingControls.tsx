import { useEffect, useRef, useState } from "react";
import { AlertDialog } from "radix-ui";
import { Info, LoaderCircle, Pause, Play, ShieldCheck, Square, TriangleAlert, X } from "lucide-react";
import { NewGuideButton } from "@/components/NewGuideDialog";
import { Button } from "@guiderecorder/ui/button";
import { useEditor } from "@/editor/editor";
import type { RecordingSnapshot } from "@/lib/types";
import { appName } from "@/recording/apps";
import { DisplayPicker } from "@/picker/DisplayPicker";
import { WindowPicker } from "@/picker/WindowPicker";
import { recordingStore, startFlow, useRecording } from "@/recording/recording";
import { captureLostLabel, captureLostMessage, elapsedMs, formatElapsed } from "@/recording/store";
import { useThumbnail } from "@/recording/useThumbnail";

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
  const ms = useRecording((s) => elapsedMs(s.status, now));
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
        <AlertDialog.Overlay className="fixed inset-0 z-50 bg-black/40 backdrop-blur-[2px]" />
        <AlertDialog.Content className="fixed top-1/2 left-1/2 z-50 flex w-[26rem] -translate-x-1/2 -translate-y-1/2 flex-col gap-3 rounded-2xl border border-border bg-card p-5 text-card-foreground shadow-2xl">
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

/**
 * The toolbar's New Guide button (only in the library), or the live
 * recording's controls; and the display and window pickers a recording asks
 * with when the capture area is One display or Application.
 */
export function RecordingControls() {
  return (
    <>
      <Controls />
      <DisplayPicker flow={startFlow} recording={recordingStore} />
      <WindowPicker flow={startFlow} recording={recordingStore} />
    </>
  );
}

function Controls() {
  const status = useRecording((s) => s.status);
  const pending = useRecording((s) => s.pending);
  const open = useEditor((s) => s.doc !== null);
  const { pause, resume, stop } = recordingStore.getState();

  if (status.state === "idle") return open ? null : <NewGuideButton />;

  if (status.state === "saving") {
    return (
      <span role="status" className="ml-1 flex items-center gap-1.5 text-xs text-muted-foreground">
        <LoaderCircle className="size-4 animate-spin" /> Saving recording…
      </span>
    );
  }

  const paused = status.state === "paused";
  // Nothing is captured while an excluded app is in front; the clock runs on.
  const excludedApp = paused ? null : status.excludedApp;
  const excluded = excludedApp !== null;
  const lost = captureLostLabel(status);
  const busy = pending !== null;
  return (
    <div role="group" aria-label="Recording" className="ml-1 flex h-8 items-center gap-1 rounded-lg border border-border bg-card px-2 shadow-sm">
      <span
        className={`size-2 rounded-full ${excluded || lost ? "bg-muted-foreground" : "bg-[var(--status-danger)]"} ${
          paused || excluded || lost ? "opacity-40" : "animate-pulse"
        }`}
        aria-hidden
      />
      <span
        role="status"
        className="max-w-48 truncate text-xs font-medium"
        title={
          lost
            ? captureLostMessage(status) ?? undefined
            : excluded
              ? "Nothing is captured while an excluded app is in front (Settings → Recording)."
              : undefined
        }
      >
        {lost ?? (paused ? "Paused" : excluded ? `Not capturing: ${appName(excludedApp)} is in front` : "Recording")}
      </span>
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

/** Below the toolbar: a failed recording command, the capture area notice, and what a recording into a guide does. */
export function RecordingBanner({ onPermissions }: { onPermissions(): void }) {
  const error = useRecording((s) => s.error);
  const fallback = useRecording(
    (s) => (s.status.state === "recording" || s.status.state === "paused") && s.status.captureAreaFallback,
  );
  const lost = useRecording((s) => captureLostMessage(s.status));
  const into = useRecording((s) => (s.status.state === "recording" || s.status.state === "paused" ? s.into : null));
  // A refused resume says the same as the "Capture stopped" notice: one banner is enough.
  const shownError = error && error.message !== lost ? error : null;
  return (
    <>
      {lost && (
        <div role="alert" aria-label="Capture stopped" className="flex items-center gap-2 border-b border-border bg-muted px-4 py-2 text-sm">
          <TriangleAlert className="size-4 shrink-0 text-[var(--status-danger)]" />
          <span className="flex-1">{lost}</span>
        </div>
      )}
      {shownError && (
        <div role="alert" className="flex items-center gap-2 border-b border-border bg-muted px-4 py-2 text-sm">
          <TriangleAlert className="size-4 shrink-0 text-[var(--status-danger)]" />
          <span className="flex-1">Recording: {shownError.message}</span>
          {shownError.permissions && (
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
          <span>Recording all displays: no window or region was chosen for this recording.</span>
        </div>
      )}
      {into && (
        <div role="status" className="flex items-center gap-2 border-b border-border bg-muted px-4 py-2 text-sm">
          <Info className="size-4 shrink-0 text-[var(--status-info)]" />
          <span>
            {into.mode === "replace"
              ? "Re-capturing a step: the next click, shortcut or manual step replaces its screenshot."
              : "Recording steps into this guide: stop to insert them."}
          </span>
        </div>
      )}
    </>
  );
}

/** One screenshot of the strip, fetched and scaled down once; a dot marks the click. */
function Thumbnail({ snapshot, index }: { snapshot: RecordingSnapshot; index: number }) {
  const { url, failed } = useThumbnail(snapshot.id);
  const label = `Screenshot ${index + 1}`;
  const { point, width, height } = snapshot;
  return (
    <figure
      className="relative h-14 shrink-0 overflow-hidden rounded border border-border bg-background"
      style={{ aspectRatio: `${width} / ${height}` }}
      title={label}
    >
      {url ? (
        <img src={url} alt={label} className="size-full object-cover" />
      ) : (
        <span className="flex size-full items-center justify-center text-muted-foreground">
          {failed ? (
            <TriangleAlert className="size-3" aria-label={`${label} unavailable`} />
          ) : (
            <LoaderCircle className="size-3 animate-spin" aria-label={`${label} loading`} />
          )}
        </span>
      )}
      {url && point && (
        <span
          aria-hidden
          className="absolute size-2 -translate-x-1/2 -translate-y-1/2 rounded-full border border-white bg-[var(--status-danger)]"
          style={{ left: `${(point.x / width) * 100}%`, top: `${(point.y / height) * 100}%` }}
        />
      )}
    </figure>
  );
}

/**
 * Below the banner while recording: the screenshots taken so far, oldest
 * first and scrolled to the newest. They are provisional: the steps, and
 * the screenshots they keep, are decided when the recording stops.
 */
export function RecordingStrip() {
  const active = useRecording((s) => s.status.state !== "idle");
  const snapshots = useRecording((s) => s.snapshots);
  const list = useRef<HTMLUListElement>(null);
  const count = snapshots.length;
  useEffect(() => {
    const el = list.current;
    if (el) el.scrollLeft = el.scrollWidth;
  }, [count]);
  if (!active || count === 0) return null;
  return (
    <section aria-label="Screenshots so far" className="flex items-center gap-3 border-b border-border bg-muted px-4 py-2">
      <span className="shrink-0 text-xs text-muted-foreground">
        {count === 1 ? "1 screenshot" : `${count} screenshots`} so far. Steps are worked out when you stop.
      </span>
      <ul ref={list} className="flex min-w-0 flex-1 gap-2 overflow-x-auto">
        {snapshots.map((snapshot, index) => (
          <li key={snapshot.id} className="flex shrink-0">
            <Thumbnail snapshot={snapshot} index={index} />
          </li>
        ))}
      </ul>
    </section>
  );
}
