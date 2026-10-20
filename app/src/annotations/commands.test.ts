import { describe, expect, it } from "vitest";
import {
  acceptBlurSuggestion,
  acceptBlurSuggestions,
  addAnnotation,
  keepBlurSuggestion,
  keepBlurSuggestions,
  moveHotspot,
  removeAnnotation,
  setAnnotationStyle,
  setCalloutText,
  setHotspotText,
  updateAnnotation,
} from "@/annotations/commands";
import { applyPatches, coalesce, PatchError, type Command } from "@/editor/commands";
import { guide } from "@/editor/test-fixtures";
import type { Annotation, Guide, Step } from "@guiderecorder/guide-schema";

const box: Annotation = { id: "B1", type: "box", geometry: { x: 1, y: 2, width: 30, height: 40 } };
const callout: Annotation = { id: "C1", type: "callout", geometry: { x: 0, y: 0, width: 50, height: 20, text: "" } };
const crop = (id: string): Annotation => ({ id, type: "crop", geometry: { x: 0, y: 0, width: 9, height: 9 } });

function withAnnotations(...annotations: Annotation[]): Guide {
  const g = guide();
  (g.items[1] as Step).annotations = annotations;
  return g;
}

const annotationsOf = (g: Guide) => (g.items[1] as Step).annotations;

function roundTrip(g: Guide, command: Command): Guide {
  const after = applyPatches(g, command.forward);
  expect(applyPatches(after, command.inverse)).toEqual(g);
  return after;
}

describe("annotation commands", () => {
  it("add on top, undoably", () => {
    const g = withAnnotations(box);
    expect(annotationsOf(roundTrip(g, addAnnotation(g, "A", callout)))).toEqual([box, callout]);
    expect(addAnnotation(g, "A", callout).label).toBe("Add callout");
    expect(() => addAnnotation(g, "A", box)).toThrow(PatchError);
    expect(() => addAnnotation(g, "S1", box)).toThrow(PatchError);
  });

  it("a new crop replaces older ones", () => {
    const g = withAnnotations(crop("K1"), box);
    expect(annotationsOf(roundTrip(g, addAnnotation(g, "A", crop("K2")))).map((a) => a.id)).toEqual(["B1", "K2"]);
  });

  it("update in place, keeping order", () => {
    const g = withAnnotations(box, callout);
    const moved = { ...box, geometry: { ...box.geometry, x: 100 } };
    expect(annotationsOf(roundTrip(g, updateAnnotation(g, "A", moved)))).toEqual([moved, callout]);
    expect(() => updateAnnotation(g, "A", { ...callout, id: "B1" })).toThrow(PatchError);
    expect(() => updateAnnotation(g, "A", { ...box, id: "nope" })).toThrow(PatchError);
  });

  it("style edits merge per field and can reset to defaults", () => {
    const g = withAnnotations(box);
    const first = setAnnotationStyle(g, "A", "B1", { strokeWidth: 3 });
    const after = roundTrip(g, first);
    expect(annotationsOf(after)[0].style).toEqual({ strokeWidth: 3 });
    const second = setAnnotationStyle(after, "A", "B1", { strokeWidth: 5 });
    expect(coalesce(first, second)).not.toBeNull();
    expect(coalesce(first, setAnnotationStyle(after, "A", "B1", { color: "red" }))).toBeNull();
    const reset = applyPatches(after, setAnnotationStyle(after, "A", "B1", { strokeWidth: undefined }).forward);
    expect(annotationsOf(reset)[0]).toEqual(box);
  });

  it("callout text merges while typing; only for callouts", () => {
    const g = withAnnotations(callout);
    const one = setCalloutText(g, "A", "C1", "H");
    const two = setCalloutText(applyPatches(g, one.forward), "A", "C1", "Hi");
    const merged = coalesce(one, two)!;
    expect(annotationsOf(applyPatches(g, merged.forward))[0]).toMatchObject({ geometry: { text: "Hi" } });
    expect(applyPatches(applyPatches(g, merged.forward), merged.inverse)).toEqual(g);
    expect(() => setCalloutText(withAnnotations(box), "A", "B1", "x")).toThrow(PatchError);
  });

  it("hotspot title and description merge while typing, each on its own; only for hotspots", () => {
    const hotspot: Annotation = {
      id: "H1",
      type: "hotspot",
      geometry: { x: 0, y: 0, width: 50, height: 20, title: "", description: "" },
    };
    const g = withAnnotations(box, hotspot);
    expect(addAnnotation(withAnnotations(box), "A", hotspot).label).toBe("Add hotspot");
    const one = setHotspotText(g, "A", "H1", "title", "K");
    const g1 = applyPatches(g, one.forward);
    const two = setHotspotText(g1, "A", "H1", "title", "Kitchen");
    expect(one.label).toBe("Edit hotspot");
    const merged = coalesce(one, two)!;
    expect(annotationsOf(applyPatches(g, merged.forward))[1]).toEqual({
      ...hotspot,
      geometry: { ...hotspot.geometry, title: "Kitchen" },
    });
    expect(applyPatches(applyPatches(g, merged.forward), merged.inverse)).toEqual(g);
    // The description is another field: its own undo step.
    const g2 = applyPatches(g1, two.forward);
    const description = setHotspotText(g2, "A", "H1", "description", "Gas hob\nNorth window");
    expect(coalesce(two, description)).toBeNull();
    expect(annotationsOf(roundTrip(g2, description))[1]).toMatchObject({
      geometry: { title: "Kitchen", description: "Gas hob\nNorth window" },
    });
    expect(() => setHotspotText(g, "A", "B1", "title", "x")).toThrow(PatchError);
  });

  it("remove", () => {
    const g = withAnnotations(box, callout);
    const command = removeAnnotation(g, "A", "B1");
    expect(command.label).toBe("Delete box");
    expect(annotationsOf(roundTrip(g, command))).toEqual([callout]);
  });
});


