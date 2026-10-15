import { describe, expect, it, vi } from "vitest";
import { RECORDING_SAVED_EVENT, RECORDING_STATUS_EVENT, type RecordingApi } from "@/lib/ipc";
import type { RecordingSaved, RecordingStatus } from "@/lib/types";
import { createRecordingStore, elapsedMs, formatElapsed, isActive, subscribe, type Listen } from "@/recording/store";

const STARTED = "2026-10-15T10:00:00.000Z";
const T0 = Date.parse(STARTED);
const recording: RecordingStatus = { state: "recording", startedAt: STARTED, title: null, captureAreaFallback: false };
const paused: RecordingStatus = { ...recording, state: "paused" };
const SAVED: RecordingSaved = { guideId: "G1", steps: 4, skipped: 0 };

function fakeApi(overrides: Partial<RecordingApi> = {}) {
  let status: RecordingStatus = { state: "idle" };
  const api: RecordingApi = {
    start: vi.fn(async () => (status = recording)),
    pause: vi.fn(async () => (status = paused)),
    resume: vi.fn(async () => (status = recording)),
    stop: vi.fn(async () => {
      status = { state: "idle" };
      return SAVED;
    }),
    cancel: vi.fn(async () => {
      status = { state: "idle" };
    }),
    status: vi.fn(async () => status),
    ...overrides,
  };
  return api;
}

function setup(overrides: Partial<RecordingApi> = {}) {
  let time = T0;
  const api = fakeApi(overrides);
  const onSaved = vi.fn();
  const store = createRecordingStore({ api, onSaved, now: () => time });
  return { api, onSaved, store, advance: (ms: number) => (time += ms), now: () => time };
}

