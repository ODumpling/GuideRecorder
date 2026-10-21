// Building and reading guide.json on the phone. Pure: no network, no files.

import { SCHEMA_VERSION, type Decision, type Guide, type Item, type Note, type Step } from "@guiderecorder/guide-schema";

/** An empty walkthrough, as `Guide::new` makes it. The title is the user's. */
export function newGuide(id: string, title: string, language: string, now: Date = new Date()): Guide {
  const at = now.toISOString();
  return {
    schemaVersion: SCHEMA_VERSION,
    id,
    title: title.trim(),
    summary: "",
    tags: [],
    intro: "",
    prerequisites: "",
    audience: "beginner",
    kind: "walkthrough",
    language,
    createdAt: at,
    updatedAt: at,
    recording: null,
    items: [],
    aiState: { status: "template" },
    editedFields: title.trim() ? ["title"] : [],
  };
}

/** Where a photo step's image lives in the bundle. */
export function photoPath(stepId: string): string {
  return `steps/${stepId}.jpg`;
}

/** A manual step showing a photo. Text the user wrote is theirs; an empty step is left to the AI. */
export function photoStep(id: string, title: string, description: string): Step {
  const edited = Boolean(title.trim() || description.trim());
  return {
    type: "step",
    id,
    title: title.trim(),
    description: description.trim(),
    kind: "manual",
    image: { full: photoPath(id), show: "full" },
    annotations: [],
    aiState: { status: edited ? "edited" : "template" },
  };
}

/** The guide with `item` added at the end. */
export function appendItem(guide: Guide, item: Item, now: Date = new Date()): Guide {
  return { ...guide, items: [...guide.items, item], updatedAt: now.toISOString() };
}

/** The image a step shows: its crop if it shows one, else the full screenshot. */
export function shownImage(step: Step): string {
  return step.image.show === "crop" && step.image.crop ? step.image.crop : step.image.full;
}

export type ViewItem =
  | { type: "step"; step: Step; number: number }
  | { type: "section"; id: string; title: string; audiences: string[] }
  | { type: "note"; note: Note }
  | { type: "decision"; decision: Decision; options: { id: string; label: string; to: string }[] };

/**
 * The guide's items as the viewer shows them: steps numbered 1, 2, 3, …,
 * and each decision option with where it leads (`to`), as the exports say it.
 */
export function viewItems(guide: Guide): ViewItem[] {
  let number = 0;
  return guide.items.map((item, index): ViewItem => {
    if (item.type === "step") return { type: "step", step: item, number: ++number };
    if (item.type === "section") return { type: "section", id: item.id, title: item.title, audiences: (item.audiences ?? []).map((a) => a.trim()).filter(Boolean) };
    if (item.type === "decision") {
      const options = item.options.map((option) => ({ id: option.id, label: option.label.trim(), to: goTo(guide, index, option.target) }));
      return { type: "decision", decision: item, options };
    }
    return { type: "note", note: item };
  });
}

/**
 * Where an option of the decision at `from` leads, in words: the first step
 * at or after the item it names (after the decision for `next`; a gone
 * step or section reads as `next`), like core `Labels::go_to`.
 */
export function goTo(guide: Guide, from: number, target: Decision["options"][number]["target"]): string {
  if (target.kind === "guide") return target.title.trim() ? `Go to the guide “${target.title.trim()}”` : "Go to the guide";
  const named = target.kind === "next" ? -1 : guide.items.findIndex((item) => item.id === target.id);
  const at = named >= 0 ? named : from;
  const chosen = named >= 0 ? guide.items[named] : undefined;
  const section = target.kind === "section" && chosen?.type === "section" ? chosen.title.trim() : "";
  let number = 0;
  let step: number | null = null;
  for (const [i, item] of guide.items.entries()) {
    if (item.type !== "step") continue;
    number += 1;
    if (i >= at) {
      step = number;
      break;
    }
  }
  if (section && step !== null) return `Go to “${section}” (Step ${step})`;
  if (section) return `Go to “${section}”`;
  return step !== null ? `Go to Step ${step}` : "Go to the end";
}

export function stepCount(guide: Guide): number {
  return guide.items.filter((item) => item.type === "step").length;
}

/**
 * The cover's image, for the library's thumbnails: the step the author
 * chose (`coverStepId`), as its cover options ask (annotations aren't drawn
 * here), else the first step's image.
 */
export function coverImage(guide: Guide): string | null {
  const steps = guide.items.filter((item): item is Step => item.type === "step");
  const chosen = steps.find((step) => step.id === guide.coverStepId && step.image.full);
  if (chosen) return guide.coverOptions?.image === "crop" ? (chosen.image.crop ?? chosen.image.full) : chosen.image.full;
  return steps[0] ? shownImage(steps[0]) : null;
}

/** Markdown reduced to plain text, for one-line titles. */
export function plainText(markdown: string): string {
  return markdown
    .replace(/!\[[^\]]*\]\([^)]*\)/g, "")
    .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/[*_`~#>]/g, "")
    .replace(/\s+/g, " ")
    .trim();
}
