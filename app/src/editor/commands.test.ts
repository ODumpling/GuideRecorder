import { describe, expect, it } from "vitest";
import {
  applyPatch,
  applyPatches,
  coalesce,
  deleteItem,
  deleteItems,
  insertItemAfter,
  insertItemBefore,
  insertItemsAfter,
  mergeSteps,
  mergeWithNext,
  moveItems,
  newNote,
  newSection,
  splitStep,
  dropTarget,
  moveItem,
  PatchError,
  renameGuide,
  setGuideField,
  setReviewField,
  editedGuideFields,
  setImageShow,
  setCover,
  setCoverOptions,
  coverOptions,
  coverStepId,
  setItemText,
  setItemTitle,
  type Command,
} from "@/editor/commands";
import { guide, step } from "@/editor/test-fixtures";
import type { Annotation, Step } from "@guiderecorder/guide-schema";

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

  it("insert several items as one undo step", () => {
    const pages = [step("P1"), step("P2"), step("P3")];
    let after = roundTrip(insertItemsAfter(guide(), pages, "A", "Add 3 pages"));
    expect(ids(after)).toEqual(["S1", "A", "P1", "P2", "P3", "B", "N1", "C"]);
    after = roundTrip(insertItemsAfter(guide(), pages, null, "Add 3 pages"));
    expect(ids(after).slice(-3)).toEqual(["P1", "P2", "P3"]);

    const command = insertItemsAfter(guide(), pages, "B", "Add 3 pages");
    expect(command.label).toBe("Add 3 pages");
    expect(command.select).toEqual({ forward: "P1", inverse: "B" });
    expect(() => insertItemsAfter(guide(), [step("P1"), step("A")], null, "x")).toThrow(PatchError);
    expect(() => insertItemsAfter(guide(), [step("P1"), step("P1")], null, "x")).toThrow(PatchError);
    expect(() => insertItemsAfter(guide(), [], null, "x")).toThrow(PatchError);
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
      annotations: [{ id: "N1", type: "box", geometry: { x: 1, y: 2, width: 3, height: 4 } }],
    });
    Object.assign(second, {
      kind: "shortcut",
      keystrokes: "⏎",
      element: { role: "AXButton", label: "OK" },
      point: { x: 9, y: 9 },
      endPoint: { x: 10, y: 10 },
      annotations: [{ id: "N2", type: "arrow", geometry: { from: { x: 0, y: 0 }, to: { x: 5, y: 5 } } }],
    });
    const command = mergeWithNext(g, "A");
    const merged = applyPatches(g, command.forward).items[1] as Step;
    expect(merged).toMatchObject({
      kind: "type",
      keystrokes: "staging ⏎",
      element: { role: "AXTextField", label: "Env" },
      point: { x: 1, y: 2 },
      image: first.image,
      annotations: [{ id: "N1", type: "box", geometry: { x: 1, y: 2, width: 3, height: 4 } }],
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
    (g.items[1] as Step).annotations = [{ id: "N2", type: "arrow", geometry: { from: { x: 0, y: 0 }, to: { x: 5, y: 5 } } }];
    const command = splitStep(g, "A", "A2", "Before.", "After.", undefined, () => "N3");
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
      annotations: [{ id: "N3", type: "arrow", geometry: { from: { x: 0, y: 0 }, to: { x: 5, y: 5 } } }],
      aiState: { status: "edited" },
    });
    expect(applyPatches(after, command.inverse)).toEqual(g);
    expect(command.select).toEqual({ forward: "A2", inverse: "A" });
  });

  it("split keeps every redaction on both halves", () => {
    const g = guide();
    const annotations: Annotation[] = [
      { id: "R1", type: "blur", geometry: { x: 10, y: 10, width: 80, height: 20 } },
      { id: "R2", type: "blur", geometry: { x: 10, y: 40, width: 80, height: 20 }, style: { blurMode: "pixelate", blurAmount: 12 } },
      { id: "K1", type: "crop", geometry: { x: 0, y: 0, width: 300, height: 200 } },
      { id: "C1", type: "callout", geometry: { x: 5, y: 5, width: 50, height: 20, text: "Hi", anchor: { x: 1, y: 1 } } },
    ];
    (g.items[1] as Step).annotations = annotations;
    let n = 0;
    const after = applyPatches(g, splitStep(g, "A", "A2", "", "", undefined, () => `NEW${++n}`).forward);
    const [first, second] = [after.items[1] as Step, after.items[2] as Step];
    expect(first.annotations).toEqual(annotations);
    // Same shapes and styles, own ids.
    expect(second.annotations.map(({ id, ...rest }) => rest)).toEqual(annotations.map(({ id, ...rest }) => rest));
    expect(second.annotations.map((a) => a.id)).toEqual(["NEW1", "NEW2", "NEW3", "NEW4"]);
    // Copies, not shared objects: editing one half leaves the other alone.
    expect(second.annotations[3].geometry).not.toBe(first.annotations[3].geometry);
    // Default ids are fresh ULIDs.
    const auto = applyPatches(g, splitStep(g, "A", "A3", "", "").forward).items[2] as Step;
    expect(new Set(auto.annotations.map((a) => a.id)).size).toBe(4);
    expect(auto.annotations.every((a) => /^[0-9A-HJKMNP-TV-Z]{26}$/.test(a.id))).toBe(true);
  });

  it("undo split restores the original annotations", () => {
    const g = guide();
    (g.items[1] as Step).annotations = [{ id: "R1", type: "blur", geometry: { x: 1, y: 2, width: 3, height: 4 } }];
    const command = splitStep(g, "A", "A2", "x", "y");
    const after = applyPatches(g, command.forward);
    expect(applyPatches(after, command.inverse)).toEqual(g);
    expect(applyPatches(applyPatches(after, command.inverse), command.forward)).toEqual(after);
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

  it("move several items together, both ways", () => {
    // S1 A B N1 C
    expect(ids(roundTrip(moveItems(guide(), ["A", "B"], 0)))).toEqual(["A", "B", "S1", "N1", "C"]);
    expect(ids(roundTrip(moveItems(guide(), ["A", "B"], 3)))).toEqual(["S1", "N1", "C", "A", "B"]);
    // Kept in guide order, whatever order the ids come in.
    expect(ids(roundTrip(moveItems(guide(), ["N1", "A"], 1)))).toEqual(["S1", "A", "N1", "B", "C"]);
    expect(moveItems(guide(), ["A"], 2).label).toBe("Move item");
    expect(moveItems(guide(), ["A", "B"], 2).label).toBe("Move 2 items");
    // Where they already are: nothing to do.
    expect(moveItems(guide(), ["A", "B"], 1).forward).toEqual([]);
    expect(() => moveItems(guide(), ["A", "X"], 0)).toThrow(PatchError);
    expect(() => moveItems(guide(), ["A", "B"], 4)).toThrow(PatchError);
  });

  it("delete several items as one step and restore them in place", () => {
    const command = deleteItems(guide(), ["B", "A", "N1"]);
    expect(ids(roundTrip(command))).toEqual(["S1", "C"]);
    expect(command.label).toBe("Delete 3 items");
    expect(command.select).toEqual({ forward: "C", inverse: "A" });
    // At the end, the item before them is selected.
    expect(deleteItems(guide(), ["N1", "C"]).select).toEqual({ forward: "B", inverse: "N1" });
    expect(deleteItems(guide(), ids(guide())).select?.forward).toBeNull();
    // One item: the same as deleteItem.
    expect(deleteItems(guide(), ["A"])).toEqual(deleteItem(guide(), "A"));
    expect(() => deleteItems(guide(), ["A", "X"])).toThrow(PatchError);
  });

  it("merge a range of steps into the first, as one undo step", () => {
    const g = guide();
    (g.items[4] as Step).keystrokes = "⌘S";
    // S1 A B N1 C: move C up so A B C are consecutive.
    const moved = applyPatches(g, moveItem(g, "C", 3).forward);
    const command = mergeSteps(moved, ["A", "B", "C"]);
    const after = applyPatches(moved, command.forward);
    expect(ids(after)).toEqual(["S1", "A", "N1"]);
    expect(after.items[1]).toMatchObject({ description: "Step B\n\nStep C", keystrokes: "⌘S", aiState: { status: "edited" } });
    expect(applyPatches(after, command.inverse)).toEqual(moved);
    expect(command.label).toBe("Merge 3 steps");
    expect(command.select).toEqual({ forward: "A", inverse: "A" });
    // Only consecutive steps.
    expect(() => mergeSteps(guide(), ["A", "C"])).toThrow(PatchError);
    expect(() => mergeSteps(guide(), ["B", "N1"])).toThrow(PatchError);
    expect(() => mergeSteps(guide(), ["A"])).toThrow(PatchError);
  });
});

