import { and, eq, isNotNull, isNull, lt, notExists, or, sql } from "drizzle-orm";
import type { Database } from "@/db";
import { asset, changeRequest, deviceCode, guide, rateLimit, revision, revisionAsset, session, shareLink, verification, workspacePolicy } from "@/db/schema";
import { assetKey } from "./assets";
import { pruneAudit } from "./audit";
import { securityEvent } from "./security-events";
import type { ObjectStore } from "./storage";
import { purgeTrash } from "./sync";

// Data retention: one job, run hourly (POST /api/jobs/retention from cron,
// see docs/web-deployment.md), that deletes what's past its time:
// - guides in the trash for over 30 days lose their revisions (sync.ts);
// - revisions older than the workspace's `revisionRetentionDays`, except
//   each guide's head and published revision, revisions a share link pins
//   and revisions a change request names (deleting them would take the
//   link or the approval record with them);
// - images no revision names any more, from the store and the database;
// - expired sessions, verification tokens, device codes and rate counters;
// - audit events past the workspace's audit retention (`pruneAudit`).
// Every step is idempotent, so a failed run is simply retried next time.

const DAY = 24 * 60 * 60 * 1000;
/** Uploads not yet named by a revision get this long before they count as orphans. */
export const ORPHAN_GRACE_MS = DAY;
const ASSET_BATCH = 500;

export interface RetentionReport {
  trashedGuides: number;
  revisions: number;
  assets: number;
  auditEvents: number;
  expired: { sessions: number; verifications: number; deviceCodes: number; rateLimits: number };
}

export async function runRetention(db: Database, store: ObjectStore, now = new Date()): Promise<RetentionReport> {
  const report: RetentionReport = {
    trashedGuides: await purgeTrash(db, now),
    revisions: await pruneRevisions(db, now),
    assets: 0,
    auditEvents: await pruneAudit(db, now),
    expired: await deleteExpired(db, now),
  };
  // After revisions, so images they named are orphans in this same run.
  report.assets = await deleteOrphanAssets(db, store, now);
  await securityEvent({ type: "retention.run", actorId: "system", details: { ...report } });
  return report;
}

/** Workspaces with a retention period for `column`, with their cutoffs. */
async function cutoffs(db: Database, column: "revisionRetentionDays", now: Date) {
  const rows = await db
    .select({ organizationId: workspacePolicy.organizationId, days: workspacePolicy[column] })
    .from(workspacePolicy)
    .where(isNotNull(workspacePolicy[column]));
  return rows.map((r) => ({ organizationId: r.organizationId, cutoff: new Date(now.getTime() - r.days! * DAY) }));
}

/** Deletes old revisions that are neither a guide's head, its published revision, pinned by a share link nor under review. */
export async function pruneRevisions(db: Database, now = new Date()): Promise<number> {
  let deleted = 0;
  for (const { organizationId, cutoff } of await cutoffs(db, "revisionRetentionDays", now)) {
    const kept = db
      .select({ one: sql`1` })
      .from(guide)
      .where(
        and(
          eq(guide.organizationId, revision.organizationId),
          eq(guide.id, revision.guideId),
          or(eq(guide.headRevisionId, revision.id), eq(guide.publishedRevisionId, revision.id)),
        ),
      );
    const pinned = db
      .select({ one: sql`1` })
      .from(shareLink)
      .where(and(eq(shareLink.revisionId, revision.id), isNull(shareLink.revokedAt)));
    const reviewed = db
      .select({ one: sql`1` })
      .from(changeRequest)
      .where(
        or(
          eq(changeRequest.revisionId, revision.id),
          eq(changeRequest.baseRevisionId, revision.id),
          eq(changeRequest.publishedRevisionId, revision.id),
        ),
      );
    const rows = await db
      .delete(revision)
      .where(
        and(
          eq(revision.organizationId, organizationId),
          lt(revision.createdAt, cutoff),
          notExists(kept),
          notExists(pinned),
          notExists(reviewed),
        ),
      )
      .returning({ id: revision.id });
    deleted += rows.length;
  }
  return deleted;
}

/**
 * Deletes images no revision names, older than the grace period (a client
 * commits images before pushing the revision that uses them). The object
 * goes first, so a failure leaves the row to retry with.
 */
export async function deleteOrphanAssets(db: Database, store: ObjectStore, now = new Date()): Promise<number> {
  const before = new Date(now.getTime() - ORPHAN_GRACE_MS);
  let deleted = 0;
  for (;;) {
    const used = db
      .select({ one: sql`1` })
      .from(revisionAsset)
      .where(and(eq(revisionAsset.organizationId, asset.organizationId), eq(revisionAsset.sha256, asset.sha256)));
    const orphans = await db
      .select({ organizationId: asset.organizationId, sha256: asset.sha256 })
      .from(asset)
      .where(and(lt(asset.createdAt, before), notExists(used)))
      .limit(ASSET_BATCH);
    for (const a of orphans) {
      await store.delete(assetKey(a.organizationId, a.sha256));
      await db.delete(asset).where(and(eq(asset.organizationId, a.organizationId), eq(asset.sha256, a.sha256), notExists(used)));
    }
    deleted += orphans.length;
    if (orphans.length < ASSET_BATCH) return deleted;
  }
}


/** Sessions, tokens and counters that can't be used any more. */
async function deleteExpired(db: Database, now: Date) {
  const count = async (query: Promise<unknown[]>) => (await query).length;
  return {
    sessions: await count(db.delete(session).where(lt(session.expiresAt, now)).returning({ id: session.id })),
    verifications: await count(db.delete(verification).where(lt(verification.expiresAt, now)).returning({ id: verification.id })),
    deviceCodes: await count(db.delete(deviceCode).where(lt(deviceCode.expiresAt, now)).returning({ id: deviceCode.id })),
    rateLimits: await count(db.delete(rateLimit).where(lt(rateLimit.lastRequest, now.getTime() - DAY)).returning({ id: rateLimit.id })),
  };
}
