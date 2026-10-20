import { and, asc, eq, gt, inArray, isNotNull, isNull, lt, or, sql } from "drizzle-orm";
import { guideSchema, type Guide } from "@guiderecorder/guide-schema";
import {
  equal,
  imagePaths,
  type AssetMap,
  type Change,
  type ChangesResponse,
  type pushRequest,
  type ReviewState,
  type Revision,
  type RevisionSource,
} from "@guiderecorder/sync";
import type { z } from "zod";
import type { Database } from "@/db";
import { asset, changeRequest, guide, revision, revisionAsset } from "@/db/schema";
import { recordAudit, type RequestMeta } from "./audit";
import { roles, type RoleName } from "./permissions";
import { evaluate, gate, loadApprovals, openRequests, reviewStates, sameContent, withServerFields } from "./review";

// Desktop ⇄ web sync. Each guide is a chain of immutable revisions; a push
// adds one on top of the head it was made on, or gets the head back (409) to
// merge with. Every change gives the guide a new `seq`, so clients follow the
// workspace with `changes(since)`. Writes to a workspace take an advisory
// lock, so seqs commit in order and a cursor never skips a change.
//
// `headRevisionId` is the latest revision, `publishedRevisionId` what
// readers without `guide:update` (viewers, share links) see. A push
// publishes too, unless the guide is gated by an approval policy
// (`review.ts`): then the head is a draft until approvers publish it
// (`approvals.ts`), and the server owns its status, version and review date.

export const TRASH_DAYS = 30;
export const PAGE_SIZE = 200;
const DAY = 24 * 60 * 60 * 1000;

export interface Actor extends RequestMeta {
  userId: string;
  role: RoleName;
}

/** Whether a role reads drafts (each guide's head) or only what's published. */
export const readsDrafts = (role: RoleName) => roles[role].authorize({ guide: ["update"] }).success;

/** The revision a reader sees: the head, or the published revision (null if there isn't one yet). */
export function visibleRevisionId(g: { headRevisionId: string | null; publishedRevisionId: string | null }, role: RoleName) {
  return readsDrafts(role) ? g.headRevisionId : g.publishedRevisionId;
}

/**
 * The revision people outside the workspace see, e.g. through a share link
 * that isn't pinned to a revision: the published one. Ungated guides
 * publish on every push, so for them it's the head.
 */
export const publishedRevisionFor = (g: { publishedRevisionId: string | null }) => g.publishedRevisionId;

export type SyncError = { status: 400 | 403 | 404 | 422; error: string; [extra: string]: unknown };

export type PushResult =
  | { status: 200; revision: Revision; review: ReviewState }
  | { status: 409; error: string; head: Revision }
  | { status: 410; error: string; deletedAt: string; purgeAt: string }
  | SyncError;

const can = (actor: Actor, action: "read" | "create" | "update" | "delete") => roles[actor.role].authorize({ guide: [action] }).success;

/** Who may see a guide: every member, or only its owner if it's private. */
const visibleTo = (userId: string) => or(eq(guide.visibility, "workspace"), eq(guide.ownerId, userId));

export async function lockWorkspace(tx: Database, organizationId: string) {
  await tx.execute(sql`select pg_advisory_xact_lock(hashtext(${`sync:${organizationId}`}))`);
}

/**
 * Adds a revision to a guide, creating the guide if the server doesn't have
 * it. The guide must be valid, and every image it names must map to an
 * asset the workspace has committed.
 */
