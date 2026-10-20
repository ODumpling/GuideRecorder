// The inspector's privacy review (#36): the blur suggestions of a guide as
// findings, what is left to review, and which steps were scanned on this
// Mac. Pure, so the panel and the export dialog agree.

import type { SuggestionRef } from "@/annotations/commands";
import type { Item, RedactionSuggestion, Step, SuggestionReview } from "@/lib/types";

/** A blur suggestion with where it is. */
export interface Finding extends SuggestionRef {
  /** 1-based number of its step. */
  step: number;
  suggestion: RedactionSuggestion;
}

function stepsOf(items: Item[]): Step[] {
  return items.filter((item): item is Step => item.type === "step");
}

/** Every blur suggestion of the guide, in step order. */
export function findings(items: Item[]): Finding[] {
  return stepsOf(items).flatMap((s, i) =>
    (s.aiState.redactionSuggestions ?? []).map((suggestion, index) => ({ stepId: s.id, index, step: i + 1, suggestion })),
  );
}

/** The step's suggestions nobody has accepted or kept yet. */
export function pendingOf(step: Step): Finding[] {
  return (step.aiState.redactionSuggestions ?? []).flatMap((suggestion, index) =>
    suggestion.review ? [] : [{ stepId: step.id, index, step: 0, suggestion }],
  );
}

/** What kind of text a suggestion is about, as a title: "Email address". */
export function kindLabel(suggestion: RedactionSuggestion): string {
  const reason = suggestion.reason.trim() || "may be sensitive";
  return reason.charAt(0).toUpperCase() + reason.slice(1);
}

/** Findings of one kind (same reason from the same source) share this key. */
export function kindKey(suggestion: RedactionSuggestion): string {
  return `${suggestion.source ?? "ai"}:${suggestion.reason.trim().toLowerCase()}`;
}

/** Who found it. */
export function sourceLabel(suggestion: RedactionSuggestion): string {
  return suggestion.source === "onDevice" ? "on this Mac" : "by AI";
}

export const REVIEW_LABELS: Record<SuggestionReview | "pending", string> = {
  blurred: "blurred",
  kept: "kept",
  pending: "to review",
};

/** `[7]` as "Step 7", `[2, 5, 7]` as "Steps 2, 5 and 7"; after five, "and N more". */
function stepList(steps: number[]): string {
  if (steps.length === 1) return `Step ${steps[0]}`;
  const shown = steps.length > 5 ? steps.slice(0, 4) : steps.slice(0, -1);
  const rest = steps.length > 5 ? `${steps.length - 4} more` : String(steps.at(-1));
  return `Steps ${shown.join(", ")} and ${rest}`;
}

/**
 * Which steps were scanned for sensitive text on this Mac, as a sentence;
 * null for a guide with no steps.
 */
export function scanStatus(items: Item[]): string | null {
  const steps = stepsOf(items);
  if (!steps.length) return null;
  const unscanned = steps.flatMap((s, i) => (s.aiState.scanned ? [] : [i + 1]));
  if (unscanned.length === steps.length) {
    return "Not scanned on this Mac. New recordings are scanned when on-device redaction is on.";
  }
  if (!unscanned.length) return "Scanned on this Mac.";
  const one = unscanned.length === 1;
  return `Scanned on this Mac. ${stepList(unscanned)} ${one ? "wasn't" : "weren't"} scanned.`;
}
