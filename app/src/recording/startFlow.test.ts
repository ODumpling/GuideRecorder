import { describe, expect, it, vi } from "vitest";
import type { PickerApi, RecordingApi } from "@/lib/ipc";
import type { CaptureArea, CaptureWindow, RecordingStatus, StampedStatus } from "@/lib/types";
import { createStartFlow, startChoice } from "@/recording/startFlow";
import { createRecordingStore } from "@/recording/store";

const recording: RecordingStatus = {
  state: "recording",
  startedAt: "2026-10-15T10:00:00.000Z",
  title: null,
  captureAreaFallback: false,
  pausedMs: 0,
  excludedApp: null,
};
const WINDOWS: CaptureWindow[] = [
  { windowId: 7, appName: "Safari", bundleId: "com.apple.Safari", title: "Apple", width: 1200, height: 800 },
  { windowId: 9, appName: "Notes", bundleId: "com.apple.Notes", title: null, width: 600, height: 500 },
];
const REGION = { displayId: 1, rect: { x: 100, y: 50, width: 400, height: 300 } };

function setup(area: CaptureArea | null, overrides: { recording?: Partial<RecordingApi>; picker?: Partial<PickerApi> } = {}) {
  let seq = 0;
  let status: StampedStatus = { state: "idle", seq };
  const api: RecordingApi = {
    start: vi.fn(async () => (status = { ...recording, seq: ++seq })),
    pause: vi.fn(),
    resume: vi.fn(),
    stop: vi.fn(),
    cancel: vi.fn(),
    status: vi.fn(async () => status),
    snapshotImage: vi.fn(),
    ...overrides.recording,
  };
  const picker: PickerApi = {
    listCaptureWindows: vi.fn(async () => WINDOWS),
    pickRegion: vi.fn(async () => REGION),
    ...overrides.picker,
  };
  const store = createRecordingStore({ api });
  const flow = createStartFlow({ recording: store, picker, captureArea: () => area });
  return { api, picker, store, flow };
}

describe("startChoice", () => {
  it("follows the capture area, the display when unknown", () => {
    expect(startChoice("display")).toBe("display");
    expect(startChoice("window")).toBe("window");
    expect(startChoice("region")).toBe("region");
    expect(startChoice(null)).toBe("display");
    expect(startChoice(undefined)).toBe("display");
  });
});

describe("start flow", () => {
  it("display: starts at once, with no target", async () => {
    const { api, picker, store, flow } = setup("display");
    await flow.getState().begin();
    expect(api.start).toHaveBeenCalledWith(null);
    expect(picker.listCaptureWindows).not.toHaveBeenCalled();
    expect(picker.pickRegion).not.toHaveBeenCalled();
    expect(store.getState().status.state).toBe("recording");
  });

  it("settings not loaded: starts like display", async () => {
    const { api, flow } = setup(null);
    await flow.getState().begin();
    expect(api.start).toHaveBeenCalledWith(null);
  });

  it("window: opens the picker with the windows, then starts on the chosen one", async () => {
    const { api, flow } = setup("window");
    await flow.getState().begin();
    expect(api.start).not.toHaveBeenCalled();
    expect(flow.getState().windowPicker).toEqual({ windows: WINDOWS, loadError: null, startError: null });

    expect(await flow.getState().chooseWindow(9)).toBe(true);
    expect(api.start).toHaveBeenCalledWith(null, { kind: "window", windowId: 9 });
    expect(flow.getState().windowPicker).toBeNull();
  });

  it("window: cancelling starts nothing", async () => {
    const { api, flow } = setup("window");
    await flow.getState().begin();
    flow.getState().closeWindowPicker();
    expect(flow.getState().windowPicker).toBeNull();
    expect(api.start).not.toHaveBeenCalled();
  });

  it("window: a window that closed is shown in the picker and the list reloads", async () => {
    const { api, picker, store, flow } = setup("window", {
      recording: { start: vi.fn(async () => Promise.reject({ kind: "invalid", message: "That window is no longer open." })) },
    });
    await flow.getState().begin();
    expect(await flow.getState().chooseWindow(7)).toBe(false);
    expect(api.start).toHaveBeenCalledTimes(1);
    expect(flow.getState().windowPicker).toEqual({
      windows: WINDOWS,
      loadError: null,
      startError: "That window is no longer open.",
    });
    expect(picker.listCaptureWindows).toHaveBeenCalledTimes(2);
    // Shown in the picker, not the banner.
    expect(store.getState().error).toBeNull();
  });

  it("window: a missing permission closes the picker and stays in the banner", async () => {
    const { store, flow } = setup("window", {
      recording: {
        start: vi.fn(async () => Promise.reject({ kind: "invalid", message: "recording needs Accessibility permission" })),
      },
    });
    await flow.getState().begin();
    await flow.getState().chooseWindow(7);
    expect(flow.getState().windowPicker).toBeNull();
    expect(store.getState().error).toEqual({ message: "recording needs Accessibility permission", permissions: true });
  });

  it("window: no windows offers the display instead", async () => {
    const { api, flow } = setup("window", { picker: { listCaptureWindows: vi.fn(async () => []) } });
    await flow.getState().begin();
    expect(flow.getState().windowPicker?.windows).toEqual([]);
    expect(await flow.getState().recordDisplay()).toBe(true);
    expect(api.start).toHaveBeenCalledWith(null, { kind: "display" });
    expect(flow.getState().windowPicker).toBeNull();
  });

  it("window: a failed listing is shown", async () => {
    const { flow } = setup("window", {
      picker: { listCaptureWindows: vi.fn(async () => Promise.reject({ kind: "io", message: "no windows for you" })) },
    });
    await flow.getState().begin();
    expect(flow.getState().windowPicker).toEqual({ windows: [], loadError: "no windows for you", startError: null });
  });

  it("region: starts on the drawn region", async () => {
    const { api, picker, flow } = setup("region");
    await flow.getState().begin();
    expect(picker.pickRegion).toHaveBeenCalledTimes(1);
    expect(api.start).toHaveBeenCalledWith(null, { kind: "region", ...REGION });
  });

  it("region: a cancelled pick starts nothing", async () => {
    const { api, store, flow } = setup("region", { picker: { pickRegion: vi.fn(async () => null) } });
    await flow.getState().begin();
    expect(api.start).not.toHaveBeenCalled();
    expect(store.getState()).toMatchObject({ status: { state: "idle" }, pending: null, error: null });
  });

  it("region: a second begin while picking is ignored", async () => {
    let answer!: (pick: typeof REGION | null) => void;
    const pickRegion = vi.fn(() => new Promise<typeof REGION | null>((resolve) => (answer = resolve)));
    const { api, store, flow } = setup("region", { picker: { pickRegion } });
    const first = flow.getState().begin();
    expect(store.getState().pending).toBe("start");
    await flow.getState().begin();
    expect(pickRegion).toHaveBeenCalledTimes(1);
    answer(REGION);
    await first;
    expect(api.start).toHaveBeenCalledTimes(1);
  });

  it("does nothing while a recording runs", async () => {
    const { api, picker, store, flow } = setup("window");
    await store.getState().start();
    await flow.getState().begin();
    expect(api.start).toHaveBeenCalledTimes(1);
    expect(picker.listCaptureWindows).not.toHaveBeenCalled();
    expect(flow.getState().windowPicker).toBeNull();
  });
});
