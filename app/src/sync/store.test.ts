import { describe, expect, it, vi } from "vitest";
import { emptyState, setGuideEnabled, type SyncBackend, type SyncState } from "@guiderecorder/sync/engine";
import type { SyncApi } from "@/lib/ipc";
import { createSyncStore } from "@/sync/store";

/** A signed-in app with guides `a` and `b`, the state on disk, and the server's pushes. */
function setup(stored: SyncState, user = { id: "u1", name: "Dana Ortiz", email: "dana@example.com" }) {
  const pushes: string[] = [];
  const guide = (id: string) => ({ id, title: id, items: [] }) as never;
  let saved: unknown = stored;
  const api: SyncApi = {
    account: async () => ({ server: "https://gr.example", signedIn: true }),
    signInStart: async () => Promise.reject(new Error("unused")),
    signInPoll: async () => "pending",
    signOut: async () => {},
    openWeb: async () => {},
    loadState: async () => structuredClone(saved),
    saveState: async (state) => void (saved = structuredClone(state)),
  };
  const backend = {
    request: async (_method: string, path: string, body: { guideId: string; guide: unknown }) => {
      if (path === "/api/auth/get-session") return { status: 200, body: { user } };
      if (path === "/api/auth/organization/list") return { status: 200, body: [{ id: "ws", name: "Acme" }] };
      if (path.includes("/sync/changes")) return { status: 200, body: { changes: [], cursor: "0", hasMore: false } };
      if (path.endsWith("/sync/push")) {
        pushes.push(body.guideId);
        return { status: 200, body: { revision: { id: `r${pushes.length}`, guide: body.guide, assets: {} } } };
      }
      return { status: 404, body: { error: "not found" } };
    },
    readGuide: async (id: string) => ({ guide: guide(id), path: id, readOnly: false }),
    prepareGuide: async (id: string) => ({ guide: guide(id), images: [] }),
    upload: async () => {},
    applyGuide: async () => "",
    duplicateGuide: async () => "",
    listGuideIds: async () => ["a", "b"],
  } as unknown as SyncBackend;
  return { store: createSyncStore({ api, backend }), pushes };
}

describe("sync store", () => {
  it("syncs only the guides turned on", async () => {
    const { store, pushes } = setup({ ...emptyState(), workspaceId: "ws", owner: "u1@https://gr.example" });
    await store.getState().load();
    await store.getState().setGuideEnabled("a", true);
    await vi.waitFor(() => expect(pushes).toEqual(["a"]));
    await vi.waitFor(() => expect(store.getState().running).toBe(false));
    expect(pushes).toEqual(["a"]);
  });

  it("doesn't carry another account's choices over", async () => {
    const theirs = { ...setGuideEnabled({ ...emptyState(), workspaceId: "ws" }, "b", true), library: true, owner: "u1@http://localhost:3000" };
    const { store, pushes } = setup(theirs);
    await store.getState().load();
    await store.getState().syncNow();
    expect(pushes).toEqual([]);
    expect(store.getState().state).toMatchObject({ library: false, guides: {}, owner: "u1@https://gr.example" });
  });

  it("keeps the choices made before owners were recorded", async () => {
    const { store } = setup(setGuideEnabled({ ...emptyState(), workspaceId: "ws" }, "a", true));
    await store.getState().load();
    expect(store.getState().state.guides.a?.enabled).toBe(true);
    expect(store.getState().state.owner).toBe("u1@https://gr.example");
  });
});
