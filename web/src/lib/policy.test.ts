import { eq } from "drizzle-orm";
import { beforeEach, describe, expect, it } from "vitest";
import { testDb } from "../../test/db";
import type { Database } from "@/db";
import { member, organization, subscription, user, workspacePolicy } from "@/db/schema";
import { updatePolicy } from "./admin";
import { canShare, DEFAULT_POLICY, getPolicy, setPolicy, sharingAllows } from "./policy";

let db: Database;

beforeEach(async () => {
  db = await testDb();
  await db.insert(organization).values({ id: "org", name: "Acme", slug: "acme", createdAt: new Date() });
  await db.insert(user).values(["ada", "grace"].map((id) => ({ id, name: id, email: `${id}@acme.com` })));
  await db.insert(member).values([
    { id: "m1", organizationId: "org", userId: "ada", role: "owner", createdAt: new Date() },
    { id: "m2", organizationId: "org", userId: "grace", role: "editor", createdAt: new Date() },
  ]);
});

describe("sharing policy", () => {
  it("caps who guides can be shared with", () => {
    expect(sharingAllows({ sharing: "public" }, "public")).toBe(true);
    expect(sharingAllows({ sharing: "workspace" }, "public")).toBe(false);
    expect(sharingAllows({ sharing: "workspace" }, "workspace")).toBe(true);
    expect(sharingAllows({ sharing: "off" }, "workspace")).toBe(false);
  });

  it("defaults to public links and follows the workspace's setting", async () => {
    expect(await getPolicy(db, "org")).toEqual(DEFAULT_POLICY);
    expect(await canShare(db, "org", "public")).toBe(true);
    await setPolicy(db, "org", { sharing: "workspace" });
    expect(await canShare(db, "org", "public")).toBe(false);
    expect(await canShare(db, "org", "workspace")).toBe(true);
  });

  it("rejects invalid values", async () => {
    await expect(setPolicy(db, "org", { revisionRetentionDays: 1 })).rejects.toThrow();
    await expect(setPolicy(db, "org", { provisioningRole: "owner" as never })).rejects.toThrow();
  });
});

describe("updatePolicy", () => {
  const actor = (userId: string) => ({ userId, organizationId: "org", headers: new Headers() });

  it("needs Enterprise and an owner or admin", async () => {
    expect(await updatePolicy(db, actor("ada"), { sharing: "off" })).toMatchObject({ ok: false, status: 402 });
    await db.insert(subscription).values({ id: "sub", plan: "enterprise", referenceId: "org", status: "active" });
    expect(await updatePolicy(db, actor("grace"), { sharing: "off" })).toMatchObject({ ok: false, status: 403 });
    expect(await updatePolicy(db, actor("ada"), { sharing: "off", revisionRetentionDays: 90 })).toMatchObject({
      ok: true,
      policy: { sharing: "off", revisionRetentionDays: 90 },
    });
    const [row] = await db.select().from(workspacePolicy).where(eq(workspacePolicy.organizationId, "org"));
    expect(row).toMatchObject({ sharing: "off", revisionRetentionDays: 90 });
  });
});
