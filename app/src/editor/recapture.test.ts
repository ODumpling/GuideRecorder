import { describe, expect, it, vi } from "vitest";
import type { Annotation, Guide, Step } from "@guiderecorder/guide-schema";
import { applyPatches, clickOffset, moveAnnotation, recaptureStep } from "@/editor/commands";
import {
  applyRecorded,
  canRecordInto,
  createRecaptureStore,
  resolveRecapture,
  startInsert,
  startRecapture,
} from "@/editor/recapture";
import { createEditorStore } from "@/editor/store";
import { guide, step } from "@/editor/test-fixtures";
import type { EditorApi } from "@/lib/ipc";
import type { RecordedSteps, RecordInto } from "@/lib/types";
import type { StartFlowStore } from "@/recording/startFlow";

const box = (id: string, x: number, y: number): Annotation => ({
  id,
  type: "box",
  geometry: { x, y, width: 40, height: 20 },
});

/** Step B as the user left it: their text, a point, an element and annotations. */
function edited(): Guide {
  const g = guide();
  const b = g.items[2] as Step;
  Object.assign(b, {
    title: "Click **Save**",
    description: "Saves the draft.",
    narration: "now we save the draft",
    point: { x: 100, y: 100 },
    element: { role: "AXButton", label: "Save" },
    keystrokes: undefined,
    annotations: [box("X1", 80, 90)],
    aiState: {
      status: "edited",
      model: "m",
      redactionSuggestions: [{ rect: { x: 0, y: 0, width: 1, height: 1 }, reason: "email address" }],
    },
  } satisfies Partial<Step>);
  b.image = { ...b.image, show: "full" };
  return g;
}

/** What a re-capture of B recorded: a click 30 px right and 10 px up, on a new screenshot. */
function captured(over: Partial<Step> = {}): Omit<Step, "type"> {
  const { type: _, ...bare } = step("R");
  return {
    ...bare,
    title: "Click **Publish**",
    narration: "and publish it",
    point: { x: 130, y: 90 },
    element: { role: "AXButton", label: "Publish" },
    image: { full: "steps/R.webp", crop: "steps/R.crop.webp", cropRect: { x: 0, y: 0, width: 80, height: 50 }, show: "crop" },
    aiState: { status: "template", scanned: true },
    ...over,
  };
}

const findB = (g: Guide) => g.items.find((item) => item.id === "B") as Step;

describe("recaptureStep", () => {
  it("replaces the capture, keeps the text and the user's choices, and undoes", () => {
    const before = edited();
    const command = recaptureStep(before, "B", captured());
    expect(command.label).toBe("Re-capture step");
    const after = applyPatches(before, command.forward);
    const b = findB(after);
    // The capture is the new one…
    expect(b.point).toEqual({ x: 130, y: 90 });
    expect(b.element).toEqual({ role: "AXButton", label: "Publish" });
    expect(b.image.full).toBe("steps/R.webp");
    expect(b.image.crop).toBe("steps/R.crop.webp");
    // …the text, its provenance and the full-screenshot choice are the user's.
    expect(b.title).toBe("Click **Save**");
    expect(b.description).toBe("Saves the draft.");
    expect(b.narration).toBe("now we save the draft");
    expect(b.image.show).toBe("full");
    expect(b.aiState).toEqual({ status: "edited", model: "m", scanned: true });
    expect(b.annotations).toEqual([box("X1", 80, 90)]);
    expect(b.id).toBe("B");
    // Undo puts every field back.
    expect(applyPatches(after, command.inverse)).toEqual(before);
  });

  it("moves the annotations by the click offset, or clears them", () => {
    const before = edited();
    const moved = findB(applyPatches(before, recaptureStep(before, "B", captured(), "move").forward));
    expect(moved.annotations).toEqual([box("X1", 110, 80)]);
    const cleared = findB(applyPatches(before, recaptureStep(before, "B", captured(), "clear").forward));
    expect(cleared.annotations).toEqual([]);
    // Without a click there is no offset: moving keeps them where they are.
    const noPoint = captured({ point: undefined, kind: "shortcut" });
    const kept = findB(applyPatches(before, recaptureStep(before, "B", noPoint, "move").forward));
    expect(kept.annotations).toEqual([box("X1", 80, 90)]);
    expect(kept.point).toBeUndefined();
    expect(kept.kind).toBe("shortcut");
  });

  it("adds the blurs the recording applied, and shows the full screenshot without a close-up", () => {
    const before = guide();
    const blur: Annotation = { id: "Z1", type: "blur", geometry: { x: 1, y: 2, width: 3, height: 4 } };
    const shot = captured({ annotations: [blur], image: { full: "steps/R.webp", show: "full" } });
    const b = findB(applyPatches(before, recaptureStep(before, "B", shot).forward));
    expect(b.annotations).toEqual([blur]);
    expect(b.image).toEqual({ full: "steps/R.webp", show: "full" });
  });
});

