import { createStore, type StoreApi } from "zustand/vanilla";
import { errorMessage } from "@/lib/ipc";
import { appName } from "@/recording/apps";
import type { RecordingState, RecordingStore } from "@/recording/store";

/** What the toolbar shows and which of its buttons work, from the recording store's state. */
export interface ToolbarView {
  /** "Recording", "Paused", "Saving…", or why nothing is captured. */
  label: string;
  /** An excluded app is in front: nothing is captured. */
  excluded: boolean;
  paused: boolean;
  canPause: boolean;
  canResume: boolean;
  canAddStep: boolean;
  canStop: boolean;
  /**
   * Screenshots taken in this run (`recording://snapshot` events since the
   * window opened); the store drops them on a new `startedAt` or when idle.
   */
  steps: number;
}

export function toolbarView(state: Pick<RecordingState, "status" | "pending" | "snapshots">, adding = false): ToolbarView {
  const { status, pending } = state;
  const steps = state.snapshots.length;
  if (status.state !== "recording" && status.state !== "paused") {
    const label = status.state === "saving" ? "Saving…" : "Not recording";
    return { label, excluded: false, paused: false, canPause: false, canResume: false, canAddStep: false, canStop: false, steps };
  }
  const paused = status.state === "paused";
  const excludedApp = paused ? null : status.excludedApp;
  const excluded = excludedApp !== null;
  const free = pending === null;
  return {
    label: paused ? "Paused" : excluded ? `Not capturing: ${appName(excludedApp)} is in front` : "Recording",
    excluded,
    paused,
    canPause: free && !paused,
    canResume: free && paused,
    // Paused counts as not running for `add_manual_step`; an excluded app's screen is never captured.
    canAddStep: free && !paused && !excluded && !adding,
    canStop: free,
    steps,
  };
}

/** How long "Step added" or a failed manual step shows. */
export const MANUAL_STEP_NOTICE_MS = 1500;

export interface ManualStepState {
  /** `adding`: the command is in flight; `added`/`failed`: the brief notice after it. */
  phase: "ready" | "adding" | "added" | "failed";
  error: string | null;
  add(): Promise<boolean>;
}

export type ManualStepStore = StoreApi<ManualStepState>;

export interface ManualStepOptions {
  addManualStep(): Promise<void>;
  recording: RecordingStore;
  /** For tests; defaults to `setTimeout`. */
  setTimer?(fn: () => void, ms: number): unknown;
  clearTimer?(timer: unknown): void;
}

/** The toolbar's Manual step button: calls `add_manual_step`, then shows a brief confirmation or error. */
export function createManualStepStore({
  addManualStep,
  recording,
  setTimer = (fn, ms) => setTimeout(fn, ms),
  clearTimer = (t) => clearTimeout(t as ReturnType<typeof setTimeout>),
}: ManualStepOptions): ManualStepStore {
  let timer: unknown = null;
  return createStore<ManualStepState>()((set, get) => ({
    phase: "ready",
    error: null,
    async add() {
      if (get().phase === "adding" || !toolbarView(recording.getState()).canAddStep) return false;
      if (timer !== null) clearTimer(timer);
      timer = null;
      set({ phase: "adding", error: null });
      let ok = true;
      try {
        await addManualStep();
        set({ phase: "added" });
      } catch (error) {
        ok = false;
        set({ phase: "failed", error: errorMessage(error) });
        // The recorder may have paused or stopped meanwhile.
        void recording.getState().load();
      }
      timer = setTimer(() => {
        timer = null;
        set({ phase: "ready", error: null });
      }, MANUAL_STEP_NOTICE_MS);
      return ok;
    },
  }));
}
