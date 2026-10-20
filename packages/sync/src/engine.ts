// The sync loop of the desktop and mobile apps: pulls the workspace's
// changes, pushes local edits, and merges when both sides changed a guide.
// Each app supplies a `SyncBackend` that reads and writes its library and
// talks HTTP (the desktop app's src-tauri/src/sync.rs, the mobile app's
// src/lib/device.ts); every decision is made here. Local-first: nothing
// syncs unless the user turns it on for the library or a guide.

import {
  equal,
  mergeGuides,
  type AssetMap,
  type ChangesResponse,
  type Conflict,
  type PushConflict,
  type PushResponse,
  type ReviewState,
  type Revision,
  type Side,
} from "./index.ts";
import type { Guide } from "@guiderecorder/guide-schema";

export interface ApiResponse<T = unknown> {
  status: number;
  body: T;
}

export interface LocalGuide {
  guide: Guide;
  path: string;
  readOnly: boolean;
}

export interface PreparedGuide {
  guide: Guide;
  images: { path: string; sha256: string; size: number; contentType: string }[];
}

export interface Upload {
  sha256: string;
  url: string;
  method: "PUT";
  headers: Record<string, string>;
}

/** What the loop needs from the app (see `tauriSyncApi`). */
export interface SyncBackend {
  request<T>(method: "GET" | "POST" | "DELETE", path: string, body?: unknown): Promise<ApiResponse<T>>;
  readGuide(id: string): Promise<LocalGuide | null>;
  /** The guide and the upload copies of its images (blurs burned in). */
  prepareGuide(id: string): Promise<PreparedGuide>;
  upload(uploads: Upload[]): Promise<void>;
  /** Writes a pulled guide into the library; rejects with kind `conflict` if it's open in the editor. */
  applyGuide(workspaceId: string, guide: Guide, assets: AssetMap): Promise<string>;
  /** Copies a guide into a new one; returns its id. */
  duplicateGuide(id: string, path: string, title: string): Promise<string>;
  /** Every guide in the library, for syncing the whole library. */
  listGuideIds(): Promise<string[]>;
}

export type GuideStatus =
  /** Same as the last synced revision. */
  | "synced"
  /** Changed here, or waiting to be written here (open in the editor). */
  | "pending"
  /** Changed on both sides in the same place: the user picks. */
  | "conflict"
  /** Deleted on the web; sync stopped. */
  | "deleted"
  | "error";

export interface GuideSync {
  /** Synced on its own (when the whole library isn't). */
  enabled: boolean;
  /** The server revision `base` is; null before the first sync. */
  revisionId: string | null;
  /** The guide as last synced: the common ancestor for a merge. */
  base: Guide | null;
  status: GuideStatus;
  error?: string;
  /** The server's version: in conflict, or still to be written here. */
  head?: Revision;
  conflicts?: Conflict[];
  /** Where it stands in review on the web (workspaces with approvals). */
  review?: ReviewState;
}

export interface SyncState {
  version: 1;
  workspaceId: string | null;
  /** Sync every guide in the library, and pull guides made on the web. */
  library: boolean;
  /** The change feed cursor for `workspaceId`. */
  cursor: string;
  guides: Record<string, GuideSync>;
  /** Whose choices these are (the app's key for the server and user); another account starts over. */
  owner?: string;
}

export const emptyState = (): SyncState => ({ version: 1, workspaceId: null, library: false, cursor: "0", guides: {} });

/** A stored state, or a fresh one if it's missing or from another version. */
export function loadedState(value: unknown): SyncState {
  const state = value as SyncState | null;
  return state && state.version === 1 && typeof state.guides === "object" ? state : emptyState();
}

export function isTracked(state: SyncState, id: string): boolean {
  const entry = state.guides[id];
  if (entry?.status === "deleted") return false;
  return state.library || Boolean(entry?.enabled);
}

/** The guide's content, without when it was saved. */
function content(guide: Guide): Omit<Guide, "updatedAt"> {
  const { updatedAt: _, ...rest } = guide;
  return rest;
}

/** Whether the local guide differs from what was last synced. */
export function changedLocally(local: Guide, entry: GuideSync | undefined): boolean {
  return !entry?.base || !equal(content(local), content(entry.base));
}

function errorKind(error: unknown): string | undefined {
  return error && typeof error === "object" && "kind" in error ? String((error as { kind: unknown }).kind) : undefined;
}

function message(error: unknown): string {
  if (error && typeof error === "object" && "message" in error) return String((error as { message: unknown }).message);
  return String(error);
}

