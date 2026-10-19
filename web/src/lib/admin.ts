import { and, asc, eq, gt } from "drizzle-orm";
import { z } from "zod";
import type { Database } from "@/db";
import { invitation, member, ssoProvider, user } from "@/db/schema";
import { checkAccess } from "./access";
import type { Auth } from "./create-auth";
import { getPolicy, policySchema, setPolicy } from "./policy";
import { securityEvent } from "./security-events";
import { providerDomains, verificationRecordName } from "./sso";

// The admin console's operations for a workspace. Owners and admins
// (`workspace: manage`) see members and seats on any plan; SSO, SCIM, the
// sharing policy and retention need the `sso` entitlement (Enterprise).

export interface AdminActor {
  userId: string;
  organizationId: string;
  /** The caller's request headers, for Better Auth endpoints that need the session. */
  headers: Headers;
}

export type AdminResult<T> = ({ ok: true } & T) | { ok: false; status: number; error: string };

async function access(db: Database, actor: AdminActor, enterprise: boolean) {
  return checkAccess(db, actor.userId, actor.organizationId, { workspace: ["manage"] }, enterprise ? "sso" : undefined);
}

const SCIM_SCOPES = ["scim.users.read", "scim.users.write", "scim.groups.read", "scim.groups.write"] as const;

/** Everything the admin console shows. */
export async function loadAdmin(db: Database, auth: Auth, actor: AdminActor) {
  const granted = await access(db, actor, false);
  if (!granted.ok) return granted;
  const { organizationId } = actor;
  const enterprise = granted.plan.entitlements.sso;
  const [members, invitations, providers, policy] = await Promise.all([
    db
      .select({ id: member.id, userId: member.userId, role: member.role, name: user.name, email: user.email, joinedAt: member.createdAt })
      .from(member)
      .innerJoin(user, eq(user.id, member.userId))
      .where(eq(member.organizationId, organizationId))
      .orderBy(asc(member.createdAt)),
    db
      .select({ id: invitation.id, email: invitation.email, role: invitation.role, expiresAt: invitation.expiresAt })
      .from(invitation)
      .where(and(eq(invitation.organizationId, organizationId), eq(invitation.status, "pending"), gt(invitation.expiresAt, new Date()))),
    db.select().from(ssoProvider).where(eq(ssoProvider.organizationId, organizationId)),
    getPolicy(db, organizationId),
  ]);
  const scim = enterprise ? await scimConnections(auth, organizationId) : [];
  return {
    ok: true as const,
    role: granted.role,
    plan: granted.plan,
    enterprise,
    seats: { used: members.length + invitations.length, total: granted.plan.entitlements.seats },
    members,
    invitations,
    policy,
    sso: providers.map((p) => ({
      providerId: p.providerId,
      type: p.samlConfig ? ("saml" as const) : ("oidc" as const),
      issuer: p.issuer,
      domains: providerDomains(p.domain),
      domainVerified: Boolean(p.domainVerified),
      txtRecords: providerDomains(p.domain).map((d) => verificationRecordName(p.providerId, d)),
    })),
    scim,
  };
}

async function scimConnections(auth: Auth, organizationId: string) {
  const { connections } = await auth.api.listSCIMManagedConnections({ body: { provisioningDomainId: organizationId } });
  return Promise.all(
    connections
      .filter((c) => c.status !== "decommissioned")
      .map(async (c) => {
        const { credentials } = await auth.api.getSCIMManagedConnection({ body: { connectionId: c.connectionId, provisioningDomainId: organizationId } });
        return {
          connectionId: c.connectionId,
          status: c.status,
          createdAt: c.createdAt,
          credentials: credentials
            .filter((k) => k.status === "active")
            .map((k) => ({ credentialId: k.credentialId, expiresAt: k.expiresAt, lastUsedAt: k.lastUsedAt })),
        };
      }),
  );
}

export const policyUpdate = policySchema.partial();

