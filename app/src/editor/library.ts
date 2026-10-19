import type { GuideKind, GuideListItem, GuideStatus, ReviewFields } from "@/lib/types";

/** How the library sidebar orders guides (a search orders by best match instead). */
export type GuideSort = "updated" | "created" | "title";

export const GUIDE_SORTS: { value: GuideSort; label: string }[] = [
  { value: "updated", label: "Last updated" },
  { value: "created", label: "Date created" },
  { value: "title", label: "Title" },
];

const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });

function time(iso: string): number {
  const t = Date.parse(iso);
  return Number.isNaN(t) ? 0 : t;
}

/** Guides in `sort` order: newest first by date, A–Z by title. Ties go by title, then path. */
export function sortGuides(guides: GuideListItem[], sort: GuideSort): GuideListItem[] {
  const byTitle = (a: GuideListItem, b: GuideListItem) =>
    collator.compare(a.title || "Untitled", b.title || "Untitled") || (a.path < b.path ? -1 : a.path > b.path ? 1 : 0);
  const key = (g: GuideListItem) => (sort === "created" ? time(g.createdAt) : time(g.updatedAt));
  return [...guides].sort((a, b) => (sort === "title" ? byTitle(a, b) : key(b) - key(a) || byTitle(a, b)));
}

/** Every tag in `guides`, once each ignoring case (first spelling wins), A–Z. */
export function libraryTags(guides: GuideListItem[]): string[] {
  const tags = new Map<string, string>();
  for (const guide of guides) {
    for (const tag of guide.tags) {
      const key = tag.trim().toLowerCase();
      if (key && !tags.has(key)) tags.set(key, tag.trim());
    }
  }
  return [...tags.values()].sort(collator.compare);
}

/** Whether a guide has `tag`, ignoring case (as the CLI and MCP filter). */
export function hasTag(guide: GuideListItem, tag: string): boolean {
  const key = tag.trim().toLowerCase();
  return guide.tags.some((t) => t.trim().toLowerCase() === key);
}

export const GUIDE_STATUSES: { value: GuideStatus; label: string }[] = [
  { value: "draft", label: "Draft" },
  { value: "inReview", label: "In review" },
  { value: "published", label: "Published" },
  { value: "needsUpdate", label: "Needs update" },
];

/** The sidebar's status filter: a status, or guides whose review is overdue. */
export type StatusFilter = GuideStatus | "overdue";

