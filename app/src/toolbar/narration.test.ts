import { describe, expect, it, vi } from "vitest";
import { NARRATION_EVENT, type NarrationApi } from "@/lib/ipc";
import type { NarrationState } from "@/lib/types";
import { createMicStore, micView, subscribeMic } from "@/toolbar/narration";

const off: NarrationState = { supported: true, on: false, enabled: false, error: null };
const on: NarrationState = { supported: true, on: true, enabled: true, error: null };

function setup(initial: NarrationState = off, set?: NarrationApi["set"]) {
  const api: NarrationApi = {
    state: vi.fn(async () => initial),
    set: vi.fn(set ?? (async (enabled) => (enabled ? on : off))),
  };
  return { api, store: createMicStore({ api }) };
}

describe("toolbar microphone", () => {
  it("turns on and off", async () => {
    const { api, store } = setup();
    await store.getState().load();
    await store.getState().toggle();
    expect(api.set).toHaveBeenLastCalledWith(true);
    expect(store.getState().narration?.on).toBe(true);
    await store.getState().toggle();
    expect(api.set).toHaveBeenLastCalledWith(false);
    expect(store.getState().narration?.on).toBe(false);
  });

  it("does nothing where narration is not supported, or before it loaded", async () => {
    const { api, store } = setup({ ...off, supported: false });
    await store.getState().toggle();
    await store.getState().load();
    await store.getState().toggle();
    expect(api.set).not.toHaveBeenCalled();
    expect(micView(store.getState(), false).show).toBe(false);
  });

  it("says why it could not turn on", async () => {
    const missing = "Allow GuideRecorder to use the microphone in Settings → Permissions.";
    const { store } = setup(off, async () => ({ ...off, enabled: true, error: missing }));
    await store.getState().load();
    await store.getState().toggle();
    const view = micView(store.getState(), false);
    expect(view).toMatchObject({ show: true, on: false, warning: true, label: "Turn the microphone on" });
    expect(view.title).toBe(`Couldn't turn the microphone on: ${missing}`);

    const refused = setup(off, () => Promise.reject({ kind: "readOnly", message: "these settings can't be changed here" }));
    await refused.store.getState().load();
    await refused.store.getState().toggle();
    expect(refused.store.getState().busy).toBe(false);
    expect(micView(refused.store.getState(), false).title).toBe(
      "Couldn't turn the microphone on: these settings can't be changed here",
    );
  });

  it("follows recording://narration", async () => {
    const { store } = setup();
    let handler: ((payload: NarrationState) => void) | null = null;
    await subscribeMic(store, async (event, h) => {
      expect(event).toBe(NARRATION_EVENT);
      handler = h;
      return () => {};
    });
    handler!(on);
    expect(store.getState().narration).toEqual(on);
  });

  it("shows on, paused and busy", () => {
    expect(micView({ narration: on, busy: false, error: null }, false)).toMatchObject({
      show: true,
      on: true,
      label: "Turn the microphone off",
      disabled: false,
    });
    expect(micView({ narration: on, busy: false, error: null }, true).title).toBe(
      "Microphone on, and paused with the recording",
    );
    expect(micView({ narration: off, busy: true, error: null }, false).disabled).toBe(true);
  });
});
