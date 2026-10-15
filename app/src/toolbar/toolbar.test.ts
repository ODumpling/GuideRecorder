import { describe, expect, it, vi } from "vitest";
import type { RecordingApi } from "@/lib/ipc";
import type { RecordingSnapshot, RecordingStatus, StampedStatus } from "@/lib/types";
import { createRecordingStore } from "@/recording/store";
import { createManualStepStore, MANUAL_STEP_NOTICE_MS, toolbarView } from "@/toolbar/toolbar";

const run = { startedAt: "2026-10-15T10:00:00.000Z", title: null, captureAreaFallback: false, pausedMs: 0, excludedApp: null, captureLost: null };
const recording: RecordingStatus = { state: "recording", ...run };
const paused: RecordingStatus = { state: "paused", ...run, pausedAt: "2026-10-15T10:00:10.000Z" };
const snap = (id: string): RecordingSnapshot => ({ id, ts: 0, width: 16, height: 10, point: null });

function setup(initial: RecordingStatus = recording) {
  let seq = 0;
  let status: StampedStatus = { ...initial, seq };
  const api: RecordingApi = {
    start: vi.fn(),
    pause: vi.fn(async () => (status = { ...paused, seq: ++seq })),
    resume: vi.fn(async () => (status = { ...recording, seq: ++seq })),
    stop: vi.fn(),
    cancel: vi.fn(),
    status: vi.fn(async () => status),
    snapshotImage: vi.fn(),
  };
  const store = createRecordingStore({ api });
  return { api, store };
}

describe("toolbar step counter", () => {
  it("counts the run's snapshot events, once per id", async () => {
    const { store } = setup();
    await store.getState().load();
    expect(toolbarView(store.getState()).steps).toBe(0);
    store.getState().addSnapshot(snap("0-1"));
    store.getState().addSnapshot(snap("0-2"));
    store.getState().addSnapshot(snap("0-2"));
    expect(toolbarView(store.getState()).steps).toBe(2);
    // Pausing keeps the run's count.
    store.getState().setStatus({ ...paused, seq: 5 });
    expect(toolbarView(store.getState()).steps).toBe(2);
  });

  it("resets on a new startedAt and when idle", async () => {
    const { store } = setup();
    await store.getState().load();
    store.getState().addSnapshot(snap("0-1"));
    store.getState().setStatus({ ...recording, startedAt: "2026-10-15T11:00:00.000Z", seq: 2 });
    expect(toolbarView(store.getState()).steps).toBe(0);
    store.getState().addSnapshot(snap("0-1"));
    expect(toolbarView(store.getState()).steps).toBe(1);
    store.getState().setStatus({ state: "idle", seq: 3 });
    expect(toolbarView(store.getState()).steps).toBe(0);
    store.getState().addSnapshot(snap("0-2"));
    expect(toolbarView(store.getState()).steps).toBe(0);
  });
});