describe("recording store", () => {
  it("starts, pauses, resumes and stops", async () => {
    const { api, onSaved, store } = setup();
    expect(store.getState().status).toEqual({ state: "idle" });

    expect(await store.getState().start()).toBe(true);
    expect(api.start).toHaveBeenCalledWith(null);
    expect(store.getState().status).toEqual(recording);
    expect(isActive(store.getState().status)).toBe(true);

    await store.getState().pause();
    expect(store.getState().status.state).toBe("paused");
    await store.getState().resume();
    expect(store.getState().status.state).toBe("recording");

    expect(await store.getState().stop()).toEqual(SAVED);
    expect(store.getState().status).toEqual({ state: "idle" });
    expect(onSaved).toHaveBeenCalledExactlyOnceWith(SAVED);
    expect(store.getState().pending).toBeNull();
  });

  it("marks the command in flight and refuses a second one", async () => {
    let finish!: (s: RecordingStatus) => void;
    const { api, store } = setup({ start: vi.fn(() => new Promise<RecordingStatus>((r) => (finish = r))) });
    const starting = store.getState().start();
    expect(store.getState().pending).toBe("start");
    expect(await store.getState().start()).toBe(false);
    expect(api.start).toHaveBeenCalledTimes(1);
    finish(recording);
    expect(await starting).toBe(true);
    expect(store.getState().pending).toBeNull();
  });

  it("refuses a stop while another command runs, leaving the state alone", async () => {
    let finish!: (s: RecordingStatus) => void;
    const { api, onSaved, store } = setup({ pause: vi.fn(() => new Promise<RecordingStatus>((r) => (finish = r))) });
    store.getState().setStatus(recording);
    const pausing = store.getState().pause();
    expect(store.getState().pending).toBe("pause");

    expect(await store.getState().stop({ open: false })).toBeNull();
    expect(api.stop).not.toHaveBeenCalled();
    expect(store.getState().status).toEqual(recording);
    expect(store.getState().error).toBeNull();

    finish(paused);
    await pausing;
    expect(store.getState().status).toEqual(paused);
    // The refused quiet stop did not leave later saves quiet.
    await store.getState().stop();
    expect(onSaved).toHaveBeenCalledExactlyOnceWith(SAVED);
  });

  it("flags a permissions refusal and re-reads the state", async () => {
    const { api, store } = setup({
      start: vi.fn(async () => {
        throw { kind: "invalid", message: "recording needs Accessibility, Input Monitoring permission" };
      }),
    });
    expect(await store.getState().start()).toBe(false);
    expect(store.getState().error).toEqual({
      message: "recording needs Accessibility, Input Monitoring permission",
      permissions: true,
    });
    expect(api.status).toHaveBeenCalled();
    expect(store.getState().status).toEqual({ state: "idle" });
    store.getState().clearError();
    expect(store.getState().error).toBeNull();
  });

  it("keeps other errors apart from permissions", async () => {
    const { store } = setup({
      start: vi.fn(async () => {
        throw { kind: "invalid", message: "a recording is already running" };
      }),
      status: vi.fn(async () => recording),
    });
    await store.getState().start();
    expect(store.getState().error).toEqual({ message: "a recording is already running", permissions: false });
    // Re-read: it was already running.
    expect(store.getState().status).toEqual(recording);
  });

  it("is idle after a failed stop and does not open anything", async () => {
    const { onSaved, store } = setup({
      stop: vi.fn(async () => {
        throw { kind: "io", message: "disk full" };
      }),
    });
    store.getState().setStatus(recording);
    expect(await store.getState().stop()).toBeNull();
    expect(store.getState().status).toEqual({ state: "idle" });
    expect(store.getState().error?.message).toBe("disk full");
    expect(onSaved).not.toHaveBeenCalled();
  });

  it("opens a saved guide once, whether the event or the result comes first", async () => {
    const { onSaved, store } = setup({
      stop: vi.fn(async () => {
        store.getState().saved(SAVED);
        return SAVED;
      }),
    });
    store.getState().setStatus(recording);
    await store.getState().stop();
    store.getState().saved(SAVED);
    expect(onSaved).toHaveBeenCalledTimes(1);
  });

  it("stops without opening when asked (quitting)", async () => {
    const { onSaved, store } = setup({
      stop: vi.fn(async () => {
        // The saved event arriving while the stop is in flight.
        store.getState().saved(SAVED);
        return SAVED;
      }),
    });
    store.getState().setStatus(recording);
    expect(await store.getState().stop({ open: false })).toEqual(SAVED);
    store.getState().saved(SAVED);
    expect(onSaved).not.toHaveBeenCalled();
  });

  it("cancels", async () => {
    const { api, store } = setup();
    store.getState().setStatus(recording);
    expect(await store.getState().cancel()).toBe(true);
    expect(api.cancel).toHaveBeenCalled();
    expect(store.getState().status).toEqual({ state: "idle" });
  });

  it("loads the status at startup", async () => {
    const { store } = setup({ status: vi.fn(async () => paused) });
    await store.getState().load();
    expect(store.getState().status).toEqual(paused);
  });

  it("follows status and saved events", async () => {
    const { onSaved, store } = setup();
    const handlers = new Map<string, (payload: unknown) => void>();
    const unlisten = vi.fn();
    const listen: Listen = async (event, handler) => {
      handlers.set(event, handler);
      return unlisten;
    };
    const unsubscribe = subscribe(store, listen);

    handlers.get(RECORDING_STATUS_EVENT)!(recording);
    expect(store.getState().status).toEqual(recording);
    handlers.get(RECORDING_STATUS_EVENT)!({ state: "saving", startedAt: STARTED, title: null });
    expect(store.getState().status.state).toBe("saving");
    handlers.get(RECORDING_SAVED_EVENT)!(SAVED);
    expect(onSaved).toHaveBeenCalledExactlyOnceWith(SAVED);

    unsubscribe();
    await vi.waitFor(() => expect(unlisten).toHaveBeenCalledTimes(2));
  });

  it("does not count pauses in the elapsed time", () => {
    const { advance, now, store } = setup();
    const elapsed = () => elapsedMs(store.getState(), now());
    expect(elapsed()).toBe(0);

    store.getState().setStatus(recording);
    advance(10_000);
    expect(elapsed()).toBe(10_000);

    store.getState().setStatus(paused);
    advance(5_000);
    expect(elapsed()).toBe(10_000);
    // A repeated paused event keeps the pause's start.
    store.getState().setStatus(paused);
    advance(1_000);

    store.getState().setStatus(recording);
    advance(2_000);
    expect(elapsed()).toBe(12_000);

    // A new recording starts the clock over.
    store.getState().setStatus({ state: "idle" });
    store.getState().setStatus({ ...recording, startedAt: new Date(now()).toISOString() });
    advance(3_000);
    expect(elapsed()).toBe(3_000);
  });

  it("formats elapsed time", () => {
    expect(formatElapsed(0)).toBe("0:00");
    expect(formatElapsed(65_400)).toBe("1:05");
    expect(formatElapsed(3_726_000)).toBe("1:02:06");
  });
});
