import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { autoRun, createAiStore, NO_KEY_MESSAGE, summary, type Listen } from "@/ai/controller";
import { setGuideField, setItemText } from "@/editor/commands";
import { createEditorStore } from "@/editor/store";
import { guide } from "@/editor/test-fixtures";
import type { AiApi, EditorApi } from "@/lib/ipc";
import type { AiGuideEvent, AiStepEvent, Guide, Settings, Step } from "@/lib/types";

function fakeEvents() {
  const handlers = new Map<string, Set<(payload: unknown) => void>>();
  const listen: Listen = async (event, handler) => {
    const set = handlers.get(event) ?? new Set();
    handlers.set(event, set);
    const h = handler as (payload: unknown) => void;
    set.add(h);
    return () => set.delete(h);
  };
  const emit = (event: string, payload: unknown) => {
    for (const handler of handlers.get(event) ?? []) handler(payload);
  };
  const listeners = () => [...handlers.values()].reduce((n, set) => n + set.size, 0);
  return { listen, emit, listeners };
}

async function setup({ generate }: { generate?: AiApi["generate"] } = {}) {
  const saved: Guide[] = [];
  const editorApi: EditorApi = {
    listGuides: vi.fn(async () => []),
    openGuide: vi.fn(async (id: string) => ({
      guide: { ...guide(), id },
      path: "/lib/Guide.guidesya",
      readOnly: false,
      migratedFrom: null,
      migrationError: null,
    })),
    saveGuide: vi.fn(async (g: Guide) => void saved.push(g)),
    closeGuide: vi.fn(async () => []),
    stepImage: vi.fn(async () => new ArrayBuffer(0)),
    quit: vi.fn(async () => {}),
    cancelQuit: vi.fn(async () => {}),
  };
  const editor = createEditorStore({ api: editorApi });
  await editor.getState().openGuide("G1");
  const events = fakeEvents();
  let nextRun = 7;
  const api = {
    generate: vi.fn(generate ?? (async () => ({ runId: nextRun++ }))),
    cancel: vi.fn(async () => {}),
  };
  const ai = createAiStore({ api, editor, listen: events.listen });
  const e = () => editor.getState();
  return { editor, e, ai, api, events, saved };
}

const stepOf = (g: Guide, id: string) => g.items.find((item) => item.id === id) as Step;

function stepEvent(runId: number, id: string, extra: Partial<AiStepEvent> = {}): AiStepEvent {
  const base = stepOf(guide(), id);
  return {
    runId,
    guideId: "G1",
    stepId: id,
    outcome: "applied",
    step: {
      ...base,
      title: `AI title ${id}`,
      description: `AI text ${id}`,
      aiState: { status: "ai", model: "claude-haiku-5-5", promptVersion: "step.v1" },
    },
    redactionSuggestions: [],
    error: null,
    ...extra,
  };
}

function guideEvent(runId: number): AiGuideEvent {
  return {
    runId,
    guideId: "G1",
    fields: { title: "AI guide", intro: "AI intro", prerequisites: "", summary: "AI summary", tags: ["ai"] },
    aiState: { status: "ai", model: "claude-sonnet-5-5", promptVersion: "guide.v1" },
    error: null,
  };
}