export async function updatePolicy(db: Database, actor: AdminActor, update: z.infer<typeof policyUpdate>): Promise<AdminResult<{ policy: z.infer<typeof policySchema> }>> {
  const granted = await access(db, actor, true);
  if (!granted.ok) return granted;
  const before = await getPolicy(db, actor.organizationId);
  const policy = await setPolicy(db, actor.organizationId, update);
  const changed = Object.fromEntries(
    Object.entries(policy).filter(([k, v]) => before[k as keyof typeof before] !== v).map(([k, v]) => [k, { from: before[k as keyof typeof before], to: v }]),
  );
  await securityEvent({ type: "policy.updated", organizationId: actor.organizationId, actorId: actor.userId, details: changed });
  return { ok: true, policy };
}

export const ssoRegistration = z.discriminatedUnion("type", [
  z.object({
    type: z.literal("oidc"),
    providerId: z.string().regex(/^[a-z0-9-]{3,40}$/, "Use 3–40 lowercase letters, digits and dashes."),
    domain: z.string().min(3),
    issuer: z.url(),
    clientId: z.string().min(1),
    clientSecret: z.string().min(1),
    /** Defaults to `<issuer>/.well-known/openid-configuration`. */
    discoveryEndpoint: z.url().optional(),
  }),
  z.object({
    type: z.literal("saml"),
    providerId: z.string().regex(/^[a-z0-9-]{3,40}$/, "Use 3–40 lowercase letters, digits and dashes."),
    domain: z.string().min(3),
    /** The IdP's entity id. */
    issuer: z.string().min(1),
    /** The IdP's single sign-on URL. */
    entryPoint: z.url(),
    /** The IdP's signing certificate (PEM). */
    cert: z.string().min(1),
  }),
]);

/** Registers the workspace's identity provider; returns the DNS records to add. */
export async function registerSso(
  db: Database,
  auth: Auth,
  baseURL: string,
  actor: AdminActor,
  input: z.infer<typeof ssoRegistration>,
): Promise<AdminResult<{ providerId: string; txtRecords: { name: string; value: string }[] }>> {
  const granted = await access(db, actor, true);
  if (!granted.ok) return granted;
  const domain = providerDomains(input.domain).join(",");
  const common = { providerId: input.providerId, domain, organizationId: actor.organizationId };
  const body =
    input.type === "oidc"
      ? {
          ...common,
          issuer: input.issuer,
          oidcConfig: {
            clientId: input.clientId,
            clientSecret: input.clientSecret,
            ...(input.discoveryEndpoint ? { discoveryEndpoint: input.discoveryEndpoint } : {}),
            scopes: ["openid", "email", "profile"],
            pkce: true,
          },
        }
      : {
          ...common,
          issuer: `${baseURL}/api/auth/sso/saml2/sp/metadata?providerId=${input.providerId}`,
          samlConfig: {
            entryPoint: input.entryPoint,
            cert: input.cert,
            callbackUrl: `${baseURL}/dashboard`,
            idpMetadata: { entityID: input.issuer, cert: input.cert },
            wantAssertionsSigned: true,
            mapping: { email: "email", name: "displayName", firstName: "givenName", lastName: "surname" },
          },
        };
  try {
    await auth.api.registerSSOProvider({ headers: actor.headers, body });
    const { domainVerificationToken } = await auth.api.requestDomainVerification({ headers: actor.headers, body: { providerId: input.providerId } });
    return {
      ok: true,
      providerId: input.providerId,
      txtRecords: providerDomains(domain).map((d) => ({ name: verificationRecordName(input.providerId, d), value: domainVerificationToken })),
    };
  } catch (error) {
    return apiFailure(error);
  }
}

/** A fresh (or the pending) DNS token for an unverified provider. */
export async function domainVerificationToken(db: Database, auth: Auth, actor: AdminActor, providerId: string): Promise<AdminResult<{ token: string }>> {
  const granted = await access(db, actor, true);
  if (!granted.ok) return granted;
  try {
    const { domainVerificationToken: token } = await auth.api.requestDomainVerification({ headers: actor.headers, body: { providerId } });
    return { ok: true, token };
  } catch (error) {
    return apiFailure(error);
  }
}

/** Checks the TXT records; on success the domains are captured. */
export async function verifyDomain(db: Database, auth: Auth, actor: AdminActor, providerId: string): Promise<AdminResult<object>> {
  const granted = await access(db, actor, true);
  if (!granted.ok) return granted;
  try {
    await auth.api.verifyDomain({ headers: actor.headers, body: { providerId } });
    return { ok: true };
  } catch (error) {
    return apiFailure(error);
  }
}

