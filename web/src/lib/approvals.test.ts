import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { beforeEach, describe, expect, it } from "vitest";
import type { Guide } from "@guiderecorder/guide-schema";
import { imagePaths, pushRequest } from "@guiderecorder/sync";
import { testDb } from "../../test/db";
import type { Database } from "@/db";
import { asset, guide, guideOwner, member, organization, subscription, user } from "@/db/schema";
import { getReview, reviewChange, savePolicy, submitForReview, withdrawChange } from "./approvals";
import { listAudit, verifyWorkspaceChain } from "./audit";
import { today } from "./review";
import { changes, getGuide, listGuides, push, type Actor } from "./sync";

const fixture: Guide = JSON.parse(
  readFileSync(new URL("../../../fixtures/guides/Rotate an API key.guiderecorder/guide.json", import.meta.url), "utf8"),
);
const hash = (text: string) => createHash("sha256").update(text).digest("hex");
const assetsFor = (guide: Guide) => Object.fromEntries(imagePaths(guide).map((p) => [p, hash(p)]));

let db: Database;
const ada: Actor = { userId: "ada", role: "owner" };
const grace: Actor = { userId: "grace", role: "editor" };
const alan: Actor = { userId: "alan", role: "approver" };
const bob: Actor = { userId: "bob", role: "approver" };
const vic: Actor = { userId: "vic", role: "viewer" };
const eve: Actor = { userId: "eve", role: "editor" };

beforeEach(async () => {
  db = await testDb();
  const now = new Date();
  await db.insert(organization).values({ id: "org", name: "Acme", slug: "acme", createdAt: now });
  for (const a of [ada, grace, alan, bob, vic, eve]) {
    await db.insert(user).values({ id: a.userId, name: a.userId, email: `${a.userId}@example.com` });
    await db.insert(member).values({ id: `m-${a.userId}`, organizationId: "org", userId: a.userId, role: a.role, createdAt: now });
  }
  await db
    .insert(asset)
    .values(imagePaths(fixture).map((p) => ({ organizationId: "org", sha256: hash(p), size: 10, contentType: "image/webp" })));
});

const enterprise = () => db.insert(subscription).values({ id: "sub", plan: "enterprise", referenceId: "org", status: "active", seats: 10 });

/** Locks the fixture guide before it's first pushed, owned by `owners` (and whoever pushes it first). */
async function locked(owners = ["alan", "bob"]) {
  await db.insert(guide).values({ organizationId: "org", id: fixture.id, locked: true });
  if (owners.length) await db.insert(guideOwner).values(owners.map((userId) => ({ organizationId: "org", guideId: fixture.id, userId })));
}

async function pushed(actor: Actor, guide: Guide, base: string | null) {
  const result = await push(db, "org", actor, pushRequest.parse({ guideId: guide.id, baseRevision: base, guide, assets: assetsFor(guide) }), "app");
  if (result.status !== 200) throw new Error(`push failed: ${JSON.stringify(result)}`);
  return result;
}

const retitled = (title: string, extra: Partial<Guide> = {}): Guide => ({ ...structuredClone(fixture), title, ...extra });
const head = async () => (await getGuide(db, "org", ada, fixture.id))!;
const seenBy = async (reader: Actor) => (await getGuide(db, "org", reader, fixture.id))?.head ?? null;

async function submitted(actor = grace) {
  const result = await submitForReview(db, "org", actor, fixture.id, { revisionId: (await head()).guide.headRevisionId! });
  if (result.status !== 200) throw new Error(`submit failed: ${JSON.stringify(result)}`);
  return result.changeRequestId;
}

const approve = async (actor: Actor, changeRequestId: string, verdict: "approve" | "reject" | "requestChanges" | "comment" = "approve", body = "") =>
  reviewChange(db, "org", actor, changeRequestId, { verdict, revisionId: (await head()).guide.headRevisionId!, body });

