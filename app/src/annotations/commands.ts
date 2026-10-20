// Annotation edits as undoable commands (#360). Each replaces the step's
// whole `annotations` array, so the inverse is exact and order is kept.

import { findStep, PatchError, type Command, type Patch } from "@/editor/commands";
import type { Annotation, AnnotationStyle, Guide, SuggestionReview } from "@guiderecorder/guide-schema";

const NAMES: Record<Annotation["type"], string> = {
  arrow: "arrow",
  box: "box",
  highlight: "highlight",
  callout: "callout",
  blur: "blur",
  crop: "crop",
  hotspot: "hotspot",
};

function name(annotation: Annotation): string {
  return annotation.type === "blur" && annotation.style?.blurMode === "pixelate" ? "pixelate" : NAMES[annotation.type];
}

function setAnnotations(
  guide: Guide,
  stepId: string,
  annotations: Annotation[],
  label: string,
  coalesceKey?: string,
): Command {
  const step = findStep(guide, stepId);
  return {
    label,
    forward: [{ op: "setItem", id: stepId, key: "annotations", value: annotations }],
    inverse: [{ op: "setItem", id: stepId, key: "annotations", value: step.annotations }],
    coalesceKey,
  };
}

function find(guide: Guide, stepId: string, id: string): Annotation {
  const annotation = findStep(guide, stepId).annotations.find((a) => a.id === id);
  if (!annotation) throw new PatchError(`annotation ${id} not found`);
  return annotation;
}

/**
 * Adds an annotation on top. A crop replaces the step's other crops: only
 * the last one applies at export, so more would only confuse.
 */
export function addAnnotation(guide: Guide, stepId: string, annotation: Annotation): Command {
  const current = findStep(guide, stepId).annotations;
  if (current.some((a) => a.id === annotation.id)) throw new PatchError(`annotation ${annotation.id} exists`);
  const kept = annotation.type === "crop" ? current.filter((a) => a.type !== "crop") : current;
  return setAnnotations(guide, stepId, [...kept, annotation], `Add ${name(annotation)}`);
}

/**
 * Replaces an annotation (same id), e.g. after a drag. `coalesce` names
 * the field being edited, so typing or slider moves merge into one undo
 * step.
 */
export function updateAnnotation(guide: Guide, stepId: string, next: Annotation, coalesce?: string): Command {
  const current = find(guide, stepId, next.id);
  if (current.type !== next.type) throw new PatchError(`annotation ${next.id} changed type`);
  const annotations = findStep(guide, stepId).annotations.map((a) => (a.id === next.id ? next : a));
  return setAnnotations(
    guide,
    stepId,
    annotations,
    `Edit ${name(next)}`,
    coalesce ? `annotation.${next.id}.${coalesce}` : undefined,
  );
}

/** Sets style fields (undefined removes one, so the default applies). */
export function setAnnotationStyle(
  guide: Guide,
  stepId: string,
  id: string,
  changes: Partial<AnnotationStyle>,
  coalesce = Object.keys(changes).join(","),
): Command {
  const current = find(guide, stepId, id);
  const style: AnnotationStyle = { ...current.style, ...changes };
  for (const key of Object.keys(style) as (keyof AnnotationStyle)[]) {
    if (style[key] === undefined) delete style[key];
  }
  const next = { ...current, style } as Annotation;
  if (Object.keys(style).length === 0) delete next.style;
  return updateAnnotation(guide, stepId, next, `style.${coalesce}`);
}

/** Sets a callout's text. */
export function setCalloutText(guide: Guide, stepId: string, id: string, text: string): Command {
  const current = find(guide, stepId, id);
  if (current.type !== "callout") throw new PatchError(`annotation ${id} is not a callout`);
  return updateAnnotation(guide, stepId, { ...current, geometry: { ...current.geometry, text } }, "text");
}

/** A hotspot's plain-text fields. */
export type HotspotField = "title" | "description";

/** Sets a hotspot's title or description; typing in one field merges into one undo step. */
export function setHotspotText(guide: Guide, stepId: string, id: string, field: HotspotField, text: string): Command {
  const current = find(guide, stepId, id);
  if (current.type !== "hotspot") throw new PatchError(`annotation ${id} is not a hotspot`);
  return updateAnnotation(guide, stepId, { ...current, geometry: { ...current.geometry, [field]: text } }, field);
}

export function removeAnnotation(guide: Guide, stepId: string, id: string): Command {
  const current = find(guide, stepId, id);
  const annotations = findStep(guide, stepId).annotations.filter((a) => a.id !== id);
  return setAnnotations(guide, stepId, annotations, `Delete ${name(current)}`);
}

