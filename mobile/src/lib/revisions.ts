// Revisions from the sync API, as the library keeps them. Pure.

import type { Change, ReviewState, Revision } from "@guiderecorder/sync";

export interface Entry {
  revision: Revision;
  review?: ReviewState;
}

/** Applies feed changes to the cached guides. */
export function applyChanges(guides: Record<string, Entry>, changes: Change[]): Record<string, Entry> {
  const next = { ...guides };
  for (const change of changes) {
    if (change.type === "deleted") delete next[change.guideId];
    else next[change.guideId] = { revision: change.revision, review: change.review };
  }
  return next;
}

/** Newest first. */
export function sortedGuides(guides: Record<string, Entry>): Entry[] {
  return Object.values(guides).sort((a, b) => b.revision.guide.updatedAt.localeCompare(a.revision.guide.updatedAt));
}
