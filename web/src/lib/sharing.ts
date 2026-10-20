import { createHmac, randomBytes, timingSafeEqual } from "node:crypto";
import { and, desc, eq, isNull, sql } from "drizzle-orm";
import { hashPassword, verifyPassword } from "better-auth/crypto";
import { z } from "zod";
import type { Guide } from "@guiderecorder/guide-schema";
import type { AssetMap, RevisionSource } from "@guiderecorder/sync";
import type { Database } from "@/db";
import { guide, revision, revisionAsset, shareLink, user, workspaceSettings } from "@/db/schema";
import { roles } from "./permissions";
import { getGuide, publishedRevisionFor, type Actor } from "./sync";
import { workspacePlan } from "./workspace";

// Share links: a guide for people outside the workspace at `/s/<token>`.
// A link shows a pinned revision or the guide's latest published one, and
// may expire or need a password. Links stop working when they're revoked,
// the guide is trashed, the workspace loses the `sharing` entitlement, or
// an admin turns links off (or requires passwords and the link has none).

const DAY = 24 * 60 * 60 * 1000;
/** How long a correct password keeps a link open in that browser. */
export const UNLOCK_SECONDS = 12 * 60 * 60;

export const SHARE_LINK_MODES = ["anyone", "password", "off"] as const;

export const sharingSettings = z.object({
  shareLinks: z.enum(SHARE_LINK_MODES),
  shareLinkMaxDays: z.number().int().min(1).max(3650).nullable(),
});
export type SharingSettings = z.infer<typeof sharingSettings>;

const DEFAULT_SETTINGS: SharingSettings = { shareLinks: "anyone", shareLinkMaxDays: null };

export const createShareRequest = z.object({
  guideId: z.string().min(1),
  /** A revision to pin; null for the guide's latest published revision. */
  revisionId: z.string().min(1).nullable().default(null),
  expiresAt: z.iso.datetime({ offset: true }).nullable().default(null),
  password: z.string().min(4).max(200).nullable().default(null),
  allowDownload: z.boolean().default(false),
});

/** A link as its guide's sharers see it. */
export interface ShareLinkInfo {
  id: string;
  token: string;
  guideId: string;
  /** Null: the latest published revision. */
  revisionId: string | null;
  expiresAt: string | null;
  hasPassword: boolean;
  allowDownload: boolean;
  views: number;
  lastViewedAt: string | null;
  createdAt: string;
  createdBy: string | null;
  /** `blocked`: the workspace's settings stop it working. */
  state: "active" | "expired" | "blocked";
}

/** The guide as a share link shows it. */
export interface SharedGuide {
  guide: Guide;
  assets: AssetMap;
  allowDownload: boolean;
  /** When and where the shown revision was made. */
  updatedAt: string;
  source: RevisionSource;
}

export type ShareError = { status: 400 | 403 | 404; error: string };

type Row = typeof shareLink.$inferSelect;

const canShare = (actor: Actor) => roles[actor.role].authorize({ guide: ["share"] }).success;

export function shareUrl(baseURL: string, token: string) {
  return new URL(`/s/${token}`, baseURL).toString();
}

export async function getSharingSettings(db: Database, organizationId: string): Promise<SharingSettings> {
  const [row] = await db.select().from(workspaceSettings).where(eq(workspaceSettings.organizationId, organizationId));
  return row ? { shareLinks: row.shareLinks as SharingSettings["shareLinks"], shareLinkMaxDays: row.shareLinkMaxDays } : DEFAULT_SETTINGS;
}

export async function setSharingSettings(db: Database, organizationId: string, settings: SharingSettings) {
  await db
    .insert(workspaceSettings)
    .values({ organizationId, ...settings })
    .onConflictDoUpdate({ target: workspaceSettings.organizationId, set: settings });
}

/** When the link stops working: its own expiry or the workspace's limit, whichever is first. */
function expiryOf(link: Row, settings: SharingSettings): Date | null {
  const cap = settings.shareLinkMaxDays ? new Date(link.createdAt.getTime() + settings.shareLinkMaxDays * DAY) : null;
  if (!link.expiresAt) return cap;
  return cap && cap < link.expiresAt ? cap : link.expiresAt;
}

function stateOf(link: Row, settings: SharingSettings, now: Date): ShareLinkInfo["state"] {
  if (settings.shareLinks === "off" || (settings.shareLinks === "password" && !link.passwordHash)) return "blocked";
  const expiry = expiryOf(link, settings);
  return expiry && expiry <= now ? "expired" : "active";
}

function info(link: Row, createdBy: string | null, settings: SharingSettings, now: Date): ShareLinkInfo {
  return {
    id: link.id,
    token: link.token,
    guideId: link.guideId,
    revisionId: link.revisionId,
    expiresAt: expiryOf(link, settings)?.toISOString() ?? null,
    hasPassword: Boolean(link.passwordHash),
    allowDownload: link.allowDownload,
    views: link.views,
    lastViewedAt: link.lastViewedAt?.toISOString() ?? null,
    createdAt: link.createdAt.toISOString(),
    createdBy,
    state: stateOf(link, settings, now),
  };
}