export async function push(
  db: Database,
  organizationId: string,
  actor: Actor,
  request: z.infer<typeof pushRequest>,
  source: RevisionSource,
): Promise<PushResult> {
  const parsed = guideSchema.safeParse(request.guide);
  if (!parsed.success) return { status: 400, error: "The guide isn't valid.", issues: parsed.error.issues };
  // Stored as sent, without the defaults parsing fills in, so clients get
  // back exactly what they pushed (but for the server's fields of gated guides).
  const sent = request.guide as unknown as Guide;
  if (sent.id !== request.guideId) return { status: 400, error: "guideId doesn't match the guide's id." };

  const paths = imagePaths(sent);
  const missingPaths = paths.filter((p) => !(p in request.assets));
  const extraPaths = Object.keys(request.assets).filter((p) => !paths.includes(p));
  if (missingPaths.length || extraPaths.length) {
    return { status: 400, error: "assets must map exactly the guide's image paths.", missingPaths, extraPaths };
  }
  const hashes = [...new Set(Object.values(request.assets))];
  if (hashes.length) {
    const have = await db
      .select({ sha256: asset.sha256 })
      .from(asset)
      .where(and(eq(asset.organizationId, organizationId), inArray(asset.sha256, hashes)));
    const committed = new Set(have.map((r) => r.sha256));
    const missing = hashes.filter((h) => !committed.has(h));
    if (missing.length) return { status: 422, error: "Upload and commit these images first.", missing };
  }

  return db.transaction(async (tx) => {
    await lockWorkspace(tx, organizationId);
    const [row] = await tx
      .select()
      .from(guide)
      .where(and(eq(guide.organizationId, organizationId), eq(guide.id, request.guideId)));

    if (row && row.visibility !== "workspace" && row.ownerId !== actor.userId) return { status: 404, error: "Guide not found." } as const;
    // A purged guide (no revisions left) can be pushed again as new.
    const exists = row?.headRevisionId != null;
    if (!can(actor, exists ? "update" : "create")) return { status: 403, error: "Your role doesn't allow this." } as const;
    if (exists && row.deletedAt) {
      return { status: 410, error: "The guide is in the trash.", ...trashDates(row.deletedAt) } as const;
    }

    const policy = gate(await loadApprovals(tx, organizationId), row ?? {});
    const head = exists ? (await revisions(tx, [row.headRevisionId!]))[0]! : null;
    if (head) {
      // Nothing new (or a retried push that already landed): the head as is.
      const same = policy ? sameContent(head.guide, sent) : equal(head.guide, sent);
      if (same && equal(head.assets, request.assets)) return { status: 200, revision: head, review: await reviewOf(tx, organizationId, row!, head) } as const;
      if (request.baseRevision !== row.headRevisionId) {
        return { status: 409, error: "The guide changed on the server. Merge and push again.", head } as const;
      }
    }

    const id = crypto.randomUUID();
    // A gated guide's edit is a draft: open change requests move onto it,
    // and the server sets its status.
    const open = policy && head ? (await openRequests(tx, organizationId, [request.guideId])).get(request.guideId) : undefined;
    let content = sent;
    if (policy) {
      const changesRequested = open
        ? evaluate(policy, { revisionId: id, authors: [] }, open.reviews, () => true).changesRequestedBy.length > 0
        : false;
      content = withServerFields(sent, head?.guide ?? null, open ? (changesRequested ? "needsUpdate" : "inReview") : "draft");
    }

    const next = await nextSeq(tx);
    if (row) {
      await tx
        .update(guide)
        .set({ seq: next, deletedAt: null, ...(exists ? {} : { ownerId: actor.userId }) })
        .where(and(eq(guide.organizationId, organizationId), eq(guide.id, request.guideId)));
    } else {
      await tx.insert(guide).values({ organizationId, id: request.guideId, ownerId: actor.userId, seq: next });
    }
    await tx.insert(revision).values({
      id,
      organizationId,
      guideId: request.guideId,
      parentId: exists ? row.headRevisionId : null,
      guideJson: content,
      authorId: actor.userId,
      source,
    });
    const assets = Object.entries(request.assets);
    if (assets.length) {
      await tx.insert(revisionAsset).values(assets.map(([path, sha256]) => ({ revisionId: id, path, organizationId, sha256 })));
    }
    const [updated] = await tx
      .update(guide)
      .set({ headRevisionId: id, ...(policy ? {} : { publishedRevisionId: id }) })
      .where(and(eq(guide.organizationId, organizationId), eq(guide.id, request.guideId)))
      .returning();
    if (open) await tx.update(changeRequest).set({ revisionId: id }).where(eq(changeRequest.id, open.request.id));
    await recordAudit(tx, organizationId, {
      actorId: actor.userId,
      action: exists ? "guide.edit" : "guide.create",
      targetType: "guide",
      targetId: request.guideId,
      before: head ? { guide: head.guide, assets: head.assets } : undefined,
      after: { guide: content, assets: request.assets },
      metadata: { revisionId: id, source, title: content.title, published: !policy },
      ip: actor.ip,
      userAgent: actor.userAgent,
    });
    await notify(tx, organizationId, next);
    const [created] = await revisions(tx, [id]);
    return { status: 200, revision: created!, review: await reviewOf(tx, organizationId, updated!, created!) } as const;
  });
}

