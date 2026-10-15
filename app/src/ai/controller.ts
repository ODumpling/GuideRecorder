import { createStore, type StoreApi } from "zustand/vanilla";
import type { Command, Patch } from "@/editor/commands";
import type { EditorStore } from "@/editor/store";
import {
  AI_FINISHED_EVENT,
  AI_GUIDE_EVENT,
  AI_STARTED_EVENT,
  AI_STEP_EVENT,
  errorMessage,
  type AiApi,
} from "@/lib/ipc";
import type {
  AiFinishedEvent,
  AiGuideEvent,
  AiRewriteResult,
  AiStartedEvent,
  AiState,
  AiStepEvent,
  Guide,
  RewriteStyleArg,
  Settings,
  Step,
} from "@/lib/types";

/**
 * Runs AI writing for the open guide (`ai_generate`) and applies its
 * `ai://` events to the editor's live copy, which stays the only writer of
 * the guide: the backend writes a snapshot and only streams results.
 *
 * Results are applied with `applyUntracked`: they autosave but are not in
 * the undo history (no "AI wrote this guide" entry). A run streams over
 * time, interleaved with the user's own edits, so one grouped entry could
 * not be undone without undoing those too. AI never writes a step or guide
 * field the user has edited, and `applyUntracked` drops the undo and redo
 * entries for the fields it writes, so undo never brings older text back
 * over AI text.
 *
 * Explicit actions (Regenerate a step, Rewrite a step, Regenerate the guide
 * details) are the user's request: they may replace edited text, and their
 * result is ONE tracked change (`dispatch`), which Undo reverts with the
 * step's or guide's previous `aiState` (so `edited` comes back too).
 *
 * A later background run may still write a step or the guide details an
 * explicit action wrote: they are `ai`, not `edited`, so the 603e000 rule
 * applies and `applyUntracked` drops that action's undo entry, so undo
 * never brings the older text back over the newer AI text. By design.
 *
 * An explicit action belongs to the guide as it was open when it started:
 * closing it, or opening another, ends the action at once (a Regenerate
 * run is cancelled), and a late result is dropped even if the same guide
 * was reopened meanwhile.
 *
 * Conflict rule: one AI action per editor at a time. Explicit actions are
 * refused (and disabled, with a hint: {@link BUSY_HINT}) while a background
 * run is going, and "Write with AI" is refused (disabled) while an explicit
 * action is waiting. Nothing is queued.
 */

export type AiPhase = "idle" | "starting" | "running" | "stopping";

/** An explicit AI action waiting for its result. */
export interface ExplicitAction {
  kind: "regenerate" | "rewrite" | "guide";
  guideId: string;
  /** The step (null for the guide details). */
  stepId: string | null;
}

/** Why explicit actions are disabled while a background run is going. */
export const BUSY_HINT = "AI is writing this guide. Wait for it to finish, or stop it.";

export interface AiRunState {
  phase: AiPhase;
  /** The guide being written (null when idle). */
  guideId: string | null;
  runId: number | null;
  /** Steps still being written: UI state only, never written into the guide. */
  generating: ReadonlySet<string>;
  /** The explicit action waiting for its result, if any (it may also be the run). */
  explicit: ExplicitAction | null;
  /**
   * Writes the open guide in the background: saves it, then starts a run.
   * Resolves to whether a run started.
   */
  start(options?: StartOptions): Promise<boolean>;
  /** Regenerates one step, edited or not, as one undoable change. Resolves to whether it started. */
  regenerateStep(stepId: string): Promise<boolean>;
  /** Regenerates the guide details (title, intro, …), edited or not, as one undoable change. */
  regenerateGuide(): Promise<boolean>;
  /** Rewrites one step in `style`, as one undoable change. Resolves to whether it was applied. */
  rewriteStep(stepId: string, style: RewriteStyleArg): Promise<boolean>;
  /**
   * Stops the run (Stop); the summary follows when it has stopped, or after
   * `stopTimeoutMs` if the backend never says so. While the run is still
   * starting it ends at once, and a run id that comes later is cancelled.
   */
  stop(): Promise<void>;
}

