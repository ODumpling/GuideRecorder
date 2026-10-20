import { and, asc, desc, eq, inArray } from "drizzle-orm";
import { z } from "zod";
import type { Guide } from "@guiderecorder/guide-schema";
import type { ReviewState, Revision } from "@guiderecorder/sync";
import type { Database } from "@/db";
import { approvalPolicy, changeRequest, guide, member, review, revision, revisionAsset, user } from "@/db/schema";
import { recordAudit } from "./audit";
import { diffGuides } from "./diff";
import { roles } from "./permissions";
import {
  approvalProblem,
  authorsOf,
  bumpVersion,
  canApproveGuide,
  evaluate,
  folderOf,
  gate,
  loadApprovals,
  loadApprovers,
  ownersOf,
  policyFor,
  reviewStates,
  toPolicy,
  today,
  type Policy,
  type Verdict,
} from "./review";
import { getGuide, lockWorkspace, nextSeq, notify, readsDrafts, revisions, type Actor } from "./sync";

// The approval workflow (Enterprise). Editing a locked guide makes a draft
// head; submitting it opens a change request, which follows the head until
// it's closed. Reviewers approve, reject, request changes or comment, on
// the whole change or one item. When enough approvals count (`evaluate`),
// the head is published: a new revision with `status: published`, today's
// `lastReviewed` and the next `version`, which becomes both head and
// published. Each step moves the guide in the sync feed, so the desktop app
// hears about it, and goes in the audit log.

export type ApprovalError = { status: 400 | 402 | 403 | 404 | 409; error: string };

const can = (actor: Actor, action: "read" | "update" | "approve") => roles[actor.role].authorize({ guide: [action] }).success;
const forbidden = { status: 403, error: "Your role doesn't allow this." } as const;
const notFound = { status: 404, error: "Guide not found." } as const;
const noPlan = { status: 402, error: "Your plan doesn't include approvals. Upgrade to use them." } as const;

/** A guide the actor can see, locked for the rest of the transaction. */
export async function lockedGuide(tx: Database, organizationId: string, actor: Actor, guideId: string) {
  await lockWorkspace(tx, organizationId);
  const [row] = await tx.select().from(guide).where(and(eq(guide.organizationId, organizationId), eq(guide.id, guideId)));
  if (!row?.headRevisionId || row.deletedAt) return null;
  if (row.visibility !== "workspace" && row.ownerId !== actor.userId) return null;
  return row;
}

/** The policy gating a guide (null: unlocked), with its owners. */
async function gateOf(db: Database, organizationId: string, approvals: Awaited<ReturnType<typeof loadApprovals>>, row: typeof guide.$inferSelect) {
  const owners = (await ownersOf(db, organizationId, [row.id])).get(row.id) ?? [];
  return { policy: gate(approvals, row, owners), owners };
}

const notDecider = { status: 403, error: "Only the guide's owners can approve, reject or ask for changes." } as const;

/** Moves a guide in the sync feed, so clients hear its review state changed. */
export async function moved(tx: Database, organizationId: string, guideId: string) {
  const next = await nextSeq(tx);
  await tx.update(guide).set({ seq: next }).where(and(eq(guide.organizationId, organizationId), eq(guide.id, guideId)));
  await notify(tx, organizationId, next);
}

async function stateOf(tx: Database, organizationId: string, guideId: string): Promise<ReviewState> {
  const [row] = await tx.select().from(guide).where(and(eq(guide.organizationId, organizationId), eq(guide.id, guideId)));
  const loaded = await revisions(tx, [row!.headRevisionId!]);
  return (await reviewStates(tx, organizationId, [row!], new Map(loaded.map((r) => [r.id, r])))).get(guideId)!;
}

/** Where a guide the reader can see stands in review. */
export async function reviewStateOf(db: Database, organizationId: string, reader: Actor, guideId: string) {
  const found = await getGuide(db, organizationId, reader, guideId);
  if (!found || found.guide.deletedAt) return null;
  return (await reviewStates(db, organizationId, [found.guide], new Map([[found.head.id, found.head]]))).get(guideId)!;
}

