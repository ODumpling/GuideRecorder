import { describe, expect, it, vi } from "vitest";
import type { UpdaterApi } from "@/lib/ipc";
import { bannerFor, createUpdaterStore } from "@/updater/store";
import { progressText } from "@/updater/UpdateBanner";

function api(overrides: Partial<UpdaterApi> = {}): UpdaterApi {
  return {
    version: async () => "0.1.0",
    status: async () => ({ state: "idle" }),
    check: async () => {},
    install: async () => {},
    restart: async () => {},
    ...overrides,
  };
}

describe("updater store", () => {
  it("hides a put-off version until a check is asked for", () => {
    const store = createUpdaterStore(api());
    const available = { state: "available", version: "0.2.0", notes: null } as const;
    store.getState().receive(available);
    expect(bannerFor(store.getState())).toEqual(available);
    store.getState().dismiss();
    expect(bannerFor(store.getState())).toBeNull();
    store.getState().receive({ state: "checking", manual: false });
    store.getState().receive(available);
    expect(bannerFor(store.getState())).toBeNull();
    store.getState().receive({ state: "checking", manual: true });
    store.getState().receive(available);
    expect(bannerFor(store.getState())).toEqual(available);
    // A newer version shows even after Later.
    store.getState().dismiss();
    store.getState().receive({ ...available, version: "0.3.0" });
    expect(bannerFor(store.getState())).toMatchObject({ version: "0.3.0" });
  });

  it("shows a check's result only when it was asked for", () => {
    const store = createUpdaterStore(api());
    for (const manual of [false, true]) {
      for (const status of [
        { state: "checking", manual },
        { state: "upToDate", manual },
        { state: "failed", message: "offline", manual },
      ] as const) {
        store.getState().receive(status);
        expect(bannerFor(store.getState())).toEqual(manual ? status : null);
      }
    }
    store.getState().dismiss();
    expect(store.getState().status).toEqual({ state: "idle" });
  });

  it("reports a refused command", async () => {
    const install = vi.fn().mockRejectedValue({ kind: "invalid", message: "there is no update to install" });
    const store = createUpdaterStore(api({ install }));
    await store.getState().install();
    expect(store.getState().error).toBe("there is no update to install");
    store.getState().dismiss();
    expect(store.getState().error).toBeNull();
  });

  it("formats progress", () => {
    expect(progressText(250, 1000)).toBe("25%");
    expect(progressText(1000, 1000)).toBe("100%");
    expect(progressText(3 * 1024 * 1024, null)).toBe("3.0 MB");
  });
});