export interface StartOptions {
  /** After a recording: a missing key is not reported. */
  auto?: boolean;
  /** Only if this guide is (still) the open one; otherwise nothing starts. */
  guideId?: string;
  /** Passed to `ai_generate`. Default: every step not edited. */
  stepIds?: string[];
  /** Passed to `ai_generate`. Default: true. */
  guidePass?: boolean;
}

export type AiStore = StoreApi<AiRunState>;

type Unlisten = () => void;
/** Subscribes to an app event, like Tauri's `listen`. */
export type Listen = <T>(event: string, handler: (payload: T) => void) => Promise<Unlisten>;

export interface AiOptions {
  api: AiApi;
  editor: EditorStore;
  listen: Listen;
  /** How long to wait for `ai://finished` after a Stop before giving up on it. */
  stopTimeoutMs?: number;
}

/** The message `ai_generate` rejects with when no API key is set. */
export const NO_KEY_MESSAGE = "Add an API key in Settings to write with AI.";

const idle = {
  phase: "idle",
  guideId: null,
  runId: null,
  generating: new Set<string>(),
  explicit: null,
} satisfies Partial<AiRunState>;

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** Counts for the finish notice: what this editor actually did with the results. */
export interface RunCounts {
  /** Steps written (distinct: a consistency edit can write a step again). */
  written: number;
  failed: number;
  /** Left alone because the user had edited them (before or during the run). */
  kept: number;
  guideWritten: boolean;
  guideError: string | null;
}

/** What a run did so far, by step id. */
interface Tally {
  written: Set<string>;
  failed: Set<string>;
  /** Written by the run but edited here meanwhile, so left alone. */
  keptLive: Set<string>;
  guideWritten: boolean;
  guideError: string | null;
  /** The last step error, for an explicit action's notice. */
  stepError: string | null;
}

/** `skippedEdited`: the backend's count of steps it left alone as edited (from `ai://finished`). */
function counts(tally: Tally, skippedEdited: number): RunCounts {
  return {
    written: tally.written.size,
    failed: tally.failed.size,
    kept: skippedEdited + tally.keptLive.size,
    guideWritten: tally.guideWritten,
    guideError: tally.guideError,
  };
}

/** The short notice shown when a run ends. */
export function summary(counts: RunCounts, cancelled: boolean): string {
  const parts: string[] = [];
  const wrote = [counts.written ? plural(counts.written, "step") : "", counts.guideWritten ? "the guide details" : ""]
    .filter(Boolean)
    .join(" and ");
  if (cancelled) parts.push(wrote ? `Stopped writing after ${wrote}.` : "Stopped writing.");
  else parts.push(wrote ? `AI wrote ${wrote}.` : "AI wrote nothing new.");
  if (counts.failed) parts.push(`${plural(counts.failed, "step")} failed.`);
  if (counts.guideError) parts.push(`The guide details failed: ${counts.guideError.replace(/\.?$/, ".")}`);
  if (counts.kept) parts.push(`Kept your edits to ${plural(counts.kept, "step")}.`);
  return parts.join(" ");
}

const UNSAVED_MESSAGE = "Save the guide before writing with AI: it could not be saved.";

/** The notice after an explicit action; null when it went through. */
function explicitNotice(action: ExplicitAction, tally: Tally, cancelled: boolean): string | null {
  if (cancelled) return "Stopped writing.";
  const reason = (error: string | null) => (error ? `: ${error.replace(/\.?$/, ".")}` : ".");
  if (action.kind === "guide") {
    return tally.guideWritten ? null : `AI could not write the guide details${reason(tally.guideError)}`;
  }
  return tally.written.size ? null : tally.failed.size ? `AI could not write this step${reason(tally.stepError)}` : null;
}

