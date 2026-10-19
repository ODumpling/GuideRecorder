import { eq } from "drizzle-orm";
import { beforeEach, describe, expect, it } from "vitest";
import { testAuth } from "../../test/auth";
import { testDb } from "../../test/db";
import type { Database } from "@/db";
import { member, scimManagedCredential, session, subscription, user } from "@/db/schema";
import { createScimConnection, revokeScimToken, type AdminActor } from "./admin";
import { setPolicy } from "./policy";
import { roleForGroup, strongestRole } from "./scim";
import { memberRole } from "./workspace";

let db: Database;
let t: ReturnType<typeof testAuth>;
let owner: AdminActor;

const USER = "urn:ietf:params:scim:schemas:core:2.0:User";
const GROUP = "urn:ietf:params:scim:schemas:core:2.0:Group";
const PATCH = "urn:ietf:params:scim:api:messages:2.0:PatchOp";

beforeEach(async () => {
  db = await testDb();
  t = testAuth(db);
  const ada = await t.signUp("Ada", "ada@acme.com");
  const organizationId = (await t.auth.api.getSession({ headers: ada.headers }))!.session.activeOrganizationId!;
  await db.insert(subscription).values({ id: "sub", plan: "enterprise", referenceId: organizationId, status: "active", seats: 10 });
  owner = { userId: ada.userId, organizationId, headers: ada.headers };
});

async function connect() {
  const created = await createScimConnection(db, t.auth, owner);
  if (!created.ok) throw new Error(created.error);
  return created;
}

function scim(token: string, method: string, path: string, body?: object) {
  return t.auth.handler(
    new Request(`http://localhost:3000/api/auth/scim/v2${path}`, {
      method,
      headers: { authorization: `Bearer ${token}`, "content-type": "application/scim+json", accept: "application/scim+json" },
      body: body && JSON.stringify(body),
    }),
  );
}

async function provision(token: string, email: string, externalId = email) {
  const response = await scim(token, "POST", "/Users", {
    schemas: [USER],
    userName: email,
    externalId,
    name: { givenName: email.split("@")[0], familyName: "Example" },
    emails: [{ value: email, primary: true, type: "work" }],
    active: true,
  });
  expect(response.status, await response.clone().text()).toBe(201);
  return (await response.json()) as { id: string };
}

async function userIdOf(email: string) {
  const [row] = await db.select({ id: user.id }).from(user).where(eq(user.email, email));
  return row!.id;
}

describe("group names", () => {
  it("map to roles", () => {
    expect(roleForGroup("Admins")).toBe("admin");
    expect(roleForGroup("GuideRecorder Editors")).toBe("editor");
    expect(roleForGroup("guiderecorder-viewer")).toBe("viewer");
    expect(roleForGroup("Owners")).toBeNull();
    expect(roleForGroup("Engineering")).toBeNull();
    expect(strongestRole(["viewer", "admin", "editor"])).toBe("admin");
    expect(strongestRole([])).toBeNull();
  });
});