describe("toolbar buttons", () => {
  const view = (status: RecordingStatus, pending: null | "pause" = null, adding = false) =>
    toolbarView({ status, pending, snapshots: [] }, adding);
  const enabled = (v: ReturnType<typeof view>) => ({
    pause: v.canPause,
    resume: v.canResume,
    step: v.canAddStep,
    stop: v.canStop,
  });

  it("recording: pause, manual step and stop", () => {
    expect(view(recording).label).toBe("Recording");
    expect(enabled(view(recording))).toEqual({ pause: true, resume: false, step: true, stop: true });
  });

  it("paused: resume and stop; no manual step", () => {
    expect(view(paused).label).toBe("Paused");
    expect(enabled(view(paused))).toEqual({ pause: false, resume: true, step: false, stop: true });
  });

  it("an excluded app in front: named, and no manual step", () => {
    const v = view({ ...recording, excludedApp: "com.1password.1password" });
    expect(v.label).toBe("Not capturing: 1Password is in front");
    expect(v.excluded).toBe(true);
    expect(enabled(v)).toEqual({ pause: true, resume: false, step: false, stop: true });
    // Paused shows Paused, whatever is in front.
    expect(view({ ...paused, excludedApp: "com.1password.1password" }).label).toBe("Paused");
  });

  it("capture lost: says so, no manual step, pause/resume and stop still work", () => {
    const v = view({ ...recording, captureLost: "the window was closed" });
    expect(v.label).toBe("Capture stopped: the window was closed");
    expect(v.lost).toBe("Capture stopped: the window was closed. Stop to save what was recorded.");
    expect(enabled(v)).toEqual({ pause: true, resume: false, step: false, stop: true });
    // It wins over Paused and over an excluded app in front.
    const p = view({ ...paused, captureLost: "the window was closed" });
    expect(p.label).toBe("Capture stopped: the window was closed");
    expect(enabled(p)).toEqual({ pause: false, resume: true, step: false, stop: true });
    expect(view({ ...recording, excludedApp: "com.1password.1password", captureLost: "the display was disconnected" }).label).toBe(
      "Capture stopped: the display was disconnected",
    );
    expect(view(recording).lost).toBeNull();
  });

  it("nothing while a command runs, a step is being added, saving or idle", () => {
    const none = { pause: false, resume: false, step: false, stop: false };
    expect(enabled(view(recording, "pause"))).toEqual(none);
    expect(enabled(view(recording, null, true))).toEqual({ pause: true, resume: false, step: false, stop: true });
    expect(view({ state: "saving", startedAt: run.startedAt, title: null }).label).toBe("Saving…");
    expect(enabled(view({ state: "saving", startedAt: run.startedAt, title: null }))).toEqual(none);
    expect(enabled(view({ state: "idle" }))).toEqual(none);
  });
});

describe("manual step", () => {
  function manualSetup(addManualStep: () => Promise<void>, initial: RecordingStatus = recording) {
    const { api, store } = setup(initial);
    const timers: (() => void)[] = [];
    const manual = createManualStepStore({
      addManualStep: vi.fn(addManualStep),
      recording: store,
      setTimer: (fn, ms) => {
        expect(ms).toBe(MANUAL_STEP_NOTICE_MS);
        timers.push(fn);
        return timers.length;
      },
      clearTimer: () => {},
    });
    return { api, store, manual, timers };
  }

  it("confirms briefly, then is ready again", async () => {
    const { store, manual, timers } = manualSetup(async () => {});
    await store.getState().load();
    expect(await manual.getState().add()).toBe(true);
    expect(manual.getState().phase).toBe("added");
    timers.shift()!();
    expect(manual.getState()).toMatchObject({ phase: "ready", error: null });
  });

  it("shows a rejection quietly and re-reads the recorder", async () => {
    const { api, store, manual, timers } = manualSetup(async () => {
      throw { kind: "invalid", message: "nothing is recording" };
    });
    await store.getState().load();
    expect(await manual.getState().add()).toBe(false);
    expect(manual.getState()).toMatchObject({ phase: "failed", error: "nothing is recording" });
    expect(api.status).toHaveBeenCalledTimes(2);
    timers.shift()!();
    expect(manual.getState().phase).toBe("ready");
  });

  it("is not called once capture is lost", async () => {
    const add = vi.fn(async () => {});
    const { store, manual } = manualSetup(add, { ...recording, captureLost: "the window was closed" });
    await store.getState().load();
    expect(await manual.getState().add()).toBe(false);
    expect(add).not.toHaveBeenCalled();
  });

  it("is not called while paused", async () => {
    const add = vi.fn(async () => {});
    const { store, manual } = manualSetup(add, paused);
    await store.getState().load();
    expect(await manual.getState().add()).toBe(false);
    expect(add).not.toHaveBeenCalled();
    expect(manual.getState().phase).toBe("ready");
  });
});