describe("without approvals", () => {
  it("publishes every push, and viewers see the head", async () => {
    await db.insert(subscription).values({ id: "sub", plan: "pro", referenceId: "org", status: "active" });
    const { revision, review } = await pushed(grace, fixture, null);
    expect(review).toMatchObject({ gated: false });
    expect((await head()).guide.publishedRevisionId).toBe(revision.id);
    expect((await seenBy(vic))?.id).toBe(revision.id);
    // Stored as sent.
    expect(revision.guide).toEqual(fixture);
    expect(await submitForReview(db, "org", grace, fixture.id, { revisionId: revision.id })).toMatchObject({ status: 402 });
  });
});

describe("with approvals", () => {
  beforeEach(async () => {
    await enterprise();
    await locked();
  });

  it("keeps a new guide a draft that viewers can't see", async () => {
    const { revision, review } = await pushed(grace, { ...structuredClone(fixture), status: "published", lastReviewed: "2026-01-01" }, null);
    // The server owns status and review date: pushing them doesn't publish.
    expect(revision.guide.status).toBe("draft");
    expect(revision.guide.lastReviewed).toBeUndefined();
    expect(review).toMatchObject({ gated: true, status: "draft", changeRequestId: null, required: 1 });
    expect((await head()).guide.publishedRevisionId).toBeNull();

    expect(await seenBy(vic)).toBeNull();
    expect(await listGuides(db, "org", vic)).toEqual([]);
    expect((await changes(db, "org", vic, 0n)).changes).toEqual([]);
    expect((await seenBy(grace))?.id).toBe(revision.id);
  });

  it("publishes once approved, setting lastReviewed and bumping version", async () => {
    await pushed(grace, retitled("Rotate", { version: "1.4" }), null);
    const id = await submitted();
    expect((await changes(db, "org", grace, 0n)).changes[0]).toMatchObject({ review: { status: "inReview", changeRequestId: id, approvals: 0 } });

    const result = await approve(alan, id);
    expect(result).toMatchObject({ status: 200, published: true, review: { status: "published", changeRequestId: null } });
    const { guide: g, head: published } = await head();
    expect(g.publishedRevisionId).toBe(g.headRevisionId);
    expect(published.guide).toMatchObject({ title: "Rotate", status: "published", lastReviewed: today(), version: "1.5" });
    expect((await seenBy(vic))?.id).toBe(published.id);

    const actions = (await listAudit(db, "org")).map((e) => e.action).reverse();
    expect(actions).toEqual(["guide.create", "review.submit", "review.approve", "guide.publish"]);
    expect(await verifyWorkspaceChain(db, "org")).toMatchObject({ ok: true, count: 4 });
  });

  it("makes edits to a published guide a draft; viewers keep the published one", async () => {
    await pushed(grace, fixture, null);
    await approve(alan, await submitted());
    const live = (await head()).head;

    const { revision: draft } = await pushed(grace, { ...live.guide, title: "Edited", status: "published", version: "99" }, live.id);
    expect(draft.guide).toMatchObject({ title: "Edited", status: "draft", version: live.guide.version, lastReviewed: today() });
    expect((await seenBy(vic))?.id).toBe(live.id);
    expect((await seenBy(grace))?.id).toBe(draft.id);
    // Pushing only a status is no change at all.
    expect((await pushed(grace, { ...draft.guide, status: "published" }, draft.id)).revision.id).toBe(draft.id);
  });

  it("can't go live with a stale approval", async () => {
    await pushed(grace, fixture, null);
    const id = await submitted();
    const stale = (await head()).guide.headRevisionId!;
    await pushed(grace, retitled("Changed meanwhile"), stale);
    expect(await reviewChange(db, "org", alan, id, { verdict: "approve", revisionId: stale })).toMatchObject({ status: 409 });
  });

  it("follows the head and resets approvals on a new revision", async () => {
    await savePolicy(db, "org", ada, { folder: "", requiredApprovals: 2 });
    await pushed(grace, fixture, null);
    const id = await submitted();
    expect(await approve(alan, id)).toMatchObject({ published: false, review: { approvals: 1, required: 2 } });

    const { review } = await pushed(grace, retitled("Again"), (await head()).guide.headRevisionId);
    expect(review).toMatchObject({ status: "inReview", changeRequestId: id, approvals: 0 });
    await approve(alan, id);
    expect(await approve(bob, id)).toMatchObject({ published: true });
  });

  it("keeps approvals across revisions if the policy says so", async () => {
    await savePolicy(db, "org", ada, { folder: "", requiredApprovals: 2, resetOnNewRevision: false });
    await pushed(grace, fixture, null);
    const id = await submitted();
    await approve(alan, id);
    await pushed(grace, retitled("Again"), (await head()).guide.headRevisionId);
    expect(await approve(bob, id)).toMatchObject({ published: true });
  });

  it("only counts the guide's owners", async () => {
    await db.delete(guideOwner);
    await db.insert(guideOwner).values({ organizationId: "org", guideId: fixture.id, userId: "bob" });
    await pushed(grace, fixture, null);
    const id = await submitted();
    expect(await approve(alan, id)).toMatchObject({ status: 403, error: expect.stringMatching(/owners/) });
    expect(await approve(alan, id, "reject")).toMatchObject({ status: 403 });
    expect(await approve(bob, id)).toMatchObject({ published: true });
  });

  it("lets an owner with the editor role approve", async () => {
    await db.insert(guideOwner).values({ organizationId: "org", guideId: fixture.id, userId: "eve" });
    await pushed(grace, fixture, null);
    expect(await approve(eve, await submitted())).toMatchObject({ published: true });
  });

  it("falls back to roles that can approve for a guide without owners", async () => {
    await pushed(grace, fixture, null);
    await db.delete(guideOwner);
    const id = await submitted();
    expect(await approve(eve, id)).toMatchObject({ status: 403 });
    expect(await approve(alan, id)).toMatchObject({ published: true });
  });

  it("refuses self-approval unless the policy allows it", async () => {
    await pushed(ada, fixture, null);
    const id = await submitted(ada);
    expect(await approve(ada, id)).toMatchObject({ status: 403, error: expect.stringMatching(/own/) });
    await savePolicy(db, "org", ada, { folder: "", requiredApprovals: 1, allowSelfApproval: true });
    expect(await approve(ada, id)).toMatchObject({ published: true });
  });

  it("asks for changes, and rejecting closes the request", async () => {
    await pushed(grace, fixture, null);
    const id = await submitted();
    expect(await approve(alan, id, "requestChanges")).toMatchObject({ status: 400 });
    expect(await approve(alan, id, "requestChanges", "Fix step 2")).toMatchObject({ review: { status: "needsUpdate" } });
    // A new revision answers it (approvals and requests reset).
    const { review } = await pushed(grace, retitled("Fixed"), (await head()).guide.headRevisionId);
    expect(review.status).toBe("inReview");

    expect(await approve(bob, id, "reject", "Not needed")).toMatchObject({ review: { status: "draft", changeRequestId: null } });
    expect(await approve(alan, id)).toMatchObject({ status: 409 });
  });

  it("takes comments on items, by ULID", async () => {
    await pushed(grace, fixture, null);
    const id = await submitted();
    const revisionId = (await head()).guide.headRevisionId!;
    const itemId = fixture.items[1]!.id;
    expect(await reviewChange(db, "org", grace, id, { verdict: "comment", revisionId, body: "Typo", itemId })).toMatchObject({ status: 200 });
    expect(await reviewChange(db, "org", grace, id, { verdict: "comment", revisionId, body: "?", itemId: "nope" })).toMatchObject({ status: 400 });
    const page = await getReview(db, "org", grace, fixture.id);
    expect(page!.reviews).toMatchObject([{ verdict: "comment", body: "Typo", itemId, reviewer: { id: "grace" } }]);
  });

  it("lets the author withdraw", async () => {
    await pushed(grace, fixture, null);
    const id = await submitted();
    expect(await withdrawChange(db, "org", vic, id)).toMatchObject({ status: 403 });
    expect(await withdrawChange(db, "org", grace, id)).toMatchObject({ status: 200, review: { status: "draft" } });
  });

  it("returns the open request rather than opening another", async () => {
    await pushed(grace, fixture, null);
    expect(await submitted()).toBe(await submitted());
  });

  it("needs one approval for a locked guide even where the folder's policy needs none", async () => {
    await savePolicy(db, "org", ada, { folder: "", requiredApprovals: 0 });
    const { review } = await pushed(grace, fixture, null);
    expect(review).toMatchObject({ gated: true, required: 1 });
  });

  it("shows the review page what changed since the published revision", async () => {
    await pushed(grace, fixture, null);
    await approve(alan, await submitted());
    const live = (await head()).head;
    const items = structuredClone(live.guide.items);
    (items[1] as { title: string }).title = "New step title";
    await pushed(grace, { ...live.guide, items }, live.id);
    await submitted();
    const page = (await getReview(db, "org", alan, fixture.id))!;
    expect(page.before?.id).toBe(live.id);
    expect(page.diff.items.filter((i) => i.change !== "unchanged")).toMatchObject([{ id: items[1]!.id, change: "changed", fields: ["title"] }]);
    expect(page.can).toMatchObject({ approve: true, reject: true, comment: true, submit: false });
    expect(page.state).toMatchObject({ status: "inReview", required: 1 });
    expect(await getReview(db, "org", vic, fixture.id)).toBeNull();
  });
});

