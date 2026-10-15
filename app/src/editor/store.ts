import { createStore, type StoreApi } from "zustand/vanilla";
import { errorMessage, type EditorApi } from "@/lib/ipc";
import type { Guide, GuideListItem } from "@/lib/types";
import { applyPatches, coalesce, type Command } from "@/editor/commands";
import { createDebouncer, type Timers } from "@/editor/debounce";

/** The guide being edited, as opened. */
export interface OpenDoc {
  guide: Guide;
  path: string;
  readOnly: boolean;
  migratedFrom: number | null;
  migrationError: string | null;
}

export type SaveStatus = "saved" | "pending" | "saving" | "error";

/** Where the user wanted to go when unsaved changes stopped them. */
export type LeaveIntent = { kind: "close" } | { kind: "open"; id: string } | { kind: "quit" };

export type LeaveChoice = "retry" | "discard" | "cancel";

interface HistoryEntry {
  command: Command;
  /** When the entry was last extended, for coalescing. */
  at: number;
}

export interface EditorState {
  guides: GuideListItem[];
  query: string;
  listError: string | null;

  doc: OpenDoc | null;
  opening: boolean;
  openError: string | null;
  /** The selected outline item (a step, section or note id). */
  selectedId: string | null;
  /** Saving before a close, open or quit; edits are refused meanwhile. */
  busy: boolean;
  /** Set when changes could not be saved before leaving the guide. */
  leave: LeaveIntent | null;

  undoStack: HistoryEntry[];
  redoStack: HistoryEntry[];

  /** Bumped by every change; compared with `savedRevision` for autosave. */
  revision: number;
  savedRevision: number;
  saveStatus: SaveStatus;
  saveError: string | null;
  /** A message about a failed action (e.g. an image that would not load). */
  notice: string | null;

  loadGuides(): Promise<void>;
  setQuery(query: string): Promise<void>;
  /**
   * These save first. If that fails they set `leave` and return false;
   * `resolveLeave` then retries, discards the changes or stays.
   */
  openGuide(id: string): Promise<boolean>;
  closeGuide(): Promise<boolean>;
  /**
   * Saves, then quits the app (`api.quit`). While the editor is busy the
   * quit waits and runs once it is done; repeated requests share one quit.
   */
  requestQuit(): Promise<boolean>;
  /**
   * A quit stays open (the user chose to stay, or a step failed): tells
   * the backend, so a Dock Quit or logout waiting on it is cancelled now.
   */
  stayOpen(): void;
  resolveLeave(choice: LeaveChoice): Promise<boolean>;
  select(id: string | null): void;
  notify(message: string | null): void;
  /** Applies an undoable command. Returns false if it was refused. */
  dispatch(command: Command): boolean;
  undo(): void;
  redo(): void;
  /**
   * Saves pending changes now and waits for any save in flight. Resolves
   * to false if changes are still unsaved (the save failed).
   */
  flush(): Promise<boolean>;
}

export interface EditorOptions {
  api: EditorApi;
  autosaveMs?: number;
  /** Max gap between keystrokes that still merge into one undo step. */
  coalesceMs?: number;
  now?: () => number;
  timers?: Timers;
}

export type EditorStore = StoreApi<EditorState>;

/** The state change for a command's selection hint (none when absent). */
function selection(id: string | null | undefined): Partial<EditorState> {
  return id === undefined ? {} : { selectedId: id };
}

/** True if a command would change nothing (e.g. typing the same title). */
function isNoop(command: Command): boolean {
  return JSON.stringify(command.forward) === JSON.stringify(command.inverse);
}

const cleanHistory = {
  undoStack: [],
  redoStack: [],
  revision: 0,
  savedRevision: 0,
  saveStatus: "saved",
  saveError: null,
  notice: null,
} satisfies Partial<EditorState>;

const QUIT_WAITS_FOR_SAVE = "Quitting once the guide is saved…";