const finished = (runId: number, cancelled = false, skippedEdited = 0) => ({
  runId,
  guideId: "G1",
  cancelled,
  applied: 0,
  failed: 0,
  skippedEdited,
  usage: { inputTokens: 0, outputTokens: 0 },
  error: null as string | null,
});

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe("AI run controller", () => {
  it("saves first, then writes the steps and the guide, outside the undo history", async () => {
    const { e, ai, api, events, saved } = await setup();
    e().dispatch(setItemText(e().doc!.guide, "C", "description", "Mine"));
    expect(await ai.getState().start()).toBe(true);
    // Flushed before generating: the backend writes the guide as saved.
    expect(saved.at(-1)).toMatchObject({ items: expect.arrayContaining([expect.objectContaining({ id: "C", description: "Mine" })]) });
    expect(api.generate).toHaveBeenCalledWith("G1");
    expect(ai.getState()).toMatchObject({ phase: "running", runId: 7 });
    // Until started: the steps that are not edited.
    expect([...ai.getState().generating]).toEqual(["A", "B"]);

    events.emit("ai://started", { runId: 7, guideId: "G1", stepIds: ["A", "B"] });
    events.emit("ai://step", stepEvent(7, "A"));
    expect([...ai.getState().generating]).toEqual(["B"]);
    expect(stepOf(e().doc!.guide, "A")).toMatchObject({ title: "AI title A", description: "AI text A", aiState: { status: "ai" } });
    // Never written into the guide as `generating`.
    expect(stepOf(e().doc!.guide, "B").aiState.status).toBe("template");
    events.emit("ai://step", stepEvent(7, "B"));
    events.emit("ai://guide", guideEvent(7));
    expect(e().doc!.guide).toMatchObject({ title: "AI guide", intro: "AI intro", summary: "AI summary", tags: ["ai"] });
    expect(e().undoStack).toHaveLength(1);
    expect(e().saveStatus).toBe("pending");

    events.emit("ai://finished", finished(7));
    expect(ai.getState()).toMatchObject({ phase: "idle", runId: null });
    expect(ai.getState().generating.size).toBe(0);
    expect(e().notice).toBe("AI wrote 2 steps and the guide details.");
    expect(events.listeners()).toBe(0);
  });

  it("skips a step the user edited during the run", async () => {
    const { e, ai, events } = await setup();
    await ai.getState().start();
    e().dispatch(setItemText(e().doc!.guide, "A", "title", "My title"));
    events.emit("ai://step", stepEvent(7, "A"));
    events.emit("ai://step", stepEvent(7, "B", { outcome: "skippedEdited", step: null }));
    expect(stepOf(e().doc!.guide, "A")).toMatchObject({ title: "My title", description: "", aiState: { status: "edited" } });
    // The backend counts B; A is counted here.
    events.emit("ai://finished", finished(7, false, 1));
    expect(e().notice).toBe("AI wrote nothing new. Kept your edits to 2 steps.");
  });

  it("keeps guide fields the user edited", async () => {
    const { e, ai, events } = await setup();
    await ai.getState().start();
    e().dispatch(setGuideField(e().doc!.guide, "summary", "My summary"));
    events.emit("ai://guide", guideEvent(7));
    expect(e().doc!.guide).toMatchObject({ title: "Guide", intro: "", summary: "My summary", aiState: { status: "edited" } });
  });

  it("records a failed step's error and keeps its text", async () => {
    const { e, ai, events } = await setup();
    await ai.getState().start();
    const failed = stepEvent(7, "A", { outcome: "failed", error: "rate limited" });
    failed.step = { ...stepOf(guide(), "A"), aiState: { status: "template", error: "rate limited" } };
    events.emit("ai://step", failed);
    events.emit("ai://step", stepEvent(7, "B", { outcome: "failed", step: null, error: "timed out" }));
    expect(stepOf(e().doc!.guide, "A")).toMatchObject({ title: "Step A", aiState: { status: "template", error: "rate limited" } });
    expect(stepOf(e().doc!.guide, "B").aiState.error).toBe("timed out");
    events.emit("ai://finished", finished(7));
    expect(e().notice).toBe("AI wrote nothing new. 2 steps failed.");
  });

  it("shows a failure instead of the summary when the run broke off", async () => {
    const { e, ai, events } = await setup();
    await ai.getState().start();
    events.emit("ai://step", stepEvent(7, "A"));
    events.emit("ai://finished", { ...finished(7), error: "Writing stopped because of an internal error." });
    expect(ai.getState()).toMatchObject({ phase: "idle", runId: null });
    expect(ai.getState().generating.size).toBe(0);
    expect(e().notice).toBe("AI writing failed: Writing stopped because of an internal error.");
    // Results already sent still stand.
    expect(stepOf(e().doc!.guide, "A").title).toBe("AI title A");
  });

  it("ignores events of other runs and other guides", async () => {
    const { e, ai, events } = await setup();
    await ai.getState().start();
    events.emit("ai://step", stepEvent(99, "A"));
    events.emit("ai://step", { ...stepEvent(7, "A"), guideId: "OTHER" });
    events.emit("ai://guide", guideEvent(99));
    events.emit("ai://finished", finished(99));
    expect(stepOf(e().doc!.guide, "A").title).toBe("Step A");
    expect(e().doc!.guide.title).toBe("Guide");
    expect(ai.getState().phase).toBe("running");
  });

  it("takes events that arrive before the run id", async () => {
    let early: (() => void) | null = null;
    const { e, ai, events } = await setup({
      generate: async () => {
        early!();
        return { runId: 3 };
      },
    });
    early = () => {
      events.emit("ai://step", stepEvent(2, "B")); // an older run
      events.emit("ai://step", stepEvent(3, "A"));
    };
    await ai.getState().start();
    expect(stepOf(e().doc!.guide, "A").title).toBe("AI title A");
    expect(stepOf(e().doc!.guide, "B").title).toBe("Step B");
  });

  it("Stop cancels the run and reports what was written", async () => {
    const { e, ai, api, events } = await setup();
    await ai.getState().start();
    events.emit("ai://step", stepEvent(7, "A"));
    await ai.getState().stop();
    expect(api.cancel).toHaveBeenCalledWith(7);
    expect(ai.getState().phase).toBe("stopping");
    events.emit("ai://step", stepEvent(7, "B", { outcome: "cancelled", step: null }));
    events.emit("ai://finished", finished(7, true));
    expect(ai.getState().phase).toBe("idle");
    expect(stepOf(e().doc!.guide, "B").title).toBe("Step B");
    expect(e().notice).toBe("Stopped writing after 1 step.");
  });

  it("is not stuck when no finish comes after Stop", async () => {
    const { ai } = await setup();
    await ai.getState().start();
    await ai.getState().stop();
    await vi.advanceTimersByTimeAsync(10_000);
    expect(ai.getState().phase).toBe("idle");
  });

  it("Stop while ai_generate is pending ends at once and cancels the late run", async () => {
    let resolve: ((v: { runId: number }) => void) | null = null;
    const { e, ai, api, events } = await setup({ generate: () => new Promise((r) => (resolve = r)) });
    const started = ai.getState().start();
    await vi.waitFor(() => expect(resolve).not.toBeNull());
    await ai.getState().stop();
    expect(ai.getState()).toMatchObject({ phase: "idle", runId: null });
    expect(ai.getState().generating.size).toBe(0);
    expect(e().notice).toBe("Stopped writing.");
    resolve!({ runId: 5 });
    expect(await started).toBe(false);
    expect(api.cancel).toHaveBeenCalledWith(5);
    // Its events are not applied.
    events.emit("ai://step", stepEvent(5, "A"));
    expect(stepOf(e().doc!.guide, "A").title).toBe("Step A");
    expect(ai.getState().phase).toBe("idle");
  });

  it("Stop while the guide is still saving never calls ai_generate", async () => {
    const { e, ai, api } = await setup();
    e().dispatch(setItemText(e().doc!.guide, "A", "title", "Mine"));
    const started = ai.getState().start();
    expect(ai.getState().phase).toBe("starting");
    await ai.getState().stop();
    expect(ai.getState().phase).toBe("idle");
    expect(await started).toBe(false);
    expect(api.generate).not.toHaveBeenCalled();
  });

  it("counts a step written again by a consistency edit once", async () => {
    const { e, ai, events } = await setup();
    await ai.getState().start();
    events.emit("ai://step", stepEvent(7, "A"));
    events.emit("ai://step", stepEvent(7, "B"));
    events.emit("ai://step", stepEvent(7, "A", { step: { ...stepEvent(7, "A").step!, title: "AI again" } }));
    expect(stepOf(e().doc!.guide, "A").title).toBe("AI again");
    events.emit("ai://finished", finished(7));
    expect(e().notice).toBe("AI wrote 2 steps.");
  });

  it("passes step ids and the guide pass through to ai_generate", async () => {
    const { ai, api } = await setup();
    await ai.getState().start({ stepIds: ["B", "C"], guidePass: false });
    expect(api.generate).toHaveBeenCalledWith("G1", { stepIds: ["B", "C"], guidePass: false });
    expect([...ai.getState().generating]).toEqual(["B", "C"]);
  });

  it("starts only on the guide it was asked for", async () => {
    const { ai, api } = await setup();
    expect(await ai.getState().start({ guideId: "OTHER" })).toBe(false);
    expect(api.generate).not.toHaveBeenCalled();
    expect(await ai.getState().start({ guideId: "G1" })).toBe(true);
  });

  it("cancels the run when the guide is closed mid-run", async () => {
    const { e, ai, api, events } = await setup();
    await ai.getState().start();
    await e().closeGuide();
    expect(api.cancel).toHaveBeenCalledWith(7);
    expect(ai.getState()).toMatchObject({ phase: "idle", guideId: null });
    expect(events.listeners()).toBe(0);
    // A new run can start on the next guide.
    await e().openGuide("G2");
    expect(await ai.getState().start()).toBe(true);
  });

  it("cancels a run that starts after its guide was closed", async () => {
    let resolve: ((v: { runId: number }) => void) | null = null;
    const { e, ai, api } = await setup({ generate: () => new Promise((r) => (resolve = r)) });
    const started = ai.getState().start();
    await vi.waitFor(() => expect(resolve).not.toBeNull());
    await e().closeGuide();
    resolve!({ runId: 4 });
    expect(await started).toBe(false);
    expect(api.cancel).toHaveBeenCalledWith(4);
    expect(ai.getState().phase).toBe("idle");
  });

  it("shows why it can't start, except a missing key on auto-run", async () => {
    const error = (message: string) => async () => Promise.reject({ kind: "invalid", message });
    const off = await setup({ generate: error("AI is turned off in Settings.") });
    expect(await off.ai.getState().start()).toBe(false);
    expect(off.e().notice).toBe("AI is turned off in Settings.");
    expect(off.ai.getState().phase).toBe("idle");

    const noKey = await setup({ generate: error(NO_KEY_MESSAGE) });
    expect(await noKey.ai.getState().start({ auto: true })).toBe(false);
    expect(noKey.e().notice).toBeNull();
    await noKey.ai.getState().start();
    expect(noKey.e().notice).toBe(NO_KEY_MESSAGE);
  });

  it("runs one at a time", async () => {
    const { ai, api } = await setup();
    await ai.getState().start();
    expect(await ai.getState().start()).toBe(false);
    expect(api.generate).toHaveBeenCalledTimes(1);
  });
});

