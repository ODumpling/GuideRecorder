import { useStore } from "zustand";
import { tauriUpdaterApi } from "@/lib/ipc";
import { createUpdaterStore, type UpdaterState } from "@/updater/store";

/** The app's updater store; loaded at startup and fed by `UPDATE_STATUS_EVENT`. */
export const updaterStore = createUpdaterStore(tauriUpdaterApi);

export function useUpdater<T>(selector: (state: UpdaterState) => T): T {
  return useStore(updaterStore, selector);
}