/** The guide, if the actor may share it and it isn't in the trash. */
async function shareableGuide(db: Database, organizationId: string, actor: Actor, guideId: string): Promise<ShareError | null> {
  if (!canShare(actor)) return { status: 403, error: "Your role doesn't allow sharing." };
  const found = await getGuide(db, organizationId, actor, guideId);
  if (!found || found.guide.deletedAt) return { status: 404, error: "Guide not found." };
  return null;
}

/** A guide's links that haven't been revoked, newest first, with the workspace's settings. */
export async function listShareLinks(db: Database, organizationId: string, actor: Actor, guideId: string, now = new Date()) {
  const refused = await shareableGuide(db, organizationId, actor, guideId);
  if (refused) return refused;
  const [settings, rows] = await Promise.all([
    getSharingSettings(db, organizationId),
    db
      .select({ link: shareLink, createdBy: user.name })
      .from(shareLink)
      .leftJoin(user, eq(user.id, shareLink.createdBy))
      .where(and(eq(shareLink.organizationId, organizationId), eq(shareLink.guideId, guideId), isNull(shareLink.revokedAt)))
      .orderBy(desc(shareLink.createdAt)),
  ]);
  return { status: 200 as const, links: rows.map((r) => info(r.link, r.createdBy, settings, now)), settings };
}

/** Makes a link, within the workspace's settings. */
export async function createShareLink(
  db: Database,
  organizationId: string,
  actor: Actor,
  request: z.infer<typeof createShareRequest>,
  now = new Date(),
): Promise<{ status: 200; link: ShareLinkInfo } | ShareError> {
  const refused = await shareableGuide(db, organizationId, actor, request.guideId);
  if (refused) return refused;
  const settings = await getSharingSettings(db, organizationId);
  if (settings.shareLinks === "off") return { status: 403, error: "Share links are turned off for this workspace." };
  if (settings.shareLinks === "password" && !request.password) {
    return { status: 400, error: "This workspace requires a password on share links." };
  }

  let expiresAt = request.expiresAt ? new Date(request.expiresAt) : null;
  if (expiresAt && expiresAt <= now) return { status: 400, error: "The expiry must be in the future." };
  if (settings.shareLinkMaxDays) {
    const limit = new Date(now.getTime() + settings.shareLinkMaxDays * DAY);
    if (expiresAt && expiresAt > limit) {
      return { status: 400, error: `Share links in this workspace last at most ${settings.shareLinkMaxDays} days.` };
    }
    expiresAt ??= limit;
  }

  if (request.revisionId) {
    const [pinned] = await db
      .select({ id: revision.id })
      .from(revision)
      .where(and(eq(revision.id, request.revisionId), eq(revision.organizationId, organizationId), eq(revision.guideId, request.guideId)));
    if (!pinned) return { status: 404, error: "Revision not found." };
  }

  const [created] = await db
    .insert(shareLink)
    .values({
      id: crypto.randomUUID(),
      token: randomBytes(18).toString("base64url"),
      organizationId,
      guideId: request.guideId,
      revisionId: request.revisionId,
      expiresAt,
      passwordHash: request.password ? await hashPassword(request.password) : null,
      allowDownload: request.allowDownload,
      createdBy: actor.userId,
      createdAt: now,
    })
    .returning();
  const [author] = await db.select({ name: user.name }).from(user).where(eq(user.id, actor.userId));
  return { status: 200, link: info(created!, author?.name ?? null, settings, now) };
}

/** Stops a link working. Its row stays, with its views. */
export async function revokeShareLink(
  db: Database,
  organizationId: string,
  actor: Actor,
  linkId: string,
  now = new Date(),
): Promise<{ status: 200 } | ShareError> {
  const [link] = await db
    .select()
    .from(shareLink)
    .where(and(eq(shareLink.id, linkId), eq(shareLink.organizationId, organizationId), isNull(shareLink.revokedAt)));
  if (!link) return { status: 404, error: "Link not found." };
  if (!canShare(actor)) return { status: 403, error: "Your role doesn't allow sharing." };
  // A trashed guide's links can still be revoked; a private one's only by its owner.
  if (!(await getGuide(db, organizationId, actor, link.guideId))) return { status: 404, error: "Link not found." };
  await db.update(shareLink).set({ revokedAt: now }).where(eq(shareLink.id, linkId));
  return { status: 200 };
}

// Public side. Nothing below needs a session; the token is the credential.

export type OpenError =
  | { status: 404; error: string }
  | { status: 410; error: string }
  | { status: 401; error: string; needsPassword: true };

const notFound = { status: 404, error: "This link doesn't exist or was turned off." } as const;

/** Reads a cookie by name. */
export type Cookies = (name: string) => string | undefined;

