import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { eq } from "drizzle-orm";
import { beforeEach, describe, expect, it } from "vitest";
import type { Guide } from "@guiderecorder/guide-schema";
import { imagePaths, pushRequest } from "@guiderecorder/sync";
import { testDb } from "../../test/db";
import type { Database } from "@/db";
import { asset, guide, organization, subscription, user } from "@/db/schema";
import {
  cookieReader,
  createShareLink,
  createShareRequest,
  listShareLinks,
  openShareLink,
  publicGuide,
  revokeShareLink,
  setSharingSettings,
  shareAsset,
  unlockShareLink,
  type Cookies,
} from "./sharing";
import { setPolicy } from "./policy";
import { push, trashGuide, type Actor } from "./sync";

const fixture: Guide = JSON.parse(
  readFileSync(new URL("../../../fixtures/guides/Rotate an API key.guiderecorder/guide.json", import.meta.url), "utf8"),
);
const hash = (text: string) => createHash("sha256").update(text).digest("hex");
const assetsFor = (g: Guide) => Object.fromEntries(imagePaths(g).map((p) => [p, hash(p)]));
const SECRET = "test-secret";
const DAY = 24 * 60 * 60 * 1000;
const noCookies: Cookies = () => undefined;

let db: Database;
const ada: Actor = { userId: "ada", role: "owner" };
const vic: Actor = { userId: "vic", role: "viewer" };

beforeEach(async () => {
  db = await testDb();
  await db.insert(organization).values({ id: "org", name: "Acme", slug: "acme", createdAt: new Date() });
  await db.insert(user).values(["ada", "grace", "vic"].map((id) => ({ id, name: id, email: `${id}@example.com` })));
  await db.insert(subscription).values({ id: "s", plan: "pro", referenceId: "org", status: "active" });
  await db
    .insert(asset)
    .values([...imagePaths(fixture), "other.webp"].map((p) => ({ organizationId: "org", sha256: hash(p), size: 10, contentType: "image/webp" })));
});

async function pushed(g: Guide, base: string | null, actor: Actor = ada) {
  const result = await push(db, "org", actor, pushRequest.parse({ guideId: g.id, baseRevision: base, guide: g, assets: assetsFor(g) }), "app");
  if (result.status !== 200) throw new Error(`push failed: ${JSON.stringify(result)}`);
  return result.revision;
}

async function created(input: Partial<ReturnType<typeof createShareRequest.parse>> = {}, now = new Date()) {
  const result = await createShareLink(db, "org", ada, createShareRequest.parse({ guideId: fixture.id, ...input }), now);
  if (result.status !== 200) throw new Error(`create failed: ${JSON.stringify(result)}`);
  return result.link;
}

const open = (token: string, cookies = noCookies, now = new Date()) => openShareLink(db, SECRET, token, cookies, now);

