import { describe, expect, it } from "vitest";
import { applyPatches, coalesce, findStep } from "@/editor/commands";
import {
  describeIssues,
  elementIssues,
  GENERIC_ROLES,
  needsElement,
  setElementLabel,
  unclearSteps,
  unclearStepsText,
  WINDOWLESS_ROLES,
} from "@/editor/element";
import { guide, step } from "@/editor/test-fixtures";
import type { ElementInfo, Step } from "@guiderecorder/guide-schema";

const button: ElementInfo = { role: "AXButton", label: "Save", app: "TextEdit", window: "Untitled" };

function withElement(kind: Step["kind"], element?: ElementInfo | null): Step {
  return { ...step("A"), kind, element };
}

describe("element confidence", () => {
  it("finds a labelled control in a window clear", () => {
    for (const kind of ["click", "doubleClick", "rightClick", "drag", "type"] as const) {
      expect(elementIssues(withElement(kind, button))).toEqual([]);
    }
  });

  it("never flags shortcuts and manual steps", () => {
    expect(needsElement("shortcut")).toBe(false);
    expect(elementIssues(withElement("shortcut"))).toEqual([]);
    expect(elementIssues(withElement("manual", {}))).toEqual([]);
  });

  it("flags a missing element, then each missing or vague field in core's order", () => {
    expect(elementIssues(withElement("click"))).toEqual(["noElement"]);
    expect(elementIssues(withElement("click", null))).toEqual(["noElement"]);
    expect(elementIssues(withElement("click", {}))).toEqual(["noApp", "noWindow", "noLabel", "noRole"]);
    expect(elementIssues(withElement("click", { ...button, role: "AXGroup", label: "  " }))).toEqual([
      "noLabel",
      "genericRole",
    ]);
    expect(elementIssues(withElement("drag", { ...button, app: null, bundleId: "com.apple.Notes" }))).toEqual([]);
    expect(GENERIC_ROLES).toContain("AXUnknown");
  });

  it("needs no window for Dock and menu items", () => {
    for (const role of WINDOWLESS_ROLES) {
      expect(elementIssues(withElement("click", { role, label: "Calendar", app: "Dock" }))).toEqual([]);
    }
    expect(elementIssues(withElement("click", { role: "AXButton", label: "Calendar", app: "Dock" }))).toEqual([
      "noWindow",
    ]);
  });

  it("describes issues in words", () => {
    expect(describeIssues([])).toBe("");
    expect(describeIssues(["noLabel"])).toBe("no label");
    expect(describeIssues(["noApp", "noWindow", "noLabel"])).toBe("no app, no window and no label");
  });

  it("lists the unclear steps with their numbers", () => {
    const g = guide();
    const [a, b, c] = g.items.filter((i): i is Step => i.type === "step");
    a.element = button;
    b.kind = "shortcut";
    c.element = { role: "AXGroup" };
    expect(unclearSteps(g.items)).toEqual({ stepIds: ["C"], steps: [3] });
    expect(unclearStepsText([])).toBeNull();
    expect(unclearStepsText([3])).toBe(
      "The element of step 3 was not identified clearly, so agents and the replay script may not find it. Name it in the inspector.",
    );
    expect(unclearStepsText([1, 2, 4])).toMatch(/^The element of steps 1, 2 and 4 were .* find them\. Name them/);
    expect(unclearStepsText([1, 2, 3, 4, 5, 6, 7])).toContain("steps 1, 2, 3, 4 and 3 more");
  });

  it("names an element, undoably, and creates one if needed", () => {
    const g = guide();
    const named = setElementLabel(g, "A", "Save");
    const after = applyPatches(g, named.forward);
    expect(findStep(after, "A").element).toEqual({ label: "Save" });
    expect(findStep(applyPatches(after, named.inverse), "A").element).toBeUndefined();

    const withButton = applyPatches(g, setElementLabel(g, "A", "x").forward);
    const cleared = setElementLabel(withButton, "A", " ");
    expect(findStep(applyPatches(withButton, cleared.forward), "A").element).toEqual({});

    // Typing coalesces into one undo step.
    const typed = coalesce(setElementLabel(g, "A", "S"), setElementLabel(applyPatches(g, setElementLabel(g, "A", "S").forward), "A", "Sa"));
    expect(typed?.inverse).toEqual(named.inverse);
    expect(findStep(applyPatches(g, typed!.forward), "A").element).toEqual({ label: "Sa" });
  });
});