/** The link and the revision it shows, if the link works. */
async function resolve(db: Database, token: string, now: Date): Promise<{ link: Row; revisionId: string } | OpenError> {
  if (!/^[A-Za-z0-9_-]{16,64}$/.test(token)) return notFound;
  const [link] = await db.select().from(shareLink).where(eq(shareLink.token, token));
  if (!link || link.revokedAt) return notFound;
  const [settings, plan, [g]] = await Promise.all([
    getSharingSettings(db, link.organizationId),
    workspacePlan(db, link.organizationId),
    db
      .select()
      .from(guide)
      .where(and(eq(guide.organizationId, link.organizationId), eq(guide.id, link.guideId))),
  ]);
  if (!plan.entitlements.sharing) return notFound;
  const state = stateOf(link, settings, now);
  if (state === "blocked") return notFound;
  if (state === "expired") return { status: 410, error: "This link has expired." };
  if (!g?.headRevisionId || g.deletedAt) return notFound;
  // Unpinned links show what's published; a gated guide may have nothing published yet.
  const revisionId = link.revisionId ?? publishedRevisionFor(g);
  return revisionId ? { link, revisionId } : notFound;
}

export const unlockCookieName = (link: { id: string }) => `gr_share_${link.id}`;

/** Proof of the password, tied to the link and its password hash. */
function unlockValue(secret: string, link: Row) {
  return createHmac("sha256", secret).update(`${link.id}.${link.passwordHash}`).digest("base64url");
}

function unlocked(secret: string, link: Row, cookies: Cookies) {
  if (!link.passwordHash) return true;
  const given = Buffer.from(cookies(unlockCookieName(link)) ?? "");
  const expected = Buffer.from(unlockValue(secret, link));
  return given.length === expected.length && timingSafeEqual(given, expected);
}

const locked = { status: 401, error: "This guide is password protected.", needsPassword: true } as const;

/** The shared guide, counting a view; or why it can't be shown. */
export async function openShareLink(
  db: Database,
  secret: string,
  token: string,
  cookies: Cookies,
  now = new Date(),
): Promise<{ status: 200; shared: SharedGuide } | OpenError> {
  const found = await resolve(db, token, now);
  if ("error" in found) return found;
  const { link, revisionId } = found;
  if (!unlocked(secret, link, cookies)) return locked;

  const [[rev], assets] = await Promise.all([
    db.select().from(revision).where(eq(revision.id, revisionId)),
    db.select({ path: revisionAsset.path, sha256: revisionAsset.sha256 }).from(revisionAsset).where(eq(revisionAsset.revisionId, revisionId)),
  ]);
  if (!rev) return notFound;
  await db
    .update(shareLink)
    .set({ views: sql`${shareLink.views} + 1`, lastViewedAt: now })
    .where(eq(shareLink.id, link.id));
  return {
    status: 200,
    shared: {
      guide: publicGuide(rev.guideJson as Guide),
      assets: Object.fromEntries(assets.map((a) => [a.path, a.sha256])),
      allowDownload: link.allowDownload,
      updatedAt: rev.createdAt.toISOString(),
      source: rev.source as RevisionSource,
    },
  };
}

/** Checks a link's password; on success, the cookie that keeps it open. */
export async function unlockShareLink(
  db: Database,
  secret: string,
  token: string,
  password: string,
  now = new Date(),
): Promise<{ status: 200; cookie: { name: string; value: string } | null } | OpenError | { status: 403; error: string }> {
  const found = await resolve(db, token, now);
  if ("error" in found) return found;
  const { link } = found;
  if (!link.passwordHash) return { status: 200, cookie: null };
  if (!(await verifyPassword({ hash: link.passwordHash, password }))) return { status: 403, error: "That password isn't right." };
  return { status: 200, cookie: { name: unlockCookieName(link), value: unlockValue(secret, link) } };
}

/**
 * The workspace holding one of the shared revision's images, to serve it
 * from; only images the shown revision names.
 */
export async function shareAsset(
  db: Database,
  secret: string,
  token: string,
  sha256: string,
  cookies: Cookies,
  now = new Date(),
): Promise<{ status: 200; organizationId: string } | OpenError> {
  const found = await resolve(db, token, now);
  if ("error" in found) return found;
  if (!unlocked(secret, found.link, cookies)) return locked;
  const [named] = await db
    .select({ organizationId: revisionAsset.organizationId })
    .from(revisionAsset)
    .where(and(eq(revisionAsset.revisionId, found.revisionId), eq(revisionAsset.sha256, sha256)))
    .limit(1);
  return named ? { status: 200, organizationId: named.organizationId } : notFound;
}

/**
 * The guide without what only its workspace should see: where it was
 * recorded, AI provenance and review bookkeeping.
 */
export function publicGuide(g: Guide): Guide {
  const { recording: _r, aiState: _a, editedFields: _e, owner: _o, lastReviewed: _l, reviewIntervalMonths: _i, ...rest } = g;
  return {
    ...rest,
    items: g.items?.map((item) => {
      const { aiState: _ai, editedFields: _ef, ...kept } = item as typeof item & { aiState?: unknown; editedFields?: unknown };
      return kept as typeof item;
    }),
  };
}

/** Reads one cookie from a `Cookie` header. */
export function cookieReader(header: string | null): Cookies {
  const jar = new Map<string, string>();
  for (const part of (header ?? "").split(";")) {
    const at = part.indexOf("=");
    if (at > 0) jar.set(part.slice(0, at).trim(), part.slice(at + 1).trim());
  }
  return (name) => jar.get(name);
}
