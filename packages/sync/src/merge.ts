// Three-way merge of guides at item level. Items (steps, sections, notes)
// are matched by their ULID; each top-level field of an item, and of the
// guide, merges on its own. Changes to different fields or items combine;
// different edits to the same field, or an edit to an item the other side
// deleted, are conflicts.

import type { Guide } from "@guiderecorder/guide-schema";

export type Side = "ours" | "theirs";

export type Conflict =
  /** Both sides changed the same field differently. `itemId` is null for guide fields. */
  | { kind: "field"; itemId: string | null; field: string; base: unknown; ours: unknown; theirs: unknown }
  /** One side deleted an item the other changed. */
  | { kind: "deleted"; itemId: string; deletedBy: Side; base: unknown; ours: unknown; theirs: unknown }
  /** Both sides reordered items differently. */
  | { kind: "order"; base: string[]; ours: string[]; theirs: string[] };

export interface MergeResult {
  /** The merged guide; each conflict resolved to `prefer`. */
  guide: Guide;
  conflicts: Conflict[];
}

type Obj = Record<string, unknown>;
interface Item extends Obj {
  id: string;
}

/** Fields that record when or how the file was written, not what it says. */
const META_FIELDS = new Set(["items", "updatedAt", "schemaVersion"]);

/**
 * Merges the changes `ours` and `theirs` each made to `base`. A null base
 * (the guide was created on both sides) merges as if every field were new.
 * Conflicts resolve to `prefer`; the result lists them all so a caller can
 * ask the user instead.
 */
export function mergeGuides(base: Guide | null, ours: Guide, theirs: Guide, prefer: Side = "ours"): MergeResult {
  const conflicts: Conflict[] = [];
  const b = (base ?? {}) as Obj;
  const o = ours as unknown as Obj;
  const t = theirs as unknown as Obj;

  const merged = mergeFields(b, o, t, prefer, (field, values) => conflicts.push({ kind: "field", itemId: null, field, ...values }), META_FIELDS);
  merged.items = mergeItems(items(b), items(o), items(t), prefer, conflicts);
  merged.schemaVersion = Math.max(Number(o.schemaVersion ?? 0), Number(t.schemaVersion ?? 0));
  merged.updatedAt = later(o.updatedAt, t.updatedAt);
  return { guide: merged as unknown as Guide, conflicts };
}

function items(guide: Obj): Item[] {
  return Array.isArray(guide.items) ? (guide.items as Item[]) : [];
}

function later(a: unknown, b: unknown): unknown {
  if (typeof a !== "string") return b;
  if (typeof b !== "string") return a;
  return Date.parse(b) > Date.parse(a) ? b : a;
}

/** Merges each key on its own, in the preferred side's key order. */
function mergeFields(
  base: Obj,
  ours: Obj,
  theirs: Obj,
  prefer: Side,
  onConflict: (field: string, values: { base: unknown; ours: unknown; theirs: unknown }) => void,
  skip: Set<string> = new Set(),
): Obj {
  const [first, second] = prefer === "ours" ? [ours, theirs] : [theirs, ours];
  const keys = new Set([...Object.keys(first), ...Object.keys(second), ...Object.keys(base)]);
  const result: Obj = {};
  for (const key of keys) {
    if (skip.has(key)) continue;
    const [b, o, t] = [base[key], ours[key], theirs[key]];
    let value: unknown;
    if (equal(o, t)) value = o;
    else if (equal(b, o)) value = t;
    else if (equal(b, t)) value = o;
    else {
      onConflict(key, { base: b, ours: o, theirs: t });
      value = prefer === "ours" ? o : t;
    }
    if (value !== undefined) result[key] = value;
  }
  return result;
}

function mergeItems(base: Item[], ours: Item[], theirs: Item[], prefer: Side, conflicts: Conflict[]): Item[] {
  const byId = (list: Item[]) => new Map(list.map((item) => [item.id, item]));
  const [b, o, t] = [byId(base), byId(ours), byId(theirs)];
  const merged = new Map<string, Item>();

  for (const id of new Set([...o.keys(), ...t.keys()])) {
    const [bi, oi, ti] = [b.get(id), o.get(id), t.get(id)];
    if (oi && ti) {
      const fields = mergeFields(bi ?? {}, oi, ti, prefer, (field, values) =>
        conflicts.push({ kind: "field", itemId: id, field, ...values }),
      );
      merged.set(id, fields as Item);
      continue;
    }
    const kept = (oi ?? ti)!;
    const deletedBy: Side = oi ? "theirs" : "ours";
    if (!bi) merged.set(id, kept); // Added on one side.
    else if (!equal(bi, kept)) {
      // Changed on one side, deleted on the other.
      conflicts.push({ kind: "deleted", itemId: id, deletedBy, base: bi, ours: oi, theirs: ti });
      if (prefer !== deletedBy) merged.set(id, kept);
    }
    // Else unchanged on one side and deleted on the other: stays deleted.
  }

  return order(base, ours, theirs, merged, prefer, conflicts).map((id) => merged.get(id)!);
}

/**
 * The merged item order. A side that kept the base order takes the other's
 * reordering; items one side added go after the item they followed there.
 */
function order(base: Item[], ours: Item[], theirs: Item[], kept: Map<string, Item>, prefer: Side, conflicts: Conflict[]): string[] {
  const ids = (list: Item[]) => list.map((item) => item.id);
  const [b, o, t] = [ids(base), ids(ours), ids(theirs)];
  const inAll = (id: string) => b.includes(id) && o.includes(id) && t.includes(id);
  const common = (list: string[]) => list.filter(inAll);
  const oursMoved = !equal(common(o), common(b));
  const theirsMoved = !equal(common(t), common(b));
  if (oursMoved && theirsMoved && !equal(common(o), common(t))) {
    conflicts.push({ kind: "order", base: b, ours: o, theirs: t });
  }

  const primarySide: Side = theirsMoved && !oursMoved ? "theirs" : oursMoved && !theirsMoved ? "ours" : prefer;
  const [primary, secondary] = primarySide === "ours" ? [o, t] : [t, o];
  const result = primary.filter((id) => kept.has(id));
  let anchor = -1;
  for (const id of secondary) {
    const at = result.indexOf(id);
    if (at >= 0) {
      anchor = at;
    } else if (kept.has(id)) {
      result.splice(anchor + 1, 0, id);
      anchor += 1;
    }
  }
  return result;
}

/** Structural equality of JSON values; `undefined` keys count as absent. */
export function equal(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (typeof a !== "object" || typeof b !== "object" || a === null || b === null) return false;
  if (Array.isArray(a) !== Array.isArray(b)) return false;
  if (Array.isArray(a)) {
    const other = b as unknown[];
    return a.length === other.length && a.every((value, i) => equal(value, other[i]));
  }
  const keys = (o: object) => Object.entries(o).filter(([, v]) => v !== undefined).map(([k]) => k);
  const [ka, kb] = [keys(a), keys(b)];
  return ka.length === kb.length && ka.every((k) => equal((a as Obj)[k], (b as Obj)[k]));
}
