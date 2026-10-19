import { and, eq, inArray, sum } from "drizzle-orm";
import { z } from "zod";
import type { Database } from "@/db";
import { asset, guide, revision, revisionAsset } from "@/db/schema";
import type { ObjectStore, PresignedRequest } from "./storage";
import { visibleTo } from "./sync";

// Content-addressed image storage per workspace. Clients ask which hashes
// are missing, PUT those to presigned URLs (S3 checks each body's SHA-256),
// then commit them. Uploads never overwrite: a key's bytes are its hash.

export const MAX_ASSET_BYTES = 25 * 1024 * 1024;
export const MAX_BATCH = 500;
const UPLOAD_TTL = 15 * 60;
export const DOWNLOAD_TTL = 5 * 60;

const sha256 = z.string().regex(/^[0-9a-f]{64}$/, "lowercase hex SHA-256");

export const uploadRequest = z.object({
  assets: z
    .array(
      z.object({
        sha256,
        size: z.number().int().positive().max(MAX_ASSET_BYTES),
        contentType: z.enum(["image/webp", "image/png", "image/jpeg"]),
      }),
    )
    .min(1)
    .max(MAX_BATCH),
});

export const commitRequest = z.object({ sha256: z.array(sha256).min(1).max(MAX_BATCH) });

export function assetKey(organizationId: string, hash: string) {
  return `orgs/${organizationId}/assets/${hash}`;
}

export class QuotaExceededError extends Error {
  constructor(
    readonly used: number,
    readonly requested: number,
    readonly limit: number,
  ) {
    super(`Storage limit reached: ${used + requested} of ${limit} bytes`);
  }
}

export async function storageUsed(db: Database, organizationId: string): Promise<number> {
  const [row] = await db.select({ total: sum(asset.size) }).from(asset).where(eq(asset.organizationId, organizationId));
  return Number(row?.total ?? 0);
}

async function existing(db: Database, organizationId: string, hashes: string[]): Promise<Set<string>> {
  const rows = await db
    .select({ sha256: asset.sha256 })
    .from(asset)
    .where(and(eq(asset.organizationId, organizationId), inArray(asset.sha256, hashes)));
  return new Set(rows.map((r) => r.sha256));
}

/** Presigned uploads for the assets the workspace doesn't have yet. */
export async function planUploads(
  db: Database,
  store: ObjectStore,
  organizationId: string,
  request: z.infer<typeof uploadRequest>,
  storageLimit: number,
): Promise<{ present: string[]; uploads: (PresignedRequest & { sha256: string })[] }> {
  const unique = [...new Map(request.assets.map((a) => [a.sha256, a])).values()];
  const have = await existing(db, organizationId, unique.map((a) => a.sha256));
  const missing = unique.filter((a) => !have.has(a.sha256));

  const requested = missing.reduce((total, a) => total + a.size, 0);
  const used = await storageUsed(db, organizationId);
  if (requested > 0 && used + requested > storageLimit) {
    throw new QuotaExceededError(used, requested, storageLimit);
  }

  const uploads = await Promise.all(
    missing.map(async (a) => ({
      sha256: a.sha256,
      ...(await store.presignPut(assetKey(organizationId, a.sha256), a, UPLOAD_TTL)),
    })),
  );
  return { present: [...have], uploads };
}

/**
 * Records uploaded assets. Each must exist in the store; S3 verified its
 * hash on upload. Returns the hashes still missing from the store.
 */
export async function commitUploads(
  db: Database,
  store: ObjectStore,
  organizationId: string,
  userId: string,
  hashes: string[],
): Promise<{ committed: string[]; missing: string[] }> {
  const unique = [...new Set(hashes)];
  const have = await existing(db, organizationId, unique);
  const committed: string[] = [...have];
  const missing: string[] = [];
  for (const hash of unique.filter((h) => !have.has(h))) {
    const head = await store.head(assetKey(organizationId, hash));
    if (!head) {
      missing.push(hash);
      continue;
    }
    await db
      .insert(asset)
      .values({ organizationId, sha256: hash, size: head.size, contentType: head.contentType, uploadedBy: userId })
      .onConflictDoNothing();
    committed.push(hash);
  }
  return { committed, missing };
}

/** A short-lived download URL, or null if the workspace has no such asset. */
export async function downloadUrl(db: Database, store: ObjectStore, organizationId: string, hash: string) {
  if (!sha256.safeParse(hash).success) return null;
  const have = await existing(db, organizationId, [hash]);
  if (!have.has(hash)) return null;
  return store.presignGet(assetKey(organizationId, hash), DOWNLOAD_TTL);
}

/**
 * Whether the user may download the asset: they uploaded it, or a revision
 * of a guide they can see names it. Keeps a private guide's images to its
 * owner, as its revisions are.
 */
export async function canDownload(db: Database, organizationId: string, userId: string, hash: string): Promise<boolean> {
  if (!sha256.safeParse(hash).success) return false;
  const [uploaded] = await db
    .select({ sha256: asset.sha256 })
    .from(asset)
    .where(and(eq(asset.organizationId, organizationId), eq(asset.sha256, hash), eq(asset.uploadedBy, userId)));
  if (uploaded) return true;
  const [named] = await db
    .select({ id: revisionAsset.revisionId })
    .from(revisionAsset)
    .innerJoin(revision, eq(revision.id, revisionAsset.revisionId))
    .innerJoin(guide, and(eq(guide.organizationId, revision.organizationId), eq(guide.id, revision.guideId)))
    .where(and(eq(revisionAsset.organizationId, organizationId), eq(revisionAsset.sha256, hash), visibleTo(userId)))
    .limit(1);
  return Boolean(named);
}
