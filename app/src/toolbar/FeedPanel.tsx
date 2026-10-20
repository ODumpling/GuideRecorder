import { useEffect, useRef, useState } from "react";
import { useStore } from "zustand";
import { LoaderCircle, Pencil, Pin, PinOff, Trash2, TriangleAlert, Undo2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { elapsedMs, type RecordingStore } from "@/recording/store";
import { useThumbnail } from "@/recording/useThumbnail";
import { feedItems, feedVisible, takenLabel, titleRuns, type FeedItem, type FeedStore } from "@/toolbar/feed";

// Dark glass, as the bar.
const CARD = "flex items-center gap-2.5 rounded-xl bg-[#1c1a18]/85 p-2 text-white ring-1 ring-white/10 ring-inset backdrop-blur-xl";
const ICON_BUTTON = "size-6 rounded-md text-white/70 hover:bg-white/10 hover:text-white [&_svg]:size-3.5!";

/** The capture's screenshot, small; a dot marks the click. */
function Mini({ item }: { item: FeedItem }) {
  const { snapshot } = item;
  const { url, failed } = useThumbnail(snapshot.id);
  const { point, width, height } = snapshot;
  return (
    <span
      className="relative w-16 shrink-0 overflow-hidden rounded-[5px] bg-white/10"
      style={{ aspectRatio: `${width} / ${height}` }}
    >
      {url ? (
        <img src={url} alt="" className="size-full object-cover" />
      ) : (
        <span className="flex size-full items-center justify-center text-white/50">
          {failed ? <TriangleAlert className="size-3" /> : <LoaderCircle className="size-3 animate-spin" />}
        </span>
      )}
      {url && point && (
        <span
          aria-hidden
          className="absolute size-1.5 -translate-x-1/2 -translate-y-1/2 rounded-full border border-white bg-[#ff4d3a]"
          style={{ left: `${(point.x / width) * 100}%`, top: `${(point.y / height) * 100}%` }}
        />
      )}
    </span>
  );
}

/** A title, its bold runs bold. */
function Title({ markdown }: { markdown: string }) {
  return (
    <>
      {titleRuns(markdown).map((run, i) =>
        run.bold ? (
          <strong key={i} className="font-semibold">
            {run.text}
          </strong>
        ) : (
          <span key={i}>{run.text}</span>
        ),
      )}
    </>
  );
}

function Card({ item, feed, now, newest }: { item: FeedItem; feed: FeedStore; now: number; newest: boolean }) {
  const editing = useStore(feed, (s) => s.editing === item.snapshot.id);
  const [draft, setDraft] = useState(item.title);
  // Esc ends the edit without the blur that follows saving it.
  const done = useRef(false);
  const { setEditing, rename, remove } = feed.getState();
  const start = () => {
    done.current = false;
    setDraft(item.title);
    setEditing(item.snapshot.id);
  };
  const finish = (save: boolean) => {
    if (done.current) return;
    done.current = true;
    setEditing(null);
    if (save) void rename(item.snapshot.id, draft);
  };
  const label = `Step ${item.number}`;
  return (
    <li className={`${CARD} group transition-opacity ${newest ? "" : "opacity-60 hover:opacity-100"}`} aria-label={label}>
      <Mini item={item} />
      <div className="min-w-0 flex-1">
        {editing ? (
          <input
            autoFocus
            aria-label={`${label} title`}
            value={draft}
            maxLength={200}
            onChange={(e) => setDraft(e.target.value)}
            onBlur={() => finish(true)}
            onKeyDown={(e) => {
              if (e.key === "Enter") finish(true);
              if (e.key === "Escape") finish(false);
            }}
            className="w-full rounded bg-white/10 px-1 text-xs text-white outline-none ring-1 ring-white/30"
          />
        ) : (
          <button
            type="button"
            title="Rename"
            className="block w-full truncate text-left text-xs leading-snug"
            onClick={start}
          >
            {item.number}. <Title markdown={item.snapshot.title} />
          </button>
        )}
        <small className="text-[11px] text-white/55">{takenLabel(item.snapshot.elapsedMs, now)}</small>
      </div>
      {!editing && (
        <span className="flex shrink-0 opacity-0 transition-opacity group-hover:opacity-100 focus-within:opacity-100">
          <Button
            variant="ghost"
            size="icon"
            className={ICON_BUTTON}
            aria-label={`Rename ${label.toLowerCase()}`}
            title="Rename"
            onClick={start}
          >
            <Pencil />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            className={ICON_BUTTON}
            aria-label={`Delete ${label.toLowerCase()}`}
            title="Delete this capture"
            onClick={() => void remove(item.snapshot.id)}
          >
            <Trash2 />
          </Button>
        </span>
      )}
    </li>
  );
}

/** Recorded time now, ticking each second while recording. */
function useRecordedNow(recording: RecordingStore): number {
  const state = useStore(recording, (s) => s.status.state);
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    if (state !== "recording") return;
    setNow(Date.now());
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [state]);
  return useStore(recording, (s) => elapsedMs(s.status, now));
}

/**
 * Under the bar: the newest captures, each with its provisional title and
 * when it was taken, to rename or delete. Fades out a few seconds after the
 * last capture unless pinned; the window shrinks back once it has.
 */
export function FeedPanel({ recording, feed }: { recording: RecordingStore; feed: FeedStore }) {
  const snapshots = useStore(recording, (s) => s.snapshots);
  const visible = useStore(feed, (s) => feedVisible(s, snapshots.length));
  const pinned = useStore(feed, (s) => s.pinned);
  const error = useStore(feed, (s) => s.error);
  const now = useRecordedNow(recording);
  // Stays mounted while it fades out.
  const [mounted, setMounted] = useState(visible);
  useEffect(() => {
    if (visible) setMounted(true);
  }, [visible]);
  if (!mounted) return null;
  const items = feedItems(snapshots);
  const { setPinned, undoLast } = feed.getState();

  return (
    <section
      aria-label="Captured so far"
      className={`mt-2 flex flex-col gap-2 transition-opacity duration-300 ${visible ? "opacity-100" : "pointer-events-none opacity-0"}`}
      onTransitionEnd={() => {
        if (!visible) setMounted(false);
      }}
    >
      <div className="flex items-center gap-1 pl-1 text-[11px] font-semibold tracking-wider text-white/85 uppercase [text-shadow:0_1px_2px_rgb(0_0_0/0.6)]">
        <span className="flex-1">Captured</span>
        <Button
          variant="ghost"
          size="icon"
          className={ICON_BUTTON}
          aria-label="Undo the last step"
          title="Undo the last step (⌘⌥Z)"
          disabled={items.length === 0}
          onClick={() => void undoLast()}
        >
          <Undo2 />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          className={ICON_BUTTON}
          aria-label={pinned ? "Unpin the feed" : "Pin the feed"}
          aria-pressed={pinned}
          title={pinned ? "Let it fade" : "Keep it open"}
          onClick={() => setPinned(!pinned)}
        >
          {pinned ? <PinOff /> : <Pin />}
        </Button>
      </div>
      <ul className="flex flex-col gap-1.5">
        {items.map((item, i) => (
          <Card key={item.snapshot.id} item={item} feed={feed} now={now} newest={i === 0} />
        ))}
      </ul>
      <p className={`${CARD} px-3 py-1.5 text-[11px] text-white/65`} role={error ? "alert" : undefined}>
        {error ? `Couldn't change the capture: ${error}` : "Steps are worked out when you stop."}
      </p>
    </section>
  );
}
