import { describe, expect, it, vi } from "vitest";
import {
  RECORDING_SAVED_EVENT,
  RECORDING_SNAPSHOT_EVENT,
  RECORDING_SNAPSHOT_REMOVED_EVENT,
  RECORDING_SNAPSHOT_TITLE_EVENT,
  RECORDING_STATUS_EVENT,
  type RecordingApi,
} from "@/lib/ipc";
import type { RecordingSaved, RecordingSnapshot, RecordingStatus, StampedStatus } from "@/lib/types";
import {
  captureLostLabel,
  captureLostMessage,
  createRecordingStore,
  elapsedMs,
  formatElapsed,
  isActive,
  subscribe,
  type Listen,
} from "@/recording/store";

const STARTED = "2026-10-15T10:00:00.000Z";
const T0 = Date.parse(STARTED);
const run = { startedAt: STARTED, title: null, captureAreaFallback: false, pausedMs: 0, excludedApp: null, captureLost: null };
const recording: RecordingStatus = { state: "recording", ...run };
const paused: RecordingStatus = { state: "paused", ...run, pausedAt: "2026-10-15T10:00:10.000Z" };
const SAVED: RecordingSaved = { guideId: "G1", steps: 4, skipped: 0, suggestedBlurs: 0, appliedBlurs: 0, unscannedSteps: 0 };
const SNAP1: RecordingSnapshot = { id: "0-5000000", ts: 7_000_000, width: 16, height: 10, point: { x: 8, y: 5 }, title: "Click **Save**", elapsedMs: 1000 };
const SNAP2: RecordingSnapshot = { id: "0-9000000", ts: 12_000_000, width: 16, height: 10, point: null, title: "Type", elapsedMs: 2000 };

/** `status` as the recorder would send it, with `seq`. */
const stamp = (status: RecordingStatus, seq: number): StampedStatus => ({ ...status, seq });

/** A recorder that stamps each change with the next `seq`, like the Rust side. */
function fakeApi(overrides: Partial<RecordingApi> = {}) {
  let seq = 0;
  let status: StampedStatus = stamp({ state: "idle" }, seq);
  const store = (next: RecordingStatus) => (status = stamp(next, ++seq));
  const api: RecordingApi = {
    start: vi.fn(async () => store(recording)),
    pause: vi.fn(async () => store(paused)),
    resume: vi.fn(async () => store(recording)),
    stop: vi.fn(async () => {
      store({ state: "idle" });
      return SAVED;
    }),
    cancel: vi.fn(async () => {
      store({ state: "idle" });
    }),
    status: vi.fn(async () => status),
    snapshotImage: vi.fn(async () => new ArrayBuffer(0)),
    ...overrides,
  };
  return api;
}

function setup(overrides: Partial<RecordingApi> = {}) {
  const api = fakeApi(overrides);
  const onSaved = vi.fn();
  const store = createRecordingStore({ api, onSaved });
  return { api, onSaved, store };
}