export function createEditorStore({
  api,
  autosaveMs = 500,
  coalesceMs = 1000,
  now = Date.now,
  timers,
}: EditorOptions): EditorStore {
  let saving: Promise<void> | null = null;
  // The quit in progress, or one waiting for the editor to be idle.
  let quitting: Promise<boolean> | null = null;
  let queuedQuit: (() => void) | null = null;
  // Ignore list responses that a newer query has superseded.
  let listRequest = 0;

  const store = createStore<EditorState>()((set, get) => {
    const autosave = createDebouncer(() => void save(), autosaveMs, timers);

    async function save(): Promise<void> {
      if (saving) {
        // Save again once the one in flight is done.
        await saving;
        return save();
      }
      const { doc, revision, savedRevision } = get();
      if (!doc || doc.readOnly || revision === savedRevision) return;
      const guide: Guide = { ...doc.guide, updatedAt: new Date(now()).toISOString() };
      set({ saveStatus: "saving", saveError: null });
      const request = (async () => {
        try {
          await api.saveGuide(guide);
          if (get().doc?.guide.id !== guide.id) return;
          const pending = get().revision !== revision;
          set({ savedRevision: revision, saveStatus: pending ? "pending" : "saved" });
          if (pending && !autosave.pending) autosave.schedule();
        } catch (error) {
          if (get().doc?.guide.id !== guide.id) return;
          set({ saveStatus: "error", saveError: errorMessage(error) });
        }
      })().finally(() => {
        // `finally` runs after the assignment below, even if the request
        // settled synchronously.
        if (saving === request) saving = null;
      });
      saving = request;
      await request;
    }

    function isSaved(): boolean {
      const { doc, revision, savedRevision } = get();
      return !doc || doc.readOnly || revision === savedRevision;
    }

    function changed(guide: Guide, patch: Partial<EditorState>) {
      const doc = get().doc!;
      set({
        ...patch,
        doc: { ...doc, guide },
        revision: get().revision + 1,
        saveStatus: "pending",
      });
      autosave.schedule();
    }

    async function flush(): Promise<boolean> {
      autosave.cancel();
      // Edits that land during a save are saved by another round.
      for (let attempt = 0; attempt < 3 && !isSaved(); attempt++) {
        await save();
        if (get().saveStatus === "error") break;
      }
      return isSaved();
    }

    async function open(id: string) {
      set({ opening: true, openError: null });
      try {
        const opened = await api.openGuide(id);
        const first = opened.guide.items.find((item) => item.type === "step");
        set({
          doc: opened,
          opening: false,
          selectedId: first?.id ?? opened.guide.items[0]?.id ?? null,
          ...cleanHistory,
        });
      } catch (error) {
        set({ opening: false, openError: errorMessage(error) });
      }
    }

    /**
     * Tells the backend a guide is closed, which deletes step images its
     * saved copy no longer uses (kept until now for undo). Best effort.
     */
    async function release(id: string | null) {
      if (!id) return;
      try {
        await api.closeGuide(id);
      } catch (error) {
        console.error(`closing guide ${id} failed:`, error);
      }
    }

    /** Saves (unless discarding), then closes, opens or quits. */
    async function leaveTo(intent: LeaveIntent, discard = false): Promise<boolean> {
      if (get().busy) return false;
      set({ busy: true, leave: null });
      try {
        if (!discard && !(await flush())) {
          set({ leave: intent });
          return false;
        }
        autosave.cancel();
        const leaving = get().doc?.guide.id ?? null;
        switch (intent.kind) {
          case "close":
            set({ doc: null, selectedId: null, ...cleanHistory });
            await release(leaving);
            break;
          case "open":
            await open(intent.id);
            // A failed open keeps the old guide, so it stays open.
            if (get().doc?.guide.id === intent.id && leaving !== intent.id) await release(leaving);
            break;
          case "quit":
            await release(leaving);
            try {
              await api.quit();
            } catch (error) {
              // The backend closed the guide: open it again to keep editing.
              if (leaving) await open(leaving);
              get().notify(`Could not quit: ${errorMessage(error)}`);
              return false;
            }
            break;
        }
        return true;
      } finally {
        set({ busy: false });
        const run = queuedQuit;
        queuedQuit = null;
        run?.();
      }
    }

    /** Runs one quit at a time, after whatever is keeping the editor busy. */
    function quit(discard = false): Promise<boolean> {
      if (quitting) return quitting;
      const run = () => leaveTo({ kind: "quit" }, discard);
      let started: Promise<boolean>;
      if (get().busy) {
        get().notify(QUIT_WAITS_FOR_SAVE);
        started = new Promise((resolve) => {
          queuedQuit = () => {
            if (get().notice === QUIT_WAITS_FOR_SAVE) get().notify(null);
            resolve(run());
          };
        });
      } else {
        started = run();
      }
      const current = started.finally(() => {
        if (quitting === current) quitting = null;
      });
      quitting = current;
      return current;
    }

    return {
      guides: [],
      query: "",
      listError: null,
      doc: null,
      opening: false,
      openError: null,
      selectedId: null,
      busy: false,
      leave: null,
      undoStack: [],
      redoStack: [],
      revision: 0,
      savedRevision: 0,
      saveStatus: "saved",
      saveError: null,
      notice: null,

      async loadGuides() {
        const request = ++listRequest;
        try {
          const guides = await api.listGuides(get().query || undefined);
          if (request === listRequest) set({ guides, listError: null });
        } catch (error) {
          if (request === listRequest) set({ listError: errorMessage(error) });
        }
      },

      async setQuery(query) {
        set({ query });
        await get().loadGuides();
      },

      async openGuide(id) {
        if (get().doc?.guide.id === id) return true;
        return leaveTo({ kind: "open", id });
      },

      closeGuide: () => leaveTo({ kind: "close" }),

      requestQuit: () => quit(),

      stayOpen() {
        api.cancelQuit().catch((error: unknown) => console.error("cancelling the quit failed:", error));
      },

      async resolveLeave(choice) {
        const intent = get().leave;
        if (!intent) return false;
        if (choice === "cancel") {
          set({ leave: null });
          if (intent.kind === "quit") get().stayOpen();
          return false;
        }
        const discard = choice === "discard";
        return intent.kind === "quit" ? quit(discard) : leaveTo(intent, discard);
      },

      select(id) {
        set({ selectedId: id });
      },

      notify(message) {
        set({ notice: message });
      },

      dispatch(command) {
        const { doc, undoStack, busy } = get();
        if (!doc || doc.readOnly || busy || isNoop(command)) return false;
        let guide: Guide;
        try {
          guide = applyPatches(doc.guide, command.forward);
        } catch (error) {
          console.error(`command "${command.label}" failed:`, error);
          return false;
        }
        const time = now();
        const top = undoStack[undoStack.length - 1];
        const merged = top && time - top.at <= coalesceMs ? coalesce(top.command, command) : null;
        const entry = { command: merged ?? command, at: time };
        // A merge that ends where it started (typed, then deleted) undoes nothing.
        const stack = merged ? undoStack.slice(0, -1) : undoStack;
        changed(guide, {
          undoStack: merged && isNoop(merged) ? stack : [...stack, entry],
          redoStack: [],
          ...selection(command.select?.forward),
        });
        return true;
      },

      undo() {
        const { doc, undoStack, redoStack } = get();
        const entry = undoStack[undoStack.length - 1];
        if (!doc || doc.readOnly || get().busy || !entry) return;
        changed(applyPatches(doc.guide, entry.command.inverse), {
          undoStack: undoStack.slice(0, -1),
          redoStack: [...redoStack, { ...entry, at: Number.NEGATIVE_INFINITY }],
          ...selection(entry.command.select?.inverse),
        });
      },

      redo() {
        const { doc, undoStack, redoStack } = get();
        const entry = redoStack[redoStack.length - 1];
        if (!doc || doc.readOnly || get().busy || !entry) return;
        changed(applyPatches(doc.guide, entry.command.forward), {
          // An old `at`, so a redone step never merges with the next edit.
          undoStack: [...undoStack, { ...entry, at: Number.NEGATIVE_INFINITY }],
          redoStack: redoStack.slice(0, -1),
          ...selection(entry.command.select?.forward),
        });
      },

      flush,
    };
  });
  return store;
}

