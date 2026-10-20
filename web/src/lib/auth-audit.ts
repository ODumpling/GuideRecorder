import { tryGetCurrentAuthEndpointContext } from "@better-auth/core/context";
import { eq } from "drizzle-orm";
import type { Database } from "@/db";
import { member } from "@/db/schema";
import { recordAudit, requestMeta, type AuditInput } from "./audit";

// Audit events for what Better Auth does: sign-ins, membership and role
// changes, invitations and billing. Its hooks name what changed; who did it
// and from where come from the request Better Auth is handling.

/** The signed-in user of the request Better Auth is handling, and where it came from. */
function current() {
  const ctx = tryGetCurrentAuthEndpointContext();
  const session = (ctx?.context as { session?: { user?: { id?: string } } | null } | undefined)?.session;
  const headers = ctx?.request?.headers ?? (ctx?.headers instanceof Headers ? ctx.headers : undefined);
  return { actorId: session?.user?.id ?? null, ...requestMeta(headers) };
}

type Event = Omit<AuditInput, "actorId"> & { actorId?: string | null };

function log(db: Database, organizationId: string, event: Event) {
  const { actorId, ip, userAgent } = current();
  return recordAudit(db, organizationId, { ip, userAgent, ...event, actorId: event.actorId === undefined ? actorId : event.actorId });
}

/** A sign-in, logged in each of the user's workspaces. */
export async function auditSignIn(db: Database, session: { id: string; userId: string; ipAddress?: string | null; userAgent?: string | null }) {
  const memberships = await db.select({ organizationId: member.organizationId }).from(member).where(eq(member.userId, session.userId));
  for (const { organizationId } of memberships) {
    await recordAudit(db, organizationId, {
      actorId: session.userId,
      action: "user.signIn",
      targetType: "user",
      targetId: session.userId,
      ip: session.ipAddress,
      userAgent: session.userAgent,
      // The Better Auth endpoint, e.g. `/sign-in/email` or `/device/token`.
      metadata: { sessionId: session.id, via: tryGetCurrentAuthEndpointContext()?.path ?? null },
    });
  }
}

type Member = { id: string; organizationId: string; userId: string; role: string };

/** Hooks for Better Auth's organization plugin. */
export function membershipAudit(db: Database) {
  return {
    afterAddMember: async ({ member: m }: { member: Member }) => {
      await log(db, m.organizationId, {
        action: "member.add",
        targetType: "member",
        targetId: m.userId,
        after: { role: m.role },
        metadata: { memberId: m.id, role: m.role },
      });
    },
    afterRemoveMember: async ({ member: m }: { member: Member }) => {
      await log(db, m.organizationId, {
        action: "member.remove",
        targetType: "member",
        targetId: m.userId,
        before: { role: m.role },
        metadata: { memberId: m.id, role: m.role },
      });
    },
    afterUpdateMemberRole: async ({ member: m, previousRole }: { member: Member; previousRole: string }) => {
      await log(db, m.organizationId, {
        action: "member.role",
        targetType: "member",
        targetId: m.userId,
        before: { role: previousRole },
        after: { role: m.role },
        metadata: { memberId: m.id, from: previousRole, to: m.role },
      });
    },
    afterCreateInvitation: async ({ invitation, inviter }: { invitation: Invitation; inviter: { id: string } }) => {
      await log(db, invitation.organizationId, {
        actorId: inviter.id,
        action: "invitation.create",
        targetType: "invitation",
        targetId: invitation.id,
        metadata: { email: invitation.email, role: invitation.role },
      });
    },
    afterAcceptInvitation: async ({ invitation, member: m }: { invitation: Invitation; member: Member }) => {
      await log(db, invitation.organizationId, {
        actorId: m.userId,
        action: "invitation.accept",
        targetType: "invitation",
        targetId: invitation.id,
        after: { role: m.role },
        metadata: { email: invitation.email, role: m.role, memberId: m.id },
      });
    },
    afterCancelInvitation: async ({ invitation, cancelledBy }: { invitation: Invitation; cancelledBy: { id: string } }) => {
      await log(db, invitation.organizationId, {
        actorId: cancelledBy.id,
        action: "invitation.cancel",
        targetType: "invitation",
        targetId: invitation.id,
        metadata: { email: invitation.email },
      });
    },
  };
}

type Invitation = { id: string; organizationId: string; email: string; role: string | null };

type Subscription = { id: string; referenceId: string; plan: string; status?: string | null; seats?: number | null };

/** A subscription change from a Stripe webhook; `change` names the callback. */
export async function auditBilling(db: Database, change: string, subscription: Subscription, stripeEvent?: { id: string; type: string }) {
  await log(db, subscription.referenceId, {
    actorId: null,
    action: "billing.subscription",
    targetType: "subscription",
    targetId: subscription.id,
    after: { plan: subscription.plan, status: subscription.status ?? null, seats: subscription.seats ?? null },
    metadata: {
      change,
      plan: subscription.plan,
      status: subscription.status ?? null,
      seats: subscription.seats ?? null,
      ...(stripeEvent ? { stripeEvent: stripeEvent.id, stripeEventType: stripeEvent.type } : {}),
    },
  });
}
