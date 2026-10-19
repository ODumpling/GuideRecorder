import { beforeEach, describe, expect, it } from "vitest";
import { testDb } from "../../test/db";
import type { Database } from "@/db";
import { member, organization, subscription, user } from "@/db/schema";
import { checkAccess } from "./access";

let db: Database;

beforeEach(async () => {
  db = await testDb();
  const now = new Date();
  await db.insert(organization).values({ id: "org", name: "Acme", slug: "acme", createdAt: now });
  for (const [id, role] of [
    ["owner", "owner"],
    ["viewer", "viewer"],
    ["outsider", null],
  ] as const) {
    await db.insert(user).values({ id, name: id, email: `${id}@example.com` });
    if (role) await db.insert(member).values({ id: `m-${id}`, organizationId: "org", userId: id, role, createdAt: now });
  }
});

describe("checkAccess", () => {
  it("needs a signed-in user", async () => {
    expect(await checkAccess(db, undefined, "org", { guide: ["read"] })).toMatchObject({ ok: false, status: 401 });
  });

  it("hides workspaces from non-members", async () => {
    expect(await checkAccess(db, "outsider", "org", { guide: ["read"] })).toMatchObject({ ok: false, status: 404 });
  });

  it("checks the member's role", async () => {
    expect(await checkAccess(db, "viewer", "org", { guide: ["read"] })).toMatchObject({ ok: true, role: "viewer" });
    expect(await checkAccess(db, "viewer", "org", { guide: ["create"] })).toMatchObject({ ok: false, status: 403 });
    expect(await checkAccess(db, "owner", "org", { billing: ["manage"] })).toMatchObject({ ok: true });
  });

  it("checks the plan includes the feature", async () => {
    expect(await checkAccess(db, "owner", "org", { guide: ["create"] }, "sync")).toMatchObject({ ok: false, status: 402 });
    await db.insert(subscription).values({ id: "s", plan: "pro", referenceId: "org", status: "active" });
    expect(await checkAccess(db, "owner", "org", { guide: ["create"] }, "sync")).toMatchObject({ ok: true });
    expect(await checkAccess(db, "owner", "org", { audit: ["read"] }, "audit")).toMatchObject({ ok: false, status: 402 });
  });
});
