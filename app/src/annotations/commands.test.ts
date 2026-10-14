import { describe, expect, it } from "vitest";
import {
  addAnnotation,
  removeAnnotation,
  setAnnotationStyle,
  setCalloutText,
  updateAnnotation,
} from "@/annotations/commands";
import { applyPatches, coalesce, PatchError, type Command } from "@/editor/commands";
import { guide } from "@/editor/test-fixtures";
import type { Annotation, Guide, Step } from "@/lib/types";

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

  it("remove", () => {
    const g = withAnnotations(box, callout);
    const command = removeAnnotation(g, "A", "B1");
    expect(command.label).toBe("Delete box");
    expect(annotationsOf(roundTrip(g, command))).toEqual([callout]);
  });
});
