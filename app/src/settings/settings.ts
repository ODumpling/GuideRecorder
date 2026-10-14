import { useStore } from "zustand";
import { tauriSettingsApi } from "@/lib/ipc";
import { createSettingsStore, type SettingsState } from "@/settings/store";

/** The app's settings store; loaded at startup. */
export const settingsStore = createSettingsStore(tauriSettingsApi);

export function useSettings<T>(selector: (state: SettingsState) => T): T {
  return useStore(settingsStore, selector);
}
