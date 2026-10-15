import { createStore, type StoreApi } from "zustand/vanilla";
import { errorMessage, type PickerApi } from "@/lib/ipc";
import type { CaptureArea, CaptureWindow } from "@/lib/types";
import type { RecordingStore } from "@/recording/store";

/** How a recording starts, from the stored capture area (unknown: the display). */
export function startChoice(area: CaptureArea | null | undefined): CaptureArea {
  return area === "window" || area === "region" ? area : "display";
}

/** The window picker dialog, while it is open. */
export interface WindowPickerState {
  /** The windows to choose from, frontmost first; null while loading. */
  windows: CaptureWindow[] | null;
  /** Listing the windows failed. */
  loadError: string | null;
  /** Starting on the chosen window failed (it may have closed); the list was reloaded. */
  startError: string | null;
}

export interface StartFlowState {
  /** Open while the user picks a window. */
  windowPicker: WindowPickerState | null;
  /**
   * Starts a recording as the capture area says: the display at once, or
   * after picking a window or drawing a region. The Record button and
   * `recording://pick-area` both come here. Ignored unless idle.
   */
  begin(): Promise<void>;
  /** Re-reads the windows into the open picker. */
  reloadWindows(): Promise<void>;
  /** Starts recording the window; the picker closes once it started. */
  chooseWindow(windowId: number): Promise<boolean>;
  /** "Record whole display instead": closes the picker and starts on the display. */
  recordDisplay(): Promise<boolean>;
  closeWindowPicker(): void;
}

export interface StartFlowOptions {
  recording: RecordingStore;
  picker: PickerApi;
  /** The stored capture area; null if the settings aren't loaded. */
  captureArea(): CaptureArea | null;
}

export type StartFlowStore = StoreApi<StartFlowState>;

export function createStartFlow({ recording, picker, captureArea }: StartFlowOptions): StartFlowStore {
  // Only the newest listing lands in the picker.
  let listing = 0;

  return createStore<StartFlowState>()((set, get) => {
    const updatePicker = (patch: Partial<WindowPickerState>) => {
      const open = get().windowPicker;
      if (open) set({ windowPicker: { ...open, ...patch } });
    };

    return {
      windowPicker: null,

      async begin() {
        const { status, pending } = recording.getState();
        if (status.state !== "idle" || pending) return;
        switch (startChoice(captureArea())) {
          case "display":
            await recording.getState().start();
            return;
          case "window":
            if (get().windowPicker) return;
            set({ windowPicker: { windows: null, loadError: null, startError: null } });
            await get().reloadWindows();
            return;
          case "region":
            await recording.getState().startPicked(async () => {
              const pick = await picker.pickRegion();
              return pick && { kind: "region", displayId: pick.displayId, rect: pick.rect };
            });
            return;
        }
      },

      async reloadWindows() {
        if (!get().windowPicker) return;
        const id = ++listing;
        updatePicker({ windows: null, loadError: null });
        try {
          const windows = await picker.listCaptureWindows();
          if (id === listing) updatePicker({ windows });
        } catch (error) {
          if (id === listing) updatePicker({ windows: [], loadError: errorMessage(error) });
        }
      },

      async chooseWindow(windowId) {
        if (!get().windowPicker || recording.getState().pending) return false;
        updatePicker({ startError: null });
        if (await recording.getState().start({ kind: "window", windowId })) {
          set({ windowPicker: null });
          return true;
        }
        const error = recording.getState().error;
        // A missing permission stays in the banner, with its way to Settings.
        if (!error || error.permissions) {
          set({ windowPicker: null });
          return false;
        }
        recording.getState().clearError();
        updatePicker({ startError: error.message });
        await get().reloadWindows();
        return false;
      },

      async recordDisplay() {
        set({ windowPicker: null });
        return recording.getState().start({ kind: "display" });
      },

      closeWindowPicker() {
        listing++;
        set({ windowPicker: null });
      },
    };
  });
}
