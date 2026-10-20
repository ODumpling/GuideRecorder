import { describe, expect, it } from "vitest";
import type { GuideListItem } from "@/lib/types";
import {
  arrowStep,
  DEFAULT_LIST_LAYOUT,
  initialLibraryView,
  initialListLayout,
  MAX_COLUMN_WIDTH,
  MIN_COLUMN_WIDTH,
  nextSort,
  NO_SELECTION,
  pruneSelection,
  rememberLibraryView,
  rememberListLayout,
  selectGuide,
  sortRows,
  stepFocus,
} from "@/editor/libraryList";

function item(path: string, fields: Partial<GuideListItem> = {}): GuideListItem {
  return {
    id: path,
    path,
    title: path,
    summary: "",
    tags: [],
    stepCount: 1,
    kind: "walkthrough",
    hotspotCount: 0,
    createdAt: "2026-01-01T00:00:00Z",
    updatedAt: "2026-01-01T00:00:00Z",
    folder: "",
    readOnly: false,
    duplicateOf: null,
    ...fields,
  };
}

function memory(initial: Record<string, string> = {}) {
  const data = new Map(Object.entries(initial));
  return {
    getItem: (key: string) => data.get(key) ?? null,
    setItem: (key: string, value: string) => void data.set(key, value),
  };
}

const paths = (guides: GuideListItem[]) => guides.map((g) => g.path);

describe("view and layout", () => {
  it("remembers grid or list, the grid at first", () => {
    const storage = memory();
    expect(initialLibraryView(storage)).toBe("grid");
    rememberLibraryView("list", storage);
    expect(initialLibraryView(storage)).toBe("list");
    expect(initialLibraryView(memory({ "guiderecorder.libraryView": "cards" }))).toBe("grid");
  });

  it("remembers the column layout and drops what is invalid", () => {
    const storage = memory();
    expect(initialListLayout(storage)).toEqual(DEFAULT_LIST_LAYOUT);
    const layout = {
      sort: { column: "reviewDue" as const, descending: false },
      widths: { owner: 200 },
      hidden: ["folder" as const],
    };
    rememberListLayout(layout, storage);
    expect(initialListLayout(storage)).toEqual(layout);

    const odd = memory({
      "guiderecorder.libraryList": JSON.stringify({
        sort: { column: "colour", descending: true },
        widths: { owner: 5, steps: 5000, nope: 100, folder: "wide" },
        hidden: ["title", "status", "nope"],
      }),
    });
    expect(initialListLayout(odd)).toEqual({
      sort: null,
      widths: { owner: MIN_COLUMN_WIDTH, steps: MAX_COLUMN_WIDTH },
      hidden: ["status"],
    });
    expect(initialListLayout(memory({ "guiderecorder.libraryList": "{" }))).toEqual(DEFAULT_LIST_LAYOUT);
  });
});

describe("column sort", () => {
  it("cycles ascending, descending, off; dates and counts start descending", () => {
    let sort = nextSort(null, "title");
    expect(sort).toEqual({ column: "title", descending: false });
    sort = nextSort(sort, "title");
    expect(sort).toEqual({ column: "title", descending: true });
    expect(nextSort(sort, "title")).toBeNull();
    sort = nextSort(sort, "updated");
    expect(sort).toEqual({ column: "updated", descending: true });
    expect(nextSort(sort, "updated")).toEqual({ column: "updated", descending: false });
  });

  it("sorts each column, guides without a value last either way", () => {
    const a = item("a", { title: "Beta", owner: "Zed", status: "published", stepCount: 2, folder: "Work" });
    const b = item("b", { title: "alpha", status: "draft", stepCount: 10, updatedAt: "2026-03-01T00:00:00Z" });
    const c = item("c", {
      title: "Gamma",
      owner: "ana",
      stepCount: 2,
      reviewIntervalMonths: 1,
      lastReviewed: "2026-02-01",
    });
    const guides = [a, b, c];
    expect(paths(sortRows(guides, null))).toEqual(["a", "b", "c"]);
    expect(paths(sortRows(guides, { column: "title", descending: false }))).toEqual(["b", "a", "c"]);
    expect(paths(sortRows(guides, { column: "owner", descending: false }))).toEqual(["c", "a", "b"]);
    expect(paths(sortRows(guides, { column: "owner", descending: true }))).toEqual(["a", "c", "b"]);
    expect(paths(sortRows(guides, { column: "status", descending: false }))).toEqual(["b", "a", "c"]);
    // Ties keep the order given.
    expect(paths(sortRows(guides, { column: "steps", descending: true }))).toEqual(["b", "a", "c"]);
    expect(paths(sortRows(guides, { column: "updated", descending: true }))).toEqual(["b", "a", "c"]);
    expect(paths(sortRows(guides, { column: "reviewDue", descending: true }))).toEqual(["c", "a", "b"]);
    expect(paths(sortRows(guides, { column: "folder", descending: false }))).toEqual(["a", "b", "c"]);
  });
});

describe("selection", () => {
  const order = ["a", "b", "c", "d", "e"];

  it("selects one, toggles with ⌘ and selects a range with ⇧", () => {
    let s = selectGuide(NO_SELECTION, order, "b", "only");
    expect(s).toEqual({ paths: ["b"], anchor: "b" });
    s = selectGuide(s, order, "d", "range");
    expect(s).toEqual({ paths: ["b", "c", "d"], anchor: "b" });
    // The range goes from the anchor, either way, replacing the last one.
    s = selectGuide(s, order, "a", "range");
    expect(s).toEqual({ paths: ["a", "b"], anchor: "b" });
    s = selectGuide(s, order, "e", "toggle");
    expect(s).toEqual({ paths: ["a", "b", "e"], anchor: "e" });
    s = selectGuide(s, order, "a", "toggle");
    expect(s).toEqual({ paths: ["b", "e"], anchor: "a" });
    expect(selectGuide(NO_SELECTION, order, "c", "range")).toEqual({ paths: ["c"], anchor: "c" });
  });

  it("drops guides no longer shown", () => {
    const s = { paths: ["a", "c"], anchor: "c" };
    expect(pruneSelection(s, order)).toBe(s);
    expect(pruneSelection(s, ["a", "b"])).toEqual({ paths: ["a"], anchor: null });
  });

  it("moves with the arrow keys: rows of cards in the grid, one row in the list", () => {
    expect(arrowStep("ArrowDown", "grid", 4)).toBe(4);
    expect(arrowStep("ArrowUp", "grid", 4)).toBe(-4);
    expect(arrowStep("ArrowLeft", "grid", 4)).toBe(-1);
    expect(arrowStep("ArrowDown", "list", 4)).toBe(1);
    expect(arrowStep("ArrowLeft", "list", 4)).toBeNull();
    expect(arrowStep("Enter", "grid", 4)).toBeNull();

    expect(stepFocus(order, null, 1)).toBe("a");
    expect(stepFocus(order, null, -1)).toBe("e");
    expect(stepFocus(order, "b", 1)).toBe("c");
    expect(stepFocus(order, "b", 4)).toBe("e");
    expect(stepFocus(order, "b", -4)).toBe("a");
    expect(stepFocus([], "b", 1)).toBeNull();
  });
});
