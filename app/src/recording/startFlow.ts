import { createStore, type StoreApi } from "zustand/vanilla";
import type { RecordedArea } from "@guiderecorder/guide-schema";
import { errorMessage, type PickerApi } from "@/lib/ipc";
import type { CaptureArea, CaptureDisplay, CaptureWindow, RecordInto } from "@/lib/types";
import type { RecordingStore } from "@/recording/store";

/** How a recording starts, from the stored capture area (unknown: the display). */
export function startChoice(area: CaptureArea | null | undefined): CaptureArea {
  return area === "oneDisplay" || area === "window" || area === "region" ? area : "display";
}

/**
 * The open window a guide's recorded window (`recording.captureArea`) is
 * now: the same app with the same title, else the app's only window; null
 * if there is none (or several, none titled the same).
 */
export function findWindow(
  windows: CaptureWindow[],
  area: Extract<RecordedArea, { kind: "window" }>,
): CaptureWindow | null {
  const sameApp = windows.filter((w) => (area.bundleId ? w.bundleId === area.bundleId : w.appName === area.app));
  return sameApp.find((w) => (w.title ?? null) === (area.title ?? null)) ?? (sameApp.length === 1 ? sameApp[0] : null);
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

/** The display picker dialog, while it is open. */
export interface DisplayPickerState {
  displays: CaptureDisplay[];
  /** Starting on the chosen display failed (it may be disconnected); the list was reloaded. */
  startError: string | null;
}

export interface StartFlowState {
  /** Open while the user picks a window. */
  windowPicker: WindowPickerState | null;
  /** Open while the user picks a display. */
  displayPicker: DisplayPickerState | null;
  /**
   * Starts a recording as the capture area says: every display at once, or
   * after picking a display or a window or drawing a region. With one
   * display connected, One display starts on it at once. The New Guide dialog and
   * `recording://pick-area` both come here. `area` overrides the stored
   * capture area for this recording (the library's Record buttons). With
   * `into`, the recording goes into that guide (see `RecordInto`). Ignored
   * unless idle.
   */
  begin(area?: CaptureArea, into?: RecordInto): Promise<void>;
  /**
   * Starts a recording into a guide (a re-capture, steps inserted) on the
   * area it was recorded with (`recording.captureArea`): the same display
   * if it is still connected, the same region, or the same window if it
   * can be found (see `findWindow`). Otherwise the user picks, as `begin`
   * asks; a guide that doesn't say starts as the settings say.
   */
  recordInto(into: RecordInto, area: RecordedArea | null | undefined): Promise<void>;
  /** Re-reads the windows into the open picker. */
  reloadWindows(): Promise<void>;
  /** Starts recording the window; the picker closes once it started. */
  chooseWindow(windowId: number): Promise<boolean>;
  /** "Record whole display instead": closes the picker and starts on the display. */
  recordDisplay(): Promise<boolean>;
  closeWindowPicker(): void;
  /** Starts recording the display (null: every display); the picker closes once it started. */
  chooseDisplay(displayId: number | null): Promise<boolean>;
  closeDisplayPicker(): void;
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
  // A display listing runs (begin waits for it).
  let listingDisplays = false;
  // The guide the start being picked goes into, if any.
  let into: RecordInto | undefined;

  return createStore<StartFlowState>()((set, get) => {
    const updatePicker = (patch: Partial<WindowPickerState>) => {
      const open = get().windowPicker;
      if (open) set({ windowPicker: { ...open, ...patch } });
    };

    return {
      windowPicker: null,
      displayPicker: null,

      async begin(area, target) {
        const { status, pending } = recording.getState();
        if (status.state !== "idle" || pending || get().displayPicker) return;
        // A picker already open keeps its own.
        if (!get().windowPicker) into = target;
        switch (startChoice(area ?? captureArea())) {
          case "display":
            await recording.getState().start(undefined, into);
            return;
          case "oneDisplay": {
            if (listingDisplays) return;
            listingDisplays = true;
            let displays: CaptureDisplay[];
            try {
              displays = await picker.listCaptureDisplays();
            } catch {
              // Can't tell which: every display, as without a pick.
              displays = [];
            } finally {
              listingDisplays = false;
            }
            if (displays.length <= 1) {
              await recording.getState().start(displays[0] && { kind: "display", displayId: displays[0].displayId }, into);
              return;
            }
            set({ displayPicker: { displays, startError: null } });
            return;
          }
          case "window":
            if (get().windowPicker) return;
            set({ windowPicker: { windows: null, loadError: null, startError: null } });
            await get().reloadWindows();
            return;
          case "region":
            await recording.getState().startPicked(async () => {
              const pick = await picker.pickRegion();
              return pick && { kind: "region", displayId: pick.displayId, rect: pick.rect };
            }, into);
            return;
        }
      },

      async recordInto(target, area) {
        const { status, pending } = recording.getState();
        if (status.state !== "idle" || pending || get().displayPicker || get().windowPicker) return;
        const start = recording.getState().start;
        switch (area?.kind) {
          case "display": {
            if (area.displayId === undefined || area.displayId === null) {
              await start({ kind: "display" }, target);
              return;
            }
            let displays: CaptureDisplay[] = [];
            try {
              displays = await picker.listCaptureDisplays();
            } catch {
              // Can't tell: let the user pick.
            }
            if (displays.some((d) => d.displayId === area.displayId)) {
              await start({ kind: "display", displayId: area.displayId }, target);
            } else {
              await get().begin("oneDisplay", target);
            }
            return;
          }
          case "region":
            await start({ kind: "region", displayId: area.displayId ?? null, rect: area.rect }, target);
            return;
          case "window": {
            let windows: CaptureWindow[] = [];
            try {
              windows = await picker.listCaptureWindows();
            } catch {
              // Can't tell: let the user pick.
            }
            const window = findWindow(windows, area);
            if (window) await start({ kind: "window", windowId: window.windowId }, target);
            else await get().begin("window", target);
            return;
          }
          default:
            await get().begin(undefined, target);
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
        if (await recording.getState().start({ kind: "window", windowId }, into)) {
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
        return recording.getState().start({ kind: "display" }, into);
      },

      closeWindowPicker() {
        listing++;
        set({ windowPicker: null });
      },

      async chooseDisplay(displayId) {
        const open = get().displayPicker;
        if (!open || recording.getState().pending) return false;
        set({ displayPicker: { ...open, startError: null } });
        if (await recording.getState().start(displayId === null ? { kind: "display" } : { kind: "display", displayId }, into)) {
          set({ displayPicker: null });
          return true;
        }
        const error = recording.getState().error;
        if (!error || error.permissions) {
          set({ displayPicker: null });
          return false;
        }
        recording.getState().clearError();
        let displays = open.displays;
        try {
          displays = await picker.listCaptureDisplays();
        } catch {
          // Keep the old list.
        }
        if (get().displayPicker) set({ displayPicker: { displays, startError: error.message } });
        return false;
      },

      closeDisplayPicker() {
        set({ displayPicker: null });
      },
    };
  });
}