/** Today in local time, `YYYY-MM-DD` (what "Mark reviewed" records). */
export function localToday(now: Date = new Date()): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`;
}

/** `YYYY-MM-DD` plus `months`, on the same day or the month's last day (as core does); null if `day` is not a date. */
export function addMonths(day: string, months: number): string | null {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(day);
  if (!m) return null;
  const total = Number(m[1]) * 12 + Number(m[2]) - 1 + months;
  const year = Math.floor(total / 12);
  const month = (total % 12) + 1;
  const last = new Date(Date.UTC(year, month, 0)).getUTCDate();
  const pad = (n: number, w = 2) => String(n).padStart(w, "0");
  return `${pad(year, 4)}-${pad(month)}-${pad(Math.min(Number(m[3]), last))}`;
}

/**
 * When the next review is due (core `ReviewInfo::review_due`): the last
 * review day, or the day the guide was made (UTC) if it never was, plus
 * the interval. Null without an interval.
 */
export function reviewDue(guide: ReviewFields & { createdAt: string }): string | null {
  const months = guide.reviewIntervalMonths ?? 0;
  if (!(months > 0)) return null;
  let since = guide.lastReviewed ?? null;
  if (!since) {
    const created = Date.parse(guide.createdAt);
    if (Number.isNaN(created)) return null;
    since = new Date(created).toISOString().slice(0, 10);
  }
  return addMonths(since, months);
}

/** Whether the review is past its due day on `today` (`YYYY-MM-DD`); due today is not overdue yet. */
export function isReviewOverdue(guide: ReviewFields & { createdAt: string }, today: string): boolean {
  const due = reviewDue(guide);
  return due !== null && today > due;
}

/** Whether a guide passes the status filter on `today`. */
export function matchesStatus(guide: GuideListItem, filter: StatusFilter, today: string): boolean {
  return filter === "overdue" ? isReviewOverdue(guide, today) : guide.status === filter;
}

/** Every owner in `guides`, once each ignoring case (first spelling wins), A–Z. */
export function libraryOwners(guides: GuideListItem[]): string[] {
  const owners = new Map<string, string>();
  for (const guide of guides) {
    const owner = guide.owner?.trim();
    if (owner && !owners.has(owner.toLowerCase())) owners.set(owner.toLowerCase(), owner);
  }
  return [...owners.values()].sort(collator.compare);
}

/** Whether `owner` owns a guide, ignoring case. */
export function hasOwner(guide: GuideListItem, owner: string): boolean {
  return guide.owner?.trim().toLowerCase() === owner.trim().toLowerCase();
}

export const GUIDE_KINDS: { value: GuideKind; label: string }[] = [
  { value: "walkthrough", label: "Step-by-step" },
  { value: "annotated", label: "Annotation" },
];

/** What a guide holds, for its card: "7 steps", or "4 pages · 12 hotspots" for an annotation guide. */
export function guideContents(guide: Pick<GuideListItem, "kind" | "stepCount" | "hotspotCount">): string {
  const count = (n: number, one: string) => `${n} ${one}${n === 1 ? "" : "s"}`;
  return guide.kind === "annotated"
    ? `${count(guide.stepCount, "page")} · ${count(guide.hotspotCount, "hotspot")}`
    : count(guide.stepCount, "step");
}

/** The last part of a folder path (`Work/Team` → `Team`). */
export function folderName(folder: string): string {
  return folder.slice(folder.lastIndexOf("/") + 1);
}

/** The folder holding `folder` (`""` for a top-level folder). */
export function parentFolder(folder: string): string {
  const i = folder.lastIndexOf("/");
  return i < 0 ? "" : folder.slice(0, i);
}

/** A library folder with what is directly in it. */
export interface FolderNode {
  /** Relative path, `/` separated; `""` for the library itself. */
  path: string;
  name: string;
  folders: FolderNode[];
  guides: GuideListItem[];
}

/**
 * Groups guides (in the order given) into their folders. `folders` adds
 * folders that hold no listed guide; with `keepEmpty` false (a search or
 * tag filter is on) folders without a matching guide below them are left
 * out. Subfolders are A–Z.
 */
export function buildTree(guides: GuideListItem[], folders: string[], keepEmpty: boolean): FolderNode {
  const root: FolderNode = { path: "", name: "", folders: [], guides: [] };
  const nodes = new Map<string, FolderNode>([["", root]]);
  const node = (path: string): FolderNode => {
    const found = nodes.get(path);
    if (found) return found;
    const created: FolderNode = { path, name: folderName(path), folders: [], guides: [] };
    nodes.set(path, created);
    node(parentFolder(path)).folders.push(created);
    return created;
  };
  if (keepEmpty) for (const folder of folders) if (folder) node(folder);
  for (const guide of guides) node(guide.folder).guides.push(guide);
  const sortFolders = (n: FolderNode) => {
    n.folders.sort((a, b) => collator.compare(a.name, b.name));
    n.folders.forEach(sortFolders);
  };
  sortFolders(root);
  return root;
}

/** Every folder path from the top down, for a "Move to" menu. */
export function folderPaths(folders: string[], guides: GuideListItem[]): string[] {
  const all = new Set(folders.filter(Boolean));
  for (const guide of guides) {
    for (let f = guide.folder; f; f = parentFolder(f)) all.add(f);
  }
  return [...all].sort((a, b) => collator.compare(a, b));
}

/** The title of a copy: `Title copy`, then `Title copy 2`, ... avoiding `taken` titles. */
export function copyTitle(title: string, taken: string[]): string {
  const base = `${title.trim() || "Untitled"} copy`;
  const used = new Set(taken.map((t) => t.toLowerCase()));
  if (!used.has(base.toLowerCase())) return base;
  for (let n = 2; ; n++) {
    const candidate = `${base} ${n}`;
    if (!used.has(candidate.toLowerCase())) return candidate;
  }
}

/** Whether a guide is in `folder` or a folder below it (`""`: the top, so every guide). */
export function inFolder(guide: { folder: string }, folder: string): boolean {
  return folder === "" || guide.folder === folder || guide.folder.startsWith(`${folder}/`);
}

const UNITS: [Intl.RelativeTimeFormatUnit, number][] = [
  ["year", 365 * 24 * 3600],
  ["month", 30 * 24 * 3600],
  ["week", 7 * 24 * 3600],
  ["day", 24 * 3600],
  ["hour", 3600],
  ["minute", 60],
];

/** How long ago `iso` was, e.g. "2 hours ago" or "yesterday"; "just now" under a minute. */
export function timeAgo(iso: string, now: Date = new Date(), locale?: string): string {
  const seconds = (now.getTime() - time(iso)) / 1000;
  const format = new Intl.RelativeTimeFormat(locale, { numeric: "auto" });
  for (const [unit, size] of UNITS) {
    if (Math.abs(seconds) >= size) return format.format(-Math.round(seconds / size), unit);
  }
  return "just now";
}

/** Up to two initials of a name, for an avatar: "Dana Ortiz" → "DO". */
export function initials(name: string): string {
  const words = name.trim().split(/[\s._@-]+/).filter(Boolean);
  return words
    .slice(0, 2)
    .map((w) => [...w][0]!.toUpperCase())
    .join("");
}
