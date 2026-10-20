import { and, asc, eq, inArray } from "drizzle-orm";
import type { Database } from "@/db";
import { changeRequest, guide, guideOwner, member, user } from "@/db/schema";
import { lockedGuide, moved, type ApprovalError } from "./approvals";
import { recordAudit } from "./audit";
import { roles } from "./permissions";
import { loadApprovals, loadApprovers, ownersOf } from "./review";
import type { Actor } from "./sync";

// Who owns a guide, and whether it's locked. Owners approve changes to a
// locked guide (review.ts `gate`); an unlocked guide's edits publish
// straight away. A guide's owners and workspace admins (`workspace:manage`)
// manage both. Locking needs the approvals entitlement; unlocking doesn't,
// and publishes the head, closing any open change request.

const forbidden = { status: 403, error: "Only the guide's owners and workspace admins can change this." } as const;
const notFound = { status: 404, error: "Guide not found." } as const;

const isAdmin = (actor: Actor) => roles[actor.role].authorize({ workspace: ["manage"] }).success;
const canEdit = (actor: Actor) => roles[actor.role].authorize({ guide: ["update"] }).success;

/** Whether the actor may change the guide's owners and lock. */
export function managesGuide(actor: Actor, owners: string[]) {
  return isAdmin(actor) || (owners.includes(actor.userId) && canEdit(actor));
}

/** A guide's owners and lock, who could be added as owners, and what the actor may change. */
export async function guideAccess(db: Database, organizationId: string, actor: Actor, guideId: string) {
  const [row] = await db.select().from(guide).where(and(eq(guide.organizationId, organizationId), eq(guide.id, guideId)));
  if (!row?.headRevisionId || row.deletedAt) return null;
  if (row.visibility !== "workspace" && row.ownerId !== actor.userId) return null;
  const owners = (await ownersOf(db, organizationId, [guideId])).get(guideId) ?? [];
  const [approvals, approvers, people] = await Promise.all([
    loadApprovals(db, organizationId),
    loadApprovers(db, organizationId),
    db
      .select({ id: user.id, name: user.name, email: user.email })
      .from(member)
      .innerJoin(user, eq(user.id, member.userId))
      .where(eq(member.organizationId, organizationId))
      .orderBy(asc(user.name)),
  ]);
  const byId = new Map(people.map((p) => [p.id, p]));
  return {
    locked: row.locked,
    /** Owners still in the workspace, in the order they were added. */
    owners: owners.flatMap((id) => (byId.has(id) ? [byId.get(id)!] : [])),
    /** Members who can edit guides and aren't owners yet. */
    candidates: people.filter((p) => approvers.editors.has(p.id) && !owners.includes(p.id)),
    canManage: managesGuide(actor, owners),
    /** Locking needs the approvals entitlement. */
    canLock: approvals.enabled,
  };
}

/** Locks or unlocks a guide. */
export async function setLocked(
  db: Database,
  organizationId: string,
  actor: Actor,
  guideId: string,
  locked: boolean,
): Promise<{ status: 200 } | ApprovalError> {
  if (locked && !(await loadApprovals(db, organizationId)).enabled) {
    return { status: 402, error: "Your plan doesn't include approvals. Upgrade to lock guides." };
  }
  return db.transaction(async (tx) => {
    const row = await lockedGuide(tx, organizationId, actor, guideId);
    if (!row) return notFound;
    const owners = (await ownersOf(tx, organizationId, [guideId])).get(guideId) ?? [];
    if (!managesGuide(actor, owners)) return forbidden;
    if (row.locked === locked) return { status: 200 } as const;
    await tx
      .update(guide)
      .set({ locked, ...(locked ? {} : { publishedRevisionId: row.headRevisionId }) })
      .where(and(eq(guide.organizationId, organizationId), eq(guide.id, guideId)));
    if (!locked) {
      // Nothing waits for approval any more: the head is what readers see.
      await tx
        .update(changeRequest)
        .set({ status: "withdrawn", closedBy: actor.userId, closedAt: new Date() })
        .where(and(eq(changeRequest.organizationId, organizationId), eq(changeRequest.guideId, guideId), eq(changeRequest.status, "open")));
    }
    await recordAudit(tx, organizationId, {
      actorId: actor.userId,
      action: locked ? "guide.lock" : "guide.unlock",
      targetType: "guide",
      targetId: guideId,
      metadata: locked ? {} : { publishedRevisionId: row.headRevisionId },
      ip: actor.ip,
      userAgent: actor.userAgent,
    });
    await moved(tx, organizationId, guideId);
    return { status: 200 } as const;
  });
}

/** Makes a member an owner of a guide. They must be able to edit guides. */
export async function addOwner(
  db: Database,
  organizationId: string,
  actor: Actor,
  guideId: string,
  userId: string,
): Promise<{ status: 200 } | ApprovalError> {
  return db.transaction(async (tx) => {
    const row = await lockedGuide(tx, organizationId, actor, guideId);
    if (!row) return notFound;
    const owners = (await ownersOf(tx, organizationId, [guideId])).get(guideId) ?? [];
    if (!managesGuide(actor, owners)) return forbidden;
    if (owners.includes(userId)) return { status: 200 } as const;
    if (!(await loadApprovers(tx, organizationId)).editors.has(userId)) {
      return { status: 400, error: "Owners must be members whose role can edit guides." } as const;
    }
    await tx.insert(guideOwner).values({ organizationId, guideId, userId, addedBy: actor.userId }).onConflictDoNothing();
    await recordAudit(tx, organizationId, {
      actorId: actor.userId,
      action: "guide.owner.add",
      targetType: "guide",
      targetId: guideId,
      metadata: { userId },
      ip: actor.ip,
      userAgent: actor.userAgent,
    });
    await moved(tx, organizationId, guideId);
    return { status: 200 } as const;
  });
}

/** Removes an owner from a guide; a guide keeps at least one. */
export async function removeOwner(
  db: Database,
  organizationId: string,
  actor: Actor,
  guideId: string,
  userId: string,
): Promise<{ status: 200 } | ApprovalError> {
  return db.transaction(async (tx) => {
    const row = await lockedGuide(tx, organizationId, actor, guideId);
    if (!row) return notFound;
    const owners = (await ownersOf(tx, organizationId, [guideId])).get(guideId) ?? [];
    if (!managesGuide(actor, owners)) return forbidden;
    if (!owners.includes(userId)) return { status: 404, error: "They don't own this guide." } as const;
    if (owners.length === 1) return { status: 400, error: "A guide needs at least one owner. Add another first." } as const;
    await tx
      .delete(guideOwner)
      .where(and(eq(guideOwner.organizationId, organizationId), eq(guideOwner.guideId, guideId), inArray(guideOwner.userId, [userId])));
    await recordAudit(tx, organizationId, {
      actorId: actor.userId,
      action: "guide.owner.remove",
      targetType: "guide",
      targetId: guideId,
      metadata: { userId },
      ip: actor.ip,
      userAgent: actor.userAgent,
    });
    await moved(tx, organizationId, guideId);
    return { status: 200 } as const;
  });
}
