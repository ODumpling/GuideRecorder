import { and, eq, inArray } from "drizzle-orm";
import type { Guide, GuideStatus } from "@guiderecorder/guide-schema";
import { equal, type ReviewState, type Revision } from "@guiderecorder/sync";
import type { Database } from "@/db";
import { approvalPolicy, changeRequest, guideOwner, member, review } from "@/db/schema";
import { roles, type RoleName } from "./permissions";
import { workspacePlan } from "./workspace";

// Approval rules (Enterprise): which guides are gated, whose approvals
// count, and the review status of a guide. Pure, apart from loading a
// workspace's policies; `approvals.ts` runs the workflow on top.
//
// A guide is gated while it's locked. Its folder's policy says how many
// approvals it needs, whether authors may approve and whether new edits
// reset approvals; its owners are the ones who approve.
//
// The status flow maps onto guide.json's `GuideStatus`: `draft` (the head
// has changes nobody has submitted), `inReview` (a change request is open),
// `needsUpdate` (a reviewer asked for changes) and `published` (the head is
// what readers see).

export interface Policy {
  /** Library folder path; "" for the whole workspace. */
  folder: string;
  requiredApprovals: number;
  /** Who may approve; empty: anyone whose role has `guide:approve`. */
  approverIds: string[];
  allowSelfApproval: boolean;
  resetOnNewRevision: boolean;
}

/** What applies in a workspace with approvals and no policy of its own. */
export const DEFAULT_POLICY: Policy = {
  folder: "",
  requiredApprovals: 1,
  approverIds: [],
  allowSelfApproval: false,
  resetOnNewRevision: true,
};

/** The fields of guide.json the server owns for gated guides. */
export const SERVER_FIELDS = ["status", "lastReviewed", "version"] as const;

export type Verdict = "approve" | "reject" | "requestChanges" | "comment";
export const VERDICTS: readonly Verdict[] = ["approve", "reject", "requestChanges", "comment"];

/** The folder a guide is filed in. The `folder` column comes with the web library; until then every guide is unfiled. */
export function folderOf(row: object): string {
  const folder = (row as { folder?: string | null }).folder;
  return folder ? folder.replace(/^\/+|\/+$/g, "") : "";
}

/** The policy for a folder: its own, its nearest parent's, the workspace's, or the default. */
export function policyFor(policies: Policy[], folder: string): Policy {
  const byFolder = new Map(policies.map((p) => [p.folder, p]));
  const parts = folder ? folder.split("/") : [];
  for (let n = parts.length; n >= 0; n -= 1) {
    const found = byFolder.get(parts.slice(0, n).join("/"));
    if (found) return found;
  }
  return DEFAULT_POLICY;
}

/** A workspace's approval settings: off unless its plan has `approvals`. */
export interface Approvals {
  enabled: boolean;
  policies: Policy[];
}

export async function loadApprovals(db: Database, organizationId: string): Promise<Approvals> {
  const { entitlements } = await workspacePlan(db, organizationId);
  if (!entitlements.approvals) return { enabled: false, policies: [] };
  const rows = await db.select().from(approvalPolicy).where(eq(approvalPolicy.organizationId, organizationId));
  return { enabled: true, policies: rows.map(toPolicy) };
}

export function toPolicy(row: typeof approvalPolicy.$inferSelect): Policy {
  const { folder, requiredApprovals, approverIds, allowSelfApproval, resetOnNewRevision } = row;
  return { folder, requiredApprovals, approverIds, allowSelfApproval, resetOnNewRevision };
}

/**
 * The policy gating a guide, or null if its edits publish straight away:
 * null unless the guide is locked. Its owners approve (with none, anyone
 * whose role has `guide:approve`); it needs at least one approval, and
 * never more than it has owners.
 */
export function gate(approvals: Approvals, row: object, owners: string[]): Policy | null {
  if (!approvals.enabled || !(row as { locked?: boolean }).locked) return null;
  const policy = policyFor(approvals.policies, folderOf(row));
  const most = Math.max(1, owners.length);
  return { ...policy, approverIds: owners, requiredApprovals: Math.min(most, Math.max(1, policy.requiredApprovals)) };
}

/** The owners of these guides, by guide id. */
export async function ownersOf(db: Database, organizationId: string, guideIds: string[]): Promise<Map<string, string[]>> {
  const result = new Map<string, string[]>(guideIds.map((id) => [id, []]));
  if (!guideIds.length) return result;
  const rows = await db
    .select({ guideId: guideOwner.guideId, userId: guideOwner.userId })
    .from(guideOwner)
    .where(and(eq(guideOwner.organizationId, organizationId), inArray(guideOwner.guideId, guideIds)))
    .orderBy(guideOwner.createdAt);
  for (const r of rows) result.get(r.guideId)?.push(r.userId);
  return result;
}

