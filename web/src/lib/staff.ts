import { and, count, desc, eq, gt, ilike, inArray, isNotNull, max, or, sql } from "drizzle-orm";
import { z } from "zod";
import type { Database } from "@/db";
import { account, auditEvent, guide, licenceGrant, member, organization, revision, session, staff, subscription, user } from "@/db/schema";
import { recordAudit, type RequestMeta } from "./audit";
import { ENTITLEMENTS, grantIsCurrent } from "./plans";
import { securityEvent } from "./security-events";
import { staffRestoreGuide, TRASH_DAYS } from "./sync";
import { workspacePlan } from "./workspace";

// The staff console (`/staff`): GuideRecorder's developers and helpdesk look
// up any user, give workspaces a plan for a period (licence grants, outside
// Stripe) and take guides out of the trash before they're purged. Only users
// in the `staff` table get in; everyone else sees a 404. Every change is
// logged as a security event and in the workspace's audit log.

const DAY = 24 * 60 * 60 * 1000;
/** The longest a licence may be granted for. */
export const MAX_GRANT_DAYS = 5 * 366;
const PAGE = 50;

export interface StaffActor extends RequestMeta {
  userId: string;
}

export type StaffResult<T> = ({ ok: true } & T) | { ok: false; error: string };

export async function isStaff(db: Database, userId: string): Promise<boolean> {
  const [row] = await db.select({ userId: staff.userId }).from(staff).where(eq(staff.userId, userId));
  return Boolean(row);
}

/**
 * Makes sure each email has a user who is staff: creates missing users
 * (without a password; they sign in with a magic link or Google, which
 * verifies the address) and adds them to `staff`. Run on every deploy with
 * STAFF_EMAILS (`pnpm staff ensure`). Never removes anyone.
 */
export async function ensureStaff(db: Database, emails: string[]): Promise<{ email: string; created: boolean }[]> {
  const result: { email: string; created: boolean }[] = [];
  for (const raw of emails) {
    const email = raw.trim().toLowerCase();
    if (!email) continue;
    if (!z.email().safeParse(email).success) throw new Error(`Not an email: ${raw}`);
    const created = await db.transaction(async (tx) => {
      const [inserted] = await tx
        .insert(user)
        .values({ id: crypto.randomUUID(), name: email.split("@")[0]!, email })
        .onConflictDoNothing({ target: user.email })
        .returning({ id: user.id });
      const [found] = await tx.select({ id: user.id }).from(user).where(eq(user.email, email));
      await tx.insert(staff).values({ userId: found!.id, addedBy: "STAFF_EMAILS" }).onConflictDoNothing();
      return Boolean(inserted);
    });
    result.push({ email, created });
  }
  return result;
}

/** Users whose name, email or id matches, newest first; the newest users without a query. */
export async function findUsers(db: Database, query = "") {
  const q = query.trim();
  const pattern = `%${q.replace(/[\\%_]/g, (c) => `\\${c}`)}%`;
  const lastSeen = db
    .select({ userId: session.userId, at: max(session.updatedAt).as("at") })
    .from(session)
    .groupBy(session.userId)
    .as("last_seen");
  const workspaces = db
    .select({ userId: member.userId, n: count().as("n") })
    .from(member)
    .groupBy(member.userId)
    .as("workspaces");
  return db
    .select({
      id: user.id,
      name: user.name,
      email: user.email,
      emailVerified: user.emailVerified,
      createdAt: user.createdAt,
      lastSeenAt: lastSeen.at,
      workspaces: sql<number>`coalesce(${workspaces.n}, 0)`.mapWith(Number),
    })
    .from(user)
    .leftJoin(lastSeen, eq(lastSeen.userId, user.id))
    .leftJoin(workspaces, eq(workspaces.userId, user.id))
    .where(q ? or(ilike(user.email, pattern), ilike(user.name, pattern), eq(user.id, q)) : undefined)
    .orderBy(desc(user.createdAt))
    .limit(PAGE);
}

/**
 * Everything the console shows about a user: their sign-in methods, their
 * workspaces with each one's plan, subscriptions and licence grants, and the
 * guides in those workspaces' trash that can still be restored.
 */
