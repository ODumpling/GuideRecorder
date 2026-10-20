import { describe, expect, it } from "vitest";
import type { Item } from "@guiderecorder/guide-schema";
import { applyPatches, moveItems } from "@/editor/commands";
import {
  blockDropTarget,
  canMergeRange,
  moveAnnouncement,
  outlineKeyFor,
  rangeIds,
  sectionOf,
  sectionStepCount,
  selectionAnnouncement,
  shiftTarget,
  visibleItems,
  withCollapsed,
} from "@/editor/outline";
import { guide, step } from "@/editor/test-fixtures";

/** S1 A B N1 S2 C, with a section around C. */
function items(): Item[] {
  const g = guide();
  return [...g.items.slice(0, 4), { type: "section", id: "S2", title: "**Finish**" }, step("C")];
}

const ids = (list: Item[]) => list.map((item) => item.id);

const key = (key: string, mods: Partial<Record<"metaKey" | "ctrlKey" | "shiftKey" | "altKey", boolean>> = {}) => ({
  key,
  metaKey: false,
  ctrlKey: false,
  shiftKey: false,
  altKey: false,
  ...mods,
});

describe("sections", () => {
  it("know which section an item is under", () => {
    expect(sectionOf(items(), "A")).toBe("S1");
    expect(sectionOf(items(), "N1")).toBe("S1");
    expect(sectionOf(items(), "C")).toBe("S2");
    // Sections are not under each other.
    expect(sectionOf(items(), "S2")).toBeNull();
    expect(sectionOf(items(), "X")).toBeNull();
    expect(sectionOf([step("A"), ...items()], "A")).toBeNull();
  });

  it("hide their items up to the next section when collapsed", () => {
    expect(ids(visibleItems(items(), new Set()))).toEqual(["S1", "A", "B", "N1", "S2", "C"]);
    expect(ids(visibleItems(items(), new Set(["S1"])))).toEqual(["S1", "S2", "C"]);
    expect(ids(visibleItems(items(), new Set(["S1", "S2"])))).toEqual(["S1", "S2"]);
  });

  it("count their steps, not notes", () => {
    expect(sectionStepCount(items(), "S1")).toBe(2);
    expect(sectionStepCount(items(), "S2")).toBe(1);
    expect(sectionStepCount(items(), "A")).toBe(0);
  });

  it("move with what they hide when collapsed", () => {
    expect(withCollapsed(items(), ["S1"], new Set(["S1"]))).toEqual(["S1", "A", "B", "N1"]);
    expect(withCollapsed(items(), ["S1"], new Set())).toEqual(["S1"]);
    expect(withCollapsed(items(), ["S2", "N1"], new Set(["S2"]))).toEqual(["N1", "S2", "C"]);
  });
});

describe("ranges", () => {
  it("run from the anchor to the focus, either way, in guide order", () => {
    expect(rangeIds(items(), "A", "N1")).toEqual(["A", "B", "N1"]);
    expect(rangeIds(items(), "N1", "A")).toEqual(["A", "B", "N1"]);
    expect(rangeIds(items(), "A", "A")).toEqual(["A"]);
    expect(rangeIds(items(), "A", "X")).toEqual([]);
  });

  it("move one place up or down, stopping at the ends", () => {
    const list = items();
    expect(shiftTarget(list, ["A", "B"], -1)).toBe(0);
    expect(shiftTarget(list, ["A", "B"], 1)).toBe(2);
    expect(shiftTarget(list, ["S1"], -1)).toBeNull();
    expect(shiftTarget(list, ["S2", "C"], 1)).toBeNull();
    expect(shiftTarget(list, ["X"], 1)).toBeNull();
    const g = { ...guide(), items: list };
    expect(ids(applyPatches(g, moveItems(g, ["A", "B"], shiftTarget(list, ["A", "B"], 1)!).forward).items)).toEqual([
      "S1",
      "N1",
      "A",
      "B",
      "S2",
      "C",
    ]);
  });

  it("move past a collapsed section above as a whole, and into one below", () => {
    // S1 [A B N1] S2 C, with S1 collapsed.
    const list = items();
    const folded = new Set(["S1"]);
    const g = { ...guide(), items: list };
    const shifted = (block: string[], by: -1 | 1, collapsed: ReadonlySet<string>) =>
      ids(applyPatches(g, moveItems(g, block, shiftTarget(list, block, by, collapsed)!).forward).items);
    expect(shifted(["S2"], -1, folded)).toEqual(["S2", "S1", "A", "B", "N1", "C"]);
    expect(shifted(["N1"], 1, new Set(["S2"]))).toEqual(["S1", "A", "B", "S2", "N1", "C"]);
  });

  it("move whole sections past whole sections", () => {
    const list = items();
    const g = { ...guide(), items: list };
    const shifted = (block: string[], by: -1 | 1) =>
      ids(applyPatches(g, moveItems(g, block, shiftTarget(list, block, by)!).forward).items);
    expect(shifted(["S1", "A", "B", "N1"], 1)).toEqual(["S2", "C", "S1", "A", "B", "N1"]);
    expect(shifted(["S2", "C"], -1)).toEqual(["S2", "C", "S1", "A", "B", "N1"]);
    expect(shiftTarget(list, ["S2", "C"], 1)).toBeNull();
    expect(shiftTarget(list, ["S1", "A", "B", "N1"], -1)).toBeNull();
    // Past items outside any section one at a time.
    const loose = [step("X"), ...list];
    const h = { ...guide(), items: loose };
    expect(ids(applyPatches(h, moveItems(h, ["S1", "A", "B", "N1"], shiftTarget(loose, ["S1", "A", "B", "N1"], -1)!).forward).items)).toEqual(
      ["S1", "A", "B", "N1", "X", "S2", "C"],
    );
  });

  it("drop before the indicated item, and at the end", () => {
    const list = items();
    const g = { ...guide(), items: list };
    const drop = (block: string[], before: number) =>
      ids(applyPatches(g, moveItems(g, block, blockDropTarget(list, block, before)).forward).items);
    expect(drop(["A", "B"], 0)).toEqual(["A", "B", "S1", "N1", "S2", "C"]);
    expect(drop(["A", "B"], 5)).toEqual(["S1", "N1", "S2", "A", "B", "C"]);
    expect(drop(["A", "B"], 6)).toEqual(["S1", "N1", "S2", "C", "A", "B"]);
    // Onto itself: nowhere.
    expect(drop(["A", "B"], 2)).toEqual(ids(list));
    expect(drop(["C"], 1)).toEqual(["S1", "C", "A", "B", "N1", "S2"]);
  });

  it("merge only two or more steps in a row", () => {
    expect(canMergeRange(items(), ["A", "B"])).toBe(true);
    expect(canMergeRange(items(), ["A", "B", "N1"])).toBe(false);
    expect(canMergeRange(items(), ["A"])).toBe(false);
    expect(canMergeRange(items(), ["A", "N1"])).toBe(false);
    expect(canMergeRange(items(), ["X", "Y"])).toBe(false);
  });
});

