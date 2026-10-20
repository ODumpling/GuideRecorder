import { useEffect, useRef, useState } from "react";
import { useStore } from "zustand";
import { Camera, Check, LoaderCircle, Pause, Play, Plus, Square, TriangleAlert } from "lucide-react";
import { Button } from "@/components/ui/button";
import { elapsedMs, formatElapsed, type RecordingStore } from "@/recording/store";
import { feedVisible, type FeedStore } from "@/toolbar/feed";
import { FeedPanel } from "@/toolbar/FeedPanel";
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
    <span aria-label="Recording time" className="font-mono text-[13px] text-white tabular-nums">
      {formatElapsed(ms)}
    </span>
  );
}

// The toolbar floats over any app and wallpaper, so it is dark glass in every theme.
const HUD_BUTTON = "size-8 rounded-lg text-white/85 hover:bg-white/10 hover:text-white disabled:opacity-40";

/**
 * The floating toolbar: state, clock, step count, and Pause/Resume, Manual
 * step, Stop; the live feed panel under it. The window is sized to fit
 * (`resize`, with the content's height).
 */
export function FloatingToolbar({
  recording,
  manual,
  feed,
  resize,
}: {
  recording: RecordingStore;
  manual: ManualStepStore;
  feed: FeedStore;
  resize(height: number): void;
}) {
  const status = useStore(recording, (s) => s.status);
  const pending = useStore(recording, (s) => s.pending);
  const snapshots = useStore(recording, (s) => s.snapshots);
  const phase = useStore(manual, (s) => s.phase);
  const manualError = useStore(manual, (s) => s.error);
  const feedOpen = useStore(feed, (s) => feedVisible(s, snapshots.length));
  const view = toolbarView({ status, pending, snapshots }, phase === "adding");
  const { pause, resume, stop } = recording.getState();
  const root = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = root.current;
    if (!el) return;
    const observer = new ResizeObserver(() => resize(Math.ceil(el.getBoundingClientRect().height)));
    observer.observe(el);
    return () => observer.disconnect();
  }, [resize]);

  return (
    <div ref={root} className="flex w-full flex-col">
      <div
        data-tauri-drag-region
        role="toolbar"
        aria-label="Recording toolbar"
        className="flex h-11 w-full shrink-0 items-center gap-1 rounded-[14px] bg-[#181614]/90 px-1.5 text-white ring-1 ring-white/10 backdrop-blur-xl select-none ring-inset"
      >
        <span
          data-tauri-drag-region
          aria-hidden
          className={`ml-1.5 size-2.5 shrink-0 rounded-full ${view.excluded || view.lost ? "bg-white/40" : "bg-[#ff4d3a] shadow-[0_0_0_3px_rgb(255_77_58/0.25)]"} ${
            view.paused || view.excluded || view.lost ? "opacity-50" : "animate-pulse"
          }`}
        />
        <span
          data-tauri-drag-region
          role="status"
          className="min-w-0 flex-1 truncate pl-1 text-xs font-medium text-white/75"
          title={
            view.lost ??
            (view.excluded ? "Nothing is captured while an excluded app is in front (Settings → Recording)." : view.label)
          }
        >
          {view.label}
        </span>
        <Elapsed recording={recording} />
        <button
          type="button"
          title={feedOpen ? "Screenshots taken so far: hide them" : "Screenshots taken so far: show them"}
          aria-expanded={feedOpen}
          onClick={() => feed.getState().toggle()}
          className={`ml-1 flex items-center gap-1 rounded-md border-l border-white/10 py-0.5 pr-1 pl-2 text-xs tabular-nums transition-colors hover:bg-white/10 ${
            phase === "added" ? "bg-white/15 text-white" : "text-white/70"
          }`}
        >
          <Camera aria-hidden className="size-3.5" />
          <span aria-label="Screenshots">{view.steps}</span>
        </button>
        {view.paused ? (
          <Button
            variant="ghost"
            size="icon"
            className={HUD_BUTTON}
            aria-label="Resume recording"
            title="Resume"
            disabled={!view.canResume}
            onClick={() => void resume()}
          >
            <Play />
          </Button>
        ) : (
          <Button
            variant="ghost"
            size="icon"
            className={HUD_BUTTON}
            aria-label="Pause recording"
            title="Pause"
            disabled={!view.canPause}
            onClick={() => void pause()}
          >
            <Pause />
          </Button>
        )}
        <Button
          variant="ghost"
          size="icon"
          className={HUD_BUTTON}
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
            <Check className="text-[#4be3a8]" />
          ) : phase === "failed" ? (
            <TriangleAlert className="text-[#ffb547]" />
          ) : (
            <Plus />
          )}
        </Button>
        <span className="sr-only" role="alert">
          {phase === "added" ? "Step added" : phase === "failed" ? `Couldn't add a step: ${manualError}` : ""}
        </span>
        <Button
          size="sm"
          className="ml-0.5 h-8 rounded-lg bg-[#ff4d3a] px-3 font-semibold text-white hover:bg-[#ff4d3a]/90 disabled:opacity-50"
          aria-label="Stop recording"
          title="Stop and save"
          disabled={!view.canStop}
          onClick={() => void stop()}
        >
          <Square className="size-3! fill-current" /> Done
        </Button>
      </div>
      <FeedPanel recording={recording} feed={feed} />
    </div>
  );
}
