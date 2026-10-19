import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { beforeEach, describe, expect, it } from "vitest";
import type { Guide } from "@guiderecorder/guide-schema";
import { imagePaths, pushRequest } from "@guiderecorder/sync";
import { testDb } from "../../test/db";
import type { Database } from "@/db";
import { asset, organization, user } from "@/db/schema";
import { changes, getGuide, listGuides, purgeTrash, push, restoreGuide, trashGuide, type Actor } from "./sync";

const fixture: Guide = JSON.parse(
  readFileSync(new URL("../../../fixtures/guides/Rotate an API key.guiderecorder/guide.json", import.meta.url), "utf8"),
);
const hash = (text: string) => createHash("sha256").update(text).digest("hex");
const assetsFor = (guide: Guide) => Object.fromEntries(imagePaths(guide).map((p) => [p, hash(p)]));

let db: Database;
const ada: Actor = { userId: "ada", role: "owner" };
const grace: Actor = { userId: "grace", role: "editor" };
const viewer: Actor = { userId: "vic", role: "viewer" };

beforeEach(async () => {
  db = await testDb();
  await db.insert(organization).values({ id: "org", name: "Acme", slug: "acme", createdAt: new Date() });
  await db.insert(user).values(["ada", "grace", "vic"].map((id) => ({ id, name: id, email: `${id}@example.com` })));
  await db
    .insert(asset)
    .values(imagePaths(fixture).map((p) => ({ organizationId: "org", sha256: hash(p), size: 10, contentType: "image/webp" })));
});

const request = (guide: Guide, baseRevision: string | null, assets = assetsFor(guide)) =>
  pushRequest.parse({ guideId: guide.id, baseRevision, guide, assets });

const retitled = (title: string): Guide => ({ ...structuredClone(fixture), title });

async function pushed(actor: Actor, guide: Guide, base: string | null) {
  const result = await push(db, "org", actor, request(guide, base), "app");
  if (result.status !== 200) throw new Error(`push failed: ${JSON.stringify(result)}`);
  return result.revision;
}

describe("push", () => {
  it("creates the guide, then adds revisions on its head", async () => {
    const first = await pushed(ada, fixture, null);
    expect(first).toMatchObject({ guideId: fixture.id, parentId: null, authorId: "ada", source: "app" });
    expect(first.assets).toEqual(assetsFor(fixture));

    const second = await pushed(grace, retitled("New title"), first.id);
    expect(second.parentId).toBe(first.id);
    expect((await getGuide(db, "org", "ada", fixture.id))!.head.guide.title).toBe("New title");
  });

  it("returns the head when the change wasn't made on it", async () => {
    const first = await pushed(ada, fixture, null);
    const second = await pushed(ada, retitled("Ada's"), first.id);
    const stale = await push(db, "org", grace, request(retitled("Grace's"), first.id), "web");
    expect(stale).toMatchObject({ status: 409, head: { id: second.id, guide: { title: "Ada's" } } });
    // A guide that already exists can't be created again.
    expect((await push(db, "org", grace, request(retitled("Grace's"), null), "web")).status).toBe(409);
  });

  it("accepts a retried push that already landed", async () => {
    const first = await pushed(ada, fixture, null);
    const second = await pushed(ada, retitled("Again"), first.id);
    expect(await push(db, "org", ada, request(retitled("Again"), first.id), "app")).toMatchObject({ status: 200, revision: { id: second.id } });
  });

  it("rejects invalid guides and mismatched ids", async () => {
    const bad = { ...structuredClone(fixture), items: [{ type: "step", kind: "teleport" }] } as unknown as Guide;
    expect((await push(db, "org", ada, request(bad, null, {}), "app")).status).toBe(400);
    const other = pushRequest.parse({ guideId: "01M4WZCE2000000000000009ZZ", baseRevision: null, guide: fixture, assets: assetsFor(fixture) });
    expect((await push(db, "org", ada, other, "app")).status).toBe(400);
  });

  it("requires every image path to name a committed asset", async () => {
    const [first, ...rest] = imagePaths(fixture);
    const without = Object.fromEntries(rest.map((p) => [p, hash(p)]));
    expect(await push(db, "org", ada, request(fixture, null, without), "app")).toMatchObject({ status: 400, missingPaths: [first] });
    expect(await push(db, "org", ada, request(fixture, null, { ...assetsFor(fixture), "extra.webp": hash("x") }), "app")).toMatchObject({
      status: 400,
      extraPaths: ["extra.webp"],
    });
    const uncommitted = { ...assetsFor(fixture), [first!]: hash("not uploaded") };
    expect(await push(db, "org", ada, request(fixture, null, uncommitted), "app")).toMatchObject({ status: 422, missing: [hash("not uploaded")] });
  });

  it("follows the role's permissions", async () => {
    expect((await push(db, "org", viewer, request(fixture, null), "app")).status).toBe(403);
  });
});