export async function userDetail(db: Database, userId: string, now = new Date()) {
  const [found] = await db.select().from(user).where(eq(user.id, userId));
  if (!found) return null;
  const [methods, [seen], memberships] = await Promise.all([
    db.select({ providerId: account.providerId }).from(account).where(eq(account.userId, userId)),
    db
      .select({ lastSeenAt: max(session.updatedAt), active: count() })
      .from(session)
      .where(and(eq(session.userId, userId), gt(session.expiresAt, now))),
    db
      .select({ id: organization.id, name: organization.name, slug: organization.slug, role: member.role, joinedAt: member.createdAt })
      .from(member)
      .innerJoin(organization, eq(organization.id, member.organizationId))
      .where(eq(member.userId, userId))
      .orderBy(member.createdAt),
  ]);
  const orgIds = memberships.map((m) => m.id);
  const [memberCounts, subs, grants, trash] = await Promise.all([
    orgIds.length
      ? db.select({ id: member.organizationId, n: count() }).from(member).where(inArray(member.organizationId, orgIds)).groupBy(member.organizationId)
      : [],
    orgIds.length ? db.select().from(subscription).where(inArray(subscription.referenceId, orgIds)) : [],
    orgIds.length
      ? db
          .select({ grant: licenceGrant, issuedBy: user.email })
          .from(licenceGrant)
          .leftJoin(user, eq(user.id, licenceGrant.issuedBy))
          .where(inArray(licenceGrant.organizationId, orgIds))
          .orderBy(desc(licenceGrant.createdAt))
      : [],
    restorable(db, orgIds, now),
  ]);
  const workspaces = await Promise.all(
    memberships.map(async (m) => ({
      ...m,
      members: memberCounts.find((c) => c.id === m.id)?.n ?? 0,
      plan: (await workspacePlan(db, m.id, now)).plan,
      subscriptions: subs
        .filter((s) => s.referenceId === m.id)
        .map((s) => ({ id: s.id, plan: s.plan, status: s.status, periodEnd: s.periodEnd, stripe: Boolean(s.stripeSubscriptionId) })),
      grants: grants
        .filter((g) => g.grant.organizationId === m.id)
        .map(({ grant: g, issuedBy }) => ({
          id: g.id,
          plan: g.plan,
          seats: g.seats,
          startsAt: g.startsAt,
          endsAt: g.endsAt,
          note: g.note,
          issuedBy,
          revokedAt: g.revokedAt,
          current: grantIsCurrent(g, now),
        })),
      trash: trash.filter((t) => t.organizationId === m.id),
    })),
  );
  return {
    user: { id: found.id, name: found.name, email: found.email, emailVerified: found.emailVerified, createdAt: found.createdAt },
    signInMethods: [...new Set(methods.map((m) => m.providerId))],
    sessions: { active: seen?.active ?? 0, lastSeenAt: seen?.lastSeenAt ?? null },
    staff: await isStaff(db, userId),
    workspaces,
  };
}

/** Guides in the workspaces' trash for under TRASH_DAYS (and not yet purged), with who deleted them. */
async function restorable(db: Database, organizationIds: string[], now: Date) {
  if (!organizationIds.length) return [];
  const cutoff = new Date(now.getTime() - TRASH_DAYS * DAY);
  const rows = await db
    .select({
      organizationId: guide.organizationId,
      id: guide.id,
      visibility: guide.visibility,
      deletedAt: guide.deletedAt,
      title: sql<string>`${revision.guideJson}->>'title'`,
      ownerEmail: user.email,
    })
    .from(guide)
    .innerJoin(revision, eq(revision.id, guide.headRevisionId))
    .leftJoin(user, eq(user.id, guide.ownerId))
    .where(and(inArray(guide.organizationId, organizationIds), isNotNull(guide.deletedAt), gt(guide.deletedAt, cutoff)))
    .orderBy(desc(guide.deletedAt));
  if (!rows.length) return [];
  const deletes = await db
    .select({ organizationId: auditEvent.organizationId, targetId: auditEvent.targetId, email: user.email, seq: auditEvent.seq })
    .from(auditEvent)
    .leftJoin(user, eq(user.id, auditEvent.actorId))
    .where(
      and(
        inArray(auditEvent.organizationId, organizationIds),
        eq(auditEvent.action, "guide.delete"),
        inArray(auditEvent.targetId, rows.map((r) => r.id)),
      ),
    )
    .orderBy(desc(auditEvent.seq));
  return rows.map((r) => ({
    ...r,
    deletedAt: r.deletedAt!,
    purgeAt: new Date(r.deletedAt!.getTime() + TRASH_DAYS * DAY),
    deletedBy: deletes.find((d) => d.organizationId === r.organizationId && d.targetId === r.id)?.email ?? null,
  }));
}

