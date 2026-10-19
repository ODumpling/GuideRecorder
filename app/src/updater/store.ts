import { createStore, type StoreApi } from "zustand/vanilla";
import { errorMessage, type UpdaterApi } from "@/lib/ipc";
import type { UpdateStatus } from "@/lib/types";

export interface UpdaterState {
  status: UpdateStatus;
  /** The available version the user put off with Later: its banner stays hidden. */
  dismissed: string | null;
  /** A refused command, for the UI. */
  error: string | null;
  load(): Promise<void>;
  /** Takes a status from `UPDATE_STATUS_EVENT`. */
  receive(status: UpdateStatus): void;
  check(): Promise<void>;
  install(): Promise<void>;
  restart(): Promise<void>;
  /** Hides the banner: Later on an available update, or closing a check's result. */
  dismiss(): void;
}

export type UpdaterStore = StoreApi<UpdaterState>;

export function createUpdaterStore(api: UpdaterApi): UpdaterStore {
  const run = (set: UpdaterStore["setState"], command: () => Promise<void>) =>
    command().then(
      () => set({ error: null }),
      (error) => set({ error: errorMessage(error) }),
    );

  return createStore<UpdaterState>()((set, get) => ({
    status: { state: "idle" },
    dismissed: null,
    error: null,

    async load() {
      try {
        get().receive(await api.status());
      } catch (error) {
        set({ error: errorMessage(error) });
      }
    },

    receive(status) {
      // A check the user asks for shows its result again, even for a version put off before.
      set(status.state === "checking" && status.manual ? { status, dismissed: null } : { status });
    },

    check: () => run(set, api.check),
    install: () => run(set, api.install),
    restart: () => run(set, api.restart),

    dismiss() {
      const { status } = get();
      if (status.state === "available") set({ dismissed: status.version, error: null });
      else if (status.state === "upToDate" || status.state === "failed") set({ status: { state: "idle" }, error: null });
      else set({ error: null });
    },
  }));
}

/** What the banner shows for a state, or null for none. */
export function bannerFor(state: Pick<UpdaterState, "status" | "dismissed" | "error">): UpdateStatus | null {
  const { status, dismissed } = state;
  switch (status.state) {
    case "available":
      return status.version === dismissed ? null : status;
    case "downloading":
    case "ready":
      return status;
    case "checking":
    case "upToDate":
    case "failed":
      return status.manual ? status : null;
    default:
      return null;
  }
}
