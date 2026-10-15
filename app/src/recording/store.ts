import { createStore, type StoreApi } from "zustand/vanilla";
import {
  errorMessage,
  RECORDING_SAVED_EVENT,
  RECORDING_SNAPSHOT_EVENT,
  RECORDING_STATUS_EVENT,
  type RecordingApi,
} from "@/lib/ipc";
import type {
  AppError,
  CaptureTargetArg,
  RecordingSaved,
  RecordingSnapshot,
  RecordingStatus,
  StampedStatus,
} from "@/lib/types";

export type RecordingAction = "start" | "pause" | "resume" | "stop" | "cancel";

export interface RecordingError {
  message: string;
  /** Refused for a missing permission: the fix is in Settings → Permissions. */
  permissions: boolean;
}

export interface RecordingState {
  status: RecordingStatus;
  /** The command in flight, if any; the controls are disabled meanwhile. */
  pending: RecordingAction | null;
  error: RecordingError | null;
  /**
   * Screenshots the running recording has taken, oldest first. Provisional
   * (the steps are decided at stop); cleared when the recording ends.
   */
  snapshots: RecordingSnapshot[];

  /** Reads the recorder's state (once at startup: the app may have reloaded mid-recording). */
  load(): Promise<void>;
  /**
   * Applies a `recording://status` event or a command's result; one older
   * (lower `seq`) than the last applied is ignored, one without a `seq`
   * always applies.
   */
  setStatus(status: RecordingStatus | StampedStatus): void;
  /** Handles a `recording://snapshot` event (ignored when idle, and once per id). */
  addSnapshot(snapshot: RecordingSnapshot): void;
  /** Handles a `recording://saved` event or `stop`'s result, once per guide. */
  saved(saved: RecordingSaved): void;
  /** Starts recording `target`; without one, the capture area comes from the settings. */
  start(target?: CaptureTargetArg): Promise<boolean>;
  /**
   * Starts recording what `pick` resolves to (null: cancelled, nothing
   * starts). The pick runs as part of the start, so a second start
   * meanwhile is ignored and a failed pick is shown like a failed start.
   */
  startPicked(pick: () => Promise<CaptureTargetArg | null>): Promise<boolean>;
  pause(): Promise<boolean>;
  resume(): Promise<boolean>;
  /**
   * Saves the recording as a guide. `open: false` (when quitting) skips
   * `onSaved`. Resolves to null if it failed (see `error`).
   */
  stop(options?: { open?: boolean }): Promise<RecordingSaved | null>;
  cancel(): Promise<boolean>;
  clearError(): void;
}

export interface RecordingOptions {
  api: RecordingApi;
  /** A recording became a guide: open it, refresh the library. */
  onSaved?(saved: RecordingSaved): void;
}

export type RecordingStore = StoreApi<RecordingState>;

/** True while a recording is running or paused (stopping would save it). */
export function isActive(
  status: RecordingStatus,
): status is Extract<RecordingStatus, { state: "recording" | "paused" }> {
  return status.state === "recording" || status.state === "paused";
}

/**
 * Milliseconds recorded so far, not counting pauses, from the status alone
 * (so it survives a reload); 0 unless recording or paused.
 */
export function elapsedMs(status: RecordingStatus, now: number): number {
  if (!isActive(status)) return 0;
  const started = Date.parse(status.startedAt);
  if (Number.isNaN(started)) return 0;
  const pausedAt = status.state === "paused" ? Date.parse(status.pausedAt) : NaN;
  const end = Number.isNaN(pausedAt) ? now : Math.min(now, pausedAt);
  return Math.max(0, end - started - status.pausedMs);
}

/** Why capture stopped by itself, as a short label ("Capture stopped: the window was closed"), or null. */
export function captureLostLabel(status: RecordingStatus): string | null {
  return isActive(status) && status.captureLost !== null ? `Capture stopped: ${status.captureLost}` : null;
}

/** The full notice for a lost capture, telling the user what to do, or null. */
export function captureLostMessage(status: RecordingStatus): string | null {
  const label = captureLostLabel(status);
  return label === null ? null : `${label}. Stop to save what was recorded.`;
}