/** Opens a change request for the guide's head (or returns the one open). */
export async function submitForReview(
  db: Database,
  organizationId: string,
  actor: Actor,
  guideId: string,
  { revisionId, note = "" }: { revisionId: string; note?: string },
): Promise<{ status: 200; changeRequestId: string; review: ReviewState } | ApprovalError> {
  if (!can(actor, "update")) return forbidden;
  return db.transaction(async (tx) => {
    const row = await lockedGuide(tx, organizationId, actor, guideId);
    if (!row) return notFound;
    const approvals = await loadApprovals(tx, organizationId);
    if (!approvals.enabled) return noPlan;
    if (!(await gateOf(tx, organizationId, approvals, row)).policy) {
      return { status: 400, error: "This guide is unlocked: edits publish straight away." } as const;
    }
    if (revisionId !== row.headRevisionId) return { status: 409, error: "The guide changed since. Sync, then submit again." } as const;
    const [open] = await tx
      .select()
      .from(changeRequest)
      .where(and(eq(changeRequest.organizationId, organizationId), eq(changeRequest.guideId, guideId), eq(changeRequest.status, "open")));
    if (open) return { status: 200, changeRequestId: open.id, review: await stateOf(tx, organizationId, guideId) } as const;
    if (row.headRevisionId === row.publishedRevisionId) return { status: 400, error: "Nothing to review: the guide is published as it is." } as const;

    const id = crypto.randomUUID();
    await tx.insert(changeRequest).values({
      id,
      organizationId,
      guideId,
      revisionId: row.headRevisionId!,
      baseRevisionId: row.publishedRevisionId,
      authorId: actor.userId,
      note,
    });
    await recordAudit(tx, organizationId, {
      actorId: actor.userId,
      action: "review.submit",
      targetType: "changeRequest",
      targetId: id,
      metadata: { guideId, revisionId: row.headRevisionId, note },
      ip: actor.ip,
      userAgent: actor.userAgent,
    });
    await moved(tx, organizationId, guideId);
    return { status: 200, changeRequestId: id, review: await stateOf(tx, organizationId, guideId) } as const;
  });
}

export const reviewRequest = z.object({
  verdict: z.enum(["approve", "reject", "requestChanges", "comment"]),
  /** The revision the reviewer looked at; must still be the one under review. */
  revisionId: z.string(),
  body: z.string().max(10_000).default(""),
  /** An item's ULID, for a comment on one step, section or note. */
  itemId: z.string().nullable().default(null),
});
export type ReviewRequest = z.infer<typeof reviewRequest>;

/**
 * Records a review. Approving, rejecting and asking for changes are for the
 * guide's owners (`canApproveGuide`); approving publishes once enough
 * approvals count, rejecting closes the request. Commenting needs only to
 * see drafts (`guide:update`).
 */
export async function reviewChange(
  db: Database,
  organizationId: string,
  actor: Actor,
  changeRequestId: string,
  request: z.input<typeof reviewRequest>,
): Promise<{ status: 200; published: boolean; review: ReviewState } | ApprovalError> {
  const { verdict, revisionId, body, itemId } = reviewRequest.parse(request);
  if (!can(actor, "update")) return forbidden;
  if (verdict === "requestChanges" && !body.trim()) return { status: 400, error: "Say what needs to change." };
  return db.transaction(async (tx) => {
    const [cr] = await tx
      .select()
      .from(changeRequest)
      .where(and(eq(changeRequest.organizationId, organizationId), eq(changeRequest.id, changeRequestId)));
    if (!cr) return { status: 404, error: "Change request not found." } as const;
    const row = await lockedGuide(tx, organizationId, actor, cr.guideId);
    if (!row) return notFound;
    // Re-read under the lock: a push may have moved it meanwhile.
    const [current] = await tx.select().from(changeRequest).where(eq(changeRequest.id, cr.id));
    if (current!.status !== "open") return { status: 409, error: "This change request is closed." } as const;
    const approvals = await loadApprovals(tx, organizationId);
    if (!approvals.enabled) return noPlan;
    const { policy, owners } = await gateOf(tx, organizationId, approvals, row);
    if (!policy && verdict !== "comment") return { status: 400, error: "This guide is unlocked now: edits publish straight away." } as const;
    if (verdict !== "comment" && revisionId !== current!.revisionId) {
      return { status: 409, error: "The change was updated since you opened it. Review the new version." } as const;
    }
    const [head] = await revisions(tx, [current!.revisionId]);
    if (itemId && !head!.guide.items.some((i) => i.id === itemId)) return { status: 400, error: "No such step in this change." } as const;

    const authors = authorsOf(current!, head);
    const canApprove = canApproveGuide(await loadApprovers(tx, organizationId), owners);
    if (verdict === "approve") {
      const problem = approvalProblem(policy!, actor.userId, authors, canApprove);
      if (problem) return { status: 403, error: problem } as const;
    } else if (verdict !== "comment" && !canApprove(actor.userId)) {
      return notDecider;
    }

    await tx.insert(review).values({
      id: crypto.randomUUID(),
      changeRequestId: cr.id,
      reviewerId: actor.userId,
      revisionId: verdict === "comment" ? revisionId : current!.revisionId,
      verdict,
      body,
      itemId,
    });
    await recordAudit(tx, organizationId, {
      actorId: actor.userId,
      action: `review.${verdict}` as const,
      targetType: "changeRequest",
      targetId: cr.id,
      metadata: { guideId: cr.guideId, revisionId: current!.revisionId, ...(itemId ? { itemId } : {}) },
      ip: actor.ip,
      userAgent: actor.userAgent,
    });

    let published = false;
    if (verdict === "reject") {
      await tx.update(changeRequest).set({ status: "rejected", closedBy: actor.userId, closedAt: new Date() }).where(eq(changeRequest.id, cr.id));
    } else if (verdict === "approve") {
      const reviews = await tx.select().from(review).where(eq(review.changeRequestId, cr.id));
      if (evaluate(policy!, { revisionId: current!.revisionId, authors }, reviews, canApprove).satisfied) {
        await publish(tx, organizationId, actor, row, current!, head!);
        published = true;
      }
    }
    if (verdict !== "comment") await moved(tx, organizationId, cr.guideId);
    return { status: 200, published, review: await stateOf(tx, organizationId, cr.guideId) } as const;
  });
}