/** Why the server refused, from its JSON error body. */
function refusal(response: ApiResponse): string {
  const body = response.body as { error?: string } | null;
  return body?.error ?? `The server answered ${response.status}.`;
}

/** Progress the loop reports, e.g. to persist state between steps. */
export type OnChange = (state: SyncState) => void | Promise<void>;

/** Errors that stop the whole run (signed out, no access to the workspace). */
export class SyncStopped extends Error {
  constructor(
    message: string,
    readonly status: number,
  ) {
    super(message);
  }
}

/**
 * One sync run in the chosen workspace: pull every change since the cursor,
 * then push every tracked guide changed here. Returns the new state; a
 * guide's failure is recorded on it and doesn't stop the others.
 */
export async function syncOnce(backend: SyncBackend, initial: SyncState, onChange: OnChange = () => {}): Promise<SyncState> {
  const state = structuredClone(initial);
  const workspaceId = state.workspaceId;
  if (!workspaceId) return state;
  const base = `/api/workspaces/${workspaceId}/sync`;
  const ctx: Ctx = { backend, state, workspaceId, base };

  // Pull.
  for (;;) {
    const response = await backend.request<ChangesResponse>("GET", `${base}/changes?since=${state.cursor}`);
    if (response.status !== 200) throw new SyncStopped(refusal(response), response.status);
    for (const change of response.body.changes) {
      if (change.type === "deleted") {
        const entry = state.guides[change.guideId];
        if (entry && isTracked(state, change.guideId)) {
          state.guides[change.guideId] = { ...entry, enabled: false, status: "deleted", head: undefined, conflicts: undefined };
        }
      } else {
        await guarded(ctx, change.guideId, () => pull(ctx, change.revision, change.review));
      }
    }
    state.cursor = response.body.cursor;
    await onChange(state);
    if (!response.body.hasMore) break;
  }

  // Retry pulls that couldn't be written last time.
  for (const [id, entry] of Object.entries(state.guides)) {
    if (entry.head && entry.status !== "conflict" && isTracked(state, id)) await guarded(ctx, id, () => pull(ctx, entry.head!));
  }

  // Push.
  const ids = new Set(Object.keys(state.guides).filter((id) => isTracked(state, id)));
  if (state.library) for (const id of await backend.listGuideIds()) if (state.guides[id]?.status !== "deleted") ids.add(id);
  for (const id of ids) {
    const entry = state.guides[id];
    // Conflicts wait for the user; a pull still to be written comes first.
    if (entry?.status === "conflict" || entry?.head) continue;
    await guarded(ctx, id, async () => {
      const local = await backend.readGuide(id);
      if (!local || local.readOnly) return;
      if (changedLocally(local.guide, entry)) await push(ctx, id);
      else if (entry && entry.status !== "synced") state.guides[id] = { ...entry, status: "synced", error: undefined };
    });
    await onChange(state);
  }
  return state;
}

interface Ctx {
  backend: SyncBackend;
  state: SyncState;
  workspaceId: string;
  base: string;
}

/** Runs one guide's step, recording a failure on the guide. */
async function guarded(ctx: Ctx, id: string, step: () => Promise<void>) {
  try {
    await step();
  } catch (error) {
    if (error instanceof SyncStopped) throw error;
    const entry = ctx.state.guides[id] ?? newEntry();
    const pending = errorKind(error) === "conflict";
    ctx.state.guides[id] = {
      ...entry,
      status: pending ? "pending" : "error",
      error: pending ? "Waiting for the guide to close in the editor." : message(error),
    };
  }
}

const newEntry = (): GuideSync => ({ enabled: false, revisionId: null, base: null, status: "pending" });

/** Takes a revision from the feed: write it here, merge it in, or flag a conflict. */
async function pull(ctx: Ctx, revision: Revision, review?: ReviewState) {
  const { backend, state } = ctx;
  const id = revision.guideId;
  if (!isTracked(state, id)) return;
  // Review steps move a guide in the feed without a new revision.
  if (review) state.guides[id] = { ...(state.guides[id] ?? newEntry()), review };
  const entry = state.guides[id];
  // Already here, unless it's a revision still to be written (`head`).
  if (entry?.revisionId === revision.id && entry.head?.id !== revision.id) return;

  // Kept until it's written here, so a pull that fails (the guide is open
  // in the editor) is retried next run; the cursor has moved past it.
  state.guides[id] = { ...(entry ?? newEntry()), head: revision };
  const local = await backend.readGuide(id);
  if (local?.readOnly) return;
  if (!local || !changedLocally(local.guide, entry)) {
    await accept(ctx, revision, revision.guide, revision.assets);
    return;
  }
  await mergeIn(ctx, id, local.guide, revision);
}