// Blur suggestions, from the AI (#366) or found on this Mac (#10). They
// live in the step's `aiState` only, so the export never sees them;
// accepting one makes it a real blur. A reviewed suggestion stays, marked
// `review`, so the privacy review can show what was decided and a later
// scan or AI pass does not suggest the same area again.

/** A blur suggestion: its step and index in the step's `redactionSuggestions`. */
export interface SuggestionRef {
  stepId: string;
  index: number;
}

/**
 * Marks the pending suggestions `refs` (of any steps) `review`; for
 * "blurred", a blur annotation `ids[i]` goes over suggestion `refs[i]`
 * (full-screenshot pixels, like annotations). One undo step.
 */
function reviewSuggestions(
  guide: Guide,
  refs: SuggestionRef[],
  review: SuggestionReview,
  ids: string[],
  label: string,
): Command {
  if (!refs.length) throw new PatchError("no blur suggestions");
  if (review === "blurred" && ids.length !== refs.length) {
    throw new PatchError(`${ids.length} ids for ${refs.length} suggestions`);
  }
  const byStep = new Map<string, { index: number; id: string }[]>();
  refs.forEach(({ stepId, index }, i) => byStep.set(stepId, [...(byStep.get(stepId) ?? []), { index, id: ids[i] }]));
  const forward: Patch[] = [];
  const inverse: Patch[] = [];
  for (const [stepId, picks] of byStep) {
    const step = findStep(guide, stepId);
    const suggestions = step.aiState.redactionSuggestions ?? [];
    for (const { index } of picks) {
      if (!suggestions[index]) throw new PatchError(`step ${stepId} has no blur suggestion ${index}`);
      if (suggestions[index].review) throw new PatchError(`blur suggestion ${index} of step ${stepId} is reviewed`);
    }
    const chosen = new Set(picks.map((p) => p.index));
    const redactionSuggestions = suggestions.map((s, i) => (chosen.has(i) ? { ...s, review } : s));
    if (review === "blurred") {
      const blurs: Annotation[] = picks.map(({ index, id }) => ({ id, type: "blur", geometry: { ...suggestions[index].rect } }));
      if (blurs.some((b) => step.annotations.some((a) => a.id === b.id))) throw new PatchError("annotation exists");
      forward.push({ op: "setItem", id: stepId, key: "annotations", value: [...step.annotations, ...blurs] });
      inverse.unshift({ op: "setItem", id: stepId, key: "annotations", value: step.annotations });
    }
    forward.push({ op: "setItem", id: stepId, key: "aiState", value: { ...step.aiState, redactionSuggestions } });
    inverse.unshift({ op: "setItem", id: stepId, key: "aiState", value: step.aiState });
  }
  return { label, forward, inverse };
}

/** Accepts the step's blur suggestion `index`: a blur annotation `id` goes over it. */
export function acceptBlurSuggestion(guide: Guide, stepId: string, index: number, id: string): Command {
  return reviewSuggestions(guide, [{ stepId, index }], "blurred", [id], "Accept blur suggestion");
}

/** Keeps the area of the step's blur suggestion `index` as it is. */
export function keepBlurSuggestion(guide: Guide, stepId: string, index: number): Command {
  return reviewSuggestions(guide, [{ stepId, index }], "kept", [], "Keep suggested area");
}

/** Accepts the pending suggestions `refs`, of any steps: blur `ids[i]` over `refs[i]`. One undo step. */
export function acceptBlurSuggestions(guide: Guide, refs: SuggestionRef[], ids: string[]): Command {
  return reviewSuggestions(guide, refs, "blurred", ids, "Accept blur suggestions");
}

/** Keeps the areas of the pending suggestions `refs`, of any steps. One undo step. */
export function keepBlurSuggestions(guide: Guide, refs: SuggestionRef[]): Command {
  return reviewSuggestions(guide, refs, "kept", [], "Keep suggested areas");
}

/**
 * Moves hotspot `id` `by` places among the step's hotspots (their numbers
 * follow their order); the other annotations keep their places.
 */
export function moveHotspot(guide: Guide, stepId: string, id: string, by: number): Command {
  const annotations = findStep(guide, stepId).annotations;
  const slots = annotations.flatMap((a, i) => (a.type === "hotspot" ? [i] : []));
  const from = slots.findIndex((i) => annotations[i].id === id);
  if (from < 0) throw new PatchError(`hotspot ${id} not found`);
  const to = from + by;
  if (to < 0 || to >= slots.length) throw new PatchError(`hotspot ${id} cannot move by ${by}`);
  const order = slots.map((i) => annotations[i]);
  order.splice(to, 0, ...order.splice(from, 1));
  const next = [...annotations];
  slots.forEach((slot, i) => (next[slot] = order[i]));
  return setAnnotations(guide, stepId, next, "Move hotspot");
}