describe("outlineKeyFor", () => {
  it("maps arrows to selecting, extending and moving", () => {
    expect(outlineKeyFor(key("ArrowUp"))).toBe("previous");
    expect(outlineKeyFor(key("ArrowDown"))).toBe("next");
    expect(outlineKeyFor(key("ArrowUp", { shiftKey: true }))).toBe("extendUp");
    expect(outlineKeyFor(key("ArrowDown", { shiftKey: true }))).toBe("extendDown");
    expect(outlineKeyFor(key("ArrowUp", { altKey: true }))).toBe("moveUp");
    expect(outlineKeyFor(key("ArrowDown", { altKey: true }))).toBe("moveDown");
    expect(outlineKeyFor(key("ArrowLeft"))).toBe("collapse");
    expect(outlineKeyFor(key("ArrowRight"))).toBe("expand");
  });

  it("maps Home, End, Enter and Delete", () => {
    expect(outlineKeyFor(key("Home"))).toBe("first");
    expect(outlineKeyFor(key("End"))).toBe("last");
    expect(outlineKeyFor(key("Enter"))).toBe("edit");
    expect(outlineKeyFor(key("Delete"))).toBe("delete");
    expect(outlineKeyFor(key("Backspace"))).toBe("delete");
  });

  it("leaves other keys and Cmd or Ctrl alone", () => {
    expect(outlineKeyFor(key("ArrowUp", { metaKey: true }))).toBeNull();
    expect(outlineKeyFor(key("ArrowDown", { ctrlKey: true }))).toBeNull();
    expect(outlineKeyFor(key("ArrowUp", { altKey: true, shiftKey: true }))).toBeNull();
    expect(outlineKeyFor(key("Enter", { shiftKey: true }))).toBeNull();
    expect(outlineKeyFor(key("j"))).toBeNull();
  });
});

describe("announcements", () => {
  it("say what is selected", () => {
    expect(selectionAnnouncement(items(), "B", 1, false)).toBe("Step 2 of 3 selected");
    expect(selectionAnnouncement(items(), "C", 1, true)).toBe("Page 3 of 3 selected");
    expect(selectionAnnouncement(items(), "S2", 1, false)).toBe('Section "Finish" selected');
    expect(selectionAnnouncement(items(), "N1", 1, false)).toBe("Tip selected");
    expect(selectionAnnouncement(items(), null, 0, false)).toBe("Guide details selected");
    expect(selectionAnnouncement(items(), "B", 3, false)).toBe("3 items selected");
    expect(selectionAnnouncement(items(), "X", 1, false)).toBe("");
  });

  it("say where items moved", () => {
    expect(moveAnnouncement(items(), ["B"])).toBe("Moved to position 3 of 6");
    expect(moveAnnouncement(items(), ["A", "B"])).toBe("Moved 2 items to position 2 of 6");
    expect(moveAnnouncement(items(), ["X"])).toBe("");
  });
});
