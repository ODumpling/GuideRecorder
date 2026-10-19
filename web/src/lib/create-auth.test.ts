import { getAuthTables } from "better-auth/db";
import { getTableColumns, getTableName, type Table } from "drizzle-orm";
import { beforeEach, describe, expect, it } from "vitest";
import { testAuth } from "../../test/auth";
import { testDb } from "../../test/db";
import type { Database } from "@/db";
import * as schema from "@/db/schema";
import { subscription } from "@/db/schema";
import { memberRole } from "./workspace";

let db: Database;
let t: ReturnType<typeof testAuth>;

beforeEach(async () => {
  db = await testDb();
  t = testAuth(db);
});

describe("schema", () => {
  it("has every table and field Better Auth expects", () => {
    const tables = Object.values(schema) as Table[];
    const byName = new Map<string, object>(tables.map((table) => [getTableName(table), getTableColumns(table)]));
    for (const table of Object.values(getAuthTables(t.auth.options))) {
      const columns = byName.get(table.modelName);
      expect(columns, table.modelName).toBeDefined();
      for (const field of Object.keys(table.fields)) {
        expect(Object.keys(columns!), `${table.modelName}.${field}`).toContain(field);
      }
    }
  });
});

describe("sign-up", () => {
  it("gives a new user a personal workspace they own, active in their session", async () => {
    const { userId, headers } = await t.signUp("Ada");
    const session = await t.auth.api.getSession({ headers });
    const workspaceId = session?.session.activeOrganizationId;
    expect(workspaceId).toBeTruthy();
    expect(await memberRole(db, workspaceId!, userId)).toBe("owner");
  });
});

describe("invitations", () => {
  async function inviteAs(headers: Headers, organizationId: string, email: string) {
    return t.auth.api.createInvitation({ headers, body: { organizationId, email, role: "editor" }, asResponse: true });
  }

  it("are limited by the plan's seats", async () => {
    const owner = await t.signUp("Ada");
    const organizationId = (await t.auth.api.getSession({ headers: owner.headers }))!.session.activeOrganizationId!;
    await t.signUp("Grace");

    // Free and Pro have one seat, taken by the owner.
    expect((await inviteAs(owner.headers, organizationId, "grace@example.com")).status).toBe(403);

    await db.insert(subscription).values({ id: "sub", plan: "enterprise", referenceId: organizationId, status: "active", seats: 5 });
    const invited = await inviteAs(owner.headers, organizationId, "grace@example.com");
    expect(invited.status).toBe(200);
    expect(t.sent.at(-1)?.to).toBe("grace@example.com");
  });

  it("let the invitee join with the invited role", async () => {
    const owner = await t.signUp("Ada");
    const organizationId = (await t.auth.api.getSession({ headers: owner.headers }))!.session.activeOrganizationId!;
    await db.insert(subscription).values({ id: "sub", plan: "enterprise", referenceId: organizationId, status: "active", seats: 5 });
    const grace = await t.signUp("Grace");
    const { id } = (await (await inviteAs(owner.headers, organizationId, "grace@example.com")).json()) as { id: string };
    expect(t.sent.at(-1)?.text).toContain(`/invitations/${id}`);

    await t.auth.api.acceptInvitation({ headers: grace.headers, body: { invitationId: id } });
    expect(await memberRole(db, organizationId, grace.userId)).toBe("editor");
  });
});
