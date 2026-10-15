import { createStore, type StoreApi } from "zustand/vanilla";
import type { Patch } from "@/editor/commands";
import type { EditorStore } from "@/editor/store";
import {
  AI_FINISHED_EVENT,
  AI_GUIDE_EVENT,
  AI_STARTED_EVENT,
  AI_STEP_EVENT,
  errorMessage,
  type AiApi,
} from "@/lib/ipc";
import type { AiFinishedEvent, AiGuideEvent, AiStartedEvent, AiStepEvent, Guide, Settings, Step } from "@/lib/types";

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
 */

export type AiPhase = "idle" | "starting" | "running" | "stopping";

export interface AiRunState {
  phase: AiPhase;
  /** The guide being written (null when idle). */
  guideId: string | null;
  runId: number | null;
  /** Steps still being written: UI state only, never written into the guide. */
  generating: ReadonlySet<string>;
  /**
   * Writes the open guide: saves it, then starts a run. Resolves to whether
   * a run started.
   */
  start(options?: StartOptions): Promise<boolean>;
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

const idle = { phase: "idle", guideId: null, runId: null, generating: new Set<string>() } satisfies Partial<AiRunState>;

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
      if (event.outcome === "failed") tally.failed.add(event.stepId);
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
      const tally = run.tally;
      end();
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

    return {
      ...idle,

      async start({ auto = false, guideId: wanted, stepIds, guidePass } = {}) {
        const doc = editor.getState().doc;
        if (run || !doc || doc.readOnly) return false;
        const guideId = doc.guide.id;
        if (wanted !== undefined && wanted !== guideId) return false;
        const current: Run = {
          guideId,
          runId: null,
          early: [],
          unlisten: [],
          stopTimer: null,
          tally: { written: new Set(), failed: new Set(), keptLive: new Set(), guideWritten: false, guideError: null },
          unwatch: editor.subscribe((state) => {
            if (run === current && state.doc?.guide.id !== guideId) abandon();
          }),
        };
        run = current;
        // Until `ai://started` names them: the steps asked for (default: all) that are not edited.
        const pending = doc.guide.items.filter(
          (item) => item.type === "step" && item.aiState.status !== "edited" && (!stepIds || stepIds.includes(item.id)),
        );
        set({ phase: "starting", guideId, runId: null, generating: new Set(pending.map((item) => item.id)) });

        const fail = (message: string | null) => {
          if (run !== current) return false;
          end();
          if (message) editor.getState().notify(message);
          return false;
        };

        // The backend writes the guide as last saved.
        if (!(await editor.getState().flush())) {
          return fail(auto ? null : "Save the guide before writing with AI: it could not be saved.");
        }
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
          stepIds === undefined && guidePass === undefined
            ? undefined
            : { ...(stepIds && { stepIds }), ...(guidePass !== undefined && { guidePass }) };
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
