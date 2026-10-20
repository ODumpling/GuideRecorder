import { createStore, type StoreApi } from "zustand/vanilla";
import { errorMessage, type FeedApi } from "@/lib/ipc";
import type { RecordingSnapshot } from "@/lib/types";
import { formatElapsed, type RecordingStore } from "@/recording/store";
import { markdownToPlainText, parseMarkdown } from "@/text/markdown";

/** How many of the newest captures the feed shows. */
export const FEED_SIZE = 4;
/** The panel fades out after this long without a new capture, unless pinned. */
export const FEED_IDLE_MS = 5000;
/** A capture this recent (recorded time) reads "just now". */
export const JUST_NOW_MS = 5000;

/** One card of the feed. */
export interface FeedItem {
  snapshot: RecordingSnapshot;
  /** 1-based, among the captures still in the recording. */
  number: number;
  /** The title as plain text. */
  title: string;
}

/** The newest captures, newest first. */
export function feedItems(snapshots: RecordingSnapshot[], size = FEED_SIZE): FeedItem[] {
  const start = Math.max(0, snapshots.length - size);
  return snapshots
    .slice(start)
    .map((snapshot, i) => ({ snapshot, number: start + i + 1, title: markdownToPlainText(snapshot.title) }))
    .reverse();
}

/** A title's text in runs, bold or not ("Click **Submit**": "Click ", then bold "Submit"). */
export function titleRuns(markdown: string): { text: string; bold: boolean }[] {
  const line = parseMarkdown(markdown, { singleLine: true }).content?.[0]?.content ?? [];
  return line
    .filter((node) => node.type === "text" && node.text)
    .map((node) => ({ text: node.text ?? "", bold: !!node.marks?.some((mark) => mark.type === "bold") }));
}

/** When a capture was taken: "just now", else the recorded time then ("0:58"). */
export function takenLabel(elapsedAtMs: number, nowElapsedMs: number): string {
  return nowElapsedMs - elapsedAtMs < JUST_NOW_MS ? "just now" : formatElapsed(elapsedAtMs);
}

export interface FeedState {
  /** The user has the panel open (the counter toggles it). */
  expanded: boolean;
  /** Kept open, never fading. */
  pinned: boolean;
  /** A capture came in less than `FEED_IDLE_MS` ago (or the panel was just opened). */
  awake: boolean;
  /** The capture whose title is being edited. */
  editing: string | null;
  /** The last rename or delete that failed. */
  error: string | null;
  /** Opens the panel (awake) or, when it shows, closes it. */
  toggle(): void;
  setPinned(pinned: boolean): void;
  /** Starts or stops editing a title; the panel stays awake meanwhile. */
  setEditing(id: string | null): void;
  /** Leaves a capture out of the guide. */
  remove(id: string): Promise<boolean>;
  /** Removes the newest capture, as ⌘⌥Z does. */
  undoLast(): Promise<boolean>;
  /** Renames a capture; an empty or unchanged title is no change. */
  rename(id: string, title: string): Promise<boolean>;
}

export type FeedStore = StoreApi<FeedState>;

/** True when the panel shows: open, with captures, and pinned, awake or being edited. */
export function feedVisible(state: Pick<FeedState, "expanded" | "pinned" | "awake" | "editing">, count: number): boolean {
  return state.expanded && count > 0 && (state.pinned || state.awake || state.editing !== null);
}

export interface FeedOptions {
  api: Pick<FeedApi, "deleteCapture" | "renameCapture" | "setToolbarTyping">;
  recording: RecordingStore;
  /** For tests; defaults to `setTimeout`. */
  setTimer?(fn: () => void, ms: number): unknown;
  clearTimer?(timer: unknown): void;
}

/**
 * The toolbar's feed panel: wakes on each new capture and fades
 * `FEED_IDLE_MS` later unless pinned; deletes and renames go to Rust,
 * whose events update the recording store.
 */
export function createFeedStore({
  api,
  recording,
  setTimer = (fn, ms) => setTimeout(fn, ms),
  clearTimer = (t) => clearTimeout(t as ReturnType<typeof setTimeout>),
}: FeedOptions): FeedStore {
  let timer: unknown = null;
  const store = createStore<FeedState>()((set, get) => {
    async function run(command: () => Promise<unknown>): Promise<boolean> {
      try {
        await command();
        set({ error: null });
        return true;
      } catch (error) {
        set({ error: errorMessage(error) });
        return false;
      }
    }

    return {
      expanded: true,
      pinned: false,
      awake: false,
      editing: null,
      error: null,

      toggle() {
        const state = get();
        if (feedVisible(state, recording.getState().snapshots.length)) {
          get().setEditing(null);
          set({ expanded: false, pinned: false });
        } else {
          set({ expanded: true });
          wake();
        }
      },

      setPinned(pinned) {
        set({ pinned });
        if (!pinned) wake();
      },

      setEditing(editing) {
        const was = get().editing;
        if (was === editing) return;
        set({ editing });
        // The window takes the keyboard only while a title is edited.
        if ((was === null) !== (editing === null)) {
          api.setToolbarTyping(editing !== null).catch((error) => console.error("toolbar keyboard:", error));
        }
        if (editing === null) wake();
      },

      remove: (id) => run(() => api.deleteCapture(id)),

      async undoLast() {
        const { snapshots } = recording.getState();
        const last = snapshots[snapshots.length - 1];
        return last ? get().remove(last.id) : false;
      },

      async rename(id, title) {
        const text = title.trim();
        const current = recording.getState().snapshots.find((s) => s.id === id);
        if (!current || !text || text === markdownToPlainText(current.title)) return false;
        return run(() => api.renameCapture(id, text));
      },
    };
  });

  /** Awake now, asleep `FEED_IDLE_MS` from now. */
  function wake() {
    if (timer !== null) clearTimer(timer);
    store.setState({ awake: true });
    timer = setTimer(() => {
      timer = null;
      store.setState({ awake: false });
    }, FEED_IDLE_MS);
  }

  // Each new capture wakes the panel; a new recording starts closed-up again.
  let seen = new Set(recording.getState().snapshots.map((s) => s.id));
  recording.subscribe((state) => {
    const editing = store.getState().editing;
    if (editing !== null && !state.snapshots.some((s) => s.id === editing)) store.getState().setEditing(null);
    if (state.status.state === "idle") {
      seen = new Set();
      return;
    }
    const added = state.snapshots.some((s) => !seen.has(s.id));
    seen = new Set(state.snapshots.map((s) => s.id));
    if (added) wake();
  });
  return store;
}