/**
 * Merges the server's `head` into the local guide. Without conflicts, the
 * result is written here, and pushed by the caller or the next run.
 */
async function mergeIn(ctx: Ctx, id: string, local: Guide, head: Revision): Promise<boolean> {
  const entry = ctx.state.guides[id] ?? newEntry();
  const merged = mergeGuides(entry.base && entry.base, local, head.guide);
  if (merged.conflicts.length) {
    ctx.state.guides[id] = { ...entry, status: "conflict", head, conflicts: merged.conflicts, error: undefined };
    return false;
  }
  await writeMerged(ctx, id, head, merged.guide);
  return true;
}

/** Writes a merge of the local guide and `head` here; the next push sends it. */
async function writeMerged(ctx: Ctx, id: string, head: Revision, merged: Guide) {
  const entry = ctx.state.guides[id] ?? newEntry();
  const local = await ctx.backend.prepareGuide(id);
  // Images only the local guide has are on disk already.
  const assets = { ...Object.fromEntries(local.images.map((i) => [i.path, i.sha256])), ...head.assets };
  if (!equal(content(merged), content(local.guide))) await ctx.backend.applyGuide(ctx.workspaceId, merged, assets);
  ctx.state.guides[id] = {
    ...entry,
    revisionId: head.id,
    base: head.guide,
    status: "pending",
    error: undefined,
    head: undefined,
    conflicts: undefined,
  };
}

/** Writes the server's revision here as is. */
async function accept(ctx: Ctx, revision: Revision, guide: Guide, assets: AssetMap) {
  const id = revision.guideId;
  const entry = ctx.state.guides[id] ?? newEntry();
  await ctx.backend.applyGuide(ctx.workspaceId, guide, assets);
  // The base is the guide as saved here, so the next run sees no change.
  const saved = await ctx.backend.readGuide(id);
  ctx.state.guides[id] = {
    ...entry,
    revisionId: revision.id,
    base: saved?.guide ?? guide,
    status: "synced",
    error: undefined,
    head: undefined,
    conflicts: undefined,
  };
}

const MAX_ATTEMPTS = 3;

/** Uploads the guide's images and pushes it, merging if the server moved on. */
async function push(ctx: Ctx, id: string) {
  const { backend, base } = ctx;
  for (let attempt = 0; attempt < MAX_ATTEMPTS; attempt++) {
    const entry = ctx.state.guides[id] ?? newEntry();
    const prepared = await backend.prepareGuide(id);
    const assets: AssetMap = Object.fromEntries(prepared.images.map((i) => [i.path, i.sha256]));
    await uploadImages(ctx, prepared);

    const response = await backend.request<PushResponse | PushConflict | { error: string }>("POST", `${base}/push`, {
      guideId: id,
      baseRevision: entry.revisionId,
      guide: prepared.guide,
      assets,
    });
    if (response.status === 200) {
      const { revision, review } = response.body as PushResponse;
      ctx.state.guides[id] = { ...entry, revisionId: revision.id, base: prepared.guide, status: "synced", error: undefined, review };
      // The server owns a gated guide's status, version and review date, so
      // a push can't publish it: write the server's version back here (kept
      // as `head` until written, if the guide is open in the editor).
      if (!equal(content(revision.guide), content(prepared.guide))) {
        ctx.state.guides[id] = { ...ctx.state.guides[id]!, head: revision };
        await accept(ctx, revision, revision.guide, revision.assets);
      }
      return;
    }
    if (response.status === 409) {
      const { head } = response.body as PushConflict;
      if (!(await mergeIn(ctx, id, prepared.guide, head))) return;
      continue;
    }
    if (response.status === 410) {
      ctx.state.guides[id] = { ...entry, enabled: false, status: "deleted", error: undefined };
      return;
    }
    if (response.status === 401 || response.status === 402) throw new SyncStopped(refusal(response), response.status);
    ctx.state.guides[id] = { ...entry, status: "error", error: refusal(response) };
    return;
  }
}