async function reviewOf(db: Database, organizationId: string, row: typeof guide.$inferSelect, head: Revision): Promise<ReviewState> {
  return (await reviewStates(db, organizationId, [row], new Map([[head.id, head]]))).get(row.id)!;
}

/** Moves a guide to the trash; it's purged after TRASH_DAYS. */
export async function trashGuide(db: Database, organizationId: string, actor: Actor, guideId: string, now = new Date()) {
  return setDeleted(db, organizationId, actor, guideId, now);
}

/** Takes a guide out of the trash. */
export async function restoreGuide(db: Database, organizationId: string, actor: Actor, guideId: string) {
  return setDeleted(db, organizationId, actor, guideId, null);
}

async function setDeleted(
  db: Database,
  organizationId: string,
  actor: Actor,
  guideId: string,
  deletedAt: Date | null,
): Promise<{ status: 200; seq: bigint } | SyncError> {
  if (!can(actor, "delete")) return { status: 403, error: "Your role doesn't allow this." };
  return db.transaction(async (tx) => {
    await lockWorkspace(tx, organizationId);
    const [row] = await tx
      .select()
      .from(guide)
      .where(and(eq(guide.organizationId, organizationId), eq(guide.id, guideId), visibleTo(actor.userId)));
    if (!row?.headRevisionId) return { status: 404, error: "Guide not found." } as const;
    if (Boolean(row.deletedAt) === Boolean(deletedAt)) return { status: 200, seq: row.seq } as const;
    const next = await nextSeq(tx);
    await tx.update(guide).set({ deletedAt, seq: next }).where(and(eq(guide.organizationId, organizationId), eq(guide.id, guideId)));
    await recordAudit(tx, organizationId, {
      actorId: actor.userId,
      action: deletedAt ? "guide.delete" : "guide.restore",
      targetType: "guide",
      targetId: guideId,
      metadata: { revisionId: row.headRevisionId },
      ip: actor.ip,
      userAgent: actor.userAgent,
    });
    await notify(tx, organizationId, next);
    return { status: 200, seq: next } as const;
  });
}

/**
 * Deletes the revisions of guides in the trash for over TRASH_DAYS. The
 * guide rows stay as tombstones so clients still hear about the delete.
 */
export async function purgeTrash(db: Database, now = new Date()): Promise<number> {
  const cutoff = new Date(now.getTime() - TRASH_DAYS * DAY);
  return db.transaction(async (tx) => {
    const expired = await tx
      .update(guide)
      .set({ headRevisionId: null, publishedRevisionId: null })
      .where(and(isNotNull(guide.deletedAt), lt(guide.deletedAt, cutoff), isNotNull(guide.headRevisionId)))
      .returning({ organizationId: guide.organizationId, id: guide.id });
    for (const g of expired) {
      await tx.delete(revision).where(and(eq(revision.organizationId, g.organizationId), eq(revision.guideId, g.id)));
    }
    return expired.length;
  });
}

/** Who reads guides: their role decides whether they see drafts. */
export type Reader = Pick<Actor, "userId" | "role">;

/**
 * The workspace's guides changed after `since`, oldest change first: each
 * guide's revision the reader sees (`visibleRevisionId`) and where it
 * stands in review, or a tombstone if it's deleted.
 */
export async function changes(
  db: Database,
  organizationId: string,
  reader: Reader,
  since: bigint,
  limit = PAGE_SIZE,
): Promise<ChangesResponse> {
  const rowsFound = await db
    .select()
    .from(guide)
    .where(and(eq(guide.organizationId, organizationId), gt(guide.seq, since), visibleTo(reader.userId)))
    .orderBy(asc(guide.seq))
    .limit(limit + 1);
  const page = rowsFound.slice(0, limit);
  const live = page.filter((g) => !g.deletedAt && visibleRevisionId(g, reader.role));
  const heads = new Map((await revisions(db, live.map((g) => visibleRevisionId(g, reader.role)!))).map((r) => [r.id, r]));
  const review = await reviewStates(db, organizationId, live, heads);

  const list: Change[] = page.flatMap((g): Change[] => {
    const seq = g.seq.toString();
    if (g.deletedAt) return [{ type: "deleted", guideId: g.id, seq, ...trashDates(g.deletedAt) }];
    const id = visibleRevisionId(g, reader.role);
    const head = id ? heads.get(id) : undefined;
    return head ? [{ type: "revision", guideId: g.id, seq, revision: head, review: review.get(g.id) }] : [];
  });
  return { changes: list, cursor: (page.at(-1)?.seq ?? since).toString(), hasMore: rowsFound.length > limit };
}