/** `m:ss`, or `h:mm:ss` from an hour. */
export function formatElapsed(ms: number): string {
  const total = Math.floor(ms / 1000);
  const s = String(total % 60).padStart(2, "0");
  const m = Math.floor(total / 60) % 60;
  const h = Math.floor(total / 3600);
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${s}` : `${m}:${s}`;
}

/** A refusal from the permissions gate (`ensure_can_record`). */
function isPermissionError(error: unknown): boolean {
  const e = error as Partial<AppError> | null;
  return !!e && e.kind === "invalid" && /^recording needs .* permission/.test(String(e.message));
}

/** Like Tauri's `listen`, handing over the event's payload. */
export type Listen = (event: string, handler: (payload: unknown) => void) => Promise<() => void>;

/** Keeps the store in step with the recorder's events. Returns an unsubscribe. */
export function subscribe(store: RecordingStore, listen: Listen): () => void {
  const stops = [
    listen(RECORDING_STATUS_EVENT, (status) => store.getState().setStatus(status as StampedStatus)),
    listen(RECORDING_SAVED_EVENT, (saved) => store.getState().saved(saved as RecordingSaved)),
    listen(RECORDING_SNAPSHOT_EVENT, (snapshot) => store.getState().addSnapshot(snapshot as RecordingSnapshot)),
  ];
  return () => {
    for (const stop of stops) void stop.then((unlisten) => unlisten());
  };
}

export function createRecordingStore({ api, onSaved }: RecordingOptions): RecordingStore {
  // Guides already handled: `stop`'s result and the saved event both arrive.
  const handled = new Set<string>();
  // A stop before quitting: its guide is not opened.
  let quietStop = false;
  // The newest `seq` applied: events and command results can arrive out of order.
  let lastSeq = -1;

  return createStore<RecordingState>()((set, get) => {
    /** Runs a command; on failure records the error and re-reads the state. */
    async function run<T>(action: RecordingAction, command: () => Promise<T>): Promise<T | null> {
      if (get().pending) return null;
      set({ pending: action, error: null });
      try {
        return await command();
      } catch (error) {
        set({ error: { message: errorMessage(error), permissions: isPermissionError(error) } });
        await get().load();
        return null;
      } finally {
        set({ pending: null });
      }
    }

    /**
     * Applies `status` unless it is older than the last one applied. A status
     * without a `seq` (the idle a stop or cancel ends in) always applies;
     * `reset` (a fresh read) applies and becomes the baseline.
     */
    function applyStatus(status: RecordingStatus | StampedStatus, reset = false) {
      if ("seq" in status) {
        if (!reset && status.seq < lastSeq) return;
        lastSeq = status.seq;
        const { seq: _, ...rest } = status;
        status = rest as RecordingStatus;
      }
      const prev = get().status;
      const sameRun = prev.state !== "idle" && status.state !== "idle" && prev.startedAt === status.startedAt;
      // The strip stays up while saving; it is gone once the guide opens.
      const snapshots = status.state === "idle" ? [] : sameRun ? get().snapshots : [];
      set({ status, snapshots });
    }

    return {
      status: { state: "idle" },
      pending: null,
      error: null,
      snapshots: [],

      async load() {
        try {
          applyStatus(await api.status(), true);
        } catch (error) {
          console.error("reading the recording status failed:", error);
        }
      },

      setStatus: (status) => applyStatus(status),

      addSnapshot(snapshot) {
        const { status, snapshots } = get();
        if (status.state === "idle") return;
        if (snapshots.some((s) => s.id === snapshot.id)) return;
        set({ snapshots: [...snapshots, snapshot] });
      },

      saved(saved) {
        if (handled.has(saved.guideId)) return;
        handled.add(saved.guideId);
        if (quietStop) return;
        onSaved?.(saved);
      },

      async start(target) {
        const status = await run("start", () => (target ? api.start(null, target) : api.start(null)));
        if (status) applyStatus(status);
        return status !== null;
      },

      async startPicked(pick) {
        const status = await run("start", async () => {
          const target = await pick();
          return target && (await api.start(null, target));
        });
        if (status) applyStatus(status);
        return status !== null;
      },

      async pause() {
        const status = await run("pause", () => api.pause());
        if (status) applyStatus(status);
        return status !== null;
      },

      async resume() {
        const status = await run("resume", () => api.resume());
        if (status) applyStatus(status);
        return status !== null;
      },

      async stop({ open = true } = {}) {
        // Refused while another command runs: the recorder's state is unchanged.
        if (get().pending) return null;
        quietStop = !open;
        try {
          const saved = await run("stop", () => api.stop());
          // The recorder is idle after a stop, saved or not.
          applyStatus({ state: "idle" });
          if (!saved) return null;
          get().saved(saved);
          return saved;
        } finally {
          quietStop = false;
        }
      },

      async cancel() {
        const done = (await run("cancel", async () => (await api.cancel(), true))) === true;
        if (done) applyStatus({ state: "idle" });
        return done;
      },

      clearError() {
        set({ error: null });
      },
    };
  });
}
