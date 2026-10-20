import { sql, type SQL } from "drizzle-orm";
import type { Guide } from "@guiderecorder/guide-schema";

// Library search: Postgres full-text search over each guide's head revision.
// The `simple` configuration (lowercase, no stemming) because guides come in
// any language; queries match word prefixes instead, so "rot" finds
// "rotate". Weights: A title and tags, B summary, section and step titles,
// C everything else a reader sees (intro, prerequisites, step text, notes,
// hotspots and callouts). Migration 0002 backfills the same way in SQL.

type ItemText = {
  type: string;
  title?: string;
  description?: string;
  body?: string;
  annotations?: { geometry?: { title?: string; description?: string; text?: string } }[];
};

const join = (parts: (string | null | undefined)[]) => parts.filter((p) => p?.trim()).join(" ");

/** The text a guide is found by, in weight order. */
export function searchText(guide: Guide): { a: string; b: string; c: string } {
  const items = (guide.items ?? []) as ItemText[];
  return {
    a: join([guide.title, ...(guide.tags ?? [])]),
    b: join([guide.summary, ...items.filter((i) => i.type === "step" || i.type === "section").map((i) => i.title)]),
    c: join([
      guide.intro,
      guide.prerequisites,
      ...items.flatMap((i) => [i.description, i.body]),
      ...items.flatMap((i) => (i.annotations ?? []).flatMap((a) => [a.geometry?.title, a.geometry?.description, a.geometry?.text])),
    ]),
  };
}

/** The guide's search document, for `guide.search`. */
export function searchVector(guide: Guide): SQL {
  const { a, b, c } = searchText(guide);
  return sql`setweight(to_tsvector('simple', ${a}), 'A') || setweight(to_tsvector('simple', ${b}), 'B') || setweight(to_tsvector('simple', ${c}), 'C')`;
}

/**
 * A `to_tsquery` string matching every word of `query` as a prefix, or null
 * if it has no words. Only letters and digits survive, so the result is
 * always valid tsquery syntax.
 */
export function prefixQuery(query: string): string | null {
  const words = query.toLowerCase().match(/[\p{L}\p{N}]+/gu) ?? [];
  return words.length ? words.slice(0, 16).map((w) => `${w}:*`).join(" & ") : null;
}
