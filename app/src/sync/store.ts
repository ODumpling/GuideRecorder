import { createStore, type StoreApi } from "zustand/vanilla";
import { realTimers, type Timers } from "@/editor/debounce";
import { errorMessage, type DeviceCode, type SyncAccount, type SyncApi } from "@/lib/ipc";
import {
  emptyState,
  loadedState,
  resolveConflict,
  submitForReview,
  setGuideEnabled,
  setWorkspace,
  syncOnce,
  SyncStopped,
  type GuideStatus,
  type Resolution,
  type SyncBackend,
  type SyncState,
} from "@guiderecorder/sync/engine";

export interface Workspace {
  id: string;
  name: string;
}

export interface SyncStoreState {
  account: SyncAccount | null;
  state: SyncState;
  workspaces: Workspace[];
  /** Whether the sign-in dialog is open. */
  signInOpen: boolean;
  /** While signing in: the code to show. */
  signIn: DeviceCode | null;
  running: boolean;
  /** Why the last run stopped, or a refused action. */
  error: string | null;
  lastSynced: string | null;
  /** The guide whose conflict dialog is open. */
  resolving: string | null;

  load(): Promise<void>;
  /** Opens the sign-in dialog; closing it cancels a sign-in under way. */
  openSignIn(open: boolean): void;
  startSignIn(server: string): Promise<void>;
  cancelSignIn(): void;
  signOut(): Promise<void>;
  loadWorkspaces(): Promise<void>;
  chooseWorkspace(id: string): Promise<void>;
  setLibrary(on: boolean): Promise<void>;
  setGuideEnabled(id: string, on: boolean): Promise<void>;
  /** Runs a sync now, or once more after the one running. */
  syncNow(): Promise<void>;
  openConflict(id: string | null): void;
  resolve(id: string, resolution: Resolution): Promise<void>;
  /** Submits a synced guide for review on the web. */
  submitForReview(id: string, note?: string): Promise<void>;
}

export type SyncStore = StoreApi<SyncStoreState>;

/** What a guide's card shows: its status, or null if it doesn't sync. */
export function guideSyncStatus(s: SyncStoreState, id: string): GuideStatus | null {
  if (!s.account?.signedIn || !s.state.workspaceId) return null;
  const entry = s.state.guides[id];
  if (entry?.status === "deleted") return "deleted";
  if (!s.state.library && !entry?.enabled) return null;
  return entry?.status ?? "pending";
}