/**
 * Publishes an approved change: a new revision on top of it with the
 * server's fields set, as both head and published.
 */
async function publish(
  tx: Database,
  organizationId: string,
  actor: Actor,
  row: typeof guide.$inferSelect,
  cr: typeof changeRequest.$inferSelect,
  head: Revision,
) {
  const previous = row.publishedRevisionId ? (await revisions(tx, [row.publishedRevisionId]))[0] : undefined;
  const content: Guide = { ...head.guide, status: "published", lastReviewed: today(), version: bumpVersion(head.guide.version) };
  const id = crypto.randomUUID();
  await tx.insert(revision).values({
    id,
    organizationId,
    guideId: row.id,
    parentId: head.id,
    guideJson: content,
    authorId: actor.userId,
    source: "web",
  });
  const assets = Object.entries(head.assets);
  if (assets.length) await tx.insert(revisionAsset).values(assets.map(([path, sha256]) => ({ revisionId: id, path, organizationId, sha256 })));
  await tx
    .update(guide)
    .set({ headRevisionId: id, publishedRevisionId: id })
    .where(and(eq(guide.organizationId, organizationId), eq(guide.id, row.id)));
  await tx
    .update(changeRequest)
    .set({ status: "approved", publishedRevisionId: id, closedBy: actor.userId, closedAt: new Date() })
    .where(eq(changeRequest.id, cr.id));
  await recordAudit(tx, organizationId, {
    actorId: actor.userId,
    action: "guide.publish",
    targetType: "guide",
    targetId: row.id,
    before: previous ? { guide: previous.guide, assets: previous.assets } : undefined,
    after: { guide: content, assets: head.assets },
    metadata: { revisionId: id, changeRequestId: cr.id, version: content.version, title: content.title },
    ip: actor.ip,
    userAgent: actor.userAgent,
  });
}

/** Withdraws an open change request: its author, or one of the guide's owners. */
export async function withdrawChange(
  db: Database,
  organizationId: string,
  actor: Actor,
  changeRequestId: string,
): Promise<{ status: 200; review: ReviewState } | ApprovalError> {
  return db.transaction(async (tx) => {
    const [cr] = await tx
      .select()
      .from(changeRequest)
      .where(and(eq(changeRequest.organizationId, organizationId), eq(changeRequest.id, changeRequestId)));
    if (!cr) return { status: 404, error: "Change request not found." } as const;
    const row = await lockedGuide(tx, organizationId, actor, cr.guideId);
    if (!row) return notFound;
    const owners = (await ownersOf(tx, organizationId, [row.id])).get(row.id) ?? [];
    if (cr.authorId !== actor.userId && !canApproveGuide(await loadApprovers(tx, organizationId), owners)(actor.userId)) return forbidden;
    const [current] = await tx.select().from(changeRequest).where(eq(changeRequest.id, cr.id));
    if (current!.status !== "open") return { status: 409, error: "This change request is closed." } as const;
    await tx.update(changeRequest).set({ status: "withdrawn", closedBy: actor.userId, closedAt: new Date() }).where(eq(changeRequest.id, cr.id));
    await recordAudit(tx, organizationId, {
      actorId: actor.userId,
      action: "review.withdraw",
      targetType: "changeRequest",
      targetId: cr.id,
      metadata: { guideId: cr.guideId },
      ip: actor.ip,
      userAgent: actor.userAgent,
    });
    await moved(tx, organizationId, cr.guideId);
    return { status: 200, review: await stateOf(tx, organizationId, cr.guideId) } as const;
  });
}