export interface ReviewRecord {
  reviewerId: string | null;
  revisionId: string;
  verdict: string;
  createdAt: Date;
}

export interface Evaluation {
  /** Approvals that count, by reviewer. */
  approvedBy: string[];
  /** Reviewers whose latest word is "request changes". */
  changesRequestedBy: string[];
  required: number;
  /** Enough approvals and no outstanding request for changes: publish. */
  satisfied: boolean;
}

/**
 * Counts a change request's approvals under `policy`. A reviewer's latest
 * approve or request-changes is their verdict; with `resetOnNewRevision`
 * only verdicts on the revision now under review count. Approvals count if
 * the reviewer is one of the policy's approvers (the guide's owners, if
 * it has any), may still approve (`canApprove`, see `canApproveGuide`),
 * and isn't an author unless self-approval is allowed.
 */
export function evaluate(
  policy: Policy,
  request: { revisionId: string; authors: (string | null)[] },
  reviews: ReviewRecord[],
  canApprove: (userId: string) => boolean,
): Evaluation {
  const latest = new Map<string, ReviewRecord>();
  const ordered = [...reviews].sort((a, b) => a.createdAt.getTime() - b.createdAt.getTime());
  for (const r of ordered) {
    if (!r.reviewerId || (r.verdict !== "approve" && r.verdict !== "requestChanges")) continue;
    if (policy.resetOnNewRevision && r.revisionId !== request.revisionId) continue;
    latest.set(r.reviewerId, r);
  }
  const approvedBy: string[] = [];
  const changesRequestedBy: string[] = [];
  for (const [reviewerId, r] of latest) {
    if (r.verdict === "requestChanges") changesRequestedBy.push(reviewerId);
    else if (!approvalProblem(policy, reviewerId, request.authors, canApprove)) approvedBy.push(reviewerId);
  }
  return {
    approvedBy,
    changesRequestedBy,
    required: policy.requiredApprovals,
    satisfied: approvedBy.length >= policy.requiredApprovals && changesRequestedBy.length === 0,
  };
}

/** Why `userId` may not approve under `policy`, or null if they may. */
export function approvalProblem(
  policy: Policy,
  userId: string,
  authors: (string | null)[],
  canApprove: (userId: string) => boolean,
): string | null {
  if (policy.approverIds.length && !policy.approverIds.includes(userId)) return "Only the guide's owners can approve.";
  if (!canApprove(userId)) return "Your role doesn't allow approving.";
  if (!policy.allowSelfApproval && authors.includes(userId)) return "You can't approve your own change.";
  return null;
}

/** A gated guide's status. */
export function reviewStatus(
  g: { headRevisionId: string | null; publishedRevisionId: string | null },
  open: { changesRequested: boolean } | null,
): GuideStatus {
  if (open) return open.changesRequested ? "needsUpdate" : "inReview";
  return g.headRevisionId && g.headRevisionId === g.publishedRevisionId ? "published" : "draft";
}

/** The next version: the last number goes up ("1.2" → "1.3"); none yet → "1". */
export function bumpVersion(version: string | null | undefined): string {
  const v = (version ?? "").trim();
  if (!v) return "1";
  const m = /^(.*?)(\d+)(\D*)$/.exec(v);
  if (!m) return `${v}.1`;
  return `${m[1]}${Number(m[2]) + 1}${m[3]}`;
}

/** `YYYY-MM-DD` in UTC, as `lastReviewed` is written. */
export const today = (now = new Date()) => now.toISOString().slice(0, 10);

/**
 * A pushed guide with the server's fields put back: `lastReviewed` and
 * `version` as they were on the head (unset on a new guide, which hasn't
 * been reviewed; a new guide keeps the version it was made with), and
 * `status`. A client can't publish by setting them.
 */
export function withServerFields(sent: Guide, head: Guide | null, status: GuideStatus): Guide {
  const result: Record<string, unknown> = { ...sent, status };
  if (head) {
    for (const field of ["lastReviewed", "version"] as const) {
      if (head[field] == null) delete result[field];
      else result[field] = head[field];
    }
  } else {
    delete result.lastReviewed;
  }
  return result as unknown as Guide;
}

// Reading review state.

export interface OpenRequest {
  request: typeof changeRequest.$inferSelect;
  reviews: (typeof review.$inferSelect)[];
}

