import { describe, expect, it } from "vitest";
import type { ApiResponse } from "@guiderecorder/sync/engine";
import { createShareStore, type ShareLink } from "./share";

const link = (id: string, extra: Partial<ShareLink> = {}): ShareLink => ({
  id,
  url: `https://app.example/s/${id}`,
  revisionId: null,
  expiresAt: null,
  hasPassword: false,
  allowDownload: false,
  views: 0,
  lastViewedAt: null,
  createdAt: "2026-10-19T00:00:00.000Z",
  createdBy: "Ada",
  state: "active",
  ...extra,
});

/** The web app's share API, in memory. */
function fakeServer({ status }: { status?: number } = {}) {
  const links: ShareLink[] = [];
  const calls: string[] = [];
  const backend = {
    async request<T>(method: "GET" | "POST" | "DELETE", path: string, body?: unknown): Promise<ApiResponse<T>> {
      calls.push(`${method} ${path}`);
      if (status) return { status, body: { error: "Nope." } as T };
      if (method === "GET") return { status: 200, body: { links: [...links], settings: { shareLinks: "anyone", shareLinkMaxDays: null } } as T };
      if (method === "POST") {
        const created = link(`l${links.length + 1}`, { revisionId: (body as { revisionId: string | null }).revisionId });
        links.unshift(created);
        return { status: 200, body: { link: created } as T };
      }
      const id = path.split("/").at(-1);
      links.splice(links.findIndex((l) => l.id === id), 1);
      return { status: 204, body: null as T };
    },
  };
  return { backend, links, calls };
}

describe("share store", () => {
  it("lists, creates and revokes a guide's links", async () => {
    const server = fakeServer();
    const store = createShareStore({ backend: server.backend, workspaceId: () => "ws" });
    await store.getState().open("g1", "r1");
    expect(server.calls).toEqual(["GET /api/workspaces/ws/shares?guideId=g1"]);
    expect(store.getState()).toMatchObject({ guideId: "g1", links: [], settings: { shareLinks: "anyone" } });

    const created = await store.getState().create({ revisionId: "r1", expiresAt: null, password: null, allowDownload: true });
    expect(created).toMatchObject({ id: "l1", revisionId: "r1" });
    expect(store.getState().links.map((l) => l.id)).toEqual(["l1"]);

    await store.getState().revoke("l1");
    expect(server.calls.at(-2)).toBe("DELETE /api/workspaces/ws/shares/l1");
    expect(store.getState().links).toEqual([]);

    store.getState().close();
    expect(store.getState().guideId).toBeNull();
  });

  it("explains refusals", async () => {
    const store = createShareStore({ backend: fakeServer({ status: 402 }).backend, workspaceId: () => "ws" });
    await store.getState().open("g1", null);
    expect(store.getState().error).toMatch(/Pro/);
    expect(await store.getState().create({ revisionId: null, expiresAt: null, password: null, allowDownload: false })).toBeNull();

    const forbidden = createShareStore({ backend: fakeServer({ status: 403 }).backend, workspaceId: () => "ws" });
    await forbidden.getState().open("g1", null);
    expect(forbidden.getState().error).toBe("Nope.");
  });

  it("does nothing without a workspace", async () => {
    const server = fakeServer();
    const store = createShareStore({ backend: server.backend, workspaceId: () => null });
    await store.getState().open("g1", null);
    expect(server.calls).toEqual([]);
  });
});
