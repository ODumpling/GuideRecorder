import { sql } from "drizzle-orm";
import { beforeEach, describe, expect, it } from "vitest";
import { testAuth } from "../../test/auth";
import { testDb } from "../../test/db";
import type { Database } from "@/db";
import { auditEvent, organization, subscription } from "@/db/schema";
import {
  GENESIS,
  canonical,
  listAudit,
  pruneAudit,
  recordAudit,
  requestMeta,
  setAuditRetention,
  toCsv,
  verifyChain,
  verifyWorkspaceChain,
  type AuditEvent,
} from "./audit";

let db: Database;
const DAY = 24 * 60 * 60 * 1000;

beforeEach(async () => {
  db = await testDb();
  await db.insert(organization).values({ id: "org", name: "Acme", slug: "acme", createdAt: new Date() });
});

const event = (n: number, at = new Date()) =>
  recordAudit(db, "org", {
    actorId: "ada",
    action: "guide.edit",
    targetType: "guide",
    targetId: `g${n}`,
    before: { n },
    after: { n: n + 1 },
    metadata: { n, nested: { b: 1, a: [1, 2] } },
    ip: "203.0.113.7",
    userAgent: "test",
    at,
  });

const all = () => db.select().from(auditEvent).orderBy(auditEvent.seq);

describe("canonical", () => {
  it("serializes equal values the same, whatever their key order", () => {
    expect(canonical({ b: 1, a: { d: 2, c: [3, { f: 4, e: 5 }] } })).toBe(canonical({ a: { c: [3, { e: 5, f: 4 }], d: 2 }, b: 1 }));
  });
});

describe("the hash chain", () => {
  it("links each event to the one before", async () => {
    const first = await event(1);
    const second = await event(2);
    expect(first).toMatchObject({ seq: 1, prevHash: GENESIS });
    expect(second).toMatchObject({ seq: 2, prevHash: first.hash });
    expect(first.beforeHash).toMatch(/^[0-9a-f]{64}$/);
    expect(await verifyWorkspaceChain(db, "org")).toEqual({ ok: true, count: 2, firstSeq: 1 });
  });

  it("verifies events as read back from the database", async () => {
    for (const n of [1, 2, 3]) await event(n);
    expect(verifyChain(await all())).toMatchObject({ ok: true, count: 3 });
  });

  it("shows a changed event", async () => {
    for (const n of [1, 2, 3]) await event(n);
    const events = await all();
    const tampered: AuditEvent[] = events.map((e) => (e.seq === 2 ? { ...e, actorId: "mallory" } : e));
    expect(verifyChain(tampered)).toMatchObject({ ok: false, seq: 2, reason: "It was changed after it was written." });
    // Rehashing the changed event doesn't help: the next one no longer follows it.
    const rehashed = tampered.map((e) => (e.seq === 2 ? { ...e, hash: "f".repeat(64) } : e));
    expect(verifyChain(rehashed)).toMatchObject({ ok: false, seq: 2 });
  });

  it("shows a missing event", async () => {
    for (const n of [1, 2, 3]) await event(n);
    const events = await all();
    expect(verifyChain(events.filter((e) => e.seq !== 2))).toMatchObject({ ok: false, seq: 3 });
    expect(verifyChain(events.filter((e) => e.seq !== 1))).toMatchObject({ ok: false, seq: 2, reason: "Events before this one are missing." });
  });

  it("is append-only in the database", async () => {
    await event(1);
    await expect(db.execute(sql`update audit_event set actor_id = 'mallory'`)).rejects.toThrow();
    await expect(db.execute(sql`delete from audit_event`)).rejects.toThrow();
    expect(await all()).toHaveLength(1);
  });

  it("keeps separate chains per workspace", async () => {
    await db.insert(organization).values({ id: "other", name: "Other", slug: "other", createdAt: new Date() });
    await event(1);
    const other = await recordAudit(db, "other", { actorId: null, action: "user.signIn", targetType: "user", targetId: "u" });
    expect(other).toMatchObject({ seq: 1, prevHash: GENESIS });
    expect(await verifyWorkspaceChain(db, "other")).toMatchObject({ ok: true, count: 1 });
  });
});

