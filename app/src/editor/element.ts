// Element confidence (#15): whether a step's recorded element is clear
// enough for an agent skill or the replay script to find it again. The
// same rules as core `confidence.rs`; keep the two in step. Pure, so the
// inspector and the export dialog agree.

import type { Command } from "@/editor/commands";
import { findStep } from "@/editor/commands";
import type { ElementInfo, Guide, Item, Step, StepKind } from "@guiderecorder/guide-schema";

/** Why a step's element is unclear (core `ElementIssue`). */
export type ElementIssue = "noElement" | "noApp" | "noWindow" | "noLabel" | "noRole" | "genericRole";

/** Container and unknown roles, which say nothing about the element (core `GENERIC_ROLES`). */
export const GENERIC_ROLES = [
  "AXGroup",
  "AXUnknown",
  "AXScrollArea",
  "AXSplitGroup",
  "AXLayoutArea",
  "AXLayoutItem",
  "AXWebArea",
  "AXWindow",
  "AXApplication",
];

/** Roles of elements outside any window (core `WINDOWLESS_ROLES`). */
export const WINDOWLESS_ROLES = ["AXDockItem", "AXMenuBarItem", "AXMenuItem"];

const ISSUE_TEXT: Record<ElementIssue, string> = {
  noElement: "no element was recorded",
  noApp: "no app",
  noWindow: "no window",
  noLabel: "no label",
  noRole: "no role",
  genericRole: "a generic role",
};

/** Clicks, drags and typing act on one element; shortcuts and manual steps don't. */
export function needsElement(kind: StepKind): boolean {
  return kind !== "shortcut" && kind !== "manual";
}

function isSet(value: string | null | undefined): boolean {
  return !!value?.trim();
}

/** Why `step`'s element is unclear, in core's order; empty when it is clear or not needed. */
export function elementIssues(step: Step): ElementIssue[] {
  if (!needsElement(step.kind)) return [];
  const element: ElementInfo | null | undefined = step.element;
  if (!element) return ["noElement"];
  const role = element.role?.trim() || null;
  const issues: ElementIssue[] = [];
  if (!isSet(element.app) && !isSet(element.bundleId)) issues.push("noApp");
  if (!isSet(element.window) && !(role && WINDOWLESS_ROLES.includes(role))) issues.push("noWindow");
  if (!isSet(element.label)) issues.push("noLabel");
  if (!role) issues.push("noRole");
  else if (GENERIC_ROLES.includes(role)) issues.push("genericRole");
  return issues;
}

/** `["noLabel", "genericRole"]` as "no label and a generic role". */
export function describeIssues(issues: ElementIssue[]): string {
  const words = issues.map((i) => ISSUE_TEXT[i]);
  if (words.length <= 1) return words[0] ?? "";
  return `${words.slice(0, -1).join(", ")} and ${words.at(-1)}`;
}

/** The steps whose element is unclear: their ids and numbers. */
export function unclearSteps(items: Item[]): { stepIds: string[]; steps: number[] } {
  const unclear = { stepIds: [] as string[], steps: [] as number[] };
  items
    .filter((item): item is Step => item.type === "step")
    .forEach((step, i) => {
      if (elementIssues(step).length === 0) return;
      unclear.stepIds.push(step.id);
      unclear.steps.push(i + 1);
    });
  return unclear;
}

/** The export dialog's note about unclear elements; null when there are none. */
export function unclearStepsText(steps: number[]): string | null {
  if (!steps.length) return null;
  const list =
    steps.length === 1
      ? `step ${steps[0]}`
      : steps.length > 5
        ? `steps ${steps.slice(0, 4).join(", ")} and ${steps.length - 4} more`
        : `steps ${steps.slice(0, -1).join(", ")} and ${steps.at(-1)}`;
  const one = steps.length === 1;
  return `The element of ${list} ${one ? "was" : "were"} not identified clearly, so agents and the replay script may not find ${one ? "it" : "them"}. Name ${one ? "it" : "them"} in the inspector.`;
}

/**
 * Names the step's element: sets its label as typed (exporters trim it; a
 * blank name clears it). A step without an element gets one with just the
 * label. Typing into one step's name is one undo step.
 */
export function setElementLabel(guide: Guide, id: string, label: string): Command {
  const step = findStep(guide, id);
  const element: ElementInfo = { ...(step.element ?? {}) };
  if (label.trim()) element.label = label;
  else delete element.label;
  return {
    label: "Name element",
    forward: [{ op: "setItem", id, key: "element", value: element }],
    inverse: [{ op: "setItem", id, key: "element", value: step.element }],
    coalesceKey: `element-label:${id}`,
  };
}