/** RFC 3339 for `ms` after the recording started. */
const after = (ms: number) => new Date(T0 + ms).toISOString();

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
    let finish!: (s: StampedStatus) => void;
    const { api, store } = setup({ start: vi.fn(() => new Promise<StampedStatus>((r) => (finish = r))) });
    const starting = store.getState().start();
    expect(store.getState().pending).toBe("start");
    expect(await store.getState().start()).toBe(false);
    expect(api.start).toHaveBeenCalledTimes(1);
    finish(stamp(recording, 1));
    expect(await starting).toBe(true);
    expect(store.getState().pending).toBeNull();
  });

  it("refuses a stop while another command runs, leaving the state alone", async () => {
    let finish!: (s: StampedStatus) => void;
    const { api, onSaved, store } = setup({ pause: vi.fn(() => new Promise<StampedStatus>((r) => (finish = r))) });
    store.getState().setStatus(recording);
    const pausing = store.getState().pause();
    expect(store.getState().pending).toBe("pause");

    expect(await store.getState().stop({ open: false })).toBeNull();
    expect(api.stop).not.toHaveBeenCalled();
    expect(store.getState().status).toEqual(recording);
    expect(store.getState().error).toBeNull();

    finish(stamp(paused, 1));
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
      status: vi.fn(async () => stamp(recording, 1)),
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
    const { store } = setup({ status: vi.fn(async () => stamp(paused, 1)) });
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
    handlers.get(RECORDING_STATUS_EVENT)!(recording);
    handlers.get(RECORDING_SNAPSHOT_EVENT)!(SNAP1);
    expect(store.getState().snapshots).toEqual([SNAP1]);
    handlers.get(RECORDING_SNAPSHOT_EVENT)!(SNAP2);
    handlers.get(RECORDING_SNAPSHOT_TITLE_EVENT)!({ id: SNAP1.id, title: "Click **Submit**" });
    expect(store.getState().snapshots[0].title).toBe("Click **Submit**");
    handlers.get(RECORDING_SNAPSHOT_REMOVED_EVENT)!({ id: SNAP1.id });
    expect(store.getState().snapshots).toEqual([SNAP2]);

    unsubscribe();
    await vi.waitFor(() => expect(unlisten).toHaveBeenCalledTimes(5));
  });

  it("ignores titles and removals of captures it does not have", async () => {
    const { store } = setup();
    store.getState().setStatus(recording);
    store.getState().addSnapshot(SNAP1);
    const before = store.getState().snapshots;
    store.getState().retitleSnapshot({ id: "0-1", title: "x" });
    store.getState().retitleSnapshot({ id: SNAP1.id, title: SNAP1.title });
    store.getState().removeSnapshot("0-1");
    expect(store.getState().snapshots).toBe(before);
  });

  it("collects the screenshots of the running recording, once each, until it ends", async () => {
    const { store } = setup();
    // Nothing recording: not kept (a late event from a recording already over).
    store.getState().addSnapshot(SNAP1);
    expect(store.getState().snapshots).toEqual([]);

    await store.getState().start();
    store.getState().addSnapshot(SNAP1);
    store.getState().addSnapshot(SNAP2);
    store.getState().addSnapshot(SNAP1);
    expect(store.getState().snapshots).toEqual([SNAP1, SNAP2]);
    // Kept across a pause and while saving.
    await store.getState().pause();
    store.getState().setStatus({ state: "saving", startedAt: STARTED, title: null });
    expect(store.getState().snapshots).toEqual([SNAP1, SNAP2]);
    store.getState().setStatus({ state: "idle" });
    expect(store.getState().snapshots).toEqual([]);

    // A new recording starts empty.
    await store.getState().start();
    store.getState().addSnapshot(SNAP2);
    expect(store.getState().snapshots).toEqual([SNAP2]);
    await store.getState().cancel();
    expect(store.getState().snapshots).toEqual([]);
  });

  it("drops the screenshots when the status says another recording is running", async () => {
    const { store } = setup();
    await store.getState().start();
    store.getState().addSnapshot(SNAP1);
    store.getState().setStatus({ ...recording, startedAt: "2026-10-15T11:00:00.000Z" });
    expect(store.getState().snapshots).toEqual([]);
  });

  it("takes the elapsed time from the status, not counting pauses", () => {
    const at = (ms: number) => T0 + ms;
    expect(elapsedMs({ state: "idle" }, at(5_000))).toBe(0);
    expect(elapsedMs({ state: "saving", startedAt: STARTED, title: null }, at(5_000))).toBe(0);

    expect(elapsedMs(recording, at(10_000))).toBe(10_000);
    // Paused at 10 s after an earlier 2 s pause: frozen at 8 s.
    const pausedAt10 = { ...paused, pausedMs: 2_000, pausedAt: after(10_000) };
    expect(elapsedMs(pausedAt10, at(10_000))).toBe(8_000);
    expect(elapsedMs(pausedAt10, at(60_000))).toBe(8_000);
    // Resumed at 15 s: the 5 s pause is in `pausedMs` now.
    const resumed = { ...recording, pausedMs: 7_000 };
    expect(elapsedMs(resumed, at(20_000))).toBe(13_000);
    // Never negative, and a bad time is 0.
    expect(elapsedMs(recording, at(-1_000))).toBe(0);
    expect(elapsedMs({ ...recording, startedAt: "soon" }, at(1_000))).toBe(0);
  });

  it("keeps the elapsed time across a reload (a fresh store reads the status)", async () => {
    const status: RecordingStatus = { ...paused, pausedMs: 3_000, pausedAt: after(30_000) };
    const { store } = setup({ status: vi.fn(async () => stamp(status, 4)) });
    await store.getState().load();
    expect(elapsedMs(store.getState().status, T0 + 90_000)).toBe(27_000);
  });

  it("follows the excluded app in front", () => {
    const { store } = setup();
    store.getState().setStatus(recording);
    store.getState().setStatus({ ...recording, excludedApp: "com.1password.1password" });
    const status = store.getState().status;
    expect(status.state === "recording" && status.excludedApp).toBe("com.1password.1password");
    store.getState().setStatus(recording);
    expect(store.getState().status).toEqual(recording);
  });

  it("follows a lost capture, and words it for the banner and toolbar", () => {
    const { store } = setup();
    store.getState().setStatus(stamp(recording, 1));
    expect(captureLostMessage(store.getState().status)).toBeNull();
    store.getState().setStatus(stamp({ ...recording, captureLost: "the window was closed" }, 2));
    const status = store.getState().status;
    expect(captureLostLabel(status)).toBe("Capture stopped: the window was closed");
    expect(captureLostMessage(status)).toBe("Capture stopped: the window was closed. Stop to save what was recorded.");
    // Kept while paused; a late sink event older than a newer status is dropped.
    store.getState().setStatus(stamp({ ...paused, captureLost: "the window was closed" }, 4));
    store.getState().setStatus(stamp({ ...recording, captureLost: null }, 3));
    expect(captureLostLabel(store.getState().status)).toBe("Capture stopped: the window was closed");
    expect(captureLostMessage({ state: "idle" })).toBeNull();
    expect(captureLostMessage({ state: "saving", startedAt: STARTED, title: null })).toBeNull();
  });

  it("ignores a status older than the last one applied", async () => {
    const { store } = setup();
    // A stale event (the exclusion sink's, sent late) after the newer pause.
    store.getState().setStatus(stamp(recording, 3));
    store.getState().setStatus(stamp(paused, 5));
    store.getState().setStatus(stamp({ ...recording, excludedApp: "com.1password.1password" }, 4));
    expect(store.getState().status).toEqual(paused);
    // The same seq again (the event and the command result) still applies.
    store.getState().setStatus(stamp(paused, 5));
    expect(store.getState().status).toEqual(paused);
    // A late recording event after idle is ignored too.
    store.getState().setStatus(stamp({ state: "idle" }, 7));
    store.getState().setStatus(stamp(recording, 6));
    expect(store.getState().status).toEqual({ state: "idle" });
    // A command's result goes through the same check.
    await store.getState().start();
    expect(store.getState().status).toEqual({ state: "idle" });
  });

  it("takes a fresh read as the new baseline", async () => {
    const { store } = setup({ status: vi.fn(async () => stamp(recording, 2)) });
    store.getState().setStatus(stamp({ state: "idle" }, 9));
    // As after the app restarted: the recorder counts from 0 again.
    await store.getState().load();
    expect(store.getState().status).toEqual(recording);
    store.getState().setStatus(stamp(paused, 3));
    expect(store.getState().status).toEqual(paused);
  });

  it("formats elapsed time", () => {
    expect(formatElapsed(0)).toBe("0:00");
    expect(formatElapsed(65_400)).toBe("1:05");
    expect(formatElapsed(3_726_000)).toBe("1:02:06");
  });
});