/** Removes a provider. Allowed without Enterprise, to clean up after a downgrade. */
export async function deleteSso(db: Database, auth: Auth, actor: AdminActor, providerId: string): Promise<AdminResult<object>> {
  const granted = await access(db, actor, false);
  if (!granted.ok) return granted;
  try {
    await auth.api.deleteSSOProvider({ headers: actor.headers, body: { providerId } });
    return { ok: true };
  } catch (error) {
    return apiFailure(error);
  }
}

/** How long new SCIM tokens last. */
export const SCIM_TOKEN_DAYS = 365;

/** Makes a SCIM connection; the token is only ever returned here. */
export async function createScimConnection(
  db: Database,
  auth: Auth,
  actor: AdminActor,
  now = new Date(),
): Promise<AdminResult<{ connectionId: string; token: string; expiresAt: Date }>> {
  const granted = await access(db, actor, true);
  if (!granted.ok) return granted;
  const { connection, credential, token } = await auth.api.createSCIMManagedConnection({
    body: {
      scopes: [...SCIM_SCOPES],
      expiresAt: new Date(now.getTime() + SCIM_TOKEN_DAYS * 24 * 60 * 60 * 1000),
      creationRequestId: crypto.randomUUID(),
      provisioningDomainId: actor.organizationId,
      actorId: actor.userId,
    },
  });
  await securityEvent({ type: "scim.connection_created", organizationId: actor.organizationId, actorId: actor.userId, targetId: connection.connectionId });
  return { ok: true, connectionId: connection.connectionId, token, expiresAt: credential.expiresAt };
}

/** A new token for a connection; the old one keeps working until revoked, for a clean handover. */
export async function rotateScimToken(
  db: Database,
  auth: Auth,
  actor: AdminActor,
  connectionId: string,
  now = new Date(),
): Promise<AdminResult<{ token: string; credentialId: string }>> {
  const granted = await access(db, actor, true);
  if (!granted.ok) return granted;
  try {
    const { credential, token } = await auth.api.rotateSCIMManagedCredential({
      body: {
        scopes: [...SCIM_SCOPES],
        expiresAt: new Date(now.getTime() + SCIM_TOKEN_DAYS * 24 * 60 * 60 * 1000),
        connectionId,
        provisioningDomainId: actor.organizationId,
        actorId: actor.userId,
      },
    });
    await securityEvent({ type: "scim.credential_rotated", organizationId: actor.organizationId, actorId: actor.userId, targetId: credential.credentialId });
    return { ok: true, token, credentialId: credential.credentialId };
  } catch (error) {
    return apiFailure(error);
  }
}

export async function revokeScimToken(db: Database, auth: Auth, actor: AdminActor, connectionId: string, credentialId: string): Promise<AdminResult<object>> {
  const granted = await access(db, actor, false);
  if (!granted.ok) return granted;
  try {
    await auth.api.revokeSCIMManagedCredential({ body: { connectionId, credentialId, provisioningDomainId: actor.organizationId, actorId: actor.userId } });
    await securityEvent({ type: "scim.credential_revoked", organizationId: actor.organizationId, actorId: actor.userId, targetId: credentialId });
    return { ok: true };
  } catch (error) {
    return apiFailure(error);
  }
}

/** Ends a connection: its tokens stop working and the users it provisioned are removed from the workspace. */
export async function decommissionScim(db: Database, auth: Auth, actor: AdminActor, connectionId: string): Promise<AdminResult<object>> {
  const granted = await access(db, actor, false);
  if (!granted.ok) return granted;
  try {
    await auth.api.decommissionSCIMManagedConnection({ body: { connectionId, provisioningDomainId: actor.organizationId, actorId: actor.userId } });
    await securityEvent({ type: "scim.connection_decommissioned", organizationId: actor.organizationId, actorId: actor.userId, targetId: connectionId });
    return { ok: true };
  } catch (error) {
    return apiFailure(error);
  }
}

function apiFailure(error: unknown): { ok: false; status: number; error: string } {
  const e = error as { statusCode?: number; body?: { message?: string }; message?: string };
  if (typeof e?.statusCode === "number") return { ok: false, status: e.statusCode, error: e.body?.message ?? e.message ?? "Something went wrong." };
  throw error;
}
