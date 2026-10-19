// The library's list view and selection: columns and their sort, the
// view and column layout remembered between launches, and multi-select.

import type { GuideListItem } from "@/lib/types";
import { GUIDE_STATUSES, reviewDue } from "@/editor/library";

/** How the library shows its guides: cards or a dense list. */
export type LibraryView = "grid" | "list";

/** The view chosen last time (on this Mac). */
const VIEW_KEY = "guiderecorder.libraryView";

/** The view chosen last time; the grid at first. */
export function initialLibraryView(storage: Pick<Storage, "getItem"> = localStorage): LibraryView {
  try {
    return storage.getItem(VIEW_KEY) === "list" ? "list" : "grid";
  } catch {
    return "grid";
  }
}

/** Remembers the view, also after a restart. */
export function rememberLibraryView(view: LibraryView, storage: Pick<Storage, "setItem"> = localStorage) {
  try {
    storage.setItem(VIEW_KEY, view);
  } catch {
    // Not remembered; the grid shows next time.
  }
}

export type ListColumn = "title" | "folder" | "status" | "owner" | "steps" | "updated" | "reviewDue";

export interface ListColumnSpec {
  column: ListColumn;
  label: string;
  /** Default width in pixels; the title takes what is left (its width is its minimum). */
  width: number;
  /** A first click sorts this column descending (largest, newest first). */
  descendingFirst?: boolean;
  align?: "end";
}

/** The list's columns, in order. The title cannot be hidden. */
export const LIST_COLUMNS: ListColumnSpec[] = [
  { column: "title", label: "Title", width: 240 },
  { column: "folder", label: "Folder", width: 140 },
  { column: "status", label: "Status", width: 120 },
  { column: "owner", label: "Owner", width: 140 },
  { column: "steps", label: "Steps", width: 70, descendingFirst: true, align: "end" },
  { column: "updated", label: "Updated", width: 120, descendingFirst: true },
  { column: "reviewDue", label: "Review due", width: 120 },
];

export const MIN_COLUMN_WIDTH = 60;
export const MAX_COLUMN_WIDTH = 800;

/** A column sort; null keeps the library's order (the sort menu, or best match in a search). */
export type ListSort = { column: ListColumn; descending: boolean } | null;

export interface ListLayout {
  sort: ListSort;
  /** Widths the user set, in pixels. */
  widths: Partial<Record<ListColumn, number>>;
  hidden: ListColumn[];
}

export const DEFAULT_LIST_LAYOUT: ListLayout = { sort: null, widths: {}, hidden: [] };

const LAYOUT_KEY = "guiderecorder.libraryList";

function isColumn(value: unknown): value is ListColumn {
  return LIST_COLUMNS.some((c) => c.column === value);
}

/** The column layout set last time, else the default; anything invalid is dropped. */
export function initialListLayout(storage: Pick<Storage, "getItem"> = localStorage): ListLayout {
  let stored: unknown = null;
  try {
    stored = JSON.parse(storage.getItem(LAYOUT_KEY) ?? "null");
  } catch {
    // Unreadable: the default applies.
  }
  if (!stored || typeof stored !== "object") return { ...DEFAULT_LIST_LAYOUT };
  const s = stored as Partial<Record<keyof ListLayout, unknown>>;
  const sort = s.sort as { column?: unknown; descending?: unknown } | null | undefined;
  const widths: ListLayout["widths"] = {};
  if (s.widths && typeof s.widths === "object") {
    for (const [column, width] of Object.entries(s.widths)) {
      if (isColumn(column) && typeof width === "number" && Number.isFinite(width)) {
        widths[column] = clampWidth(width);
      }
    }
  }
  return {
    sort:
      sort && isColumn(sort.column) && typeof sort.descending === "boolean"
        ? { column: sort.column, descending: sort.descending }
        : null,
    widths,
    hidden: Array.isArray(s.hidden) ? s.hidden.filter((c): c is ListColumn => isColumn(c) && c !== "title") : [],
  };
}

/** Remembers the column layout, also after a restart. */
export function rememberListLayout(layout: ListLayout, storage: Pick<Storage, "setItem"> = localStorage) {
  try {
    storage.setItem(LAYOUT_KEY, JSON.stringify(layout));
  } catch {
    // Not remembered; the default applies next time.
  }
}

export function clampWidth(width: number): number {
  return Math.round(Math.min(MAX_COLUMN_WIDTH, Math.max(MIN_COLUMN_WIDTH, width)));
}