describe("SCIM provisioning", () => {
  it("issues a token once and keeps only its digest", async () => {
    const { token } = await connect();
    expect(token).toMatch(/^ba_scim_credential_/);
    const stored = await db.select().from(scimManagedCredential);
    expect(stored).toHaveLength(1);
    expect(JSON.stringify(stored)).not.toContain(token.split(".")[1]);
  });

  it("adds provisioned users as members with the policy's role", async () => {
    await setPolicy(db, owner.organizationId, { provisioningRole: "viewer" });
    const { token } = await connect();
    await provision(token, "bob@acme.com");
    expect(await memberRole(db, owner.organizationId, await userIdOf("bob@acme.com"))).toBe("viewer");

    const listed = (await (await scim(token, "GET", "/Users")).json()) as { totalResults: number };
    expect(listed.totalResults).toBe(1);
  });

  it("gives the most privileged role of the user's groups", async () => {
    const { token } = await connect();
    const bob = await provision(token, "bob@acme.com");
    const bobId = await userIdOf("bob@acme.com");
    expect(await memberRole(db, owner.organizationId, bobId)).toBe("editor");

    const viewers = await scim(token, "POST", "/Groups", { schemas: [GROUP], displayName: "Viewers", members: [{ value: bob.id }] });
    expect(viewers.status).toBe(201);
    const admins = await scim(token, "POST", "/Groups", { schemas: [GROUP], displayName: "GuideRecorder Admins", members: [{ value: bob.id }] });
    expect(admins.status).toBe(201);
    expect(await memberRole(db, owner.organizationId, bobId)).toBe("admin");

    const { id: adminsId } = (await admins.json()) as { id: string };
    expect((await scim(token, "DELETE", `/Groups/${adminsId}`)).status).toBe(204);
    expect(await memberRole(db, owner.organizationId, bobId)).toBe("viewer");
  });

  it("removes deactivated users and signs them out", async () => {
    const { token } = await connect();
    const bob = await provision(token, "bob@acme.com");
    const bobId = await userIdOf("bob@acme.com");
    await db.insert(session).values({ id: "s", token: "tok", userId: bobId, expiresAt: new Date(Date.now() + 60_000) });

    const patched = await scim(token, "PATCH", `/Users/${bob.id}`, { schemas: [PATCH], Operations: [{ op: "replace", path: "active", value: false }] });
    expect(patched.status).toBe(200);
    expect(await memberRole(db, owner.organizationId, bobId)).toBeNull();
    expect(await db.select().from(session).where(eq(session.userId, bobId))).toEqual([]);
  });

  it("never changes owners", async () => {
    const { token } = await connect();
    // Someone made owner by hand keeps it, whatever the directory says.
    const ada = await provision(token, "ada-scim@acme.com");
    await db.update(member).set({ role: "owner" }).where(eq(member.userId, await userIdOf("ada-scim@acme.com")));
    await scim(token, "PATCH", `/Users/${ada.id}`, { schemas: [PATCH], Operations: [{ op: "replace", path: "active", value: false }] });
    expect(await memberRole(db, owner.organizationId, await userIdOf("ada-scim@acme.com"))).toBe("owner");
  });

  it("respects seats", async () => {
    await db.update(subscription).set({ seats: 2 }).where(eq(subscription.id, "sub"));
    const { token } = await connect();
    await provision(token, "bob@acme.com");
    const refused = await scim(token, "POST", "/Users", { schemas: [USER], userName: "cy@acme.com", emails: [{ value: "cy@acme.com", primary: true }] });
    expect(refused.status).toBe(403);
  });

  it("stops accepting a revoked token", async () => {
    const { token, connectionId } = await connect();
    const [credential] = await db.select().from(scimManagedCredential);
    expect((await revokeScimToken(db, t.auth, owner, connectionId, credential!.credentialId)).ok).toBe(true);
    expect((await scim(token, "GET", "/Users")).status).toBe(401);
  });

  it("rejects requests without a valid token", async () => {
    await connect();
    expect((await scim("nope", "GET", "/Users")).status).toBe(401);
    expect((await scim("ba_scim_credential_x.y", "GET", "/Users")).status).toBe(401);
  });
});

describe("SCIM gating", () => {
  it("needs the sso entitlement to make connections", async () => {
    await db.update(subscription).set({ plan: "pro" }).where(eq(subscription.id, "sub"));
    expect(await createScimConnection(db, t.auth, owner)).toMatchObject({ ok: false, status: 402 });
  });

  it("refuses requests once the workspace leaves Enterprise", async () => {
    const { token } = await connect();
    await db.update(subscription).set({ status: "canceled" }).where(eq(subscription.id, "sub"));
    const response = await scim(token, "GET", "/Users");
    expect(response.status).toBe(403);
    expect(((await response.json()) as { detail: string }).detail).toContain("plan");
  });

  it("only lets owners and admins make connections", async () => {
    const grace = await t.signUp("Grace", "grace@acme.com");
    await db.insert(member).values({ id: "m", organizationId: owner.organizationId, userId: grace.userId, role: "editor", createdAt: new Date() });
    expect(await createScimConnection(db, t.auth, { ...owner, userId: grace.userId, headers: grace.headers })).toMatchObject({ ok: false, status: 403 });
  });
});
