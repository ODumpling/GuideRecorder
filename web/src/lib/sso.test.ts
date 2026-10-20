import dns from "node:dns/promises";
import { syncBuiltinESMExports } from "node:module";
import { eq } from "drizzle-orm";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { testAuth } from "../../test/auth";
import { testDb } from "../../test/db";
import type { Database } from "@/db";
import { auditEvent, session, ssoProvider, subscription, user } from "@/db/schema";
import { loadAdmin, registerSso, verifyDomain, type AdminActor } from "./admin";
import { setPolicy } from "./policy";
import { capturingWorkspace, domainCovered, emailDomain, signInDecision } from "./sso";
import { memberRole } from "./workspace";

let db: Database;
let t: ReturnType<typeof testAuth>;
let owner: AdminActor;

const saml = {
  type: "saml" as const,
  providerId: "acme-okta",
  domain: "acme.com, acme.co.uk",
  issuer: "http://www.okta.com/exk1",
  entryPoint: "https://acme.okta.com/app/sso/saml",
  cert: "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----",
};

/** Answers TXT lookups from `records`, as if they were published in DNS. */
function publish(records: Record<string, string[]>) {
  vi.spyOn(dns, "resolveTxt").mockImplementation(async (name: string) => {
    const found = records[name];
    if (!found) throw Object.assign(new Error(`queryTxt ENOTFOUND ${name}`), { code: "ENOTFOUND" });
    return found.map((r) => [r]);
  });
  // The plugin imports node:dns/promises itself; update its named exports.
  syncBuiltinESMExports();
}

beforeEach(async () => {
  db = await testDb();
  t = testAuth(db);
  const ada = await t.signUp("Ada", "ada@acme.com");
  const organizationId = (await t.auth.api.getSession({ headers: ada.headers }))!.session.activeOrganizationId!;
  await db.insert(subscription).values({ id: "sub", plan: "enterprise", referenceId: organizationId, status: "active", seats: 10 });
  owner = { userId: ada.userId, organizationId, headers: ada.headers };
});

afterEach(() => {
  vi.restoreAllMocks();
  syncBuiltinESMExports();
});

async function registered() {
  const result = await registerSso(db, t.auth, "http://localhost:3000", owner, saml);
  if (!result.ok) throw new Error(result.error);
  return result;
}

async function verified() {
  const { txtRecords } = await registered();
  publish(Object.fromEntries(txtRecords.map((r) => [r.name, [r.value]])));
  expect(await verifyDomain(db, t.auth, owner, saml.providerId)).toEqual({ ok: true });
}

describe("domains", () => {
  it("parse and match subdomains", () => {
    expect(emailDomain("Bob@Acme.COM")).toBe("acme.com");
    expect(emailDomain("nobody")).toBeNull();
    expect(domainCovered("acme.com,acme.co.uk", "eu.acme.com")).toBe(true);
    expect(domainCovered("acme.com", "notacme.com")).toBe(false);
  });
});

describe("domain verification", () => {
  it("asks for a TXT record per domain and verifies it through DNS", async () => {
    const { txtRecords } = await registered();
    expect(txtRecords.map((r) => r.name)).toEqual([
      "_guiderecorder-verification-acme-okta.acme.com",
      "_guiderecorder-verification-acme-okta.acme.co.uk",
    ]);
    const token = txtRecords[0]!.value;
    expect(token).toMatch(/^[\w-]{24}$/);

    // Nothing published yet.
    publish({});
    expect(await verifyDomain(db, t.auth, owner, saml.providerId)).toMatchObject({ ok: false, status: 502 });
    // One of two domains isn't enough.
    publish({ [txtRecords[0]!.name]: [token] });
    expect(await verifyDomain(db, t.auth, owner, saml.providerId)).toMatchObject({ ok: false, status: 502 });
    // `name=value` records count too.
    publish({ [txtRecords[0]!.name]: [token], [txtRecords[1]!.name]: [`_guiderecorder-verification-acme-okta=${token}`] });
    expect(await verifyDomain(db, t.auth, owner, saml.providerId)).toEqual({ ok: true });

    const [row] = await db.select().from(ssoProvider);
    expect(row).toMatchObject({ providerId: "acme-okta", organizationId: owner.organizationId, domainVerified: true, domain: "acme.com,acme.co.uk" });
    const admin = await loadAdmin(db, t.auth, owner);
    expect(admin).toMatchObject({ ok: true, sso: [{ providerId: "acme-okta", type: "saml", domainVerified: true }] });
  });

  it("captures the domain only once verified", async () => {
    const { txtRecords } = await registered();
    expect(await capturingWorkspace(db, "bob@acme.com")).toBeNull();
    publish(Object.fromEntries(txtRecords.map((r) => [r.name, [r.value]])));
    await verifyDomain(db, t.auth, owner, saml.providerId);
    expect(await capturingWorkspace(db, "bob@eu.acme.com")).toBe(owner.organizationId);
    expect(await capturingWorkspace(db, "bob@example.com")).toBeNull();
  });
});