export const licenceInput = z.object({
  organizationId: z.string().min(1),
  plan: z.enum(["pro", "enterprise"]),
  /** The last day it's valid (UTC), inclusive. */
  until: z.iso.date(),
  /** Enterprise only; the plan's default when unset. */
  seats: z.number().int().min(1).max(100_000).optional(),
  note: z.string().trim().min(1, "Say why, e.g. the ticket number.").max(500),
});

/** Gives a workspace a plan from now through `until`. */
export async function issueLicence(
  db: Database,
  actor: StaffActor,
  input: z.infer<typeof licenceInput>,
  now = new Date(),
): Promise<StaffResult<{ id: string; endsAt: Date }>> {
  const endsAt = new Date(new Date(`${input.until}T00:00:00Z`).getTime() + DAY);
  if (endsAt <= now) return { ok: false, error: "The end date has passed." };
  if (endsAt.getTime() - now.getTime() > MAX_GRANT_DAYS * DAY) return { ok: false, error: "Licences last at most five years." };
  const [org] = await db.select({ id: organization.id }).from(organization).where(eq(organization.id, input.organizationId));
  if (!org) return { ok: false, error: "Workspace not found." };
  const seats = input.plan === "enterprise" ? (input.seats ?? null) : null;
  const id = crypto.randomUUID();
  await db.transaction(async (tx) => {
    await tx.insert(licenceGrant).values({
      id,
      organizationId: input.organizationId,
      plan: input.plan,
      seats,
      startsAt: now,
      endsAt,
      note: input.note,
      issuedBy: actor.userId,
      createdAt: now,
    });
    await recordAudit(tx, input.organizationId, {
      actorId: actor.userId,
      action: "licence.grant",
      targetType: "licence",
      targetId: id,
      metadata: { plan: input.plan, seats: seats ?? ENTITLEMENTS[input.plan].seats, endsAt: endsAt.toISOString(), note: input.note, staff: true },
      ip: actor.ip,
      userAgent: actor.userAgent,
      at: now,
    });
  });
  await securityEvent({
    type: "staff.licence_issued",
    organizationId: input.organizationId,
    actorId: actor.userId,
    targetId: id,
    ip: actor.ip,
    details: { plan: input.plan, seats, endsAt: endsAt.toISOString(), note: input.note },
  });
  return { ok: true, id, endsAt };
}

/** Ends a licence grant now. */
export async function revokeLicence(db: Database, actor: StaffActor, grantId: string, now = new Date()): Promise<StaffResult<object>> {
  const [g] = await db.select().from(licenceGrant).where(eq(licenceGrant.id, grantId));
  if (!g) return { ok: false, error: "Licence not found." };
  if (g.revokedAt) return { ok: true };
  await db.transaction(async (tx) => {
    await tx.update(licenceGrant).set({ revokedAt: now, revokedBy: actor.userId }).where(eq(licenceGrant.id, grantId));
    await recordAudit(tx, g.organizationId, {
      actorId: actor.userId,
      action: "licence.revoke",
      targetType: "licence",
      targetId: grantId,
      metadata: { plan: g.plan, staff: true },
      ip: actor.ip,
      userAgent: actor.userAgent,
      at: now,
    });
  });
  await securityEvent({ type: "staff.licence_revoked", organizationId: g.organizationId, actorId: actor.userId, targetId: grantId, ip: actor.ip });
  return { ok: true };
}

/** Takes a guide out of the trash, if it went in under TRASH_DAYS ago. */
export async function restoreDeletedGuide(
  db: Database,
  actor: StaffActor,
  organizationId: string,
  guideId: string,
  now = new Date(),
): Promise<StaffResult<object>> {
  const [g] = await db
    .select({ deletedAt: guide.deletedAt, headRevisionId: guide.headRevisionId })
    .from(guide)
    .where(and(eq(guide.organizationId, organizationId), eq(guide.id, guideId)));
  if (!g?.headRevisionId) return { ok: false, error: "Guide not found, or already purged." };
  if (!g.deletedAt) return { ok: false, error: "The guide isn't in the trash." };
  if (now.getTime() - g.deletedAt.getTime() >= TRASH_DAYS * DAY) return { ok: false, error: `The guide was deleted over ${TRASH_DAYS} days ago.` };
  const result = await staffRestoreGuide(db, organizationId, actor, guideId);
  if (result.status !== 200) return { ok: false, error: result.error };
  await securityEvent({ type: "staff.guide_restored", organizationId, actorId: actor.userId, targetId: guideId, ip: actor.ip });
  return { ok: true };
}
