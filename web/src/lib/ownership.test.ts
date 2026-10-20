import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { beforeEach, describe, expect, it } from "vitest";
import type { Guide } from "@guiderecorder/guide-schema";
import { imagePaths, pushRequest } from "@guiderecorder/sync";
import { testDb } from "../../test/db";
import type { Database } from "@/db";
import { asset, member, organization, subscription, user } from "@/db/schema";
import { getReview, submitForReview } from "./approvals";
import { listAudit } from "./audit";
import { addOwner, guideAccess, removeOwner, setLocked } from "./ownership";
import { getGuide, push, type Actor } from "./sync";

const fixture: Guide = JSON.parse(
  readFileSync(new URL("../../../fixtures/guides/Rotate an API key.guiderecorder/guide.json", import.meta.url), "utf8"),
);
const hash = (text: string) => createHash("sha256").update(text).digest("hex");
const assetsFor = (guide: Guide) => Object.fromEntries(imagePaths(guide).map((p) => [p, hash(p)]));

let db: Database;
const ada: Actor = { userId: "ada", role: "admin" };
const grace: Actor = { userId: "grace", role: "editor" };
const eve: Actor = { userId: "eve", role: "editor" };
const vic: Actor = { userId: "vic", role: "viewer" };

beforeEach(async () => {
  db = await testDb();
  const now = new Date();
  await db.insert(organization).values({ id: "org", name: "Acme", slug: "acme", createdAt: now });
  for (const a of [ada, grace, eve, vic]) {
    await db.insert(user).values({ id: a.userId, name: a.userId, email: `${a.userId}@example.com` });
    await db.insert(member).values({ id: `m-${a.userId}`, organizationId: "org", userId: a.userId, role: a.role, createdAt: now });
  }
  await db
    .insert(asset)
    .values(imagePaths(fixture).map((p) => ({ organizationId: "org", sha256: hash(p), size: 10, contentType: "image/webp" })));
  await db.insert(subscription).values({ id: "sub", plan: "enterprise", referenceId: "org", status: "active", seats: 10 });
});

async function pushed(actor: Actor, guide: Guide, base: string | null) {
  const result = await push(db, "org", actor, pushRequest.parse({ guideId: guide.id, baseRevision: base, guide, assets: assetsFor(guide) }), "app");
  if (result.status !== 200) throw new Error(`push failed: ${JSON.stringify(result)}`);
  return result;
}
const access = async (actor: Actor) => (await guideAccess(db, "org", actor, fixture.id))!;
const head = async () => (await getGuide(db, "org", grace, fixture.id))!;

describe("ownership", () => {
  it("makes whoever syncs a guide first its owner, and starts it unlocked", async () => {
    await pushed(grace, fixture, null);
    await pushed(eve, { ...fixture, title: "Edited" }, (await head()).guide.headRevisionId);
    expect(await access(grace)).toMatchObject({ locked: false, owners: [{ id: "grace" }], canManage: true, canLock: true });
    expect((await access(eve)).canManage).toBe(false);
    expect((await access(ada)).canManage).toBe(true);
    expect((await access(grace)).candidates.map((c) => c.id)).toEqual(["ada", "eve"]);
  });

  it("lets owners and admins add and remove owners; a guide keeps one", async () => {
    await pushed(grace, fixture, null);
    expect(await addOwner(db, "org", eve, fixture.id, "eve")).toMatchObject({ status: 403 });
    expect(await addOwner(db, "org", grace, fixture.id, "vic")).toMatchObject({ status: 400 });
    expect(await addOwner(db, "org", grace, fixture.id, "eve")).toMatchObject({ status: 200 });
    expect((await access(eve)).owners.map((o) => o.id)).toEqual(["grace", "eve"]);
    expect(await removeOwner(db, "org", ada, fixture.id, "grace")).toMatchObject({ status: 200 });
    expect(await removeOwner(db, "org", eve, fixture.id, "eve")).toMatchObject({ status: 400 });
    expect(await removeOwner(db, "org", vic, fixture.id, "eve")).toMatchObject({ status: 403 });
    const actions = (await listAudit(db, "org")).map((e) => e.action).reverse();
    expect(actions).toEqual(["guide.create", "guide.owner.add", "guide.owner.remove"]);
  });

  it("locks a guide so edits wait for an owner's approval", async () => {
    const { revision } = await pushed(grace, fixture, null);
    expect(await setLocked(db, "org", eve, fixture.id, true)).toMatchObject({ status: 403 });
    expect(await setLocked(db, "org", grace, fixture.id, true)).toMatchObject({ status: 200 });
    const { revision: draft, review } = await pushed(eve, { ...fixture, title: "Edited" }, revision.id);
    expect(review).toMatchObject({ gated: true, status: "draft" });
    expect((await getGuide(db, "org", vic, fixture.id))?.head.id).toBe(revision.id);
    await submitForReview(db, "org", eve, fixture.id, { revisionId: draft.id });
    expect((await getReview(db, "org", grace, fixture.id))!.approvers.map((a) => a.id)).toEqual(["grace"]);
  });

  it("publishes the head and closes the open request on unlocking", async () => {
    const { revision } = await pushed(grace, fixture, null);
    await setLocked(db, "org", grace, fixture.id, true);
    const { revision: draft } = await pushed(eve, { ...fixture, title: "Edited" }, revision.id);
    await submitForReview(db, "org", eve, fixture.id, { revisionId: draft.id });
    expect(await setLocked(db, "org", ada, fixture.id, false)).toMatchObject({ status: 200 });
    expect((await getGuide(db, "org", vic, fixture.id))?.head.id).toBe(draft.id);
    expect((await getReview(db, "org", grace, fixture.id))!.changeRequest).toMatchObject({ status: "withdrawn" });
  });

  it("needs the approvals entitlement to lock", async () => {
    await pushed(grace, fixture, null);
    await db.delete(subscription);
    await db.insert(subscription).values({ id: "sub", plan: "pro", referenceId: "org", status: "active" });
    expect(await setLocked(db, "org", grace, fixture.id, true)).toMatchObject({ status: 402 });
    expect((await access(grace)).canLock).toBe(false);
  });
});
