import { useEffect, useRef, useState } from "react";
import { AlertDialog } from "radix-ui";
import { Circle, Info, LoaderCircle, Pause, Play, ShieldCheck, Square, TriangleAlert, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { tauriRecordingApi } from "@/lib/ipc";
import type { RecordingSnapshot } from "@/lib/types";
import { appName } from "@/recording/apps";
import { WindowPicker } from "@/picker/WindowPicker";
import { recordingStore, startFlow, useRecording } from "@/recording/recording";
import { elapsedMs, formatElapsed } from "@/recording/store";
import { makeThumbnail, THUMBNAIL_SIDE } from "@/recording/thumbnails";

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

/**
 * The toolbar's Record button, or the live recording's controls; and the
 * window picker a Record asks with when the capture area is Window.
 */
export function RecordingControls() {
  return (
    <>
      <Controls />
      <WindowPicker flow={startFlow} recording={recordingStore} />
    </>
  );
}

function Controls() {
  const status = useRecording((s) => s.status);
  const pending = useRecording((s) => s.pending);
  const { pause, resume, stop } = recordingStore.getState();

  if (status.state === "idle") {
    return (
      <Button
        variant="outline"
        size="sm"
        className="ml-1"
        disabled={pending !== null}
        onClick={() => void startFlow.getState().begin()}
      >
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
  // Nothing is captured while an excluded app is in front; the clock runs on.
  const excludedApp = paused ? null : status.excludedApp;
  const excluded = excludedApp !== null;
  const busy = pending !== null;
  return (
    <div role="group" aria-label="Recording" className="ml-1 flex items-center gap-1 rounded-md border border-border px-1.5">
      <span
        className={`size-2 rounded-full ${excluded ? "bg-muted-foreground" : "bg-[var(--status-danger)]"} ${
          paused || excluded ? "opacity-40" : "animate-pulse"
        }`}
        aria-hidden
      />
      <span
        role="status"
        className="max-w-48 truncate text-xs font-medium"
        title={excluded ? "Nothing is captured while an excluded app is in front (Settings → Recording)." : undefined}
      >
        {paused ? "Paused" : excluded ? `Not capturing: ${appName(excludedApp)} is in front` : "Recording"}
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
          <span>Recording all displays: no window or region was chosen for this recording.</span>
        </div>
      )}
    </>
  );
}

/** One screenshot of the strip, fetched and scaled down once; a dot marks the click. */
function Thumbnail({ snapshot, index }: { snapshot: RecordingSnapshot; index: number }) {
  const [url, setUrl] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    let revoke: string | null = null;
    let gone = false;
    tauriRecordingApi
      .snapshotImage(snapshot.id)
      .then((bytes) => makeThumbnail(bytes, THUMBNAIL_SIDE * Math.min(2, window.devicePixelRatio || 1)))
      .then((made) => {
        if (gone || !made) {
          if (made) URL.revokeObjectURL(made);
          else setFailed(true);
          return;
        }
        revoke = made;
        setUrl(made);
      })
      .catch(() => setFailed(true));
    return () => {
      gone = true;
      if (revoke) URL.revokeObjectURL(revoke);
    };
  }, [snapshot.id]);

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