describe("guide-level fields", () => {
  it("mark the guide edited when the title, intro, prerequisites, summary or tags change", () => {
    const edits = [
      renameGuide(guide(), "New"),
      setGuideField(guide(), "intro", "Before you start…"),
      setGuideField(guide(), "prerequisites", "- An account"),
      setGuideField(guide(), "summary", "Shares an event."),
      setGuideField(guide(), "tags", ["calendar"]),
    ];
    for (const command of edits) {
      const after = roundTrip(command);
      expect(after.aiState).toEqual({ status: "edited" });
      expect(after.editedFields).toEqual([command.forward[0].op === "setGuide" && command.forward[0].key]);
    }
    expect(roundTrip(setGuideField(guide(), "tags", ["a", "b"])).tags).toEqual(["a", "b"]);
  });

  it("keep the guide's other AI state, and undo back to it", () => {
    const before = { ...guide(), aiState: { status: "ai" as const, model: "m", promptVersion: "guide.v1" } };
    const command = setGuideField(before, "summary", "Mine");
    const after = applyPatches(before, command.forward);
    expect(after.aiState).toEqual({ status: "edited", model: "m", promptVersion: "guide.v1" });
    expect(applyPatches(after, command.inverse)).toEqual(before);
  });

  it("coalesce typing into one undo step that still marks the guide edited", () => {
    const g0 = guide();
    const first = setGuideField(g0, "intro", "H");
    const g1 = applyPatches(g0, first.forward);
    const merged = coalesce(first, setGuideField(g1, "intro", "Hi"))!;
    const g2 = applyPatches(g0, merged.forward);
    expect(g2).toMatchObject({ intro: "Hi", aiState: { status: "edited" } });
    expect(applyPatches(g2, merged.inverse)).toEqual(g0);
  });

  it("set review fields without marking anything edited, and unset empty ones", () => {
    const after = roundTrip(setReviewField(guide(), "owner", "IT"));
    expect(after.owner).toBe("IT");
    expect(after.aiState).toEqual(guide().aiState);
    expect(after.editedFields).toEqual(guide().editedFields);
    expect(roundTrip(setReviewField(guide(), "status", "needsUpdate")).status).toBe("needsUpdate");
    expect(roundTrip(setReviewField(guide(), "reviewIntervalMonths", 6)).reviewIntervalMonths).toBe(6);
    const g1 = { ...guide(), owner: "IT", reviewIntervalMonths: 6 };
    for (const [key, value] of [
      ["owner", ""],
      ["reviewIntervalMonths", 0],
    ] as const) {
      const command = setReviewField(g1, key, value);
      const g2 = applyPatches(g1, command.forward);
      expect(key in JSON.parse(JSON.stringify(g2))).toBe(false);
      expect(applyPatches(g2, command.inverse)).toEqual(g1);
    }
  });

  it("add each edited field to the others, in field order", () => {
    const g0 = guide();
    const g1 = applyPatches(g0, setGuideField(g0, "tags", ["x"]).forward);
    const g2 = applyPatches(g1, setGuideField(g1, "intro", "Hi").forward);
    expect(g2.editedFields).toEqual(["intro", "tags"]);
    expect(editedGuideFields(g2)).toEqual(["intro", "tags"]);
    expect(editedGuideFields(g0)).toEqual([]);
  });

  it("treat an older edited guide as having every field edited", () => {
    const legacy = { ...guide(), aiState: { status: "edited" as const } };
    expect(editedGuideFields(legacy)).toEqual(["title", "intro", "prerequisites", "summary", "tags"]);
    const command = setGuideField(legacy, "summary", "Mine");
    const after = applyPatches(legacy, command.forward);
    expect(after.editedFields).toEqual(["title", "intro", "prerequisites", "summary", "tags"]);
    expect(applyPatches(after, command.inverse)).toEqual(legacy);
  });

  it("change nothing, not even the AI state, when the value is the same", () => {
    const command = setGuideField(guide(), "title", "Guide");
    expect(command.forward).toEqual(command.inverse);
  });
});

