import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { eq } from "drizzle-orm";
import { beforeEach, describe, expect, it } from "vitest";
import type { Guide } from "@guiderecorder/guide-schema";
import { imagePaths, pushRequest } from "@guiderecorder/sync";
import { testDb } from "../../test/db";
import { memoryStore } from "../../test/store";
import type { Database } from "@/db";
import { asset, changeRequest, guide, organization, revision, session, shareLink, user } from "@/db/schema";
import { assetKey } from "./assets";
import { setPolicy } from "./policy";
import { recordAudit, setAuditRetention } from "./audit";
import { deleteOrphanAssets, pruneRevisions, runRetention } from "./retention";
import { push, trashGuide, type Actor } from "./sync";

const fixture: Guide = JSON.parse(
  readFileSync(new URL("../../../fixtures/guides/Rotate an API key.guiderecorder/guide.json", import.meta.url), "utf8"),
);
const hash = (text: string) => createHash("sha256").update(text).digest("hex");
const assetsFor = (g: Guide) => Object.fromEntries(imagePaths(g).map((p) => [p, hash(p)]));
const DAY = 24 * 60 * 60 * 1000;
const NOW = new Date("2026-10-19T12:00:00Z");
const ada: Actor = { userId: "ada", role: "owner" };

let db: Database;
let s3: ReturnType<typeof memoryStore>;

beforeEach(async () => {
  db = await testDb();
  s3 = memoryStore();
  await db.insert(organization).values({ id: "org", name: "Acme", slug: "acme", createdAt: new Date() });
  await db.insert(user).values({ id: "ada", name: "Ada", email: "ada@acme.com" });
  await db
    .insert(asset)
    .values(imagePaths(fixture).map((p) => ({ organizationId: "org", sha256: hash(p), size: 10, contentType: "image/webp", createdAt: new Date(NOW.getTime() - 10 * DAY) })));
});

async function pushed(g: Guide, base: string | null) {
  const result = await push(db, "org", ada, pushRequest.parse({ guideId: g.id, baseRevision: base, guide: g, assets: assetsFor(g) }), "app");
  if (result.status !== 200) throw new Error(JSON.stringify(result));
  return result.revision.id;
}

/** Three revisions, all 100 days old; the first is published. */
async function history() {
  const first = await pushed(fixture, null);
  const second = await pushed({ ...structuredClone(fixture), title: "Second" }, first);
  const third = await pushed({ ...structuredClone(fixture), title: "Third" }, second);
  await db.update(guide).set({ publishedRevisionId: first });
  await db.update(revision).set({ createdAt: new Date(NOW.getTime() - 100 * DAY) });
  return { first, second, third };
}

const revisionIds = async () => (await db.select({ id: revision.id }).from(revision)).map((r) => r.id).sort();

describe("revision retention", () => {
  it("keeps everything unless the workspace sets a period", async () => {
    await history();
    expect(await pruneRevisions(db, NOW)).toBe(0);
  });

  it("deletes old revisions but never a head or published one", async () => {
    const { first, third } = await history();
    await setPolicy(db, "org", { revisionRetentionDays: 30 });
    expect(await pruneRevisions(db, NOW)).toBe(1);
    expect(await revisionIds()).toEqual([first, third].sort());
    // Younger than the period: kept.
    await db.update(revision).set({ createdAt: new Date(NOW.getTime() - 10 * DAY) });
    await pushed({ ...structuredClone(fixture), title: "Fourth" }, third);
    expect(await pruneRevisions(db, NOW)).toBe(0);
  });

  it("keeps revisions a share link pins or a change request names", async () => {
    const { first, second, third } = await history();
    const link = { organizationId: "org", guideId: fixture.id, revisionId: second };
    await db.insert(shareLink).values({ id: "l", token: "t", ...link });
    await db.insert(changeRequest).values({ id: "cr", organizationId: "org", guideId: fixture.id, revisionId: second, status: "approved" });
    await setPolicy(db, "org", { revisionRetentionDays: 30 });
    expect(await pruneRevisions(db, NOW)).toBe(0);

    // Once neither names it, it goes.
    await db.update(shareLink).set({ revokedAt: NOW });
    await db.delete(changeRequest);
    expect(await pruneRevisions(db, NOW)).toBe(1);
    expect(await revisionIds()).toEqual([first, third].sort());
  });
});

describe("orphan images", () => {
  it("are deleted from storage and the database after a grace period", async () => {
    await pushed(fixture, null);
    const old = hash("unused old");
    const fresh = hash("unused fresh");
    await db.insert(asset).values([
      { organizationId: "org", sha256: old, size: 1, contentType: "image/webp", createdAt: new Date(NOW.getTime() - 2 * DAY) },
      { organizationId: "org", sha256: fresh, size: 1, contentType: "image/webp", createdAt: new Date(NOW.getTime() - 60_000) },
    ]);
    for (const h of [old, fresh, ...Object.values(assetsFor(fixture))]) s3.put(assetKey("org", h), 1);

    expect(await deleteOrphanAssets(db, s3.store, NOW)).toBe(1);
    expect(s3.objects.has(assetKey("org", old))).toBe(false);
    expect(s3.objects.has(assetKey("org", fresh))).toBe(true);
    // Images a revision names stay.
    for (const h of Object.values(assetsFor(fixture))) expect(s3.objects.has(assetKey("org", h))).toBe(true);
    expect((await db.select().from(asset).where(eq(asset.sha256, old))).length).toBe(0);
  });
});

describe("runRetention", () => {
  it("purges the trash, expired sessions and the images left behind", async () => {
    await pushed(fixture, null);
    for (const h of Object.values(assetsFor(fixture))) s3.put(assetKey("org", h), 1);
    await trashGuide(db, "org", ada, fixture.id, new Date(NOW.getTime() - 40 * DAY));
    await db.insert(session).values([
      { id: "old", token: "a", userId: "ada", expiresAt: new Date(NOW.getTime() - 1000) },
      { id: "live", token: "b", userId: "ada", expiresAt: new Date(NOW.getTime() + DAY) },
    ]);

    const report = await runRetention(db, s3.store, NOW);
    expect(report).toMatchObject({ trashedGuides: 1, assets: imagePaths(fixture).length, expired: { sessions: 1 } });
    expect(s3.objects.size).toBe(0);
    expect((await db.select().from(session)).map((s) => s.id)).toEqual(["live"]);
  });

  it("prunes audit events past the workspace's audit retention", async () => {
    await recordAudit(db, "org", { actorId: "ada", action: "guide.edit", targetType: "guide", targetId: "g", at: new Date(NOW.getTime() - 400 * DAY) });
    await setAuditRetention(db, "org", { userId: "ada" }, 365);
    expect((await runRetention(db, s3.store, NOW)).auditEvents).toBe(1);
  });
});