/** The open change requests of these guides, with their reviews, by guide id. */
export async function openRequests(db: Database, organizationId: string, guideIds: string[]): Promise<Map<string, OpenRequest>> {
  if (!guideIds.length) return new Map();
  const requests = await db
    .select()
    .from(changeRequest)
    .where(and(eq(changeRequest.organizationId, organizationId), inArray(changeRequest.guideId, guideIds), eq(changeRequest.status, "open")));
  const reviews = requests.length
    ? await db.select().from(review).where(inArray(review.changeRequestId, requests.map((r) => r.id)))
    : [];
  return new Map(requests.map((request) => [request.guideId, { request, reviews: reviews.filter((r) => r.changeRequestId === request.id) }]));
}

/** Who may approve which guides in a workspace. */
export interface Approvers {
  /** Members whose role has `guide:approve`. */
  byRole: Set<string>;
  /** Members whose role can edit guides: an owner among them approves their own guides. */
  editors: Set<string>;
}

export async function loadApprovers(db: Database, organizationId: string): Promise<Approvers> {
  const members = await db.select({ userId: member.userId, role: member.role }).from(member).where(eq(member.organizationId, organizationId));
  const allows = (role: string, action: "approve" | "update") => role in roles && roles[role as RoleName].authorize({ guide: [action] }).success;
  return {
    byRole: new Set(members.filter((m) => allows(m.role, "approve")).map((m) => m.userId)),
    editors: new Set(members.filter((m) => allows(m.role, "update")).map((m) => m.userId)),
  };
}

/**
 * `evaluate`'s `canApprove` for a guide: its owners who can still edit
 * guides; a guide without owners falls back to roles with `guide:approve`.
 */
export function canApproveGuide(approvers: Approvers, owners: string[]) {
  return (userId: string) => (owners.length ? owners.includes(userId) && approvers.editors.has(userId) : approvers.byRole.has(userId));
}

/** Who wrote a change: whoever submitted it and whoever made the revision under review. */
export function authorsOf(request: { authorId: string | null }, head: { authorId: string | null } | undefined) {
  return [request.authorId, head?.authorId ?? null].filter(Boolean);
}

type GuideRow = { id: string; headRevisionId: string | null; publishedRevisionId: string | null; locked: boolean };

/**
 * Where each guide stands in review. `revisions` holds the revisions the
 * caller loaded (heads, or published ones for readers who don't see drafts).
 */
export async function reviewStates(
  db: Database,
  organizationId: string,
  rows: GuideRow[],
  revisions: Map<string, Revision>,
): Promise<Map<string, ReviewState>> {
  const result = new Map<string, ReviewState>();
  if (!rows.length) return result;
  const approvals = await loadApprovals(db, organizationId);
  const owners = approvals.enabled ? await ownersOf(db, organizationId, rows.filter((r) => r.locked).map((r) => r.id)) : new Map<string, string[]>();
  const gated = rows.flatMap((row) => {
    const policy = gate(approvals, row, owners.get(row.id) ?? []);
    return policy ? [{ row, policy }] : [];
  });
  const [open, approvers] = gated.length
    ? await Promise.all([openRequests(db, organizationId, gated.map((g) => g.row.id)), loadApprovers(db, organizationId)])
    : [new Map<string, OpenRequest>(), { byRole: new Set<string>(), editors: new Set<string>() }];
  for (const row of rows) {
    const shown = revisions.get(row.headRevisionId ?? "") ?? revisions.get(row.publishedRevisionId ?? "");
    result.set(row.id, { gated: false, status: shown?.guide.status ?? "draft", changeRequestId: null, approvals: 0, required: 0 });
  }
  for (const { row, policy } of gated) {
    const o = open.get(row.id);
    const evaluation = o
      ? evaluate(
          policy,
          { revisionId: o.request.revisionId, authors: authorsOf(o.request, revisions.get(o.request.revisionId)) },
          o.reviews,
          canApproveGuide(approvers, owners.get(row.id) ?? []),
        )
      : null;
    result.set(row.id, {
      gated: true,
      status: reviewStatus(row, o ? { changesRequested: evaluation!.changesRequestedBy.length > 0 } : null),
      changeRequestId: o?.request.id ?? null,
      approvals: evaluation?.approvedBy.length ?? 0,
      required: policy.requiredApprovals,
    });
  }
  return result;
}

/** Whether two guides say the same thing, leaving aside the server's fields. */
export function sameContent(a: Guide, b: Guide): boolean {
  const strip = (g: Guide) => {
    const copy: Record<string, unknown> = { ...g };
    for (const f of SERVER_FIELDS) delete copy[f];
    return copy;
  };
  return equal(strip(a), strip(b));
}
