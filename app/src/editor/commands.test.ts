import { describe, expect, it } from "vitest";
import {
  applyPatch,
  applyPatches,
  coalesce,
  dropTarget,
  moveItem,
  PatchError,
  renameGuide,
  setImageShow,
  setItemTitle,
  type Command,
} from "@/editor/commands";
import { guide, step } from "@/editor/test-fixtures";

const ids = (g: ReturnType<typeof guide>) => g.items.map((item) => item.id);

function roundTrip(command: Command) {
  const before = guide();
  const after = applyPatches(before, command.forward);
  expect(applyPatches(after, command.inverse)).toEqual(before);
  return after;
}

describe("patches", () => {
  it("never modify their input", () => {
    const g = guide();
    const copy = structuredClone(g);
    applyPatch(g, { op: "setItem", id: "A", key: "title", value: "x" });
    applyPatch(g, { op: "moveItem", id: "A", to: 4 });
    applyPatch(g, { op: "removeItem", id: "B" });
    expect(g).toEqual(copy);
  });

  it("insert and remove are inverses", () => {
    const g = guide();
    const inserted = applyPatch(g, { op: "insertItem", index: 2, item: step("D") });
    expect(ids(inserted)).toEqual(["S1", "A", "D", "B", "N1", "C"]);
    expect(applyPatch(inserted, { op: "removeItem", id: "D" })).toEqual(g);
  });

  it("reject unknown ids and out-of-range indexes", () => {
    const g = guide();
    expect(() => applyPatch(g, { op: "setItem", id: "nope", key: "title", value: "" })).toThrow(
      PatchError,
    );
    expect(() => applyPatch(g, { op: "moveItem", id: "A", to: 5 })).toThrow(PatchError);
    expect(() => applyPatch(g, { op: "insertItem", index: 6, item: step("D") })).toThrow(PatchError);
  });
});

describe("commands", () => {
  it("rename the guide", () => {
    expect(roundTrip(renameGuide(guide(), "New")).title).toBe("New");
  });

  it("edit step and section titles, but not notes", () => {
    const after = roundTrip(setItemTitle(guide(), "B", "Click **OK**"));
    expect(after.items[2]).toMatchObject({ id: "B", title: "Click **OK**" });
    expect(roundTrip(setItemTitle(guide(), "S1", "Begin")).items[0]).toMatchObject({
      title: "Begin",
    });
    expect(() => setItemTitle(guide(), "N1", "x")).toThrow(PatchError);
  });

  it("move items both ways", () => {
    expect(ids(roundTrip(moveItem(guide(), "A", 3)))).toEqual(["S1", "B", "N1", "A", "C"]);
    expect(ids(roundTrip(moveItem(guide(), "C", 0)))).toEqual(["C", "S1", "A", "B", "N1"]);
  });

  it("drop before the indicated item, both ways, and at the end", () => {
    const g = guide(); // S1 A B N1 C
    const drop = (id: string, before: number) =>
      ids(applyPatches(g, moveItem(g, id, dropTarget(ids(g).indexOf(id), before)).forward));
    expect(drop("A", 3)).toEqual(["S1", "B", "A", "N1", "C"]);
    expect(drop("A", 5)).toEqual(["S1", "B", "N1", "C", "A"]);
    expect(drop("C", 1)).toEqual(["S1", "C", "A", "B", "N1"]);
    expect(drop("C", 0)).toEqual(["C", "S1", "A", "B", "N1"]);
    // Dropping just before or after itself goes nowhere.
    expect(dropTarget(1, 1)).toBe(1);
    expect(dropTarget(1, 2)).toBe(1);
  });

  it("toggle a step's image", () => {
    const after = roundTrip(setImageShow(guide(), "A", "full"));
    expect(after.items[1]).toMatchObject({ image: { show: "full", crop: "steps/A.crop.webp" } });
  });

  it("coalesce edits of the same field only", () => {
    const g = guide();
    const first = setItemTitle(g, "A", "S");
    const second = setItemTitle(applyPatches(g, first.forward), "A", "Sa");
    const merged = coalesce(first, second)!;
    expect(merged.forward).toEqual(second.forward);
    expect(merged.inverse).toEqual(first.inverse);
    expect(coalesce(first, setItemTitle(g, "B", "x"))).toBeNull();
    expect(coalesce(moveItem(g, "A", 2), moveItem(g, "A", 3))).toBeNull();
  });
});