describe("sign-in policy", () => {
  beforeEach(verified);

  it("adds verified users on a captured domain to the workspace", async () => {
    const bob = await t.signUp("Bob", "bob@acme.com");
    expect(await memberRole(db, owner.organizationId, bob.userId)).toBeNull();
    await db.update(user).set({ emailVerified: true }).where(eq(user.id, bob.userId));

    const signedIn = await t.auth.api.signInEmail({ body: { email: "bob@acme.com", password: "correct horse battery" }, asResponse: true });
    expect(signedIn.status).toBe(200);
    expect(await memberRole(db, owner.organizationId, bob.userId)).toBe("editor");
    // …and it's their active workspace.
    const cookie = signedIn.headers.getSetCookie().map((c) => c.split(";")[0]).join("; ");
    expect((await t.auth.api.getSession({ headers: new Headers({ cookie }) }))?.session.activeOrganizationId).toBe(owner.organizationId);
  });

  it("requires SSO for everyone on the domain but owners", async () => {
    const bob = await t.signUp("Bob", "bob@acme.com");
    await setPolicy(db, owner.organizationId, { requireSso: true });
    const sessions = async () => (await db.select().from(session).where(eq(session.userId, bob.userId))).length;
    const before = await sessions();

    const refused = await t.auth.api.signInEmail({ body: { email: "bob@acme.com", password: "correct horse battery" }, asResponse: true });
    expect(refused.status).toBe(403);
    expect(((await refused.json()) as { code: string }).code).toBe("SSO_REQUIRED");
    expect(await sessions()).toBe(before);
    // Bob is a member only through sign-up's personal workspace; a refused sign-in isn't one.
    const signIns = (await db.select().from(auditEvent).where(eq(auditEvent.action, "user.signIn"))).filter((e) => e.actorId === bob.userId);
    expect(signIns).toHaveLength(1);

    const ada = await t.auth.api.signInEmail({ body: { email: "ada@acme.com", password: "correct horse battery" }, asResponse: true });
    expect(ada.status).toBe(200);
  });

  it("lets SSO sign-ins through and joins the provider's workspace", async () => {
    await setPolicy(db, owner.organizationId, { requireSso: true, provisioningRole: "viewer" });
    const decision = await signInDecision(db, { id: "x", email: "bob@acme.com", emailVerified: false }, { path: "/sso/saml2/sp/acs/:providerId", providerId: "acme-okta" });
    expect(decision).toMatchObject({ allow: true, join: { organizationId: owner.organizationId, policy: { provisioningRole: "viewer" } } });
  });

  it("refuses SSO sign-ins once the workspace leaves Enterprise", async () => {
    await db.update(subscription).set({ plan: "pro" }).where(eq(subscription.id, "sub"));
    const decision = await signInDecision(db, { id: "x", email: "bob@acme.com", emailVerified: true }, { path: "/sso/callback/:providerId", providerId: "acme-okta" });
    expect(decision).toMatchObject({ allow: false, code: "SSO_NOT_AVAILABLE" });
    // And the domain isn't captured any more.
    expect(await capturingWorkspace(db, "bob@acme.com")).toBeNull();
  });
});

describe("SSO gating", () => {
  it("needs the sso entitlement", async () => {
    await db.update(subscription).set({ plan: "pro" }).where(eq(subscription.id, "sub"));
    expect(await registerSso(db, t.auth, "http://localhost:3000", owner, saml)).toMatchObject({ ok: false, status: 402 });
    // Calling Better Auth directly is gated too.
    const direct = t.auth.api.registerSSOProvider({
      headers: owner.headers,
      body: { providerId: "direct", issuer: "https://idp.example.com", domain: "acme.com", organizationId: owner.organizationId },
    });
    await expect(direct).rejects.toMatchObject({ statusCode: 402 });
  });

  it("keeps providers in a workspace", async () => {
    const direct = t.auth.api.registerSSOProvider({ headers: owner.headers, body: { providerId: "mine", issuer: "https://idp.example.com", domain: "acme.com" } });
    await expect(direct).rejects.toMatchObject({ statusCode: 400 });
  });

  it("only lets owners and admins register providers", async () => {
    const grace = await t.signUp("Grace", "grace@acme.com");
    const asGrace = { ...owner, userId: grace.userId, headers: grace.headers };
    expect(await registerSso(db, t.auth, "http://localhost:3000", asGrace, saml)).toMatchObject({ ok: false, status: 404 });
  });
});
