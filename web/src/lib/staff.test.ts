import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { and, eq } from "drizzle-orm";
import { beforeEach, describe, expect, it } from "vitest";
import type { Guide } from "@guiderecorder/guide-schema";
import { imagePaths, pushRequest } from "@guiderecorder/sync";
import { testDb } from "../../test/db";
import type { Database } from "@/db";
import { asset, auditEvent, guide, member, organization, staff, subscription, user } from "@/db/schema";
import { findUsers, isStaff, issueLicence, restoreDeletedGuide, revokeLicence, userDetail } from "./staff";
import { getGuide, push, restoreGuide, trashGuide, type Actor } from "./sync";
import { workspacePlan } from "./workspace";

const fixture: Guide = JSON.parse(
  readFileSync(new URL("../../../fixtures/guides/Rotate an API key.guiderecorder/guide.json", import.meta.url), "utf8"),
);
const hash = (text: string) => createHash("sha256").update(text).digest("hex");
const DAY = 24 * 60 * 60 * 1000;
const NOW = new Date("2026-10-20T12:00:00Z");
const ada: Actor = { userId: "ada", role: "owner" };
const helpdesk = { userId: "help" };

let db: Database;

beforeEach(async () => {
  db = await testDb();
  await db.insert(organization).values({ id: "org", name: "Acme", slug: "acme", createdAt: NOW });
  await db.insert(user).values([
    { id: "ada", name: "Ada Lovelace", email: "ada@acme.com" },
    { id: "help", name: "Helpdesk", email: "help@guiderecorder.app" },
  ]);
  await db.insert(member).values({ id: "m1", organizationId: "org", userId: "ada", role: "owner", createdAt: NOW });
  await db.insert(staff).values({ userId: "help" });
  await db
    .insert(asset)
    .values(imagePaths(fixture).map((p) => ({ organizationId: "org", sha256: hash(p), size: 10, contentType: "image/webp" })));
});

async function pushed() {
  const assets = Object.fromEntries(imagePaths(fixture).map((p) => [p, hash(p)]));
  const result = await push(db, "org", ada, pushRequest.parse({ guideId: fixture.id, baseRevision: null, guide: fixture, assets }), "app");
  if (result.status !== 200) throw new Error(JSON.stringify(result));
}

const licence = (until: string, plan: "pro" | "enterprise" = "pro", seats?: number) =>
  issueLicence(db, helpdesk, { organizationId: "org", plan, until, seats, note: "TICKET-1" }, NOW);

describe("staff", () => {
  it("lets in only users in the staff table", async () => {
    expect(await isStaff(db, "help")).toBe(true);
    expect(await isStaff(db, "ada")).toBe(false);
  });

  it("finds users by name, email or id", async () => {
    expect((await findUsers(db, "lovelace")).map((u) => u.id)).toEqual(["ada"]);
    expect((await findUsers(db, "ACME.COM")).map((u) => u.id)).toEqual(["ada"]);
    expect((await findUsers(db, "help")).map((u) => u.id)).toEqual(["help"]);
    expect(await findUsers(db, "%")).toEqual([]);
    expect(await findUsers(db)).toHaveLength(2);
    expect((await findUsers(db, "ada"))[0]!.workspaces).toBe(1);
  });
});

describe("licences", () => {
  it("gives the workspace the plan through the last day, then lapses", async () => {
    expect((await workspacePlan(db, "org", NOW)).plan).toBe("free");
    const issued = await licence("2026-11-30");
    expect(issued).toMatchObject({ ok: true, endsAt: new Date("2026-12-01T00:00:00Z") });
    expect((await workspacePlan(db, "org", NOW)).plan).toBe("pro");
    expect((await workspacePlan(db, "org", new Date("2026-11-30T23:59:00Z"))).plan).toBe("pro");
    expect((await workspacePlan(db, "org", new Date("2026-12-01T00:00:00Z"))).plan).toBe("free");
  });

  it("sets Enterprise seats, and the better plan wins over a subscription", async () => {
    await db.insert(subscription).values({ id: "s", plan: "pro", referenceId: "org", status: "active" });
    await licence("2027-01-01", "enterprise", 40);
    const plan = await workspacePlan(db, "org", NOW);
    expect(plan.plan).toBe("enterprise");
    expect(plan.entitlements.seats).toBe(40);
  });

  it("refuses past dates, more than five years and unknown workspaces", async () => {
    expect(await licence("2026-10-19")).toMatchObject({ ok: false });
    expect(await licence("2032-01-01")).toMatchObject({ ok: false });
    expect(await issueLicence(db, helpdesk, { organizationId: "nope", plan: "pro", until: "2027-01-01", note: "x" }, NOW)).toMatchObject({
      ok: false,
      error: "Workspace not found.",
    });
  });

  it("ends on revoke, and both show in the workspace's audit log", async () => {
    const issued = await licence("2027-01-01");
    if (!issued.ok) throw new Error(issued.error);
    expect(await revokeLicence(db, helpdesk, issued.id, NOW)).toEqual({ ok: true });
    expect((await workspacePlan(db, "org", NOW)).plan).toBe("free");
    const events = await db.select().from(auditEvent).where(eq(auditEvent.organizationId, "org"));
    expect(events.map((e) => [e.action, e.actorId])).toEqual([
      ["licence.grant", "help"],
      ["licence.revoke", "help"],
    ]);
  });

  it("shows in the user's detail", async () => {
    await licence("2027-01-01");
    const detail = await userDetail(db, "ada", NOW);
    expect(detail!.workspaces[0]).toMatchObject({ id: "org", plan: "pro", grants: [{ plan: "pro", note: "TICKET-1", issuedBy: "help@guiderecorder.app", current: true }] });
  });
});

describe("restoring deleted guides", () => {
  it("restores a guide deleted under 30 days ago, even a private one", async () => {
    await pushed();
    await db.update(guide).set({ visibility: "private" });
    await trashGuide(db, "org", ada, fixture.id, new Date(NOW.getTime() - 29 * DAY));
    // A workspace member who isn't its owner couldn't.
    expect((await restoreGuide(db, "org", { userId: "help", role: "owner" }, fixture.id)).status).toBe(404);

    const detail = await userDetail(db, "ada", NOW);
    expect(detail!.workspaces[0]!.trash).toMatchObject([{ id: fixture.id, title: fixture.title, deletedBy: "ada@acme.com" }]);

    expect(await restoreDeletedGuide(db, helpdesk, "org", fixture.id, NOW)).toEqual({ ok: true });
    expect((await getGuide(db, "org", ada, fixture.id))?.guide.deletedAt).toBeNull();
    const [restored] = await db
      .select()
      .from(auditEvent)
      .where(and(eq(auditEvent.organizationId, "org"), eq(auditEvent.action, "guide.restore")));
    expect(restored).toMatchObject({ actorId: "help", metadata: { staff: true } });
  });

  it("refuses guides deleted 30 days ago or more, and guides not in the trash", async () => {
    await pushed();
    expect(await restoreDeletedGuide(db, helpdesk, "org", fixture.id, NOW)).toMatchObject({ ok: false });
    await trashGuide(db, "org", ada, fixture.id, new Date(NOW.getTime() - 30 * DAY));
    expect(await restoreDeletedGuide(db, helpdesk, "org", fixture.id, NOW)).toMatchObject({ ok: false });
    expect((await userDetail(db, "ada", NOW))!.workspaces[0]!.trash).toEqual([]);
  });
});
