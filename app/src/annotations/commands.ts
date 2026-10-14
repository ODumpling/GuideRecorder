// Annotation edits as undoable commands (#360). Each replaces the step's
// whole `annotations` array, so the inverse is exact and order is kept.

import { findStep, PatchError, type Command } from "@/editor/commands";
import type { Annotation, AnnotationStyle, Guide } from "@/lib/types";

const NAMES: Record<Annotation["type"], string> = {
  arrow: "arrow",
  box: "box",
  highlight: "highlight",
  callout: "callout",
  blur: "blur",
  crop: "crop",
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

export function removeAnnotation(guide: Guide, stepId: string, id: string): Command {
  const current = find(guide, stepId, id);
  const annotations = findStep(guide, stepId).annotations.filter((a) => a.id !== id);
  return setAnnotations(guide, stepId, annotations, `Delete ${name(current)}`);
}
