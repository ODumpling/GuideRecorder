import { describe, expect, it, vi } from "vitest";
import type { SettingsApi } from "@/lib/ipc";
import type { Settings } from "@/lib/types";
import { createSettingsStore } from "@/settings/store";

const DEFAULTS: Settings = {
  version: 1,
  general: { library: null, launchAtLogin: false, menuBarIcon: false },
  recording: {
    hotkeys: { record: "CmdOrCtrl+Shift+R", pause: "CmdOrCtrl+Shift+P", manualStep: "CmdOrCtrl+Shift+M" },
    captureArea: "display",
    afterFrames: true,
    afterFrameDelayMs: 400,
    excludedApps: [],
    typingIdleMs: 1500,
  },
  ai: { enabled: true, autoRun: true, stepModel: "claude-haiku-5-5", guideModel: "claude-sonnet-5-5", language: "", audience: "beginner" },
  export: { format: "pdf", folder: null, logo: null, accent: null },
};

function api(overrides: Partial<SettingsApi> = {}): SettingsApi {
  const fail = () => Promise.reject(new Error("not mocked"));
  return {
    get: async () => ({ settings: structuredClone(DEFAULTS), readOnly: false, loadError: null }),
    update: async (s) => structuredClone(s),
    libraryInfo: fail,
    chooseLibrary: fail,
    switchLibrary: fail,
    launchAtLogin: fail,
    setLaunchAtLogin: fail,
    apiKeyStored: fail,
    setApiKey: fail,
    deleteApiKey: fail,
    permissions: fail,
    openPermissionSettings: fail,
    mcpSetup: fail,
    ...overrides,
  };
}

describe("settings store", () => {
  it("loads, then saves each change in order", async () => {
    const update = vi.fn(async (s: Settings) => structuredClone(s));
    const store = createSettingsStore(api({ update }));
    await store.getState().load();
    expect(store.getState().settings?.ai.enabled).toBe(true);

    const first = store.getState().change((s) => ({ ...s, ai: { ...s.ai, enabled: false } }));
    // Shown at once, before the save returns.
    expect(store.getState().settings?.ai.enabled).toBe(false);
    const second = store.getState().change((s) => ({ ...s, ai: { ...s.ai, language: "de" } }));
    expect(await first).toBe(true);
    expect(await second).toBe(true);
    expect(update.mock.calls.map(([s]) => [s.ai.enabled, s.ai.language])).toEqual([
      [false, ""],
      [false, "de"],
    ]);
    expect(store.getState().settings?.ai).toMatchObject({ enabled: false, language: "de" });
  });

  it("reverts a refused change and reports it", async () => {
    const store = createSettingsStore(
      api({ update: async () => Promise.reject({ kind: "invalid", message: "the typing pause must be 300–10000 ms" }) }),
    );
    await store.getState().load();
    const ok = await store.getState().change((s) => ({ ...s, recording: { ...s.recording, typingIdleMs: 5 } }));
    expect(ok).toBe(false);
    expect(store.getState().settings?.recording.typingIdleMs).toBe(1500);
    expect(store.getState().error).toContain("typing pause");
    store.getState().clearError();
    expect(store.getState().error).toBeNull();
  });

  it("does not save over newer settings", async () => {
    const update = vi.fn();
    const store = createSettingsStore(
      api({ get: async () => ({ settings: structuredClone(DEFAULTS), readOnly: true, loadError: null }), update }),
    );
    await store.getState().load();
    expect(await store.getState().change((s) => s)).toBe(false);
    expect(update).not.toHaveBeenCalled();
    expect(store.getState().error).toContain("newer GuidesYa");
  });

  it("reports a failed load", async () => {
    const store = createSettingsStore(api({ get: () => Promise.reject(new Error("no settings")) }));
    await store.getState().load();
    expect(store.getState().settings).toBeNull();
    expect(store.getState().loadError).toBe("no settings");
    expect(await store.getState().change((s) => s)).toBe(false);
  });
});