/**
 * Everything the review page shows for a guide: its open change request
 * (or the latest closed one, or none), what it changes against the
 * published revision, the reviews, and what the actor may do.
 */
export async function getReview(db: Database, organizationId: string, actor: Actor, guideId: string) {
  if (!readsDrafts(actor.role)) return null;
  const [row] = await db.select().from(guide).where(and(eq(guide.organizationId, organizationId), eq(guide.id, guideId)));
  if (!row?.headRevisionId || row.deletedAt) return null;
  if (row.visibility !== "workspace" && row.ownerId !== actor.userId) return null;

  const approvals = await loadApprovals(db, organizationId);
  const { policy, owners } = await gateOf(db, organizationId, approvals, row);
  const [cr] = await db
    .select()
    .from(changeRequest)
    .where(and(eq(changeRequest.organizationId, organizationId), eq(changeRequest.guideId, guideId)))
    .orderBy(desc(changeRequest.createdAt))
    .limit(1);
  const open = cr?.status === "open" ? cr : undefined;
  // An open request (or none) compares the head with what's live; a closed one, what it reviewed.
  const [beforeId, afterId] = open || !cr ? [row.publishedRevisionId, row.headRevisionId] : [cr.baseRevisionId, cr.revisionId];
  const loaded = new Map((await revisions(db, [beforeId, afterId, row.headRevisionId].filter((id): id is string => !!id))).map((r) => [r.id, r]));
  const before = beforeId ? (loaded.get(beforeId) ?? null) : null;
  const after = loaded.get(afterId!) ?? loaded.get(row.headRevisionId)!;

  const reviewRows = cr
    ? await db
        .select({ review, name: user.name, email: user.email })
        .from(review)
        .leftJoin(user, eq(user.id, review.reviewerId))
        .where(eq(review.changeRequestId, cr.id))
        .orderBy(asc(review.createdAt))
    : [];
  const approvers = await loadApprovers(db, organizationId);
  const canApprove = canApproveGuide(approvers, owners);
  const authors = cr ? authorsOf(cr, loaded.get(cr.revisionId)) : [];
  const evaluation = open && policy ? evaluate(policy, { revisionId: open.revisionId, authors }, reviewRows.map((r) => r.review), canApprove) : null;
  const state = (await reviewStates(db, organizationId, [row], loaded)).get(guideId)!;
  const effective = policy ?? policyFor(approvals.policies, folderOf(row));
  const eligibleIds = [...(owners.length ? owners.filter((id) => approvers.editors.has(id)) : approvers.byRole)];
  const eligible = eligibleIds.length
    ? await db
        .select({ id: user.id, name: user.name, email: user.email })
        .from(member)
        .innerJoin(user, eq(user.id, member.userId))
        .where(and(eq(member.organizationId, organizationId), inArray(member.userId, eligibleIds)))
    : [];
  const people = await namesOf(db, [cr?.authorId ?? null, cr?.closedBy ?? null]);

  return {
    guideId,
    enabled: approvals.enabled,
    gated: policy !== null,
    state,
    policy: effective,
    approvers: eligible,
    changeRequest: cr
      ? {
          id: cr.id,
          status: cr.status,
          note: cr.note,
          revisionId: cr.revisionId,
          author: cr.authorId ? (people.get(cr.authorId) ?? null) : null,
          closedBy: cr.closedBy ? (people.get(cr.closedBy) ?? null) : null,
          createdAt: cr.createdAt.toISOString(),
          closedAt: cr.closedAt?.toISOString() ?? null,
        }
      : null,
    reviews: reviewRows.map(({ review: r, name, email }) => ({
      id: r.id,
      verdict: r.verdict as Verdict,
      body: r.body,
      itemId: r.itemId,
      revisionId: r.revisionId,
      reviewer: r.reviewerId ? { id: r.reviewerId, name: name ?? r.reviewerId, email: email ?? null } : null,
      createdAt: r.createdAt.toISOString(),
    })),
    evaluation,
    before,
    after,
    diff: diffGuides(before, after),
    can: {
      submit: Boolean(policy && !open && can(actor, "update") && row.headRevisionId !== row.publishedRevisionId),
      approve: Boolean(open && policy && !approvalProblem(policy, actor.userId, authors, canApprove)),
      reject: Boolean(open && policy && canApprove(actor.userId)),
      comment: Boolean(open),
      withdraw: Boolean(open && (open.authorId === actor.userId || canApprove(actor.userId))),
    },
    approveProblem: open && policy ? approvalProblem(policy, actor.userId, authors, canApprove) : null,
  };
}

