import { and, desc, eq, isNotNull, isNull, sql } from "drizzle-orm";
import type { GuideStatus } from "@guiderecorder/guide-schema";
import type { Database } from "@/db";
import { guide, revision, user } from "@/db/schema";
import { GUIDE_STATUSES } from "./library-search";
import { roles } from "./permissions";
import { prefixQuery } from "./search";
import { readsDrafts, visibleTo, type Actor, type Reader, type SyncError } from "./sync";

// The cloud library: the workspace's synced guides as the signed-in member
// may see them, with folders, tags, status and review dates from the
// guide.json of the revision they see (the head, or for viewers the
// published one), and full-text search (see search.ts).

/** A review due within this many days shows as due soon. */
export const DUE_SOON_DAYS = 30;
export const MAX_FOLDER_LENGTH = 100;

export interface LibraryFilter {
  /** Words to search for; results are then ordered by best match. */
  query?: string;
  folder?: string;
  tag?: string;
  status?: GuideStatus;
  /** Only guides whose review is overdue or due soon. */
  reviewDue?: boolean;
}

export type ReviewState = "overdue" | "soon" | null;

export interface LibraryGuide {
  id: string;
  title: string;
  summary: string;
  tags: string[];
  folder: string | null;
  /** guide.json's `owner` (who keeps it current), else the member who synced it first. */
  owner: string | null;
  status: GuideStatus | null;
  /** `YYYY-MM-DD`, or null without a review schedule. */
  reviewDue: string | null;
  review: ReviewState;
  steps: number;
  /** When the head revision was made. */
  updatedAt: string;
  source: string;
  /** Only its owner sees it. */
  private: boolean;
}

export interface Facet {
  name: string;
  count: number;
}

const DAY = 24 * 60 * 60 * 1000;

/** `date` (`YYYY-MM-DD`) plus `months`, clamped to the month's last day like chrono's `checked_add_months`. */
export function addMonths(date: string, months: number): string {
  const [y, m, d] = date.split("-").map(Number) as [number, number, number];
  const index = y * 12 + (m - 1) + months;
  const [year, month] = [Math.floor(index / 12), index % 12];
  const last = new Date(Date.UTC(year, month + 1, 0)).getUTCDate();
  return new Date(Date.UTC(year, month, Math.min(d, last))).toISOString().slice(0, 10);
}

/**
 * When the guide's next review is due: `lastReviewed` (or the day it was
 * made) plus `reviewIntervalMonths`; null without a schedule. As core's
 * `ReviewInfo::review_due`.
 */
export function reviewDue(g: { lastReviewed?: string | null; reviewIntervalMonths?: number | null; createdAt: string }): string | null {
  const months = g.reviewIntervalMonths;
  if (!months || months <= 0 || !Number.isInteger(months)) return null;
  const from = g.lastReviewed && /^\d{4}-\d{2}-\d{2}$/.test(g.lastReviewed) ? g.lastReviewed : g.createdAt.slice(0, 10);
  return /^\d{4}-\d{2}-\d{2}$/.test(from) ? addMonths(from, months) : null;
}

/** Overdue past the due day (due today isn't yet), soon within DUE_SOON_DAYS. */
export function reviewState(due: string | null, today: Date): ReviewState {
  if (!due) return null;
  const day = today.toISOString().slice(0, 10);
  if (day > due) return "overdue";
  return Date.parse(due) - Date.parse(day) <= DUE_SOON_DAYS * DAY ? "soon" : null;
}

const json = revision.guideJson;