describe("unlocked guides", () => {
  beforeEach(enterprise);

  it("publish every push, even with approvals", async () => {
    const { revision, review } = await pushed(grace, fixture, null);
    expect(review.gated).toBe(false);
    expect((await seenBy(vic))?.id).toBe(revision.id);
    expect(await submitForReview(db, "org", grace, fixture.id, { revisionId: revision.id })).toMatchObject({ status: 400 });
  });
});

describe("permissions", () => {
  beforeEach(async () => {
    await enterprise();
    await locked();
  });

  it("gates each step on the role and ownership", async () => {
    await pushed(grace, fixture, null);
    const revisionId = (await head()).guide.headRevisionId!;
    expect(await submitForReview(db, "org", vic, fixture.id, { revisionId })).toMatchObject({ status: 403 });
    const id = await submitted();
    expect(await approve(grace, id)).toMatchObject({ status: 403 });
    expect(await approve(eve, id, "reject")).toMatchObject({ status: 403 });
    expect(await withdrawChange(db, "org", eve, id)).toMatchObject({ status: 403 });
    expect(await approve(vic, id, "comment", "hi")).toMatchObject({ status: 403 });
    expect(await savePolicy(db, "org", grace, { folder: "", requiredApprovals: 2 })).toMatchObject({ status: 403 });
    expect(await savePolicy(db, "org", ada, { folder: "", requiredApprovals: 2, approverIds: ["grace"] })).toMatchObject({ status: 400 });
    expect(await savePolicy(db, "org", ada, { folder: "", requiredApprovals: 3, approverIds: ["alan", "bob"] })).toMatchObject({ status: 400 });
    expect(await savePolicy(db, "org", ada, { folder: "/HR/", requiredApprovals: 2 })).toMatchObject({ status: 200, policy: { folder: "HR" } });
  });

  it("needs the approvals entitlement", async () => {
    await db.delete(subscription);
    expect(await savePolicy(db, "org", ada, { folder: "", requiredApprovals: 2 })).toMatchObject({ status: 402 });
  });
});