describe("share links", () => {
  it("shares the latest revision, counts views and revokes", async () => {
    const first = await pushed(fixture, null);
    const link = await created();
    expect(link).toMatchObject({ revisionId: null, hasPassword: false, views: 0, state: "active", createdBy: "ada" });

    expect(await open(link.token)).toMatchObject({ status: 200, shared: { guide: { title: fixture.title }, assets: assetsFor(fixture) } });
    await pushed({ ...structuredClone(fixture), title: "Newer" }, first.id);
    expect(await open(link.token)).toMatchObject({ status: 200, shared: { guide: { title: "Newer" } } });

    const listed = await listShareLinks(db, "org", ada, fixture.id);
    expect(listed).toMatchObject({ status: 200, links: [{ id: link.id, views: 2 }], settings: { shareLinks: "anyone" } });

    expect(await revokeShareLink(db, "org", ada, link.id)).toEqual({ status: 200 });
    expect((await open(link.token)).status).toBe(404);
    expect(await listShareLinks(db, "org", ada, fixture.id)).toMatchObject({ links: [] });
  });

  it("keeps a pinned revision", async () => {
    const first = await pushed(fixture, null);
    const link = await created({ revisionId: first.id });
    await pushed({ ...structuredClone(fixture), title: "Newer" }, first.id);
    expect(await open(link.token)).toMatchObject({ status: 200, shared: { guide: { title: fixture.title } } });
    const other = await createShareLink(db, "org", ada, createShareRequest.parse({ guideId: fixture.id, revisionId: "nope" }));
    expect(other.status).toBe(404);
  });

  it("shows only what's published when approvals gate the guide", async () => {
    await db.update(subscription).set({ plan: "enterprise", seats: 10 });
    const draft = await pushed(fixture, null);
    const link = await created();
    expect((await open(link.token)).status).toBe(404);

    // Approved (as approvals.ts would), then edited again: the link keeps the approved one.
    await db.update(guide).set({ publishedRevisionId: draft.id });
    await pushed({ ...structuredClone(fixture), title: "Unapproved" }, draft.id);
    expect(await open(link.token)).toMatchObject({ status: 200, shared: { guide: { title: fixture.title } } });
  });

  it("follows roles and guide visibility", async () => {
    await pushed(fixture, null);
    expect((await createShareLink(db, "org", vic, createShareRequest.parse({ guideId: fixture.id }))).status).toBe(403);
    expect((await listShareLinks(db, "org", vic, fixture.id)).status).toBe(403);
    await db.update(guide).set({ visibility: "private" });
    const grace: Actor = { userId: "grace", role: "editor" };
    expect((await createShareLink(db, "org", grace, createShareRequest.parse({ guideId: fixture.id }))).status).toBe(404);
  });

  it("shows only the guide", async () => {
    await pushed({ ...structuredClone(fixture), owner: "Platform team" }, null);
    const result = await open((await created()).token);
    if (result.status !== 200) throw new Error("not shared");
    expect(result.shared.guide).not.toHaveProperty("recording");
    expect(result.shared.guide).not.toHaveProperty("owner");
    expect(result.shared.guide.items?.some((i) => "aiState" in i)).toBe(false);
    expect(result.shared).not.toHaveProperty("organizationId");
  });

  it("serves only the shared revision's images", async () => {
    await pushed(fixture, null);
    const { token } = await created();
    const [path] = imagePaths(fixture);
    expect(await shareAsset(db, SECRET, token, hash(path!), noCookies)).toEqual({ status: 200, organizationId: "org" });
    expect((await shareAsset(db, SECRET, token, hash("other.webp"), noCookies)).status).toBe(404);
    expect((await shareAsset(db, SECRET, "not-a-real-token-at-all", hash(path!), noCookies)).status).toBe(404);
  });

  it("asks for the password", async () => {
    await pushed(fixture, null);
    const link = await created({ password: "open sesame" });
    expect(link.hasPassword).toBe(true);
    expect(await open(link.token)).toMatchObject({ status: 401, needsPassword: true });
    expect((await shareAsset(db, SECRET, link.token, hash(imagePaths(fixture)[0]!), noCookies)).status).toBe(401);
    expect((await unlockShareLink(db, SECRET, link.token, "wrong")).status).toBe(403);

    const unlocked = await unlockShareLink(db, SECRET, link.token, "open sesame");
    if (unlocked.status !== 200 || !unlocked.cookie) throw new Error("not unlocked");
    const cookies = cookieReader(`other=1; ${unlocked.cookie.name}=${unlocked.cookie.value}`);
    expect((await open(link.token, cookies)).status).toBe(200);
    expect((await open(link.token, cookieReader(`${unlocked.cookie.name}=forged`))).status).toBe(401);
    // Views count only when the guide is shown.
    expect(await listShareLinks(db, "org", ada, fixture.id)).toMatchObject({ links: [{ views: 1 }] });
  });

  it("expires", async () => {
    await pushed(fixture, null);
    const now = new Date();
    const link = await created({ expiresAt: new Date(now.getTime() + DAY).toISOString() }, now);
    expect((await open(link.token, noCookies, now)).status).toBe(200);
    expect(await open(link.token, noCookies, new Date(now.getTime() + 2 * DAY))).toMatchObject({ status: 410 });
    const past = await createShareLink(db, "org", ada, createShareRequest.parse({ guideId: fixture.id, expiresAt: new Date(now.getTime() - 1).toISOString() }));
    expect(past.status).toBe(400);
  });

  it("stops working when the guide is trashed or the plan lapses", async () => {
    await pushed(fixture, null);
    const { token } = await created();
    await trashGuide(db, "org", ada, fixture.id);
    expect((await open(token)).status).toBe(404);
    expect((await createShareLink(db, "org", ada, createShareRequest.parse({ guideId: fixture.id }))).status).toBe(404);

    await db.update(guide).set({ deletedAt: null });
    expect((await open(token)).status).toBe(200);
    await db.update(subscription).set({ status: "canceled" }).where(eq(subscription.id, "s"));
    expect((await open(token)).status).toBe(404);
  });
});

