import { beforeEach, describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { equal, imagePaths, type AssetMap, type Change, type ReviewState, type Revision } from "./index.ts";
import type { Guide, Step } from "@guiderecorder/guide-schema";
import {
  emptyState,
  canSubmitForReview,
  resolveConflict,
  setGuideEnabled,
  submitForReview,
  syncOnce,
  type ApiResponse,
  type SyncBackend,
  type SyncState,
} from "./engine.ts";

const fixture: Guide = JSON.parse(readFileSync(new URL("../../../fixtures/guides/Rotate an API key.guiderecorder/guide.json", import.meta.url), "utf8"));
const WS = "ws";
const sha = (path: string) => path.replace(/[^0-9a-f]/g, "").padEnd(64, "0").slice(0, 64);

/** The web app's sync API, in memory, with the server's push rules. */
class FakeServer {
  seq = 0;
  guides = new Map<string, { head: Revision; seq: number; deleted: boolean }>();
  assets = new Set<string>();
  pushes = 0;
  /** Approvals on: pushes are drafts. */
  gated = false;
  submitted: string[] = [];

  review(status: ReviewState["status"], changeRequestId: string | null): ReviewState {
    return { gated: true, status, changeRequestId, approvals: 0, required: 1 };
  }

  commit(id: string, guide: Guide, assets: AssetMap, parentId: string | null, source: "app" | "web" = "web"): Revision {
    const revision: Revision = {
      id: `r${++this.seq}`,
      guideId: id,
      parentId,
      guide: structuredClone(guide),
      assets,
      authorId: "u",
      source,
      createdAt: new Date().toISOString(),
    };
    this.guides.set(id, { head: revision, seq: this.seq, deleted: false });
    return revision;
  }

  /** An edit made on the web. */
  edit(id: string, change: (g: Guide) => void) {
    const current = this.guides.get(id)!;
    const guide = structuredClone(current.head.guide) as unknown as Guide;
    change(guide);
    return this.commit(id, guide, current.head.assets, current.head.id);
  }

  delete(id: string) {
    this.guides.get(id)!.deleted = true;
    this.guides.get(id)!.seq = ++this.seq;
  }

  handle(_method: string, path: string, body: any): ApiResponse<any> {
    const url = new URL(`http://x${path}`);
    if (url.pathname.endsWith("/sync/changes")) {
      const since = Number(url.searchParams.get("since"));
      const changes: Change[] = [...this.guides.values()]
        .filter((g) => g.seq > since)
        .sort((a, b) => a.seq - b.seq)
        .map((g) =>
          g.deleted
            ? { type: "deleted", guideId: g.head.guideId, seq: String(g.seq), deletedAt: "", purgeAt: "" }
            : { type: "revision", guideId: g.head.guideId, seq: String(g.seq), revision: g.head },
        );
      return { status: 200, body: { changes, cursor: String(changes.at(-1)?.seq ?? since), hasMore: false } };
    }
    if (url.pathname.endsWith("/assets/uploads")) {
      const missing = body.assets.filter((a: { sha256: string }) => !this.assets.has(a.sha256));
      return { status: 200, body: { present: [], uploads: missing.map((a: { sha256: string }) => ({ sha256: a.sha256, url: "u", method: "PUT", headers: {} })) } };
    }
    if (url.pathname.endsWith("/assets/commit")) {
      for (const h of body.sha256) this.assets.add(h);
      return { status: 200, body: { committed: body.sha256, missing: [] } };
    }
    if (url.pathname.endsWith("/sync/push")) {
      this.pushes++;
      for (const p of imagePaths(body.guide)) if (!this.assets.has(body.assets[p])) return { status: 422, body: { error: "missing" } };
      const current = this.guides.get(body.guideId);
      if (current?.deleted) return { status: 410, body: { error: "trashed" } };
      if (current && equal(current.head.guide, body.guide)) return { status: 200, body: { revision: current.head } };
      if (current && body.baseRevision !== current.head.id) return { status: 409, body: { error: "moved", head: current.head } };
      // A gated guide: the server sets its status, so a push can't publish it.
      const guide = this.gated ? { ...body.guide, status: "draft" } : body.guide;
      const revision = this.commit(body.guideId, guide, body.assets, current?.head.id ?? null, "app");
      return { status: 200, body: { revision, review: this.gated ? this.review("draft", null) : undefined } };
    }
    if (url.pathname.endsWith("/review")) {
      this.submitted.push(body.revisionId);
      return { status: 200, body: { review: this.review("inReview", "cr1") } };
    }
    return { status: 404, body: { error: "not found" } };
  }
}

/** The library on disk, in memory. */
class FakeLibrary implements SyncBackend {
  guides = new Map<string, Guide>();
  open = new Set<string>();
  duplicates: string[] = [];

  constructor(readonly server: FakeServer) {}

  async request<T>(method: "GET" | "POST" | "DELETE", path: string, body?: unknown) {
    return this.server.handle(method, path, structuredClone(body)) as ApiResponse<T>;
  }
  async readGuide(id: string) {
    const guide = this.guides.get(id);
    return guide ? { guide: structuredClone(guide), path: `/lib/${id}`, readOnly: false } : null;
  }
  async prepareGuide(id: string) {
    const guide = structuredClone(this.guides.get(id)!);
    const images = imagePaths(guide).map((path) => ({ path, sha256: sha(path), size: 1, contentType: "image/webp" }));
    return { guide, images };
  }
  async upload() {}
  async applyGuide(_ws: string, guide: Guide) {
    if (this.open.has(guide.id)) throw { kind: "conflict", message: "open in the editor" };
    // Saving stamps the time, as `Library::edit` does.
    this.guides.set(guide.id, { ...structuredClone(guide), updatedAt: new Date(Date.now() + 1000).toISOString() });
    return `/lib/${guide.id}`;
  }
  async duplicateGuide(id: string, _path: string, title: string) {
    const copy = { ...structuredClone(this.guides.get(id)!), id: `${id}-copy`, title };
    this.guides.set(copy.id, copy);
    this.duplicates.push(copy.id);
    return copy.id;
  }
  async listGuideIds() {
    return [...this.guides.keys()];
  }

  edit(id: string, change: (g: Guide) => void) {
    const guide = structuredClone(this.guides.get(id)!);
    change(guide);
    this.guides.set(id, guide);
  }
}

const steps = (guide: Guide) => (guide.items as Step[]).filter((i) => i.type === "step");
const stepTitle = (guide: Guide, n: number) => steps(guide)[n]!.title;
const setStepTitle = (n: number, title: string) => (g: Guide) => (steps(g)[n]!.title = title);

let server: FakeServer;
let library: FakeLibrary;
let state: SyncState;
const id = fixture.id;
const run = async () => (state = await syncOnce(library, state));

beforeEach(() => {
  server = new FakeServer();
  library = new FakeLibrary(server);
  library.guides.set(id, structuredClone(fixture));
  state = { ...emptyState(), workspaceId: WS };
});

describe("syncOnce", () => {
  it("syncs nothing until a guide is turned on", async () => {
    await run();
    expect(server.pushes).toBe(0);
    expect(server.guides.size).toBe(0);
  });

  it("pushes a guide turned on, then only when it changes", async () => {
    state = setGuideEnabled(state, id, true);
    await run();
    expect(server.guides.get(id)!.head.guide.title).toBe(fixture.title);
    expect(state.guides[id]).toMatchObject({ status: "synced", revisionId: "r1" });

    await run();
    expect(server.pushes).toBe(1);

    library.edit(id, (g) => (g.title = "Edited here"));
    await run();
    expect(server.guides.get(id)!.head.guide.title).toBe("Edited here");
    expect(server.pushes).toBe(2);
  });

  it("pulls edits made on the web", async () => {
    state = setGuideEnabled(state, id, true);
    await run();
    server.edit(id, setStepTitle(0, "From the web"));
    await run();
    expect(stepTitle(library.guides.get(id)!, 0)).toBe("From the web");
    expect(state.guides[id]).toMatchObject({ status: "synced", revisionId: "r2" });
    // Writing it here isn't a change to push back.
    await run();
    expect(server.pushes).toBe(1);
  });

  it("merges concurrent edits to different steps on both sides", async () => {
    state = setGuideEnabled(state, id, true);
    await run();
    server.edit(id, setStepTitle(0, "Web"));
    library.edit(id, setStepTitle(1, "Desktop"));
    await run();
    for (const guide of [library.guides.get(id)!, server.guides.get(id)!.head.guide]) {
      expect(stepTitle(guide, 0)).toBe("Web");
      expect(stepTitle(guide, 1)).toBe("Desktop");
    }
    expect(state.guides[id]!.status).toBe("synced");
  });

  it("merges when a push finds the server moved on", async () => {
    state = setGuideEnabled(state, id, true);
    await run();
    library.edit(id, setStepTitle(1, "Desktop"));
    // The web edit lands after this run's pull.
    const realHandle = server.handle.bind(server);
    let edited = false;
    server.handle = (method, path, body) => {
      if (path.endsWith("/push") && !edited) {
        edited = true;
        server.edit(id, setStepTitle(0, "Web"));
      }
      return realHandle(method, path, body);
    };
    await run();
    expect(stepTitle(server.guides.get(id)!.head.guide, 0)).toBe("Web");
    expect(stepTitle(server.guides.get(id)!.head.guide, 1)).toBe("Desktop");
  });

  it("stops at same-field edits until the user picks", async () => {
    state = setGuideEnabled(state, id, true);
    await run();
    server.edit(id, setStepTitle(0, "Web"));
    library.edit(id, setStepTitle(0, "Desktop"));
    await run();
    expect(state.guides[id]).toMatchObject({ status: "conflict", conflicts: [{ kind: "field", field: "title" }] });
    // Nothing is overwritten while it waits.
    await run();
    expect(stepTitle(library.guides.get(id)!, 0)).toBe("Desktop");
    expect(stepTitle(server.guides.get(id)!.head.guide, 0)).toBe("Web");
    return state;
  });

  it.each([
    ["mine", "Desktop"],
    ["theirs", "Web"],
  ] as const)("resolves a conflict keeping %s", async (resolution, expected) => {
    state = setGuideEnabled(state, id, true);
    await run();
    server.edit(id, setStepTitle(0, "Web"));
    library.edit(id, (g) => ((steps(g)[0]!.title = "Desktop"), (steps(g)[1]!.title = "Also desktop")));
    await run();

    state = await resolveConflict(library, state, id, resolution);
    expect(state.guides[id]!.status).toBe("synced");
    for (const guide of [library.guides.get(id)!, server.guides.get(id)!.head.guide]) {
      expect(stepTitle(guide, 0)).toBe(expected);
      // Changes that didn't conflict stay either way.
      expect(stepTitle(guide, 1)).toBe("Also desktop");
    }
  });

  it("resolves a conflict keeping both as a conflicted copy", async () => {
    state = setGuideEnabled(state, id, true);
    await run();
    server.edit(id, setStepTitle(0, "Web"));
    library.edit(id, setStepTitle(0, "Desktop"));
    await run();
    state = await resolveConflict(library, state, id, "both");
    expect(stepTitle(library.guides.get(id)!, 0)).toBe("Web");
    const copy = library.guides.get(library.duplicates[0]!)!;
    expect(copy.title).toBe(`${fixture.title} (conflicted copy)`);
    expect(stepTitle(copy, 0)).toBe("Desktop");
    expect(state.guides[id]!.status).toBe("synced");
  });

  it("waits for a guide open in the editor, then writes the pulled change", async () => {
    state = setGuideEnabled(state, id, true);
    await run();
    server.edit(id, setStepTitle(0, "Web"));
    library.open.add(id);
    await run();
    expect(state.guides[id]!.status).toBe("pending");
    expect(stepTitle(library.guides.get(id)!, 0)).toBe(stepTitle(fixture, 0));

    library.open.delete(id);
    await run();
    expect(stepTitle(library.guides.get(id)!, 0)).toBe("Web");
    expect(state.guides[id]!.status).toBe("synced");
  });

  it("syncing the library pulls guides made elsewhere and pushes every local guide", async () => {
    const other = { ...structuredClone(fixture), id: "01M4WZCE2000000000000009AA", title: "From elsewhere" };
    server.commit(other.id, other, Object.fromEntries(imagePaths(other).map((p) => [p, sha(p)])), null);
    state = { ...state, library: true };
    await run();
    expect(library.guides.get(other.id)!.title).toBe("From elsewhere");
    expect(server.guides.get(id)).toBeDefined();
  });

  it("stops syncing a guide deleted on the web", async () => {
    state = setGuideEnabled(state, id, true);
    await run();
    server.delete(id);
    await run();
    expect(state.guides[id]).toMatchObject({ status: "deleted", enabled: false });
    expect(library.guides.get(id)).toBeDefined();
    library.edit(id, (g) => (g.title = "Still here"));
    await run();
    expect(server.pushes).toBe(1);
  });
});

describe("approvals", () => {
  beforeEach(() => {
    server.gated = true;
    state = setGuideEnabled(state, id, true);
  });

  it("takes the server's status for a gated guide: a push doesn't publish it", async () => {
    library.edit(id, (g) => (g.status = "published"));
    await run();
    expect(library.guides.get(id)!.status).toBe("draft");
    expect(state.guides[id]).toMatchObject({ status: "synced", review: { gated: true, status: "draft" } });
    // Nothing left to push.
    await run();
    expect(server.pushes).toBe(1);
  });

  it("writes the server's version once the guide closes in the editor", async () => {
    library.open.add(id);
    library.edit(id, (g) => (g.status = "published"));
    await run();
    expect(library.guides.get(id)!.status).toBe("published");
    library.open.delete(id);
    await run();
    expect(library.guides.get(id)!.status).toBe("draft");
    expect(server.pushes).toBe(1);
  });

  it("submits the synced revision for review", async () => {
    await run();
    expect(canSubmitForReview(state.guides[id])).toBe(true);
    const result = await submitForReview(library, state, id, "Please check");
    expect(result.error).toBeNull();
    expect(server.submitted).toEqual([state.guides[id]!.revisionId]);
    expect(result.state.guides[id]!.review).toMatchObject({ status: "inReview", changeRequestId: "cr1" });
    expect(canSubmitForReview(result.state.guides[id])).toBe(false);
  });

  it("won't submit changes that haven't synced", async () => {
    await run();
    library.edit(id, (g) => (g.title = "Not pushed yet"));
    expect((await submitForReview(library, state, id)).error).toMatch(/syncing/);
    expect(server.submitted).toEqual([]);
  });

  it("hears review steps through the feed", async () => {
    await run();
    const g = server.guides.get(id)!;
    g.seq = ++server.seq;
    const original = server.handle.bind(server);
    server.handle = (method, path, body) => {
      const response = original(method, path, body);
      if (path.includes("/sync/changes")) for (const c of response.body.changes) c.review = server.review("needsUpdate", "cr1");
      return response;
    };
    await run();
    expect(state.guides[id]!.review).toMatchObject({ status: "needsUpdate" });
    expect(server.pushes).toBe(1);
  });
});
