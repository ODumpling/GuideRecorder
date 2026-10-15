import { createStore, type StoreApi } from "zustand/vanilla";
import { errorMessage, type SettingsApi } from "@/lib/ipc";
import type { Settings } from "@/lib/types";

export interface SettingsState {
  /** null until loaded. */
  settings: Settings | null;
  readOnly: boolean;
  /** Why the stored settings could not be used (or loaded), if so. */
  loadError: string | null;
  /** The last refused change, for the UI. */
  error: string | null;
  load(): Promise<void>;
  /**
   * Applies a change at once and saves it. Saves run one after another; a
   * refused one is reported and the shown settings go back to the last
   * stored ones. Resolves to whether this change was stored.
   */
  change(update: (settings: Settings) => Settings): Promise<boolean>;
  /** Takes settings the Rust side changed itself (library switch, login item). */
  replace(settings: Settings): void;
  clearError(): void;
}

export type SettingsStore = StoreApi<SettingsState>;

export function createSettingsStore(api: SettingsApi): SettingsStore {
  // The settings as last confirmed by the Rust side.
  let stored: Settings | null = null;
  let queue: Promise<unknown> = Promise.resolve();

  return createStore<SettingsState>()((set, get) => ({
    settings: null,
    readOnly: false,
    loadError: null,
    error: null,

    async load() {
      try {
        const view = await api.get();
        stored = view.settings;
        set({ settings: view.settings, readOnly: view.readOnly, loadError: view.loadError });
      } catch (error) {
        set({ loadError: errorMessage(error) });
      }
    },

    change(update) {
      const current = get().settings;
      if (!current) return Promise.resolve(false);
      if (get().readOnly) {
        set({ error: "These settings were saved by a newer GuideRecorder and can't be changed here." });
        return Promise.resolve(false);
      }
      const next = update(structuredClone(current));
      set({ settings: next, error: null });
      const result = queue.then(async () => {
        try {
          const saved = await api.update(next);
          stored = saved;
          // Keep a newer change that is still waiting to be saved.
          if (get().settings === next) set({ settings: saved });
          return true;
        } catch (error) {
          set({ settings: stored, error: errorMessage(error) });
          return false;
        }
      });
      queue = result;
      return result;
    },

    replace(settings) {
      stored = settings;
      set({ settings });
    },

    clearError() {
      set({ error: null });
    },
  }));
}
