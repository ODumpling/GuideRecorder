import { and, asc, eq, gt, inArray, isNotNull, isNull, lt, or, sql } from "drizzle-orm";
import { guideSchema, type Guide } from "@guiderecorder/guide-schema";
import {
  equal,
  imagePaths,
  type AssetMap,
  type Change,
  type ChangesResponse,
  type pushRequest,
  type Revision,
  type RevisionSource,
} from "@guiderecorder/sync";
import type { z } from "zod";
import type { Database } from "@/db";
import { asset, guide, revision, revisionAsset } from "@/db/schema";
import { roles, type RoleName } from "./permissions";
import { searchVector } from "./search";

// Desktop ⇄ web sync. Each guide is a chain of immutable revisions; a push
// adds one on top of the head it was made on, or gets the head back (409) to
// merge with. Every change gives the guide a new `seq`, so clients follow the
// workspace with `changes(since)`. Writes to a workspace take an advisory
// lock, so seqs commit in order and a cursor never skips a change.

export const TRASH_DAYS = 30;
export const PAGE_SIZE = 200;
const DAY = 24 * 60 * 60 * 1000;

export interface Actor {
  userId: string;
  role: RoleName;
}

export type SyncError = { status: 400 | 403 | 404 | 422; error: string; [extra: string]: unknown };

export type PushResult =
  | { status: 200; revision: Revision }
  | { status: 409; error: string; head: Revision }
  | { status: 410; error: string; deletedAt: string; purgeAt: string }
  | SyncError;

const can = (actor: Actor, action: "read" | "create" | "update" | "delete") => roles[actor.role].authorize({ guide: [action] }).success;

/** Who may see a guide: every member, or only its owner if it's private. */
export const visibleTo = (userId: string) => or(eq(guide.visibility, "workspace"), eq(guide.ownerId, userId));

async function lockWorkspace(tx: Database, organizationId: string) {
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
  // back exactly what they pushed.
  const content = request.guide as unknown as Guide;
  if (content.id !== request.guideId) return { status: 400, error: "guideId doesn't match the guide's id." };

  const paths = imagePaths(content);
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

    if (exists) {
      const [head] = await revisions(tx, [row.headRevisionId!]);
      // Nothing new (or a retried push that already landed): the head as is.
      if (head && equal(head.guide, content) && equal(head.assets, request.assets)) return { status: 200, revision: head } as const;
      if (request.baseRevision !== row.headRevisionId) {
        return { status: 409, error: "The guide changed on the server. Merge and push again.", head: head! } as const;
      }
    }

    const id = crypto.randomUUID();
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
    await tx
      .update(guide)
      .set({ headRevisionId: id, search: searchVector(content) })
      .where(and(eq(guide.organizationId, organizationId), eq(guide.id, request.guideId)));
    await notify(tx, organizationId, next);
    const [created] = await revisions(tx, [id]);
    return { status: 200, revision: created! } as const;
  });
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
      .set({ headRevisionId: null, publishedRevisionId: null, search: null })
      .where(and(isNotNull(guide.deletedAt), lt(guide.deletedAt, cutoff), isNotNull(guide.headRevisionId)))
      .returning({ organizationId: guide.organizationId, id: guide.id });
    for (const g of expired) {
      await tx.delete(revision).where(and(eq(revision.organizationId, g.organizationId), eq(revision.guideId, g.id)));
    }
    return expired.length;
  });
}

/**
 * The workspace's guides changed after `since`, oldest change first: each
 * guide's head revision, or a tombstone if it's deleted.
 */
export async function changes(
  db: Database,
  organizationId: string,
  userId: string,
  since: bigint,
  limit = PAGE_SIZE,
): Promise<ChangesResponse> {
  const rowsFound = await db
    .select()
    .from(guide)
    .where(and(eq(guide.organizationId, organizationId), gt(guide.seq, since), visibleTo(userId)))
    .orderBy(asc(guide.seq))
    .limit(limit + 1);
  const page = rowsFound.slice(0, limit);
  const live = page.filter((g) => !g.deletedAt && g.headRevisionId);
  const heads = new Map((await revisions(db, live.map((g) => g.headRevisionId!))).map((r) => [r.id, r]));

  const list: Change[] = page.flatMap((g): Change[] => {
    const seq = g.seq.toString();
    if (g.deletedAt) return [{ type: "deleted", guideId: g.id, seq, ...trashDates(g.deletedAt) }];
    const head = g.headRevisionId ? heads.get(g.headRevisionId) : undefined;
    return head ? [{ type: "revision", guideId: g.id, seq, revision: head }] : [];
  });
  return { changes: list, cursor: (page.at(-1)?.seq ?? since).toString(), hasMore: rowsFound.length > limit };
}

/** The guides the user can see in the workspace, newest change first, with their head revisions. */
export async function listGuides(db: Database, organizationId: string, userId: string, { trashed = false } = {}) {
  const found = await db
    .select()
    .from(guide)
    .where(
      and(
        eq(guide.organizationId, organizationId),
        visibleTo(userId),
        isNotNull(guide.headRevisionId),
        trashed ? isNotNull(guide.deletedAt) : isNull(guide.deletedAt),
      ),
    )
    .orderBy(sql`${guide.seq} desc`);
  const heads = new Map((await revisions(db, found.map((g) => g.headRevisionId!))).map((r) => [r.id, r]));
  return found.map((g) => ({ guide: g, head: heads.get(g.headRevisionId!)! }));
}

/** One guide's head revision, if the user can see it. */
export async function getGuide(db: Database, organizationId: string, userId: string, guideId: string) {
  const [g] = await db
    .select()
    .from(guide)
    .where(and(eq(guide.organizationId, organizationId), eq(guide.id, guideId), visibleTo(userId)));
  if (!g?.headRevisionId) return null;
  const [head] = await revisions(db, [g.headRevisionId]);
  return { guide: g, head: head! };
}

/** Revisions by id, with their asset maps. */
async function revisions(db: Database, ids: string[]): Promise<Revision[]> {
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
async function notify(tx: Database, organizationId: string, seq: bigint) {
  await tx.execute(sql`select pg_notify(${SYNC_CHANNEL}, ${`${organizationId} ${seq}`})`);
}

export const SYNC_CHANNEL = "guiderecorder_sync";

async function nextSeq(tx: Database): Promise<bigint> {
  const result: unknown = await tx.execute(sql`select nextval('sync_seq')::text as seq`);
  // postgres-js returns the rows; PGlite returns `{ rows }`.
  const [row] = (Array.isArray(result) ? result : (result as { rows: unknown[] }).rows) as { seq: string }[];
  return BigInt(row!.seq);
}