describe("cover", () => {
  it("is set, changed and cleared undoably", () => {
    const g0 = guide();
    expect(coverStepId(g0)).toBeNull();
    expect(coverOptions(g0)).toEqual({ image: "full", annotations: false });
    const g1 = roundTrip(setCover(g0, "B"));
    expect(coverStepId(g1)).toBe("B");
    expect(setCover(g1, "B").forward).toEqual(setCover(g1, "B").inverse);
    const options = setCoverOptions(g1, { image: "crop" });
    const g2 = applyPatches(g1, options.forward);
    expect(coverOptions(g2)).toEqual({ image: "crop", annotations: false });
    expect(applyPatches(g2, options.inverse)).toEqual(g1);
    const cleared = setCover(g2, null);
    const g3 = applyPatches(g2, cleared.forward);
    expect("coverStepId" in JSON.parse(JSON.stringify(g3))).toBe(false);
    expect(applyPatches(g3, cleared.inverse)).toEqual(g2);
  });

  it("is only a step with a screenshot", () => {
    const g = guide();
    expect(() => setCover(g, "S1")).toThrow(PatchError);
    expect(coverStepId({ ...g, coverStepId: "S1" })).toBeNull();
    expect(coverStepId({ ...g, coverStepId: "gone" })).toBeNull();
    const items = g.items.map((item) => (item.id === "A" && item.type === "step" ? { ...item, image: { ...item.image, full: "" } } : item));
    expect(coverStepId({ ...g, coverStepId: "A", items })).toBeNull();
  });

  it("goes with its step, and comes back on undo", () => {
    const g = { ...guide(), coverStepId: "B" };
    const deleted = deleteItem(g, "B");
    const after = applyPatches(g, deleted.forward);
    expect(after.coverStepId).toBeUndefined();
    expect(applyPatches(after, deleted.inverse)).toEqual(g);
    // Other deletes leave it alone.
    expect(applyPatches(g, deleteItem(g, "C").forward).coverStepId).toBe("B");

    // Merging removes the next step, the cover here.
    const base = { ...g, coverStepId: "C", items: g.items.filter((item) => item.id !== "N1") };
    const merged = mergeWithNext(base, "B");
    expect(applyPatches(base, merged.forward).coverStepId).toBeUndefined();
    expect(applyPatches(applyPatches(base, merged.forward), merged.inverse)).toEqual(base);

    // So does deleting a range that holds it.
    const range = deleteItems(g, ["A", "B"]);
    expect(applyPatches(g, range.forward).coverStepId).toBeUndefined();
    expect(applyPatches(applyPatches(g, range.forward), range.inverse)).toEqual(g);
    expect(applyPatches(g, deleteItems(g, ["A", "C"]).forward).coverStepId).toBe("B");
  });
});
