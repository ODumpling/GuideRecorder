import { getAuthTables } from "better-auth/db";
import { getTableColumns, getTableName, is, type Table } from "drizzle-orm";
import { PgTable } from "drizzle-orm/pg-core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { testAuth } from "../../test/auth";
import { testDb } from "../../test/db";
import type { Database } from "@/db";
import * as schema from "@/db/schema";
import { subscription } from "@/db/schema";
import { createAuth, DESKTOP_CLIENT_ID } from "./create-auth";
import { memberRole } from "./workspace";

let db: Database;
let t: ReturnType<typeof testAuth>;

beforeEach(async () => {
  db = await testDb();
  t = testAuth(db);
});

describe("schema", () => {
  it("has every table and field Better Auth expects", () => {
    // Better Auth finds a model by its table name or its export name.
    const tables = Object.entries(schema).filter(([, value]) => is(value, PgTable)) as [string, Table][];
    const byName = new Map<string, object>(
      tables.flatMap(([key, table]) => [
        [getTableName(table), getTableColumns(table)],
        [key, getTableColumns(table)],
      ]),
    );
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

describe("desktop sign-in", () => {
  const post = (path: string, body: object, headers?: Headers) =>
    t.auth.handler(
      new Request(`http://localhost:3000/api/auth${path}`, {
        method: "POST",
        headers: { "content-type": "application/json", ...Object.fromEntries(headers ?? []) },
        body: JSON.stringify(body),
      }),
    );

  it("approves a device code on the web and calls the API with a bearer token", async () => {
    const ada = await t.signUp("Ada");
    const code = (await (await post("/device/code", { client_id: DESKTOP_CLIENT_ID })).json()) as {
      device_code: string;
      user_code: string;
      verification_uri: string;
    };
    expect(code.verification_uri).toContain("/device");
    const token = () => post("/device/token", { grant_type: "urn:ietf:params:oauth:grant-type:device_code", device_code: code.device_code, client_id: DESKTOP_CLIENT_ID });
    expect(((await (await token()).json()) as { error: string }).error).toBe("authorization_pending");

    // Opening /device?user_code=… claims the code for the signed-in user; then they approve it.
    await t.auth.handler(new Request(`http://localhost:3000/api/auth/device?user_code=${code.user_code}`, { headers: ada.headers }));
    expect((await post("/device/approve", { userCode: code.user_code }, ada.headers)).status).toBe(200);

    // Wait out the polling interval.
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(Date.now() + 10_000);
    const { access_token } = (await (await token()).json()) as { access_token: string };
    vi.useRealTimers();
    const session = await t.auth.api.getSession({ headers: new Headers({ authorization: `Bearer ${access_token}` }) });
    expect(session?.user.id).toBe(ada.userId);
  });

  it("only issues codes to the desktop app", async () => {
    expect((await post("/device/code", { client_id: "someone-else" })).status).toBe(400);
  });
});

describe("rate limits", () => {
  it("limit sign-in attempts per address, in the database", async () => {
    const auth = createAuth({ db, baseURL: "http://localhost:3000", secret: "test-secret-test-secret-test-secret-0123", sendEmail: async () => {}, rateLimit: true });
    const attempt = (ip: string) =>
      auth.handler(
        new Request("http://localhost:3000/api/auth/sign-in/email", {
          method: "POST",
          headers: { "content-type": "application/json", "x-forwarded-for": ip },
          body: JSON.stringify({ email: "nobody@example.com", password: "wrong password" }),
        }),
      );
    const statuses = [];
    for (let i = 0; i < 11; i++) statuses.push((await attempt("203.0.113.9")).status);
    expect(statuses.slice(0, 10).every((s) => s === 401)).toBe(true);
    expect(statuses[10]).toBe(429);
    expect((await attempt("203.0.113.10")).status).toBe(401);
    expect((await db.select().from(schema.rateLimit)).length).toBeGreaterThan(0);
  });
});