describe("auto-run", () => {
  const settings = (enabled: boolean, autoRun: boolean) => ({ ai: { enabled, autoRun } }) as Settings;

  it("starts only with AI on, auto-run on and a key set", async () => {
    const { ai, api } = await setup();
    expect(await autoRun(ai, "G1", settings(false, true), async () => true)).toBe(false);
    expect(await autoRun(ai, "G1", settings(true, false), async () => true)).toBe(false);
    expect(await autoRun(ai, "G1", settings(true, true), async () => false)).toBe(false);
    expect(await autoRun(ai, "G1", null, async () => true)).toBe(false);
    expect(api.generate).not.toHaveBeenCalled();
    expect(await autoRun(ai, "G1", settings(true, true), async () => true)).toBe(true);
    expect(api.generate).toHaveBeenCalledWith("G1");
  });

  it("does nothing when another guide was opened during the key check", async () => {
    const { e, ai, api } = await setup();
    const run = autoRun(ai, "G1", settings(true, true), async () => {
      await e().openGuide("G2");
      return true;
    });
    expect(await run).toBe(false);
    expect(e().doc!.guide.id).toBe("G2");
    expect(api.generate).not.toHaveBeenCalled();
  });
});

describe("summary", () => {
  it("counts what was written, failed and kept", () => {
    const counts = { written: 1, failed: 1, kept: 2, guideWritten: false, guideError: "overloaded" };
    expect(summary(counts, false)).toBe(
      "AI wrote 1 step. 1 step failed. The guide details failed: overloaded. Kept your edits to 2 steps.",
    );
    expect(summary({ ...counts, written: 0, failed: 0, kept: 0, guideError: null }, true)).toBe("Stopped writing.");
  });
});
