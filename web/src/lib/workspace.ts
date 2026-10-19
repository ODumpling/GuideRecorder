import { and, asc, count, eq, gt } from "drizzle-orm";
import type { Database } from "@/db";
import { invitation, member, organization, subscription } from "@/db/schema";
import { ENTITLEMENTS, planOf, type Entitlements, type PlanName } from "./plans";

export interface WorkspacePlan {
  plan: PlanName;
  entitlements: Entitlements;
}

/** The workspace's plan from its subscriptions; seats follow an Enterprise subscription's seat count. */
export async function workspacePlan(db: Database, organizationId: string): Promise<WorkspacePlan> {
  const subs = await db
    .select({ plan: subscription.plan, status: subscription.status, seats: subscription.seats })
    .from(subscription)
    .where(eq(subscription.referenceId, organizationId));
  const plan = planOf(subs);
  const entitlements = { ...ENTITLEMENTS[plan] };
  const seats = subs.find((s) => s.plan === plan)?.seats;
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