describe("workspace sharing settings", () => {
  it("turns links off", async () => {
    await pushed(fixture, null);
    const { token } = await created();
    await setSharingSettings(db, "org", { shareLinks: "off", shareLinkMaxDays: null });
    expect((await open(token)).status).toBe(404);
    expect((await createShareLink(db, "org", ada, createShareRequest.parse({ guideId: fixture.id }))).status).toBe(403);
    expect(await listShareLinks(db, "org", ada, fixture.id)).toMatchObject({ links: [{ state: "blocked" }], settings: { shareLinks: "off" } });
  });

  it("follows the workspace's sharing policy", async () => {
    await pushed(fixture, null);
    const { token } = await created();
    await setPolicy(db, "org", { sharing: "workspace" });
    expect((await open(token)).status).toBe(404);
    expect((await createShareLink(db, "org", ada, createShareRequest.parse({ guideId: fixture.id }))).status).toBe(403);
    await setPolicy(db, "org", { sharing: "public" });
    expect((await open(token)).status).toBe(200);
  });

  it("requires passwords", async () => {
    await pushed(fixture, null);
    const open1 = await created();
    await setSharingSettings(db, "org", { shareLinks: "password", shareLinkMaxDays: null });
    expect((await open(open1.token)).status).toBe(404);
    expect((await createShareLink(db, "org", ada, createShareRequest.parse({ guideId: fixture.id }))).status).toBe(400);
    expect((await created({ password: "secret!" })).state).toBe("active");
  });

  it("caps how long links last", async () => {
    await pushed(fixture, null);
    const now = new Date();
    const old = await created({}, now);
    await setSharingSettings(db, "org", { shareLinks: "anyone", shareLinkMaxDays: 7 });
    // Existing links get the cap too.
    expect(await open(old.token, noCookies, new Date(now.getTime() + 8 * DAY))).toMatchObject({ status: 410 });

    const capped = await created({}, now);
    expect(capped.expiresAt).toBe(new Date(now.getTime() + 7 * DAY).toISOString());
    const tooLong = await createShareLink(
      db,
      "org",
      ada,
      createShareRequest.parse({ guideId: fixture.id, expiresAt: new Date(now.getTime() + 30 * DAY).toISOString() }),
      now,
    );
    expect(tooLong).toMatchObject({ status: 400 });
  });
});

describe("publicGuide", () => {
  it("drops workspace-only fields", () => {
    const g = publicGuide({ ...fixture, owner: "x", lastReviewed: "2026-01-01", aiState: { status: "ai" } });
    expect(Object.keys(g)).not.toEqual(expect.arrayContaining(["owner"]));
    expect(g).not.toHaveProperty("lastReviewed");
    expect(g).not.toHaveProperty("aiState");
    expect(g.title).toBe(fixture.title);
    expect(g.items).toHaveLength(fixture.items!.length);
  });
});