describe("blur suggestions", () => {
  const suggestions = [
    { rect: { x: 10, y: 20, width: 30, height: 40 }, reason: "email" },
    { rect: { x: 50, y: 60, width: 70, height: 80 }, reason: "token" },
  ];
  function withSuggestions(): Guide {
    const g = withAnnotations(box);
    (g.items[1] as Step).aiState = { status: "edited", redactionSuggestions: suggestions };
    (g.items[2] as Step).aiState = { status: "ai", redactionSuggestions: [suggestions[0]] };
    return g;
  }
  const stepOf = (g: Guide, i = 1) => g.items[i] as Step;

  it("Accept adds a blur over the suggestion and marks it blurred, undoably", () => {
    const g = withSuggestions();
    const command = acceptBlurSuggestion(g, "A", 1, "BL1");
    const after = roundTrip(g, command);
    expect(stepOf(after).annotations).toEqual([box, { id: "BL1", type: "blur", geometry: suggestions[1].rect }]);
    // The text status stays.
    expect(stepOf(after).aiState).toEqual({
      status: "edited",
      redactionSuggestions: [suggestions[0], { ...suggestions[1], review: "blurred" }],
    });
    expect(command.label).toBe("Accept blur suggestion");
  });

  it("Keep marks the suggestion kept only, undoably", () => {
    const g = withSuggestions();
    const after = roundTrip(g, keepBlurSuggestion(g, "A", 0));
    expect(stepOf(after).annotations).toEqual([box]);
    expect(stepOf(after).aiState.redactionSuggestions).toEqual([{ ...suggestions[0], review: "kept" }, suggestions[1]]);
  });

  it("refuses a suggestion that is not there or already reviewed", () => {
    expect(() => keepBlurSuggestion(withSuggestions(), "A", 2)).toThrow(PatchError);
    expect(() => acceptBlurSuggestion(guide(), "A", 0, "X")).toThrow(PatchError);
    expect(() => acceptBlurSuggestions(guide(), [], [])).toThrow(PatchError);
    expect(() => acceptBlurSuggestions(withSuggestions(), [{ stepId: "A", index: 0 }], [])).toThrow(PatchError);
    const g = withSuggestions();
    const kept = applyPatches(g, keepBlurSuggestion(g, "A", 0).forward);
    expect(() => acceptBlurSuggestion(kept, "A", 0, "X")).toThrow(PatchError);
    expect(() => acceptBlurSuggestion(g, "A", 0, "B1")).toThrow(PatchError);
  });

  it("bulk review spans steps as one undo step", () => {
    const g = withSuggestions();
    const refs = [
      { stepId: "A", index: 0 },
      { stepId: "B", index: 0 },
      { stepId: "A", index: 1 },
    ];
    const command = acceptBlurSuggestions(g, refs, ["BL1", "BL2", "BL3"]);
    const after = roundTrip(g, command);
    expect(stepOf(after).annotations).toEqual([
      box,
      { id: "BL1", type: "blur", geometry: suggestions[0].rect },
      { id: "BL3", type: "blur", geometry: suggestions[1].rect },
    ]);
    expect(stepOf(after, 2).annotations).toEqual([{ id: "BL2", type: "blur", geometry: suggestions[0].rect }]);
    expect(stepOf(after).aiState.redactionSuggestions!.every((s) => s.review === "blurred")).toBe(true);
    expect(command.label).toBe("Accept blur suggestions");

    const kept = roundTrip(g, keepBlurSuggestions(g, [{ stepId: "B", index: 0 }]));
    expect(stepOf(kept, 2).aiState.redactionSuggestions).toEqual([{ ...suggestions[0], review: "kept" }]);
    expect(stepOf(kept, 2).annotations).toEqual([]);
  });
});

describe("moveHotspot", () => {
  const hotspot = (id: string): Annotation => ({
    id,
    type: "hotspot",
    geometry: { x: 0, y: 0, width: 5, height: 5, title: id, description: "" },
  });

  it("reorders hotspots among themselves, leaving other annotations in place", () => {
    const g = withAnnotations(hotspot("H1"), box, hotspot("H2"), hotspot("H3"));
    const ids = (c: Command) => annotationsOf(roundTrip(g, c)).map((a) => a.id);
    expect(ids(moveHotspot(g, "A", "H1", 1))).toEqual(["H2", "B1", "H1", "H3"]);
    expect(ids(moveHotspot(g, "A", "H3", -2))).toEqual(["H3", "B1", "H1", "H2"]);
    expect(moveHotspot(g, "A", "H1", 1).label).toBe("Move hotspot");
    expect(() => moveHotspot(g, "A", "H1", -1)).toThrow(PatchError);
    expect(() => moveHotspot(g, "A", "H3", 1)).toThrow(PatchError);
    expect(() => moveHotspot(g, "A", "B1", 1)).toThrow(PatchError);
  });
});