function findStep(guide: Guide, id: string): Step | null {
  const item = guide.items.find((other) => other.id === id);
  return item?.type === "step" ? item : null;
}

/** The patches that apply a step result to the live step; null to leave it. */
export function stepPatches(live: Step | null, event: AiStepEvent): Patch[] | null {
  if (!live || live.aiState.status === "edited") return null;
  const id = live.id;
  if (event.outcome === "applied" && event.step) {
    const aiState = {
      ...event.step.aiState,
      redactionSuggestions: event.step.aiState.redactionSuggestions ?? event.redactionSuggestions,
    };
    return [
      { op: "setItem", id, key: "title", value: event.step.title },
      { op: "setItem", id, key: "description", value: event.step.description },
      { op: "setItem", id, key: "aiState", value: aiState },
    ];
  }
  if (event.outcome === "failed") {
    // The text stays; only the error is recorded.
    const aiState = event.step ? event.step.aiState : { ...live.aiState, error: event.error };
    return [{ op: "setItem", id, key: "aiState", value: { ...aiState, error: aiState.error ?? event.error } }];
  }
  return null;
}

/** The patches that apply the guide pass to the live guide; null to leave it. */
export function guidePatches(live: Guide, event: AiGuideEvent): Patch[] | null {
  if (live.aiState?.status === "edited") return null;
  if (event.fields) {
    const { title, intro, prerequisites, summary, tags } = event.fields;
    return [
      { op: "setGuide", key: "title", value: title },
      { op: "setGuide", key: "intro", value: intro },
      { op: "setGuide", key: "prerequisites", value: prerequisites },
      { op: "setGuide", key: "summary", value: summary },
      { op: "setGuide", key: "tags", value: tags },
      { op: "setGuide", key: "aiState", value: event.aiState },
    ];
  }
  // Failed: record the error, leave the fields.
  if (event.error) return [{ op: "setGuide", key: "aiState", value: { ...event.aiState, error: event.error } }];
  return null;
}

/** The step's text and `aiState` as one undoable command (an explicit action's result). */
export function stepResultCommand(
  live: Step,
  label: string,
  result: { title: string; description: string; aiState: AiState },
): Command {
  const id = live.id;
  return {
    label,
    forward: [
      { op: "setItem", id, key: "title", value: result.title },
      { op: "setItem", id, key: "description", value: result.description },
      { op: "setItem", id, key: "aiState", value: result.aiState },
    ],
    inverse: [
      { op: "setItem", id, key: "aiState", value: live.aiState },
      { op: "setItem", id, key: "description", value: live.description },
      { op: "setItem", id, key: "title", value: live.title },
    ],
    select: { forward: id, inverse: id },
  };
}

/** The guide pass's fields and `aiState` as one undoable command. */
export function guideResultCommand(live: Guide, event: AiGuideEvent & { fields: NonNullable<AiGuideEvent["fields"]> }): Command {
  const keys = ["title", "intro", "prerequisites", "summary", "tags"] as const;
  return {
    label: "Regenerate guide details",
    forward: [
      ...keys.map((key): Patch => ({ op: "setGuide", key, value: event.fields[key] })),
      { op: "setGuide", key: "aiState", value: event.aiState },
    ],
    inverse: [
      { op: "setGuide", key: "aiState", value: live.aiState },
      ...keys.map((key): Patch => ({ op: "setGuide", key, value: live[key] })),
    ],
  };
}

/** A run being started or running. */
interface Run {
  guideId: string;
  /** null until `ai_generate` returns it. */
  runId: number | null;
  /** Events that came before the run id did. */
  early: { event: string; payload: { runId: number } }[];
  unlisten: Unlisten[];
  stopTimer: ReturnType<typeof setTimeout> | null;
  tally: Tally;
  /** Stops watching the editor for the guide closing. */
  unwatch: Unlisten;
  /** Set for an explicit (forced) run: results are tracked, edited or not. */
  explicit: ExplicitAction | null;
}

interface LaunchOptions extends StartOptions {
  force?: boolean;
  explicit?: ExplicitAction;
}