describe("clickOffset and moveAnnotation", () => {
  it("measures the move between two points", () => {
    const b = findB(edited());
    expect(clickOffset(b, { point: { x: 90, y: 130 } })).toEqual({ dx: -10, dy: 30 });
    expect(clickOffset(b, { point: { x: 100, y: 100 } })).toBeNull();
    expect(clickOffset(b, { point: null })).toBeNull();
    expect(clickOffset(step("A"), { point: { x: 1, y: 1 } })).toBeNull();
  });

  it("moves every shape", () => {
    const arrow: Annotation = { id: "1", type: "arrow", geometry: { from: { x: 0, y: 0 }, to: { x: 10, y: 10 } } };
    expect(moveAnnotation(arrow, 5, -5).geometry).toEqual({ from: { x: 5, y: -5 }, to: { x: 15, y: 5 } });
    const callout: Annotation = {
      id: "2",
      type: "callout",
      geometry: { x: 1, y: 1, width: 10, height: 10, text: "Here", anchor: { x: 0, y: 0 } },
    };
    expect(moveAnnotation(callout, 2, 3).geometry).toEqual({
      x: 3,
      y: 4,
      width: 10,
      height: 10,
      text: "Here",
      anchor: { x: 2, y: 3 },
    });
    const hotspot: Annotation = {
      id: "3",
      type: "hotspot",
      geometry: { x: 0, y: 0, width: 5, height: 5, title: "A", description: "" },
      style: { color: "#ff3b30" },
    };
    expect(moveAnnotation(hotspot, 1, 1)).toEqual({ ...hotspot, geometry: { ...hotspot.geometry, x: 1, y: 1 } });
  });
});

async function editor(g: Guide = edited(), readOnly = false) {
  const api = {
    openGuide: async () => ({ guide: g, path: "", readOnly, migratedFrom: null, migrationError: null }),
  } as unknown as EditorApi;
  const s = createEditorStore({ api });
  await s.getState().openGuide("G1");
  return s;
}

const recorded = (into: RecordInto, steps: Omit<Step, "type">[]): RecordedSteps => ({ id: "rec-1", into, steps });
const ids = (s: Awaited<ReturnType<typeof editor>>) => s.getState().doc!.guide.items.map((item) => item.id);