async function namesOf(db: Database, ids: (string | null)[]) {
  const wanted = [...new Set(ids.filter((id): id is string => !!id))];
  if (!wanted.length) return new Map<string, { id: string; name: string }>();
  const rows = await db.select({ id: user.id, name: user.name }).from(user).where(inArray(user.id, wanted));
  return new Map(rows.map((r) => [r.id, r]));
}

// Policies.

export const policyInput = z.object({
  folder: z
    .string()
    .max(500)
    .transform((f) => f.trim().replace(/^\/+|\/+$/g, "")),
  requiredApprovals: z.number().int().min(0).max(20),
  approverIds: z.array(z.string()).max(100).default([]),
  allowSelfApproval: z.boolean().default(false),
  resetOnNewRevision: z.boolean().default(true),
});

const canManage = (actor: Actor) => roles[actor.role].authorize({ organization: ["update"] }).success;

/** The workspace's policies, with the default for folders that have none. */
export async function listPolicies(db: Database, organizationId: string): Promise<Policy[]> {
  const rows = await db.select().from(approvalPolicy).where(eq(approvalPolicy.organizationId, organizationId)).orderBy(asc(approvalPolicy.folder));
  return rows.map(toPolicy);
}

/** Sets a folder's policy. Needs `organization:update` and the `approvals` entitlement. */
export async function savePolicy(
  db: Database,
  organizationId: string,
  actor: Actor,
  input: z.input<typeof policyInput>,
): Promise<{ status: 200; policy: Policy } | ApprovalError> {
  if (!canManage(actor)) return forbidden;
  const parsed = policyInput.safeParse(input);
  if (!parsed.success) return { status: 400, error: "Check the policy's fields." };
  const policy = parsed.data;
  if (!(await loadApprovals(db, organizationId)).enabled) return noPlan;
  const approvers = (await loadApprovers(db, organizationId)).byRole;
  if (policy.approverIds.some((id) => !approvers.has(id))) return { status: 400, error: "Approvers must be members whose role can approve." };
  if (policy.approverIds.length && policy.requiredApprovals > policy.approverIds.length) {
    return { status: 400, error: "More approvals are required than there are approvers." };
  }
  return db.transaction(async (tx) => {
    const [old] = await tx
      .select()
      .from(approvalPolicy)
      .where(and(eq(approvalPolicy.organizationId, organizationId), eq(approvalPolicy.folder, policy.folder)));
    const values = { ...policy, updatedBy: actor.userId, updatedAt: new Date() };
    await tx
      .insert(approvalPolicy)
      .values({ organizationId, ...values })
      .onConflictDoUpdate({ target: [approvalPolicy.organizationId, approvalPolicy.folder], set: values });
    await recordAudit(tx, organizationId, {
      actorId: actor.userId,
      action: "policy.update",
      targetType: "policy",
      targetId: policy.folder || "/",
      before: old ? toPolicy(old) : undefined,
      after: policy,
      metadata: { ...policy },
      ip: actor.ip,
      userAgent: actor.userAgent,
    });
    return { status: 200, policy } as const;
  });
}

/** Removes a folder's policy; its guides follow the nearest parent's. */
export async function deletePolicy(db: Database, organizationId: string, actor: Actor, folder: string): Promise<{ status: 200 } | ApprovalError> {
  if (!canManage(actor)) return forbidden;
  if (!(await loadApprovals(db, organizationId)).enabled) return noPlan;
  return db.transaction(async (tx) => {
    const [old] = await tx
      .delete(approvalPolicy)
      .where(and(eq(approvalPolicy.organizationId, organizationId), eq(approvalPolicy.folder, folder)))
      .returning();
    if (!old) return { status: 404, error: "No policy for that folder." } as const;
    await recordAudit(tx, organizationId, {
      actorId: actor.userId,
      action: "policy.delete",
      targetType: "policy",
      targetId: folder || "/",
      before: toPolicy(old),
      ip: actor.ip,
      userAgent: actor.userAgent,
    });
    return { status: 200 } as const;
  });
}