export function createSyncStore({
  api,
  backend,
  onGuidesWritten = () => {},
  timers = realTimers,
}: {
  api: SyncApi;
  backend: SyncBackend;
  /** After a run wrote guides into the library: refresh the list. */
  onGuidesWritten?: () => void;
  timers?: Timers;
}): SyncStore {
  let poll: unknown;
  let again = false;
  /** Changes made while a run was going, replayed on its result. */
  let queued: ((s: SyncState) => SyncState)[] = [];

  return createStore<SyncStoreState>()((set, get) => {
    /** Changes the sync state and stores it. */
    async function update(change: (s: SyncState) => SyncState) {
      if (get().running) queued.push(change);
      const state = change(get().state);
      set({ state });
      await api.saveState(state).catch((e) => set({ error: errorMessage(e) }));
    }

    function stopPolling() {
      if (poll !== undefined) timers.clearTimeout(poll);
      poll = undefined;
    }

    return {
      account: null,
      state: emptyState(),
      workspaces: [],
      signInOpen: false,
      signIn: null,
      running: false,
      error: null,
      lastSynced: null,
      resolving: null,

      async load() {
        try {
          const [account, stored] = await Promise.all([api.account(), api.loadState()]);
          set({ account, state: loadedState(stored) });
          if (account.signedIn) await get().loadWorkspaces();
        } catch (error) {
          set({ error: errorMessage(error) });
        }
      },

      openSignIn(open) {
        if (!open) get().cancelSignIn();
        set({ signInOpen: open, error: null });
      },

      async startSignIn(server) {
        stopPolling();
        set({ error: null });
        let code: DeviceCode;
        try {
          code = await api.signInStart(server);
        } catch (error) {
          set({ error: errorMessage(error) });
          return;
        }
        set({ signIn: code, account: { server, signedIn: false } });
        const deadline = Date.now() + code.expiresIn * 1000;
        let interval = Math.max(code.interval, 1) * 1000;
        const check = async () => {
          if (get().signIn?.deviceCode !== code.deviceCode) return;
          if (Date.now() > deadline) return set({ signIn: null, error: "The code expired. Sign in again." });
          try {
            const status = await api.signInPoll(code.deviceCode);
            if (get().signIn?.deviceCode !== code.deviceCode) return;
            if (status === "approved") {
              set({ signIn: null, account: await api.account() });
              await get().loadWorkspaces();
              return;
            }
            if (status === "denied") return set({ signIn: null, error: "Sign-in was denied in the browser." });
            if (status === "expired") return set({ signIn: null, error: "The code expired. Sign in again." });
            if (status === "slowDown") interval += 5000;
          } catch (error) {
            return set({ signIn: null, error: errorMessage(error) });
          }
          poll = timers.setTimeout(() => void check(), interval);
        };
        poll = timers.setTimeout(() => void check(), interval);
      },

      cancelSignIn() {
        stopPolling();
        set({ signIn: null });
      },

      async signOut() {
        stopPolling();
        try {
          await api.signOut();
          set({ account: await api.account(), workspaces: [], signIn: null, error: null });
        } catch (error) {
          set({ error: errorMessage(error) });
        }
      },

      async loadWorkspaces() {
        try {
          const response = await backend.request<Workspace[]>("GET", "/api/auth/organization/list");
          if (response.status === 401) {
            set({ account: { ...get().account!, signedIn: false }, error: "You were signed out. Sign in again." });
            return;
          }
          if (response.status !== 200) throw new Error(`The server answered ${response.status}.`);
          const workspaces = response.body.map(({ id, name }) => ({ id, name }));
          set({ workspaces });
          const current = get().state.workspaceId;
          if (!current || !workspaces.some((w) => w.id === current)) await get().chooseWorkspace(workspaces[0]?.id ?? "");
        } catch (error) {
          set({ error: errorMessage(error) });
        }
      },

      async chooseWorkspace(id) {
        await update((s) => setWorkspace(s, id || null));
        void get().syncNow();
      },

      async setLibrary(on) {
        await update((s) => ({ ...s, library: on }));
        if (on) void get().syncNow();
      },

      async setGuideEnabled(id, on) {
        await update((s) => setGuideEnabled(s, id, on));
        if (on) void get().syncNow();
      },

      async syncNow() {
        const { account, state } = get();
        if (!account?.signedIn || !state.workspaceId) return;
        if (!state.library && !Object.values(state.guides).some((g) => g.enabled || g.head)) return;
        if (get().running) {
          again = true;
          return;
        }
        set({ running: true });
        queued = [];
        const before = get().state;
        try {
          let next = await syncOnce(backend, before, (progress) => api.saveState(progress));
          for (const change of queued) next = change(next);
          set({ state: next, error: null, lastSynced: new Date().toISOString() });
          await api.saveState(next);
          if (wroteGuides(before, next)) onGuidesWritten();
        } catch (error) {
          if (error instanceof SyncStopped && error.status === 401) {
            set({ account: { ...account, signedIn: false } });
          }
          set({ error: errorMessage(error) });
        } finally {
          queued = [];
          set({ running: false });
        }
        if (again) {
          again = false;
          await get().syncNow();
        }
      },

      openConflict(id) {
        set({ resolving: id });
      },

      async resolve(id, resolution) {
        if (get().running) {
          set({ error: "Sync is running; try again in a moment." });
          return;
        }
        set({ running: true });
        try {
          const before = get().state;
          const next = await resolveConflict(backend, before, id, resolution);
          set({ state: next, resolving: null, error: next.guides[id]?.error ?? null });
          await api.saveState(next);
          onGuidesWritten();
        } catch (error) {
          set({ error: errorMessage(error) });
        } finally {
          set({ running: false });
        }
      },

      async submitForReview(id, note = "") {
        if (get().running) {
          set({ error: "Sync is running; try again in a moment." });
          return;
        }
        try {
          const { state, error } = await submitForReview(backend, get().state, id, note);
          set({ state, error });
          await api.saveState(state);
        } catch (error) {
          set({ error: errorMessage(error) });
        }
      },
    };
  });
}

/** Whether a run wrote any guide here (a pull or a merge). */
function wroteGuides(before: SyncState, after: SyncState): boolean {
  return Object.entries(after.guides).some(([id, g]) => {
    const old = before.guides[id];
    return !old || old.revisionId !== g.revisionId;
  });
}