describe("applyRecorded", () => {
  it("inserts the recorded steps after their item, as one undo step", async () => {
    const s = await editor();
    const pending = createRecaptureStore();
    const steps = [captured({ id: "R1" }), captured({ id: "R2" })];
    expect(applyRecorded(s, pending, recorded({ mode: "insert", guideId: "G1", afterId: "A" }, steps))).toBe(true);
    expect(ids(s)).toEqual(["S1", "A", "R1", "R2", "B", "N1", "C"]);
    expect(s.getState().undoStack.at(-1)!.command.label).toBe("Insert 2 recorded steps");
    expect(s.getState().selectedId).toBe("R1");
    s.getState().undo();
    expect(ids(s)).toEqual(["S1", "A", "B", "N1", "C"]);
    // Their item is gone (or none was given): at the end.
    applyRecorded(s, pending, recorded({ mode: "insert", guideId: "G1", afterId: "gone" }, [captured({ id: "R3" })]));
    expect(ids(s).at(-1)).toBe("R3");
  });

  it("re-captures a step without annotations at once", async () => {
    const g = guide();
    const s = await editor(g);
    const pending = createRecaptureStore();
    expect(applyRecorded(s, pending, recorded({ mode: "replace", guideId: "G1", stepId: "B" }, [captured()]))).toBe(true);
    expect(pending.getState().pending).toBeNull();
    expect(findB(s.getState().doc!.guide).image.full).toBe("steps/R.webp");
    s.getState().undo();
    expect(findB(s.getState().doc!.guide)).toEqual(findB(guide()));
  });

  it("asks what happens to the annotations, then re-captures as chosen", async () => {
    const s = await editor();
    const pending = createRecaptureStore();
    applyRecorded(s, pending, recorded({ mode: "replace", guideId: "G1", stepId: "B" }, [captured()]));
    expect(pending.getState().pending).toMatchObject({ stepId: "B", offset: { dx: 30, dy: -10 }, annotations: 1 });
    // Nothing changes until the user chooses.
    expect(findB(s.getState().doc!.guide).image.full).toBe("steps/B.webp");
    expect(resolveRecapture(s, pending, "move")).toBe(true);
    expect(pending.getState().pending).toBeNull();
    const b = findB(s.getState().doc!.guide);
    expect(b.image.full).toBe("steps/R.webp");
    expect(b.annotations).toEqual([box("X1", 110, 80)]);
    s.getState().undo();
    expect(findB(s.getState().doc!.guide)).toEqual(findB(edited()));
  });

  it("says why when the recording no longer applies", async () => {
    const s = await editor();
    const pending = createRecaptureStore();
    const notice = () => s.getState().notice;
    expect(applyRecorded(s, pending, recorded({ mode: "insert", guideId: "other", afterId: null }, [captured()]))).toBe(false);
    expect(notice()).toMatch(/no longer open/);
    expect(applyRecorded(s, pending, recorded({ mode: "replace", guideId: "G1", stepId: "B" }, []))).toBe(false);
    expect(notice()).toMatch(/Nothing was captured/);
    expect(applyRecorded(s, pending, recorded({ mode: "replace", guideId: "G1", stepId: "S1" }, [captured()]))).toBe(false);
    expect(notice()).toMatch(/deleted/);
    // Deleted while the user was choosing.
    applyRecorded(s, pending, recorded({ mode: "replace", guideId: "G1", stepId: "B" }, [captured()]));
    s.getState().select("B");
    s.getState().dispatch({ label: "Delete", forward: [{ op: "removeItem", id: "B" }], inverse: [] });
    expect(resolveRecapture(s, pending, "keep")).toBe(false);
    expect(s.getState().undoStack).toHaveLength(1);
  });
});

describe("starting a recording into the guide", () => {
  it("records on the area the guide was recorded with, into the open guide", async () => {
    const g = edited();
    g.recording = {
      appVersion: "0.2.0",
      apps: [],
      displays: [],
      captureArea: { kind: "region", displayId: 1, rect: { x: 0, y: 0, width: 100, height: 100 } },
    };
    const s = await editor(g);
    const recordInto = vi.fn(async () => {});
    const flow = { getState: () => ({ recordInto }) } as unknown as StartFlowStore;
    expect(canRecordInto(s)).toBe(true);
    await startRecapture(s, flow, "B");
    expect(recordInto).toHaveBeenLastCalledWith({ mode: "replace", guideId: "G1", stepId: "B" }, g.recording.captureArea);
    await startInsert(s, flow, null);
    expect(recordInto).toHaveBeenLastCalledWith({ mode: "insert", guideId: "G1", afterId: null }, g.recording.captureArea);
  });

  it("is not offered for read-only or annotation guides", async () => {
    expect(canRecordInto(await editor(edited(), true))).toBe(false);
    expect(canRecordInto(await editor({ ...guide(), kind: "annotated" }))).toBe(false);
    const recordInto = vi.fn();
    const flow = { getState: () => ({ recordInto }) } as unknown as StartFlowStore;
    await startRecapture(await editor(edited(), true), flow, "B");
    expect(recordInto).not.toHaveBeenCalled();
  });
});