async function uploadImages(ctx: Ctx, prepared: PreparedGuide) {
  if (!prepared.images.length) return;
  const assetsBase = `/api/workspaces/${ctx.workspaceId}/assets`;
  const unique = [...new Map(prepared.images.map((i) => [i.sha256, i])).values()];
  const plan = await ctx.backend.request<{ present: string[]; uploads: Upload[] } | { error: string }>("POST", `${assetsBase}/uploads`, {
    assets: unique.map(({ sha256, size, contentType }) => ({ sha256, size, contentType })),
  });
  if (plan.status !== 200) {
    if (plan.status === 401 || plan.status === 402) throw new SyncStopped(refusal(plan), plan.status);
    throw new Error(refusal(plan));
  }
  const { uploads } = plan.body as { uploads: Upload[] };
  if (!uploads.length) return;
  await ctx.backend.upload(uploads);
  const commit = await ctx.backend.request<{ missing: string[] }>("POST", `${assetsBase}/commit`, { sha256: uploads.map((u) => u.sha256) });
  if (commit.status !== 200 || commit.body.missing?.length) throw new Error("Some images didn't upload. They'll be retried.");
}

export type Resolution = "mine" | "theirs" | "both";

/**
 * Settles a conflict: keep this device's version of each conflicting
 * field, keep the web's, or keep both (this version as a copy, the web's
 * in place). Then pushes.
 */
export async function resolveConflict(backend: SyncBackend, initial: SyncState, id: string, resolution: Resolution): Promise<SyncState> {
  const state = structuredClone(initial);
  const entry = state.guides[id];
  const head = entry?.head;
  if (!entry || !head || !state.workspaceId) return state;
  const ctx: Ctx = { backend, state, workspaceId: state.workspaceId, base: `/api/workspaces/${state.workspaceId}/sync` };

  await guarded(ctx, id, async () => {
    const local = await backend.readGuide(id);
    if (!local) return;
    if (resolution === "both") {
      await backend.duplicateGuide(id, local.path, `${local.guide.title} (conflicted copy)`);
      await accept(ctx, head, head.guide, head.assets);
      return;
    }
    const prefer: Side = resolution === "mine" ? "ours" : "theirs";
    const merged = mergeGuides(entry.base && entry.base, local.guide, head.guide, prefer);
    await writeMerged(ctx, id, head, merged.guide);
    await push(ctx, id);
  });
  return state;
}

/**
 * Submits a synced guide for review on the web (approval-gated guides in
 * workspaces with approvals). The guide must be synced as it is here: what
 * reviewers see is the server's revision. Returns the new state, and why
 * the server refused, if it did.
 */
export async function submitForReview(
  backend: SyncBackend,
  initial: SyncState,
  id: string,
  note = "",
): Promise<{ state: SyncState; error: string | null }> {
  const state = structuredClone(initial);
  const entry = state.guides[id];
  if (!state.workspaceId || !entry?.revisionId) return { state, error: "Sync the guide first." };
  const local = await backend.readGuide(id);
  if (!local || entry.status !== "synced" || changedLocally(local.guide, entry)) {
    return { state, error: "Wait for the guide to finish syncing, then submit it." };
  }
  const response = await backend.request<{ review: ReviewState } | { error: string }>(
    "POST",
    `/api/workspaces/${state.workspaceId}/sync/guides/${id}/review`,
    { revisionId: entry.revisionId, note },
  );
  if (response.status !== 200) return { state, error: refusal(response) };
  state.guides[id] = { ...entry, review: (response.body as { review: ReviewState }).review };
  return { state, error: null };
}

/** Whether the guide can be submitted for review: gated on the web, not published as is, nothing open. */
export function canSubmitForReview(entry: GuideSync | undefined): boolean {
  const review = entry?.review;
  return Boolean(entry?.status === "synced" && review?.gated && !review.changeRequestId && review.status !== "published");
}

/** Turns sync on or off for one guide. */
export function setGuideEnabled(initial: SyncState, id: string, enabled: boolean): SyncState {
  const state = structuredClone(initial);
  const entry = state.guides[id] ?? newEntry();
  // Turning a deleted guide back on syncs it as new.
  state.guides[id] = entry.status === "deleted" && enabled ? { ...newEntry(), enabled } : { ...entry, enabled };
  return state;
}

/** Switches workspace: sync starts over there. */
export function setWorkspace(initial: SyncState, workspaceId: string | null): SyncState {
  if (initial.workspaceId === workspaceId) return initial;
  const guides = Object.fromEntries(
    Object.entries(initial.guides).map(([id, g]) => [id, { ...newEntry(), enabled: g.enabled }]),
  );
  return { ...initial, workspaceId, cursor: "0", guides };
}