/** A click on a column heading: sorts by it (descending first for dates and counts), then the other way, then not. */
export function nextSort(sort: ListSort, column: ListColumn): ListSort {
  const first = LIST_COLUMNS.find((c) => c.column === column)?.descendingFirst ?? false;
  if (sort?.column !== column) return { column, descending: first };
  return sort.descending === first ? { column, descending: !first } : null;
}

const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });

/** A guide's value in `column`; null sorts last either way. */
function sortValue(guide: GuideListItem, column: ListColumn): string | number | null {
  switch (column) {
    case "title":
      return guide.title || "Untitled";
    case "folder":
      return guide.folder || null;
    case "status": {
      const i = GUIDE_STATUSES.findIndex((s) => s.value === guide.status);
      return i < 0 ? null : i;
    }
    case "owner":
      return guide.owner?.trim() || null;
    case "steps":
      return guide.stepCount;
    case "updated": {
      const t = Date.parse(guide.updatedAt);
      return Number.isNaN(t) ? null : t;
    }
    case "reviewDue":
      return reviewDue(guide);
  }
}

function compareValues(a: string | number, b: string | number): number {
  return typeof a === "number" && typeof b === "number" ? a - b : collator.compare(String(a), String(b));
}

/**
 * `guides` sorted by a column: guides without a value (no owner, no review
 * schedule) go last either way; ties keep the order given. Null: as given.
 */
export function sortRows(guides: GuideListItem[], sort: ListSort): GuideListItem[] {
  if (!sort) return guides;
  const sign = sort.descending ? -1 : 1;
  return guides
    .map((guide, i) => ({ guide, i, value: sortValue(guide, sort.column) }))
    .sort((a, b) => {
      if (a.value === null || b.value === null) {
        return a.value === b.value ? a.i - b.i : a.value === null ? 1 : -1;
      }
      return sign * compareValues(a.value, b.value) || a.i - b.i;
    })
    .map((row) => row.guide);
}

/** Selected guides, by path (a duplicate shares its original's id). */
export interface Selection {
  paths: string[];
  /** Where a ⇧-click range starts: the last guide clicked without ⇧. */
  anchor: string | null;
}

export const NO_SELECTION: Selection = { paths: [], anchor: null };

/** How a click selects: just that guide, ⌘ toggles it, ⇧ selects the range from the anchor. */
export type SelectMode = "only" | "toggle" | "range";

/** The selection after a click on `path`; `order` is the guides as shown. */
export function selectGuide(selection: Selection, order: string[], path: string, mode: SelectMode): Selection {
  if (mode === "toggle") {
    const on = selection.paths.includes(path);
    return {
      paths: on ? selection.paths.filter((p) => p !== path) : [...selection.paths, path],
      anchor: path,
    };
  }
  if (mode === "range") {
    const from = selection.anchor ? order.indexOf(selection.anchor) : -1;
    const to = order.indexOf(path);
    if (from < 0 || to < 0) return { paths: [path], anchor: path };
    const [lo, hi] = from < to ? [from, to] : [to, from];
    return { paths: order.slice(lo, hi + 1), anchor: selection.anchor };
  }
  return { paths: [path], anchor: path };
}

/** Drops selected guides no longer shown (filtered out, moved or deleted); the same object if none. */
export function pruneSelection(selection: Selection, order: string[]): Selection {
  const shown = new Set(order);
  if (selection.paths.every((p) => shown.has(p)) && (!selection.anchor || shown.has(selection.anchor))) {
    return selection;
  }
  return {
    paths: selection.paths.filter((p) => shown.has(p)),
    anchor: selection.anchor && shown.has(selection.anchor) ? selection.anchor : null,
  };
}

/**
 * How far an arrow key moves through the guides: a row of `columns` cards
 * up or down in the grid, one guide otherwise. Null: not an arrow key (or
 * left and right in the list).
 */
export function arrowStep(key: string, view: LibraryView, columns: number): number | null {
  const across = view === "grid" ? Math.max(1, columns) : 1;
  switch (key) {
    case "ArrowUp":
      return -across;
    case "ArrowDown":
      return across;
    case "ArrowLeft":
      return view === "grid" ? -1 : null;
    case "ArrowRight":
      return view === "grid" ? 1 : null;
    default:
      return null;
  }
}

/** The guide `step` away from `current` in `order`, kept in bounds; the first (or last, going back) if none is current. */
export function stepFocus(order: string[], current: string | null, step: number): string | null {
  if (order.length === 0) return null;
  const i = current ? order.indexOf(current) : -1;
  if (i < 0) return step < 0 ? order[order.length - 1]! : order[0]!;
  return order[Math.min(order.length - 1, Math.max(0, i + step))]!;
}
