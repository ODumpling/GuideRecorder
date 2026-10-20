import { describe, expect, it, vi } from "vitest";
import type { RecordingApi } from "@/lib/ipc";
import type { RecordingSnapshot, RecordingStatus } from "@/lib/types";
import { createRecordingStore } from "@/recording/store";
import { createFeedStore, FEED_IDLE_MS, FEED_SIZE, feedItems, feedVisible, takenLabel, titleRuns } from "@/toolbar/feed";

const run = { startedAt: "2026-10-15T10:00:00.000Z", title: null, captureAreaFallback: false, pausedMs: 0, excludedApp: null, captureLost: null };
const recording: RecordingStatus = { state: "recording", ...run };
const snap = (id: string, title = "Click **Save** in Notes", elapsedMs = 0): RecordingSnapshot => ({
  id,
  ts: 0,
  width: 16,
  height: 10,
  point: null,
  title,
  elapsedMs,
});

function setup() {
  const api: RecordingApi = {
    start: vi.fn(),
    pause: vi.fn(),
    resume: vi.fn(),
    stop: vi.fn(),
    cancel: vi.fn(),
    status: vi.fn(),
    snapshotImage: vi.fn(),
  };
  const recordingStore = createRecordingStore({ api });
  recordingStore.getState().setStatus(recording);
  const feedApi = {
    deleteCapture: vi.fn(async () => {}),
    renameCapture: vi.fn(async (_id: string, title: string) => title),
    setToolbarTyping: vi.fn(async () => {}),
  };
  const timers: { fn: () => void; ms: number }[] = [];
  const feed = createFeedStore({
    api: feedApi,
    recording: recordingStore,
    setTimer: (fn, ms) => timers.push({ fn, ms }),
    clearTimer: () => {},
  });
  const visible = () => feedVisible(feed.getState(), recordingStore.getState().snapshots.length);
  return { recording: recordingStore, feed, feedApi, timers, visible };
}

describe("feed items", () => {
  it("lists the newest captures first, numbered", () => {
    const snaps = Array.from({ length: FEED_SIZE + 2 }, (_, i) => snap(`0-${i}`));
    const items = feedItems(snaps);
    expect(items).toHaveLength(FEED_SIZE);
    expect(items[0].snapshot.id).toBe(`0-${FEED_SIZE + 1}`);
    expect(items[0].number).toBe(FEED_SIZE + 2);
    expect(items.at(-1)!.number).toBe(3);
    expect(items[0].title).toBe("Click Save in Notes");
  });

  it("says when each was taken", () => {
    expect(takenLabel(58_000, 60_000)).toBe("just now");
    expect(takenLabel(58_000, 120_000)).toBe("0:58");
  });

  it("splits a title into bold runs", () => {
    expect(titleRuns("Click **Submit report** in Expensify")).toEqual([
      { text: "Click ", bold: false },
      { text: "Submit report", bold: true },
      { text: " in Expensify", bold: false },
    ]);
    expect(titleRuns("Open \\*draft\\*")).toEqual([{ text: "Open *draft*", bold: false }]);
  });
});

describe("feed panel", () => {
  it("wakes on each capture and fades after a while unless pinned", () => {
    const { recording, feed, timers, visible } = setup();
    expect(visible()).toBe(false);
    recording.getState().addSnapshot(snap("0-1"));
    expect(visible()).toBe(true);
    expect(timers.at(-1)!.ms).toBe(FEED_IDLE_MS);
    timers.at(-1)!.fn();
    expect(visible()).toBe(false);
    feed.getState().setPinned(true);
    timers.at(-1)!.fn();
    expect(visible()).toBe(true);
  });

  it("toggles from the counter", () => {
    const { recording, feed, visible } = setup();
    recording.getState().addSnapshot(snap("0-1"));
    feed.getState().toggle();
    expect(visible()).toBe(false);
    // Closed: a new capture does not open it.
    recording.getState().addSnapshot(snap("0-2"));
    expect(visible()).toBe(false);
    feed.getState().toggle();
    expect(visible()).toBe(true);
  });

  it("stays open and takes the keyboard while a title is edited", async () => {
    const { recording, feed, feedApi, timers, visible } = setup();
    recording.getState().addSnapshot(snap("0-1"));
    feed.getState().setEditing("0-1");
    expect(feedApi.setToolbarTyping).toHaveBeenCalledExactlyOnceWith(true);
    timers.at(-1)!.fn();
    expect(visible()).toBe(true);
    // Deleted meanwhile (⌘⌥Z): the edit ends and the keyboard goes back.
    recording.getState().removeSnapshot("0-1");
    expect(feed.getState().editing).toBeNull();
    expect(feedApi.setToolbarTyping).toHaveBeenLastCalledWith(false);
  });

  it("renames only real changes, and deletes", async () => {
    const { recording, feed, feedApi } = setup();
    recording.getState().addSnapshot(snap("0-1"));
    recording.getState().addSnapshot(snap("0-2"));
    expect(await feed.getState().rename("0-1", "  Click Save in Notes ")).toBe(false);
    expect(await feed.getState().rename("0-1", "   ")).toBe(false);
    expect(await feed.getState().rename("0-9", "x")).toBe(false);
    expect(await feed.getState().rename("0-1", " Save the note ")).toBe(true);
    expect(feedApi.renameCapture).toHaveBeenCalledExactlyOnceWith("0-1", "Save the note");
    expect(await feed.getState().undoLast()).toBe(true);
    expect(feedApi.deleteCapture).toHaveBeenCalledExactlyOnceWith("0-2");
  });

  it("keeps the error of a failed change", async () => {
    const { recording, feed, feedApi } = setup();
    recording.getState().addSnapshot(snap("0-1"));
    feedApi.deleteCapture.mockRejectedValueOnce({ kind: "notFound", message: "no such capture in this recording" });
    expect(await feed.getState().remove("0-1")).toBe(false);
    expect(feed.getState().error).toBe("no such capture in this recording");
    expect(await feed.getState().remove("0-1")).toBe(true);
    expect(feed.getState().error).toBeNull();
  });
});
