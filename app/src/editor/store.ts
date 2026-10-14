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

  loadGuides(): Promise<void>;
  setQuery(query: string): Promise<void>;
  /**
   * These save first. If that fails they set `leave` and return false;
   * `resolveLeave` then retries, discards the changes or stays.
   */
  openGuide(id: string): Promise<boolean>;
  closeGuide(): Promise<boolean>;
  /** Saves, then quits the app (`api.quit`). */
  requestQuit(): Promise<boolean>;
  resolveLeave(choice: LeaveChoice): Promise<boolean>;
  select(id: string | null): void;
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
} satisfies Partial<EditorState>;

export function createEditorStore({
  api,
  autosaveMs = 500,
  coalesceMs = 1000,
  now = Date.now,
  timers,
}: EditorOptions): EditorStore {
  let saving: Promise<void> | null = null;
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
        switch (intent.kind) {
          case "close":
            set({ doc: null, selectedId: null, ...cleanHistory });
            break;
          case "open":
            await open(intent.id);
            break;
          case "quit":
            await api.quit();
            break;
        }
        return true;
      } finally {
        set({ busy: false });
      }
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

      requestQuit: () => leaveTo({ kind: "quit" }),

      async resolveLeave(choice) {
        const intent = get().leave;
        if (!intent) return false;
        if (choice === "cancel") {
          set({ leave: null });
          return false;
        }
        return leaveTo(intent, choice === "discard");
      },

      select(id) {
        set({ selectedId: id });
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
        });
      },

      flush,
    };
  });
  return store;
}
