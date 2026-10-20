import { sso } from "@better-auth/sso";
import { and, eq, isNotNull } from "drizzle-orm";
import type { Database } from "@/db";
import { member, ssoProvider } from "@/db/schema";
import { getPolicy, type WorkspacePolicy } from "./policy";
import { securityEvent } from "./security-events";
import { memberRole, seatsTaken, workspacePlan } from "./workspace";

// Single sign-on: each Enterprise workspace registers its own OIDC or SAML
// identity provider for one or more email domains, then proves it owns them
// with a DNS TXT record. A verified domain is captured: people signing in
// with an email on it join the workspace, and if the workspace requires SSO
// they can only sign in through its IdP. Everything needs the `sso`
// entitlement (enterprise.ts checks it on every SSO endpoint).

/**
 * DNS TXT records prove domain ownership:
 * `_guiderecorder-verification-<providerId>.<domain>` must hold the token.
 */
export const DOMAIN_VERIFICATION_PREFIX = "guiderecorder-verification";

export function ssoPlugin() {
  return sso({
    domainVerification: { enabled: true, tokenPrefix: DOMAIN_VERIFICATION_PREFIX },
    // Membership is ours (see `signInDecision`): our roles, and seats.
    organizationProvisioning: { disabled: true },
    saml: {
      requireTimestamps: true,
      algorithms: { onDeprecated: "reject" },
    },
  });
}

/** The TXT record name for a provider's domain. */
export function verificationRecordName(providerId: string, domain: string): string {
  return `_${DOMAIN_VERIFICATION_PREFIX}-${providerId}.${domain}`;
}

/** The lowercase domain of an email address, or null if it has none. */
export function emailDomain(email: string): string | null {
  const at = email.lastIndexOf("@");
  const domain = at > 0 ? email.slice(at + 1).trim().toLowerCase() : "";
  return domain && !/[\s/\\:@]/.test(domain) ? domain : null;
}

/** A provider's domains: comma-separated, as Better Auth stores them. */
export function providerDomains(domain: string): string[] {
  return domain
    .split(",")
    .map((d) => d.trim().toLowerCase())
    .filter(Boolean);
}

/** Whether `domain` is one of the provider's domains or a subdomain of one. */
export function domainCovered(providerDomain: string, domain: string): boolean {
  return providerDomains(providerDomain).some((d) => domain === d || domain.endsWith(`.${d}`));
}

/**
 * The Enterprise workspace that captures this email: one whose verified SSO
 * domain covers it. If two workspaces verified the same domain, neither
 * captures it.
 */
export async function capturingWorkspace(db: Database, email: string): Promise<string | null> {
  const domain = emailDomain(email);
  if (!domain) return null;
  const verified = await db
    .select({ organizationId: ssoProvider.organizationId, domain: ssoProvider.domain })
    .from(ssoProvider)
    .where(and(eq(ssoProvider.domainVerified, true), isNotNull(ssoProvider.organizationId)));
  const orgs = new Set(verified.filter((p) => domainCovered(p.domain, domain)).map((p) => p.organizationId!));
  const entitled: string[] = [];
  for (const org of orgs) if ((await workspacePlan(db, org)).entitlements.sso) entitled.push(org);
  return entitled.length === 1 ? entitled[0]! : null;
}

export type SignInDecision =
  | { allow: true; /** A workspace to add the user to and make active. */ join: { organizationId: string; policy: WorkspacePolicy } | null }
  | { allow: false; code: "SSO_REQUIRED" | "SSO_NOT_AVAILABLE"; message: string; organizationId: string };

/**
 * What a new session means for the user's workspaces. `path` is the auth
 * endpoint that made it; SSO callbacks carry the provider id.
 */
export async function signInDecision(
  db: Database,
  user: { id: string; email: string; emailVerified: boolean },
  via: { path: string; providerId?: string },
): Promise<SignInDecision> {
  if (via.path.startsWith("/sso/") && via.providerId) {
    const [provider] = await db.select().from(ssoProvider).where(eq(ssoProvider.providerId, via.providerId));
    const organizationId = provider?.organizationId;
    if (!organizationId) return { allow: true, join: null };
    if (!(await workspacePlan(db, organizationId)).entitlements.sso) {
      return { allow: false, code: "SSO_NOT_AVAILABLE", message: "This workspace's plan doesn't include single sign-on.", organizationId };
    }
    return { allow: true, join: { organizationId, policy: await getPolicy(db, organizationId) } };
  }

  const organizationId = await capturingWorkspace(db, user.email);
  if (!organizationId) return { allow: true, join: null };
  const policy = await getPolicy(db, organizationId);
  // The desktop app's token comes from a web session that already passed.
  const exempt = via.path === "/device/token" || (await memberRole(db, organizationId, user.id)) === "owner";
  if (policy.requireSso && !exempt) {
    return { allow: false, code: "SSO_REQUIRED", message: "Your organization requires single sign-on. Sign in with SSO.", organizationId };
  }
  // Only a verified email proves the user is on the captured domain.
  return { allow: true, join: user.emailVerified ? { organizationId, policy } : null };
}

/**
 * Adds the user to the workspace with the policy's role if there's a seat.
 * Returns whether they're a member now.
 */
export async function joinWorkspace(db: Database, organizationId: string, userId: string, policy: WorkspacePolicy): Promise<boolean> {
  if (await memberRole(db, organizationId, userId)) return true;
  const { seats } = (await workspacePlan(db, organizationId)).entitlements;
  if ((await seatsTaken(db, organizationId)) >= seats) {
    await securityEvent({ type: "member.provisioned", organizationId, targetId: userId, actorId: "system", details: { joined: false, reason: "no seats" } });
    return false;
  }
  await db
    .insert(member)
    .values({ id: crypto.randomUUID(), organizationId, userId, role: policy.provisioningRole, createdAt: new Date() })
    .onConflictDoNothing({ target: [member.organizationId, member.userId] });
  await securityEvent({ type: "member.provisioned", organizationId, targetId: userId, actorId: "system", details: { via: "sso", role: policy.provisioningRole } });
  return true;
}