describe("retention", () => {
  it("prunes old events and the chain still verifies from where it now starts", async () => {
    const now = new Date();
    await event(1, new Date(now.getTime() - 100 * DAY));
    await event(2, new Date(now.getTime() - 60 * DAY));
    await event(3, new Date(now.getTime() - 1 * DAY));
    expect(await setAuditRetention(db, "org", { userId: "ada" }, 10)).toEqual({ ok: false, error: expect.stringContaining("30") });
    expect(await setAuditRetention(db, "org", { userId: "ada" }, 30)).toEqual({ ok: true });

    expect(await pruneAudit(db, now)).toBe(2);
    const left = await all();
    expect(left.map((e) => e.action)).toEqual(["guide.edit", "audit.retention", "audit.pruned"]);
    expect(left.at(-1)!.metadata).toMatchObject({ throughSeq: 2, count: 2 });
    expect(await verifyWorkspaceChain(db, "org")).toMatchObject({ ok: true, count: 3, firstSeq: 3 });
    // Nothing more to prune.
    expect(await pruneAudit(db, now)).toBe(0);
  });

  it("can't pass pruning off as tampering, or the other way round", async () => {
    const now = new Date();
    await event(1, new Date(now.getTime() - 100 * DAY));
    await event(2, new Date(now.getTime() - 1 * DAY));
    await event(3, new Date(now.getTime() - 1 * DAY));
    await setAuditRetention(db, "org", { userId: "ada" }, 30);
    await pruneAudit(db, now);
    const events = await all();
    // Dropping the oldest remaining event breaks the link to the prune record.
    expect(verifyChain(events.slice(1), { throughSeq: 1, lastHash: events[0]!.prevHash })).toMatchObject({ ok: false });
  });
});

describe("listing and export", () => {
  it("filters by action category, actor and time, newest first", async () => {
    await event(1);
    await recordAudit(db, "org", { actorId: "grace", action: "member.role", targetType: "member", targetId: "vic" });
    expect((await listAudit(db, "org")).map((e) => e.action)).toEqual(["member.role", "guide.edit"]);
    expect((await listAudit(db, "org", { action: "member" })).map((e) => e.actorId)).toEqual(["grace"]);
    expect((await listAudit(db, "org", { action: "guide.edit" })).map((e) => e.seq)).toEqual([1]);
    expect(await listAudit(db, "org", { actorId: "nobody" })).toEqual([]);
    expect(await listAudit(db, "org", { from: new Date(Date.now() + DAY) })).toEqual([]);
    expect((await listAudit(db, "org", { before: 2 })).map((e) => e.seq)).toEqual([1]);
  });

  it("writes CSV that spreadsheets won't run", async () => {
    await recordAudit(db, "org", { actorId: "ada", action: "guide.edit", targetType: "guide", targetId: "=HYPERLINK(1)", metadata: { a: 'x,"y"' } });
    const csv = toCsv(await listAudit(db, "org"));
    const [header, row] = csv.trim().split("\r\n");
    expect(header).toContain("seq,createdAt,actorId");
    expect(row).toContain('"=HYPERLINK(1)"');
    expect(row).toContain('"{""a"":""x,\\""y\\""""}"');
  });
});

describe("requestMeta", () => {
  it("takes the first forwarded address", () => {
    expect(requestMeta(new Headers({ "x-forwarded-for": "203.0.113.7, 10.0.0.1", "user-agent": "UA" }))).toEqual({ ip: "203.0.113.7", userAgent: "UA" });
    expect(requestMeta(new Headers())).toEqual({ ip: null, userAgent: null });
  });
});

describe("Better Auth events", () => {
  it("logs sign-ins, invitations and role changes with who did them", async () => {
    const t = testAuth(db);
    const owner = await t.signUp("Ada");
    const organizationId = (await t.auth.api.getSession({ headers: owner.headers }))!.session.activeOrganizationId!;
    await db.insert(subscription).values({ id: "sub", plan: "enterprise", referenceId: organizationId, status: "active", seats: 5 });
    const grace = await t.signUp("Grace");
    const { id } = await t.auth.api.createInvitation({ headers: owner.headers, body: { organizationId, email: "grace@example.com", role: "editor" } });
    await t.auth.api.acceptInvitation({ headers: grace.headers, body: { invitationId: id } });
    const members = await t.auth.api.listMembers({ headers: owner.headers, query: { organizationId } });
    const graceMember = members.members.find((m) => m.userId === grace.userId)!;
    await t.auth.api.updateMemberRole({ headers: owner.headers, body: { organizationId, memberId: graceMember.id, role: "approver" } });

    const events = await listAudit(db, organizationId);
    const byAction = (action: string) => events.find((e) => e.action === action);
    expect(byAction("user.signIn")).toMatchObject({ actorId: owner.userId, targetId: owner.userId });
    expect(byAction("invitation.create")).toMatchObject({ actorId: owner.userId, metadata: { email: "grace@example.com", role: "editor" } });
    expect(byAction("invitation.accept")).toMatchObject({ actorId: grace.userId });
    expect(byAction("member.role")).toMatchObject({ actorId: owner.userId, targetId: grace.userId, metadata: { from: "editor", to: "approver" } });
    expect(await verifyWorkspaceChain(db, organizationId)).toMatchObject({ ok: true });
  });
});
