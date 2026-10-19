// The sync API between the desktop app and the web app, shared by both.
//
// A synced guide is a chain of immutable revisions. Each revision holds the
// whole guide.json and the images it names: `assets` maps every image path
// in the guide (e.g. `steps/<id>.webp`) to the SHA-256 of a committed asset.

import { z } from "zod";
import type { Guide } from "@guiderecorder/guide-schema";

export const sha256 = z.string().regex(/^[0-9a-f]{64}$/, "lowercase hex SHA-256");
export const ulid = z.string().regex(/^[0-7][0-9A-HJKMNP-TV-Z]{25}$/, "ULID");

/** Image path → SHA-256. */
export const assetMap = z.record(z.string().min(1).max(512), sha256);
export type AssetMap = z.infer<typeof assetMap>;

export const revisionSource = z.enum(["app", "web"]);
export type RevisionSource = z.infer<typeof revisionSource>;

/** `POST /api/workspaces/:id/sync/push` */
export const pushRequest = z.object({
  guideId: ulid,
  /** The revision the change was made on; null for a guide new to the server. */
  baseRevision: z.string().nullable(),
  /** Validated against the guide schema by the server. */
  guide: z.record(z.string(), z.unknown()),
  assets: assetMap,
});
export type PushRequest = Omit<z.infer<typeof pushRequest>, "guide"> & { guide: Guide };

export interface Revision {
  id: string;
  guideId: string;
  parentId: string | null;
  guide: Guide;
  assets: AssetMap;
  authorId: string | null;
  source: RevisionSource;
  createdAt: string;
}

/** 200 from push. */
export interface PushResponse {
  revision: Revision;
}

/** 409 from push: the change wasn't made on the head. Merge with `head` and push again. */
export interface PushConflict {
  error: string;
  head: Revision;
}

/** One entry of the change feed: the guide's current state. */
export type Change =
  | { type: "revision"; guideId: string; seq: string; revision: Revision }
  /** Moved to the trash; `purgeAt` is when it's gone for good. */
  | { type: "deleted"; guideId: string; seq: string; deletedAt: string; purgeAt: string };

/** `GET /api/workspaces/:id/sync/changes?since=<cursor>` */
export interface ChangesResponse {
  changes: Change[];
  /** Pass as `since` next time. */
  cursor: string;
  /** More changes are waiting; ask again right away. */
  hasMore: boolean;
}

/**
 * Every bundle-relative image path the guide names, deduplicated, in item
 * order. These are the paths a revision's `assets` must cover.
 */
export function imagePaths(guide: Guide): string[] {
  const paths = new Set<string>();
  for (const item of (guide.items ?? []) as { type: string; image?: Record<string, unknown> }[]) {
    if (item.type !== "step" || !item.image) continue;
    for (const key of ["full", "crop", "after"]) {
      const path = item.image[key];
      if (typeof path === "string" && path) paths.add(path);
    }
  }
  return [...paths];
}
