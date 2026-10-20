import { scim, type SCIMProjectedUserState, type SCIMTransactionContext } from "@better-auth/scim";
import { APIError } from "better-auth/api";
import { PROVISIONING_ROLES, ROLE_RANK, type RoleName } from "./permissions";
import { DEFAULT_POLICY, type WorkspacePolicy } from "./policy";
import { securityEvent } from "./security-events";
import { domainCovered, emailDomain } from "./sso";
import { grantSubscriptions, type LicenceGrant } from "./plans";
import { planFromSubscriptions } from "./workspace";

// SCIM 2.0 provisioning (RFC 7643/7644) at /api/auth/scim/v2, from Better
// Auth's SCIM plugin. Each workspace makes connections in the admin console;
// a connection's bearer token is shown once and stored only as an HMAC. The
// plugin keeps the directory's Users and Groups; the projection below turns
// them into workspace members:
//
// - an active User is a member, with the most privileged role its Groups map
//   to (a Group named "Admins", "Approvers", "Editors" or "Viewers", with or
//   without a "GuideRecorder " prefix), else the policy's provisioning role;
// - a deactivated or deleted User stops being a member and is signed out;
// - owners are never changed.
//
// Callbacks run inside the plugin's transaction, so they only use its
// adapter. enterprise.ts rejects requests from workspaces without `sso`.

export function scimPlugin({ credentialHashSecret }: { credentialHashSecret: string }) {
  return scim({
    connections: [],
    managedConnections: { credentialHashSecret },
    compatibility: { microsoftEntra: { acceptLegacyGroupSchema: true } },
    identity: {
      // Link an existing account when its email is on one of the workspace's
      // verified domains (the workspace owns those addresses); otherwise
      // create a user.
      async resolveUser({ provisioningDomainId, resource }, { database }) {
        const email = resource.primaryEmail.toLowerCase();
        const domain = emailDomain(email);
        const existing = await database.findOne<{ id: string }>({ model: "user", where: [{ field: "email", value: email }] });
        if (!existing || !domain) return { action: "create" };
        const providers = await database.findMany<{ domain: string }>({
          model: "ssoProvider",
          where: [
            { field: "organizationId", value: provisioningDomainId },
            { field: "domainVerified", value: true },
          ],
        });
        return providers.some((p) => domainCovered(p.domain, domain))
          ? { action: "link", userId: existing.id, profile: "preserve" }
          : { action: "create" };
      },
      // Deactivated everywhere: end their sessions.
      async reconcileUser({ userId, active }, { database }) {
        if (!active) await database.deleteMany({ model: "session", where: [{ field: "userId", value: userId }] });
      },
    },
    projection: {
      roles: {
        map: ({ source }) => {
          const role = roleForGroup(source.displayName);
          return role ? [role] : [];
        },
        exists: ({ role }) => (PROVISIONING_ROLES as readonly string[]).includes(role),
      },
      reconcileUser: reconcileMember,
    },
  });
}

/** The role a SCIM Group grants, from its name: "Admins", "GuideRecorder Editors", "viewer"… */
export function roleForGroup(displayName: string): WorkspacePolicy["provisioningRole"] | null {
  const name = displayName
    .toLowerCase()
    .replace(/^guide\s*recorder[\s:_-]*/, "")
    .trim()
    .replace(/s$/, "");
  return (PROVISIONING_ROLES as readonly string[]).includes(name) ? (name as WorkspacePolicy["provisioningRole"]) : null;
}

/** The most privileged of the roles, or null for none. */
export function strongestRole(granted: string[]): RoleName | null {
  return ROLE_RANK.find((r) => granted.includes(r)) ?? null;
}

interface MemberRow {
  id: string;
  role: string;
}

/** Makes the user's membership in the workspace match the directory. */
export async function reconcileMember(state: SCIMProjectedUserState, { database }: SCIMTransactionContext) {
  const { provisioningDomainId: organizationId, userId, active } = state;
  const where = [
    { field: "organizationId", value: organizationId },
    { field: "userId", value: userId },
  ];
  const existing = await database.findOne<MemberRow>({ model: "member", where });
  if (existing?.role === "owner") return;

  if (!active) {
    if (existing) {
      await database.delete({ model: "member", where: [{ field: "id", value: existing.id }] });
      await securityEvent({ type: "member.deprovisioned", organizationId, targetId: userId, actorId: "scim" });
    }
    return;
  }

  const policy = (await database.findOne<WorkspacePolicy>({ model: "workspacePolicy", where: [{ field: "organizationId", value: organizationId }] })) ?? DEFAULT_POLICY;
  const role = strongestRole(state.grants.map((g) => g.role)) ?? policy.provisioningRole;
  if (existing) {
    if (existing.role !== role) {
      await database.update({ model: "member", where: [{ field: "id", value: existing.id }], update: { role } });
      await securityEvent({ type: "member.role_changed", organizationId, targetId: userId, actorId: "scim", details: { from: existing.role, to: role } });
    }
    return;
  }

  const subscriptions = await database.findMany<{ plan: string; status: string | null; seats: number | null }>({
    model: "subscription",
    where: [{ field: "referenceId", value: organizationId }],
  });
  const grants = await database.findMany<LicenceGrant>({ model: "licenceGrant", where: [{ field: "organizationId", value: organizationId }] });
  const { seats } = planFromSubscriptions([...subscriptions, ...grantSubscriptions(grants)]).entitlements;
  const members = await database.count({ model: "member", where: [{ field: "organizationId", value: organizationId }] });
  const pending = await database.count({
    model: "invitation",
    where: [
      { field: "organizationId", value: organizationId },
      { field: "status", value: "pending" },
      { field: "expiresAt", value: new Date(), operator: "gt" },
    ],
  });
  if (members + pending >= seats) throw new APIError("FORBIDDEN", { message: "No seats left in the workspace." });

  await database.create({ model: "member", data: { organizationId, userId, role, createdAt: new Date() } });
  await securityEvent({ type: "member.provisioned", organizationId, targetId: userId, actorId: "scim", details: { role } });
}

/** The workspace a managed SCIM token belongs to, or null if it isn't one. */
export async function scimTokenWorkspace(
  adapter: { findOne: <T>(q: { model: string; where: { field: string; value: string }[] }) => Promise<T | null> },
  authorization: string | null,
): Promise<string | null> {
  const credentialId = authorization?.match(/^Bearer\s+(ba_scim_credential_[A-Za-z0-9_-]+)\./i)?.[1];
  if (!credentialId) return null;
  const credential = await adapter.findOne<{ connectionRecordId: string }>({
    model: "scimManagedCredential",
    where: [{ field: "credentialId", value: credentialId }],
  });
  if (!credential) return null;
  const connection = await adapter.findOne<{ provisioningDomainId: string }>({
    model: "scimManagedConnection",
    where: [{ field: "id", value: credential.connectionRecordId }],
  });
  return connection?.provisioningDomainId ?? null;
}