/** What else a quit waits for, besides the editor (e.g. a recording). */
export interface QuitSteps {
  /** Why the quit must wait (shown to the user), or null when it can go ahead. */
  waitReason(): string | null;
  /** Calls `listener` whenever the wait reason may have changed. */
  subscribe(listener: () => void): () => void;
  /** Runs before the editor saves and quits; false keeps the app open. */
  prepare(): Promise<boolean>;
}

/**
 * Returns the app's quit: it waits until `steps` no longer hold it back,
 * prepares, then saves and quits through `requestQuit`. A request while
 * one is waiting or running joins it.
 */
export function createQuitFlow(editor: EditorStore, steps: QuitSteps): () => Promise<void> {
  let current: Promise<void> | null = null;

  function idle(): Promise<void> {
    let shown = steps.waitReason();
    if (!shown) return Promise.resolve();
    editor.getState().notify(shown);
    return new Promise((resolve) => {
      const stop = steps.subscribe(() => {
        const reason = steps.waitReason();
        if (reason === shown) return;
        // Replace or clear only our own notice, not one shown since.
        const ours = editor.getState().notice === shown;
        shown = reason;
        if (ours) editor.getState().notify(reason);
        if (reason) return;
        stop();
        resolve();
      });
    });
  }

  async function attempt() {
    await idle();
    if (!(await steps.prepare())) {
      editor.getState().stayOpen();
      return;
    }
    await editor.getState().requestQuit();
  }

  return () => {
    current ??= attempt()
      .catch((error: unknown) => {
        editor.getState().notify(`Could not quit: ${errorMessage(error)}`);
        editor.getState().stayOpen();
      })
      .finally(() => {
        current = null;
      });
    return current;
  };
}
