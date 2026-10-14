import { describe, expect, it } from "vitest";
import {
  applyPatch,
  applyPatches,
  coalesce,
  deleteItem,
  insertItemAfter,
  insertItemBefore,
  mergeWithNext,
  newNote,
  newSection,
  splitStep,
  dropTarget,
  moveItem,
  PatchError,
  renameGuide,
  setImageShow,
  setItemText,
  setItemTitle,
  type Command,
} from "@/editor/commands";
import { guide, step } from "@/editor/test-fixtures";
import type { Step } from "@/lib/types";

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

  it("edit step descriptions and note bodies, marking steps edited", () => {
    const after = roundTrip(setItemText(guide(), "A", "description", "Then **save**."));
    expect(after.items[1]).toMatchObject({ description: "Then **save**.", aiState: { status: "edited" } });
    expect(roundTrip(setItemText(guide(), "N1", "body", "- a")).items[3]).toMatchObject({ body: "- a" });
    expect(() => setItemText(guide(), "S1", "description", "x")).toThrow(PatchError);
    expect(() => setItemText(guide(), "A", "body", "x")).toThrow(PatchError);
  });

  it("keep the edited status when typing coalesces", () => {
    const g = guide();
    const first = setItemText(g, "A", "description", "a");
    const second = setItemText(applyPatches(g, first.forward), "A", "description", "ab");
    const merged = coalesce(first, second)!;
    const after = applyPatches(g, merged.forward);
    expect(after.items[1]).toMatchObject({ description: "ab", aiState: { status: "edited" } });
    expect(applyPatches(after, merged.inverse)).toEqual(g);
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

describe("item operations", () => {
  it("insert sections before and notes after the selection, or at the ends", () => {
    let after = roundTrip(insertItemBefore(guide(), newSection("S2"), "B"));
    expect(ids(after)).toEqual(["S1", "A", "S2", "B", "N1", "C"]);
    after = roundTrip(insertItemBefore(guide(), newSection("S2"), null));
    expect(ids(after)[0]).toBe("S2");
    after = roundTrip(insertItemAfter(guide(), newNote("T1", "warning"), "B"));
    expect(ids(after)).toEqual(["S1", "A", "B", "T1", "N1", "C"]);
    expect(after.items[3]).toEqual({ type: "note", id: "T1", kind: "warning", body: "" });
    after = roundTrip(insertItemAfter(guide(), newNote("T1", "tip"), null));
    expect(ids(after).at(-1)).toBe("T1");

    const command = insertItemAfter(guide(), newNote("T1", "tip"), "B");
    expect(command.label).toBe("Insert tip");
    expect(command.select).toEqual({ forward: "T1", inverse: "B" });
    expect(() => insertItemAfter(guide(), step("A"), null)).toThrow(PatchError);
  });

  it("delete any item and restore it in place", () => {
    for (const id of ["S1", "A", "N1", "C"]) {
      const g = guide();
      const after = roundTrip(deleteItem(g, id));
      expect(ids(after)).toEqual(ids(g).filter((other) => other !== id));
    }
    expect(deleteItem(guide(), "B").select).toEqual({ forward: "N1", inverse: "B" });
    expect(deleteItem(guide(), "C").select).toEqual({ forward: "N1", inverse: "C" });
  });

  it("merge a step with the next step", () => {
    const g = guide();
    (g.items[1] as Step).description = "First *part*.";
    (g.items[2] as Step).title = "Click **OK**";
    (g.items[2] as Step).description = "- one\n- two";
    const command = mergeWithNext(g, "A");
    const after = applyPatches(g, command.forward);
    expect(ids(after)).toEqual(["S1", "A", "N1", "C"]);
    expect(after.items[1]).toMatchObject({
      title: "Step A",
      description: "First *part*.\n\nClick **OK**\n- one\n- two",
      image: g.items[1].type === "step" ? g.items[1].image : undefined,
      aiState: { status: "edited" },
    });
    expect(applyPatches(after, command.inverse)).toEqual(g);
    // Empty parts are skipped.
    expect(applyPatches(guide(), mergeWithNext(guide(), "A").forward).items[1]).toMatchObject({
      description: "Step B",
    });
  });

  it("merge keeps the first step's data and joins keystrokes", () => {
    const g = guide();
    const [first, second] = [g.items[1] as Step, g.items[2] as Step];
    Object.assign(first, {
      kind: "type",
      keystrokes: "staging",
      element: { role: "AXTextField", label: "Env" },
      point: { x: 1, y: 2 },
      annotations: [{ type: "box" }],
    });
    Object.assign(second, {
      kind: "shortcut",
      keystrokes: "⏎",
      element: { role: "AXButton", label: "OK" },
      point: { x: 9, y: 9 },
      endPoint: { x: 10, y: 10 },
      annotations: [{ type: "arrow" }],
    });
    const command = mergeWithNext(g, "A");
    const merged = applyPatches(g, command.forward).items[1] as Step;
    expect(merged).toMatchObject({
      kind: "type",
      keystrokes: "staging ⏎",
      element: { role: "AXTextField", label: "Env" },
      point: { x: 1, y: 2 },
      image: first.image,
      annotations: [{ type: "box" }],
    });
    expect(merged.endPoint).toBeUndefined();
    expect(applyPatches(applyPatches(g, command.forward), command.inverse)).toEqual(g);

    // Only one side has keystrokes: those are kept.
    const h = guide();
    (h.items[2] as Step).keystrokes = "⌘S";
    expect((applyPatches(h, mergeWithNext(h, "A").forward).items[1] as Step).keystrokes).toBe("⌘S");
    const k = guide();
    (k.items[1] as Step).keystrokes = "abc";
    const keep = mergeWithNext(k, "A");
    expect(keep.forward.some((p) => p.op === "setItem" && p.key === "keystrokes")).toBe(false);
    expect((applyPatches(k, keep.forward).items[1] as Step).keystrokes).toBe("abc");
    // Neither: no keystrokes field appears.
    expect(applyPatches(guide(), mergeWithNext(guide(), "A").forward).items[1]).not.toHaveProperty("keystrokes");
  });

  it("merge only into a step that has a step after it", () => {
    expect(() => mergeWithNext(guide(), "B")).toThrow(PatchError); // a note follows
    expect(() => mergeWithNext(guide(), "C")).toThrow(PatchError); // last
    expect(() => mergeWithNext(guide(), "S1")).toThrow(PatchError); // not a step
  });

  it("split a step, sharing its screenshot", () => {
    const g = guide();
    (g.items[1] as Step).description = "Before.\n\nAfter.";
    (g.items[1] as Step).annotations = [{ type: "arrow" }];
    const command = splitStep(g, "A", "A2", "Before.", "After.");
    const after = applyPatches(g, command.forward);
    expect(ids(after)).toEqual(["S1", "A", "A2", "B", "N1", "C"]);
    expect(after.items[1]).toMatchObject({ description: "Before.", aiState: { status: "edited" } });
    expect(after.items[2]).toEqual({
      type: "step",
      id: "A2",
      title: "",
      description: "After.",
      kind: "manual",
      image: (g.items[1] as Step).image,
      annotations: [],
      aiState: { status: "edited" },
    });
    expect(applyPatches(after, command.inverse)).toEqual(g);
    expect(command.select).toEqual({ forward: "A2", inverse: "A" });
  });

  it("split at the end adds an empty step and leaves the first alone", () => {
    const g = guide();
    const command = splitStep(g, "A", "A2", "", "");
    expect(command.forward).toHaveLength(1);
    const after = roundTrip(command);
    expect(after.items[1]).toEqual(g.items[1]);
    expect(after.items[2]).toMatchObject({ id: "A2", aiState: { status: "template" } });
    expect(() => splitStep(g, "N1", "X", "", "")).toThrow(PatchError);
    expect(() => splitStep(g, "A", "B", "", "")).toThrow(PatchError);
  });
});
