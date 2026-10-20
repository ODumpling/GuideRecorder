import type { Guide, Item } from "@guiderecorder/guide-schema";
import { equal, type AssetMap } from "@guiderecorder/sync";

// What a change request changes: the guide's fields, and its items matched
// by ULID (as `packages/sync` merges them). A step whose screenshot changed
// counts as changed even if its image path didn't, because the path's asset
// did. Pure, for the review page.

/** Bookkeeping fields left out of a review. */
const SKIP_GUIDE = new Set(["items", "updatedAt", "schemaVersion", "aiState", "editedFields"]);
const SKIP_ITEM = new Set(["id", "type", "aiState"]);

/** A guide.json value. */
export type Json = string | number | boolean | null | Json[] | { [key: string]: Json };

export interface FieldChange {
  field: string;
  before: Json;
  after: Json;
}

export type ItemChange = "added" | "removed" | "changed" | "moved" | "unchanged";

export interface ItemDiff {
  id: string;
  type: Item["type"];
  change: ItemChange;
  /** Item fields that changed, e.g. `title`, `annotations`, `image`. */
  fields: string[];
  before: Item | null;
  after: Item | null;
  /** SHA-256 of the screenshot a step shows, before and after. */
  beforeImage: string | null;
  afterImage: string | null;
}

export interface GuideDiff {
  fields: FieldChange[];
  /** In the new order, with removed items where they were. */
  items: ItemDiff[];
  changed: boolean;
}

export interface Side {
  guide: Guide;
  assets: AssetMap;
}

/** The image path a step shows: its crop, unless it shows the full screenshot or has no crop. */
function shownPath(item: Item | null): string | null {
  if (!item || item.type !== "step") return null;
  const { image } = item;
  return image.show === "full" || !image.crop ? image.full : image.crop;
}

/** The asset hashes behind a step's images, so a changed screenshot shows even under the same path. */
function imageHashes(item: Item, assets: AssetMap) {
  if (item.type !== "step") return null;
  const { full, crop, after } = item.image;
  return [full, crop, after].map((p) => (p ? (assets[p] ?? p) : null));
}

/** Longest common subsequence of two id lists: the items that kept their relative order. */
function inOrder(a: string[], b: string[]): Set<string> {
  const n = a.length;
  const m = b.length;
  const table = Array.from({ length: n + 1 }, () => new Array<number>(m + 1).fill(0));
  for (let i = n - 1; i >= 0; i -= 1) {
    for (let j = m - 1; j >= 0; j -= 1) {
      table[i]![j] = a[i] === b[j] ? table[i + 1]![j + 1]! + 1 : Math.max(table[i + 1]![j]!, table[i]![j + 1]!);
    }
  }
  const kept = new Set<string>();
  for (let i = 0, j = 0; i < n && j < m; ) {
    if (a[i] === b[j]) {
      kept.add(a[i]!);
      i += 1;
      j += 1;
    } else if (table[i + 1]![j]! >= table[i]![j + 1]!) i += 1;
    else j += 1;
  }
  return kept;
}

/** The changes from `base` (null: a guide never published) to `head`. */
export function diffGuides(base: Side | null, head: Side): GuideDiff {
  const b = (base?.guide ?? {}) as unknown as Record<string, unknown>;
  const h = head.guide as unknown as Record<string, unknown>;
  const fields: FieldChange[] = [];
  for (const field of new Set([...Object.keys(b), ...Object.keys(h)])) {
    if (SKIP_GUIDE.has(field) || equal(b[field], h[field])) continue;
    fields.push({ field, before: (b[field] ?? null) as Json, after: (h[field] ?? null) as Json });
  }

  const baseItems = base?.guide.items ?? [];
  const headItems = head.guide.items ?? [];
  const baseById = new Map(baseItems.map((i) => [i.id, i]));
  const headIds = new Set(headItems.map((i) => i.id));
  const common = (items: Item[], other: Set<string>) => items.map((i) => i.id).filter((id) => other.has(id));
  const kept = inOrder(common(baseItems, headIds), common(headItems, new Set(baseById.keys())));

  const diffItem = (before: Item | null, after: Item | null): ItemDiff => {
    const item = (after ?? before)!;
    const changedFields: string[] = [];
    if (before && after) {
      const bo = before as unknown as Record<string, unknown>;
      const ao = after as unknown as Record<string, unknown>;
      for (const field of new Set([...Object.keys(bo), ...Object.keys(ao)])) {
        if (SKIP_ITEM.has(field)) continue;
        const same = field === "image" ? equal(imageHashes(before, base!.assets), imageHashes(after, head.assets)) : equal(bo[field], ao[field]);
        if (!same) changedFields.push(field);
      }
    }
    const change: ItemChange = !before ? "added" : !after ? "removed" : changedFields.length ? "changed" : kept.has(item.id) ? "unchanged" : "moved";
    const beforePath = shownPath(before);
    const afterPath = shownPath(after);
    return {
      id: item.id,
      type: item.type,
      change,
      fields: changedFields,
      before,
      after,
      beforeImage: beforePath ? (base!.assets[beforePath] ?? null) : null,
      afterImage: afterPath ? (head.assets[afterPath] ?? null) : null,
    };
  };

  // Walk the new order; removed items go in before the first item that came after them.
  const items: ItemDiff[] = [];
  const baseIndex = new Map(baseItems.map((i, n) => [i.id, n]));
  const removed = baseItems.filter((i) => !headIds.has(i.id));
  let r = 0;
  for (const after of headItems) {
    const at = baseIndex.get(after.id);
    if (at !== undefined) {
      while (r < removed.length && baseIndex.get(removed[r]!.id)! < at) items.push(diffItem(removed[r++]!, null));
    }
    items.push(diffItem(baseById.get(after.id) ?? null, after));
  }
  while (r < removed.length) items.push(diffItem(removed[r++]!, null));

  return { fields, items, changed: fields.length > 0 || items.some((i) => i.change !== "unchanged") };
}
