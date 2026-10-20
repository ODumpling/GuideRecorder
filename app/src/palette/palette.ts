// The command palette's search: what a query asks for (`>` commands, `#`
// tags), fuzzy matching, the order of results, and the recently opened
// guides that come first.

import { sortGuides } from "@/editor/library";
import type { GuideListItem } from "@/lib/types";

/** What the palette lists: everything, only commands (`>`), or only tags (`#`). */
export type PaletteMode = "all" | "commands" | "tags";

/** A query split into its mode prefix and the text to match. */
export function parseQuery(raw: string): { mode: PaletteMode; text: string } {
  const query = raw.trimStart();
  if (query.startsWith(">")) return { mode: "commands", text: query.slice(1).trim() };
  if (query.startsWith("#")) return { mode: "tags", text: query.slice(1).trim() };
  return { mode: "all", text: query.trim() };
}

export interface PaletteEntry {
  id: string;
  kind: "guide" | "command" | "tag";
  /** Matched first, and best. */
  title: string;
  /** Also matched, a little lower: a guide's tags and summary, a command's other names. */
  keywords?: string[];
  /** Listed only once something is typed (each step, theme or folder), so the palette starts short. */
  searchOnly?: boolean;
  /** A guide's place in the recent order (0 is the most recent). */
  recency?: number;
}

/** Scores of the kinds of match, best first; a fuzzy match stays below any substring. */
const EXACT = 1000;
const PREFIX = 900;
const WORD = 800;
const SUBSTRING = 600;
const FUZZY = 100;
const FUZZY_MAX = 500;
/** A guide the library search found by its step text, and nothing else matched. */
export const STEP_MATCH = 350;
/** Keyword matches count this much of a title match. */
const KEYWORD_WEIGHT = 0.6;
/** Recent guides listed before anything is typed. */
export const RECENT_LIMIT = 5;

function isWordStart(text: string, i: number): boolean {
  return i === 0 || !/[\p{L}\p{N}]/u.test(text[i - 1]!);
}

/**
 * How well `query` matches `text`, ignoring case: the whole text, its
 * start, a word start, anywhere, or else its letters in order (more for
 * letters at word starts or next to each other). Null if it doesn't.
 */
export function fuzzyScore(query: string, text: string): number | null {
  const q = query.trim().toLowerCase();
  const t = text.toLowerCase();
  if (!q) return 0;
  if (t === q) return EXACT;
  if (t.startsWith(q)) return PREFIX;
  const at = t.indexOf(q);
  if (at > 0) return isWordStart(t, at) ? WORD : SUBSTRING;
  let bonus = 0;
  let from = 0;
  let last = -2;
  for (const ch of q) {
    if (/\s/.test(ch)) continue;
    const i = t.indexOf(ch, from);
    if (i < 0) return null;
    if (i === last + 1) bonus += 2;
    else if (isWordStart(t, i)) bonus += 3;
    last = i;
    from = i + 1;
  }
  return Math.min(FUZZY + bonus * 10, FUZZY_MAX);
}

/** An entry's best match for `text`: its title, else a keyword, else (a guide) a step text hit. */
export function entryScore(entry: PaletteEntry, text: string, stepMatches?: ReadonlySet<string>): number | null {
  let best = fuzzyScore(text, entry.title);
  for (const keyword of entry.keywords ?? []) {
    const score = fuzzyScore(text, keyword);
    if (score !== null) best = Math.max(best ?? 0, Math.round(score * KEYWORD_WEIGHT));
  }
  if (entry.kind === "guide" && stepMatches?.has(entry.id)) best = Math.max(best ?? 0, STEP_MATCH);
  return best;
}

function inMode(entry: PaletteEntry, mode: PaletteMode): boolean {
  return mode === "all" || (mode === "commands" ? entry.kind === "command" : entry.kind === "tag");
}

const byRecency = (a: PaletteEntry, b: PaletteEntry) => (a.recency ?? Infinity) - (b.recency ?? Infinity);

/**
 * The entries `raw` asks for, best first. With nothing typed: the most
 * recent guides, then the commands in their given order (tags only after
 * `#`). Otherwise every match by score; ties go to the more recent guide,
 * then the given order. `stepMatches` are guide ids the library search
 * found by their step text.
 */
export function rankEntries<T extends PaletteEntry>(
  entries: T[],
  raw: string,
  { stepMatches, recentLimit = RECENT_LIMIT }: { stepMatches?: ReadonlySet<string>; recentLimit?: number } = {},
): T[] {
  const { mode, text } = parseQuery(raw);
  const pool = entries.filter((e) => inMode(e, mode));
  if (!text) {
    const shown = pool.filter((e) => mode === "tags" || (!e.searchOnly && e.kind !== "tag"));
    const guides = shown.filter((e) => e.kind === "guide").sort(byRecency).slice(0, recentLimit);
    return [...guides, ...shown.filter((e) => e.kind !== "guide")];
  }
  return pool
    .map((entry, index) => ({ entry, index, score: entryScore(entry, text, stepMatches) }))
    .filter((r): r is { entry: T; index: number; score: number } => r.score !== null)
    .sort((a, b) => b.score - a.score || byRecency(a.entry, b.entry) || a.index - b.index)
    .map((r) => r.entry);
}

/** Where the recently opened guides are kept (on this Mac). */
const RECENT_KEY = "guiderecorder.recentGuides";
const MAX_RECENT = 20;

/** Ids of the guides opened last, most recent first. */
export function recentGuideIds(storage: Pick<Storage, "getItem"> = localStorage): string[] {
  try {
    const ids: unknown = JSON.parse(storage.getItem(RECENT_KEY) ?? "[]");
    return Array.isArray(ids) ? ids.filter((id): id is string => typeof id === "string") : [];
  } catch {
    return [];
  }
}

/** Puts `id` first among the recently opened guides. */
export function rememberOpened(id: string, storage: Pick<Storage, "getItem" | "setItem"> = localStorage) {
  const ids = [id, ...recentGuideIds(storage).filter((other) => other !== id)].slice(0, MAX_RECENT);
  try {
    storage.setItem(RECENT_KEY, JSON.stringify(ids));
  } catch {
    // Not remembered; the guides go by their last update.
  }
}

/** Guides most recent first: those opened lately, then the rest by last update. */
export function recentFirst(guides: GuideListItem[], recentIds: string[]): GuideListItem[] {
  const opened = new Map(recentIds.map((id, i) => [id, i]));
  const rest = sortGuides(
    guides.filter((g) => !opened.has(g.id)),
    "updated",
  );
  const recent = guides.filter((g) => opened.has(g.id)).sort((a, b) => opened.get(a.id)! - opened.get(b.id)!);
  return [...recent, ...rest];
}