/** The guides the user can see in the workspace, filtered, with folder and tag counts over all of them. */
export async function library(
  db: Database,
  organizationId: string,
  reader: Reader,
  filter: LibraryFilter = {},
  today = new Date(),
): Promise<{ guides: LibraryGuide[]; folders: Facet[]; tags: Facet[] }> {
  const visible = and(eq(guide.organizationId, organizationId), visibleTo(reader.userId), isNotNull(guide.headRevisionId), isNull(guide.deletedAt));
  const rows = await db
    .select({
      id: guide.id,
      folder: guide.folder,
      visibility: guide.visibility,
      ownerName: user.name,
      updatedAt: revision.createdAt,
      source: revision.source,
      title: sql<string | null>`${json}->>'title'`,
      summary: sql<string | null>`${json}->>'summary'`,
      tags: sql<unknown>`${json}->'tags'`,
      owner: sql<string | null>`${json}->>'owner'`,
      status: sql<string | null>`${json}->>'status'`,
      lastReviewed: sql<string | null>`${json}->>'lastReviewed'`,
      reviewIntervalMonths: sql<unknown>`${json}->'reviewIntervalMonths'`,
      createdAt: sql<string | null>`${json}->>'createdAt'`,
      steps: sql<number>`jsonb_array_length(jsonb_path_query_array(${json}, '$.items[*] ? (@.type == "step")'))`,
    })
    .from(guide)
    .innerJoin(revision, eq(revision.id, readsDrafts(reader.role) ? guide.headRevisionId : guide.publishedRevisionId))
    .leftJoin(user, eq(user.id, guide.ownerId))
    .where(visible)
    .orderBy(desc(guide.seq));

  const all: LibraryGuide[] = rows.map((r) => {
    const due = reviewDue({
      lastReviewed: r.lastReviewed,
      reviewIntervalMonths: typeof r.reviewIntervalMonths === "number" ? r.reviewIntervalMonths : null,
      createdAt: r.createdAt ?? "",
    });
    return {
      id: r.id,
      title: r.title || "Untitled guide",
      summary: r.summary ?? "",
      tags: Array.isArray(r.tags) ? r.tags.filter((t): t is string => typeof t === "string" && t.trim() !== "") : [],
      folder: r.folder,
      owner: r.owner?.trim() || r.ownerName || null,
      status: GUIDE_STATUSES.includes(r.status as GuideStatus) ? (r.status as GuideStatus) : null,
      reviewDue: due,
      review: reviewState(due, today),
      steps: Number(r.steps),
      updatedAt: r.updatedAt.toISOString(),
      source: r.source,
      private: r.visibility !== "workspace",
    };
  });

  let guides = all.filter(
    (g) =>
      (filter.folder === undefined || g.folder === (filter.folder || null)) &&
      (!filter.tag || g.tags.some((t) => t.toLowerCase() === filter.tag!.toLowerCase())) &&
      (!filter.status || g.status === filter.status) &&
      (!filter.reviewDue || g.review !== null),
  );

  const tsquery = filter.query ? prefixQuery(filter.query) : null;
  if (filter.query && !tsquery) guides = [];
  if (tsquery) {
    const query = sql`to_tsquery('simple', ${tsquery})`;
    const ranked = await db
      .select({ id: guide.id, rank: sql<number>`ts_rank(${guide.search}, ${query})` })
      .from(guide)
      .where(and(visible, sql`${guide.search} @@ ${query}`));
    const ranks = new Map(ranked.map((r) => [r.id, Number(r.rank)]));
    guides = guides.filter((g) => ranks.has(g.id)).sort((x, y) => ranks.get(y.id)! - ranks.get(x.id)!);
  }

  return { guides, folders: facets(all.map((g) => g.folder).filter((f) => f !== null)), tags: facets(all.flatMap((g) => g.tags)) };
}

function facets(names: string[]): Facet[] {
  const counts = new Map<string, number>();
  for (const name of names) counts.set(name, (counts.get(name) ?? 0) + 1);
  return [...counts].map(([name, count]) => ({ name, count })).sort((a, b) => a.name.localeCompare(b.name));
}

/** Files a guide in a folder (trimmed; empty or null: unfiled). Needs `guide: update`. */
export async function setFolder(
  db: Database,
  organizationId: string,
  actor: Actor,
  guideId: string,
  folder: string | null,
): Promise<{ status: 200; folder: string | null } | SyncError> {
  if (!roles[actor.role].authorize({ guide: ["update"] }).success) return { status: 403, error: "Your role doesn't allow this." };
  const name = folder?.trim().replace(/\s+/g, " ") || null;
  if (name && name.length > MAX_FOLDER_LENGTH) return { status: 400, error: `Folder names are at most ${MAX_FOLDER_LENGTH} characters.` };
  const updated = await db
    .update(guide)
    .set({ folder: name })
    .where(
      and(
        eq(guide.organizationId, organizationId),
        eq(guide.id, guideId),
        visibleTo(actor.userId),
        isNotNull(guide.headRevisionId),
        isNull(guide.deletedAt),
      ),
    )
    .returning({ id: guide.id });
  return updated.length ? { status: 200, folder: name } : { status: 404, error: "Guide not found." };
}
