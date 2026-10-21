import { createStore, type StoreApi } from "zustand/vanilla";
import { errorMessage, NARRATION_EVENT, type NarrationApi } from "@/lib/ipc";
import { thisComputer } from "@/lib/platform";
import type { NarrationState } from "@/lib/types";

/** The toolbar's microphone: narrating while recording (see app/src-tauri/src/narration.rs). */
export interface MicState {
  /** null until loaded. */
  narration: NarrationState | null;
  /** A turn on or off is in flight. */
  busy: boolean;
  /** Why the last turn on or off failed outright (the command was refused). */
  error: string | null;
  load(): Promise<void>;
  /** Takes a state sent as `recording://narration`. */
  setNarration(narration: NarrationState): void;
  /** Turns the microphone off when on, on when off. */
  toggle(): Promise<void>;
}

export type MicStore = StoreApi<MicState>;

export function createMicStore({ api }: { api: NarrationApi }): MicStore {
  return createStore<MicState>()((set, get) => ({
    narration: null,
    busy: false,
    error: null,
    async load() {
      try {
        set({ narration: await api.state() });
      } catch (error) {
        set({ error: errorMessage(error) });
      }
    },
    setNarration(narration) {
      set({ narration });
    },
    async toggle() {
      const { narration, busy } = get();
      if (!narration?.supported || busy) return;
      set({ busy: true, error: null });
      try {
        set({ narration: await api.set(!narration.on) });
      } catch (error) {
        set({ error: errorMessage(error) });
      } finally {
        set({ busy: false });
      }
    },
  }));
}

/** Keeps `store` in line with `recording://narration`; returns the unlisten promise. */
export function subscribeMic(
  store: MicStore,
  listen: (event: string, handler: (payload: NarrationState) => void) => Promise<() => void>,
): Promise<() => void> {
  return listen(NARRATION_EVENT, (payload) => store.getState().setNarration(payload));
}

/** What the microphone button shows. */
export interface MicView {
  /** Narration works here; otherwise there is no button. */
  show: boolean;
  on: boolean;
  /** Something went wrong turning it on: the button says why. */
  warning: boolean;
  label: string;
  title: string;
  disabled: boolean;
}

export function micView({ narration, busy, error }: Pick<MicState, "narration" | "busy" | "error">, paused: boolean): MicView {
  if (!narration?.supported) {
    return { show: false, on: false, warning: false, label: "Narrate", title: "", disabled: true };
  }
  const problem = error ?? narration.error;
  if (narration.on) {
    return {
      show: true,
      on: true,
      warning: false,
      label: "Turn the microphone off",
      title: paused
        ? "Microphone on, and paused with the recording"
        : `Microphone on: what you say is transcribed on ${thisComputer()} and added to the steps. Click to turn it off.`,
      disabled: busy,
    };
  }
  return {
    show: true,
    on: false,
    warning: problem !== null,
    label: "Turn the microphone on",
    title: problem ? `Couldn't turn the microphone on: ${problem}` : "Narrate: turn the microphone on and say what you're doing",
    disabled: busy,
  };
}