describe("changes", () => {
  it("lists each changed guide's head after the cursor", async () => {
    const empty = await changes(db, "org", "ada", 0n);
    expect(empty).toEqual({ changes: [], cursor: "0", hasMore: false });

    const first = await pushed(ada, fixture, null);
    const page = await changes(db, "org", "grace", 0n);
    expect(page.changes).toMatchObject([{ type: "revision", guideId: fixture.id, revision: { id: first.id } }]);

    expect((await changes(db, "org", "grace", BigInt(page.cursor))).changes).toEqual([]);
    const second = await pushed(ada, retitled("Edited"), first.id);
    const next = await changes(db, "org", "grace", BigInt(page.cursor));
    expect(next.changes).toMatchObject([{ type: "revision", revision: { id: second.id } }]);
    expect(BigInt(next.cursor)).toBeGreaterThan(BigInt(page.cursor));
  });

  it("pages", async () => {
    for (const n of [1, 2, 3]) await pushed(ada, { ...structuredClone(fixture), id: `01M4WZCE200000000000000${n}AA` }, null);
    const first = await changes(db, "org", "ada", 0n, 2);
    expect(first.changes).toHaveLength(2);
    expect(first.hasMore).toBe(true);
    const rest = await changes(db, "org", "ada", BigInt(first.cursor), 2);
    expect(rest.changes).toHaveLength(1);
    expect(rest.hasMore).toBe(false);
  });

  it("doesn't show other workspaces' guides", async () => {
    await db.insert(organization).values({ id: "other", name: "Other", slug: "other", createdAt: new Date() });
    await pushed(ada, fixture, null);
    expect((await changes(db, "other", "ada", 0n)).changes).toEqual([]);
  });
});

describe("trash", () => {
  it("tombstones deleted guides in the feed and refuses pushes until restored", async () => {
    const first = await pushed(ada, fixture, null);
    const { cursor } = await changes(db, "org", "ada", 0n);
    expect((await trashGuide(db, "org", grace, fixture.id)).status).toBe(403);
    expect((await trashGuide(db, "org", ada, fixture.id, new Date("2026-10-01T00:00:00Z"))).status).toBe(200);

    expect((await changes(db, "org", "ada", BigInt(cursor))).changes).toMatchObject([
      { type: "deleted", guideId: fixture.id, deletedAt: "2026-10-01T00:00:00.000Z", purgeAt: "2026-10-31T00:00:00.000Z" },
    ]);
    expect(await push(db, "org", ada, request(retitled("Late"), first.id), "app")).toMatchObject({ status: 410 });
    expect(await listGuides(db, "org", "ada")).toEqual([]);
    expect(await listGuides(db, "org", "ada", { trashed: true })).toHaveLength(1);

    expect((await restoreGuide(db, "org", ada, fixture.id)).status).toBe(200);
    expect((await changes(db, "org", "ada", BigInt(cursor))).changes).toMatchObject([{ type: "revision", revision: { id: first.id } }]);
  });

  it("purges revisions after 30 days, keeping the tombstone", async () => {
    await pushed(ada, fixture, null);
    await trashGuide(db, "org", ada, fixture.id, new Date("2026-10-01T00:00:00Z"));
    expect(await purgeTrash(db, new Date("2026-10-30T00:00:00Z"))).toBe(0);
    expect(await purgeTrash(db, new Date("2026-11-01T00:00:00Z"))).toBe(1);
    expect(await getGuide(db, "org", "ada", fixture.id)).toBeNull();
    expect((await changes(db, "org", "ada", 0n)).changes).toMatchObject([{ type: "deleted" }]);

    // The guide can be synced again from scratch.
    expect((await pushed(grace, fixture, null)).parentId).toBeNull();
    expect((await changes(db, "org", "ada", 0n)).changes).toMatchObject([{ type: "revision" }]);
  });
});