export function createAiStore({ api, editor, listen, stopTimeoutMs = 10_000 }: AiOptions): AiStore {
  /** The run being started or running. */
  let run: Run | null = null;

  const store = createStore<AiRunState>()((set, get) => {
    /** Ends the run locally: no more events are taken. */
    function end() {
      if (!run) return;
      for (const stop of run.unlisten) stop();
      if (run.stopTimer) clearTimeout(run.stopTimer);
      run.unwatch();
      run = null;
      set(idle);
    }

    function cancelRun(runId: number) {
      api.cancel(runId).catch((error: unknown) => console.error(`cancelling AI run ${runId} failed:`, error));
    }

    function notGenerating(stepIds: string[]) {
      const generating = new Set(get().generating);
      for (const id of stepIds) generating.delete(id);
      set({ generating });
    }

    function onStarted(event: AiStartedEvent) {
      set({ generating: new Set(event.stepIds) });
    }

    function onStep(event: AiStepEvent) {
      notGenerating([event.stepId]);
      if (!run) return;
      const doc = editor.getState().doc;
      if (doc?.guide.id !== event.guideId) return;
      const live = findStep(doc.guide, event.stepId);
      const tally = run.tally;
      if (event.outcome === "failed") {
        tally.failed.add(event.stepId);
        tally.stepError = event.error;
      }
      if (run.explicit) {
        if (live && event.outcome === "applied" && event.step) {
          const result = {
            title: event.step.title,
            description: event.step.description,
            aiState: {
              ...event.step.aiState,
              redactionSuggestions: event.step.aiState.redactionSuggestions ?? event.redactionSuggestions,
            },
          };
          if (editor.getState().dispatch(stepResultCommand(live, "Regenerate step", result))) {
            tally.written.add(event.stepId);
          }
          return;
        }
        // Failed: the text stays and keeps its status; only the error is recorded.
        if (live && event.outcome === "failed") {
          const error = event.step?.aiState.error ?? event.error;
          editor.getState().applyUntracked([{ op: "setItem", id: live.id, key: "aiState", value: { ...live.aiState, error } }]);
        }
        return;
      }
      // Edited here during the run (`skippedEdited` ones are in the backend's count).
      if (event.outcome === "applied" && live?.aiState.status === "edited") tally.keptLive.add(event.stepId);
      const patches = stepPatches(live, event);
      if (patches && editor.getState().applyUntracked(patches) && event.outcome === "applied") {
        tally.written.add(event.stepId);
      }
    }

    function onGuide(event: AiGuideEvent) {
      if (!run) return;
      const doc = editor.getState().doc;
      if (doc?.guide.id !== event.guideId) return;
      if (event.error) run.tally.guideError = event.error;
      if (run.explicit) {
        if (event.fields) {
          const command = guideResultCommand(doc.guide, event as Parameters<typeof guideResultCommand>[1]);
          if (editor.getState().dispatch(command)) run.tally.guideWritten = true;
        } else if (event.error) {
          const aiState = { ...(doc.guide.aiState ?? { status: "template" as const }), error: event.error };
          editor.getState().applyUntracked([{ op: "setGuide", key: "aiState", value: aiState }]);
        }
        return;
      }
      const patches = guidePatches(doc.guide, event);
      if (patches && editor.getState().applyUntracked(patches) && event.fields) run.tally.guideWritten = true;
    }

    function onFinished(event: AiFinishedEvent) {
      if (event.error) {
        // Broke off: what was written stays; the counts mean nothing.
        if (!run) return;
        end();
        editor.getState().notify(`AI writing failed: ${event.error.replace(/\.?$/, ".")}`);
        return;
      }
      finish(event.cancelled, event.skippedEdited);
    }

    /** Ends the run with its notice; `skippedEdited` unknown (0) when no finish came. */
    function finish(cancelled: boolean, skippedEdited = 0) {
      if (!run) return;
      const { tally, explicit } = run;
      end();
      if (explicit) {
        // The change shows for itself; only a failure is worth a notice.
        const message = explicitNotice(explicit, tally, cancelled);
        if (message) editor.getState().notify(message);
        return;
      }
      editor.getState().notify(summary(counts(tally, skippedEdited), cancelled));
    }

    const handlers: Record<string, (payload: never) => void> = {
      [AI_STARTED_EVENT]: onStarted,
      [AI_STEP_EVENT]: onStep,
      [AI_GUIDE_EVENT]: onGuide,
      [AI_FINISHED_EVENT]: onFinished,
    };

    /** Routes an event of this run; holds it while the run id is not known yet. */
    function receive(event: string, payload: { runId: number; guideId: string }, current: Run) {
      if (!run || run !== current || payload.guideId !== run.guideId) return;
      if (run.runId === null) {
        run.early.push({ event, payload });
        return;
      }
      if (payload.runId !== run.runId) return;
      handlers[event](payload as never);
    }

    /** Stops for good: the guide closed or another opened mid-run. */
    function abandon() {
      if (!run) return;
      if (run.runId !== null) cancelRun(run.runId);
      end();
    }

    function begin(runId: number, current: Run) {
      current.runId = runId;
      set({ phase: "running", runId });
      for (const { event, payload } of current.early.splice(0)) {
        if (run !== current) break;
        if (payload.runId === runId) handlers[event](payload as never);
      }
    }

    async function requestStop(current: Run) {
      const runId = current.runId!;
      set({ phase: "stopping" });
      current.stopTimer = setTimeout(() => {
        // `ai://finished` never came: don't stay stuck on Stop.
        if (run === current) finish(true);
      }, stopTimeoutMs);
      try {
        await api.cancel(runId);
      } catch (error) {
        console.error(`cancelling AI run ${runId} failed:`, error);
        if (run === current) finish(true);
      }
    }

    /** Starts a run: a background one, or an explicit (forced) one. */
    async function launch({ auto = false, guideId: wanted, stepIds, guidePass, force, explicit }: LaunchOptions) {
      const doc = editor.getState().doc;
      // One AI action at a time (see the conflict rule above).
      if (run || get().explicit || !doc || doc.readOnly) return false;
      const guideId = doc.guide.id;
      if (wanted !== undefined && wanted !== guideId) return false;
      const current: Run = {
        guideId,
        runId: null,
        early: [],
        unlisten: [],
        stopTimer: null,
        tally: {
          written: new Set(),
          failed: new Set(),
          keptLive: new Set(),
          guideWritten: false,
          guideError: null,
          stepError: null,
        },
        unwatch: editor.subscribe((state) => {
          if (run === current && state.doc?.guide.id !== guideId) abandon();
        }),
        explicit: explicit ?? null,
      };
      run = current;
      // Until `ai://started` names them: the steps asked for (default: all) that are not edited (unless forced).
      const pending = doc.guide.items.filter(
        (item) =>
          item.type === "step" &&
          (force || item.aiState.status !== "edited") &&
          (!stepIds || stepIds.includes(item.id)),
      );
      set({
        phase: "starting",
        guideId,
        runId: null,
        generating: new Set(pending.map((item) => item.id)),
        explicit: explicit ?? null,
      });

      const fail = (message: string | null) => {
        if (run !== current) return false;
        end();
        if (message) editor.getState().notify(message);
        return false;
      };

      // The backend writes the guide as last saved.
      if (!(await editor.getState().flush())) return fail(auto ? null : UNSAVED_MESSAGE);
      if (run !== current) return false;
      try {
        current.unlisten = await Promise.all(
          Object.keys(handlers).map((event) =>
            listen<{ runId: number; guideId: string }>(event, (payload) => receive(event, payload, current)),
          ),
        );
      } catch (error) {
        return fail(`Could not write with AI: ${errorMessage(error)}`);
      }
      if (run !== current) {
        for (const stop of current.unlisten) stop();
        return false;
      }
      let runId: number;
      const options =
        stepIds === undefined && guidePass === undefined && !force
          ? undefined
          : { ...(stepIds && { stepIds }), ...(guidePass !== undefined && { guidePass }), ...(force && { force }) };
      try {
        ({ runId } = await (options ? api.generate(guideId, options) : api.generate(guideId)));
      } catch (error) {
        const message = errorMessage(error);
        return fail(auto && message === NO_KEY_MESSAGE ? null : message);
      }
      if (run !== current) {
        // Closed or stopped while starting.
        cancelRun(runId);
        return false;
      }
      begin(runId, current);
      return true;
    }

    return {
      ...idle,

      start: (options = {}) => launch(options),

      regenerateStep(stepId) {
        const doc = editor.getState().doc;
        if (!doc || !findStep(doc.guide, stepId)) return Promise.resolve(false);
        return launch({
          stepIds: [stepId],
          guidePass: false,
          force: true,
          explicit: { kind: "regenerate", guideId: doc.guide.id, stepId },
        });
      },

      regenerateGuide() {
        const doc = editor.getState().doc;
        if (!doc) return Promise.resolve(false);
        return launch({
          stepIds: [],
          guidePass: true,
          force: true,
          explicit: { kind: "guide", guideId: doc.guide.id, stepId: null },
        });
      },

      async rewriteStep(stepId, style) {
        const doc = editor.getState().doc;
        if (run || get().explicit || !doc || doc.readOnly || !findStep(doc.guide, stepId)) return false;
        const guideId = doc.guide.id;
        const action: ExplicitAction = { kind: "rewrite", guideId, stepId };
        set({ explicit: action, generating: new Set([stepId]) });
        // The action holds while its guide stays open. Once the guide is
        // closed or another opened (even if it is reopened later), it is
        // over: the state clears at once, and a late result is dropped.
        let current = true;
        const leave = () => {
          if (!current) return;
          current = false;
          unwatch();
          if (get().explicit === action) set({ explicit: null, generating: new Set() });
        };
        const unwatch = editor.subscribe((state) => {
          if (state.doc?.guide.id !== guideId) leave();
        });
        const done = (message: string | null) => {
          if (!current) return;
          leave();
          if (message) editor.getState().notify(message);
        };
        // The backend rewrites the step as last saved.
        if (!(await editor.getState().flush())) {
          done(UNSAVED_MESSAGE);
          return false;
        }
        if (!current) return false;
        let result: AiRewriteResult;
        try {
          result = await api.rewriteStep(guideId, stepId, style);
        } catch (error) {
          done(errorMessage(error));
          return false;
        }
        if (!current) return false;
        const live = editor.getState().doc;
        const step = live ? findStep(live.guide, stepId) : null;
        // The step deleted meanwhile: nothing to apply it to.
        const applied =
          !!step &&
          editor.getState().dispatch(
            stepResultCommand(step, "Rewrite step", { ...result, description: result.description ?? "" }),
          );
        done(null);
        return applied;
      },

      async stop() {
        const current = run;
        if (!current || get().phase === "stopping") return;
        if (current.runId === null) {
          // Nothing has been written yet: end now; `start` cancels the run
          // if its id still comes.
          finish(true);
          return;
        }
        await requestStop(current);
      },
    };
  });
  return store;
}

/**
 * Writes the newly recorded guide `guideId` when the settings ask for it (AI on, auto
 * run on) and a key is set; otherwise does nothing, silently.
 */
export async function autoRun(
  store: AiStore,
  guideId: string,
  settings: Settings | null,
  apiKeyStored: () => Promise<boolean>,
): Promise<boolean> {
  if (!settings?.ai.enabled || !settings.ai.autoRun) return false;
  try {
    if (!(await apiKeyStored())) return false;
  } catch {
    return false;
  }
  // Only the recorded guide, if it is still the open one after the wait.
  return store.getState().start({ auto: true, guideId });
}