/**
 * The guides the reader can see in the workspace, newest change first,
 * each with the revision they see as `head`, and where it stands in review.
 */
export async function listGuides(db: Database, organizationId: string, reader: Reader, { trashed = false } = {}) {
  const found = await db
    .select()
    .from(guide)
    .where(
      and(
        eq(guide.organizationId, organizationId),
        visibleTo(reader.userId),
        isNotNull(readsDrafts(reader.role) ? guide.headRevisionId : guide.publishedRevisionId),
        trashed ? isNotNull(guide.deletedAt) : isNull(guide.deletedAt),
      ),
    )
    .orderBy(sql`${guide.seq} desc`);
  const heads = new Map((await revisions(db, found.map((g) => visibleRevisionId(g, reader.role)!))).map((r) => [r.id, r]));
  const review = await reviewStates(db, organizationId, found, heads);
  return found.map((g) => ({ guide: g, head: heads.get(visibleRevisionId(g, reader.role)!)!, review: review.get(g.id)! }));
}

/** One guide's revision the reader sees (as `head`), if they can see it. */
export async function getGuide(db: Database, organizationId: string, reader: Reader, guideId: string) {
  const [g] = await db
    .select()
    .from(guide)
    .where(and(eq(guide.organizationId, organizationId), eq(guide.id, guideId), visibleTo(reader.userId)));
  const id = g && visibleRevisionId(g, reader.role);
  if (!g?.headRevisionId || !id) return null;
  const [head] = await revisions(db, [id]);
  return { guide: g, head: head! };
}

/** One revision of a guide, if the user can see the guide. */
export async function getRevision(db: Database, organizationId: string, userId: string, guideId: string, revisionId: string) {
  const [found] = await db
    .select({ id: revision.id })
    .from(revision)
    .innerJoin(guide, and(eq(guide.organizationId, revision.organizationId), eq(guide.id, revision.guideId)))
    .where(and(eq(revision.organizationId, organizationId), eq(revision.guideId, guideId), eq(revision.id, revisionId), visibleTo(userId)));
  if (!found) return null;
  const [r] = await revisions(db, [found.id]);
  return r ?? null;
}

/** Revisions by id, with their asset maps. */
export async function revisions(db: Database, ids: string[]): Promise<Revision[]> {
  if (!ids.length) return [];
  const [found, assets] = await Promise.all([
    db.select().from(revision).where(inArray(revision.id, ids)),
    db.select().from(revisionAsset).where(inArray(revisionAsset.revisionId, ids)),
  ]);
  const maps = new Map<string, AssetMap>();
  for (const a of assets) maps.set(a.revisionId, { ...maps.get(a.revisionId), [a.path]: a.sha256 });
  return found.map((r) => ({
    id: r.id,
    guideId: r.guideId,
    parentId: r.parentId,
    guide: r.guideJson as Guide,
    assets: maps.get(r.id) ?? {},
    authorId: r.authorId,
    source: r.source as RevisionSource,
    createdAt: r.createdAt.toISOString(),
  }));
}

function trashDates(deletedAt: Date) {
  return { deletedAt: deletedAt.toISOString(), purgeAt: new Date(deletedAt.getTime() + TRASH_DAYS * DAY).toISOString() };
}

/** Wakes the workspace's change listeners when the transaction commits. */
export async function notify(tx: Database, organizationId: string, seq: bigint) {
  await tx.execute(sql`select pg_notify(${SYNC_CHANNEL}, ${`${organizationId} ${seq}`})`);
}

export const SYNC_CHANNEL = "guiderecorder_sync";

export async function nextSeq(tx: Database): Promise<bigint> {
  const result: unknown = await tx.execute(sql`select nextval('sync_seq')::text as seq`);
  // postgres-js returns the rows; PGlite returns `{ rows }`.
  const [row] = (Array.isArray(result) ? result : (result as { rows: unknown[] }).rows) as { seq: string }[];
  return BigInt(row!.seq);
}
