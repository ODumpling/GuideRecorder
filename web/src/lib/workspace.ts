import { and, asc, count, eq, gt } from "drizzle-orm";
import type { Database } from "@/db";
import { invitation, licenceGrant, member, organization, subscription } from "@/db/schema";
import { ACTIVE, ENTITLEMENTS, grantSubscriptions, planOf, type Entitlements, type PlanName } from "./plans";

export interface WorkspacePlan {
  plan: PlanName;
  entitlements: Entitlements;
}

/**
 * The workspace's plan from its subscriptions and current licence grants;
 * seats follow an Enterprise subscription's (or grant's) seat count.
 */
export async function workspacePlan(db: Database, organizationId: string, now = new Date()): Promise<WorkspacePlan> {
  const [subs, grants] = await Promise.all([
    db
      .select({ plan: subscription.plan, status: subscription.status, seats: subscription.seats })
      .from(subscription)
      .where(eq(subscription.referenceId, organizationId)),
    db.select().from(licenceGrant).where(eq(licenceGrant.organizationId, organizationId)),
  ]);
  return planFromSubscriptions([...subs, ...grantSubscriptions(grants, now)]);
}

export function planFromSubscriptions(subs: { plan: string; status: string | null; seats?: number | null }[]): WorkspacePlan {
  const plan = planOf(subs);
  const entitlements = { ...ENTITLEMENTS[plan] };
  // The most seats any active subscription or grant on the plan gives.
  const seats = Math.max(0, ...subs.filter((s) => s.plan === plan && s.status && ACTIVE.has(s.status)).map((s) => s.seats ?? 0));
  if (plan === "enterprise" && seats) entitlements.seats = seats;
  return { plan, entitlements };
}

/** Members plus pending, unexpired invitations. */
export async function seatsTaken(db: Database, organizationId: string): Promise<number> {
  const [members] = await db.select({ n: count() }).from(member).where(eq(member.organizationId, organizationId));
  const [pending] = await db
    .select({ n: count() })
    .from(invitation)
    .where(
      and(eq(invitation.organizationId, organizationId), eq(invitation.status, "pending"), gt(invitation.expiresAt, new Date())),
    );
  return (members?.n ?? 0) + (pending?.n ?? 0);
}

/** The user's role in the workspace, or null if they aren't a member. */
export async function memberRole(db: Database, organizationId: string, userId: string): Promise<string | null> {
  const [row] = await db
    .select({ role: member.role })
    .from(member)
    .where(and(eq(member.organizationId, organizationId), eq(member.userId, userId)));
  return row?.role ?? null;
}

/** The user's oldest workspace, used as the active one for new sessions. */
export async function firstWorkspaceId(db: Database, userId: string): Promise<string | null> {
  const [row] = await db
    .select({ id: member.organizationId })
    .from(member)
    .where(eq(member.userId, userId))
    .orderBy(asc(member.createdAt))
    .limit(1);
  return row?.id ?? null;
}

/**
 * The workspace a new session starts in: the user's oldest, or a personal
 * workspace created for them (every user has one; creating it is idempotent).
 */
export async function ensureWorkspace(db: Database, user: { id: string; name: string }): Promise<string> {
  const existing = await firstWorkspaceId(db, user.id);
  if (existing) return existing;
  const slug = `personal-${user.id.toLowerCase()}`;
  const now = new Date();
  return db.transaction(async (tx) => {
    await tx
      .insert(organization)
      .values({ id: crypto.randomUUID(), name: user.name ? `${user.name}'s workspace` : "My workspace", slug, createdAt: now })
      .onConflictDoNothing({ target: organization.slug });
    const [org] = await tx.select({ id: organization.id }).from(organization).where(eq(organization.slug, slug));
    await tx
      .insert(member)
      .values({ id: crypto.randomUUID(), organizationId: org!.id, userId: user.id, role: "owner", createdAt: now })
      .onConflictDoNothing({ target: [member.organizationId, member.userId] });
    return org!.id;
  });
}
