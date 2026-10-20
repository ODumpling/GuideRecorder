// Building and reading guide.json on the phone. Pure: no network, no files.

import { SCHEMA_VERSION, type Guide, type Item, type Note, type Step } from "@guiderecorder/guide-schema";

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
  | { type: "section"; id: string; title: string }
  | { type: "note"; note: Note };

/** The guide's items as the viewer shows them: steps numbered 1, 2, 3, … */
export function viewItems(guide: Guide): ViewItem[] {
  let number = 0;
  return guide.items.map((item): ViewItem => {
    if (item.type === "step") return { type: "step", step: item, number: ++number };
    if (item.type === "section") return { type: "section", id: item.id, title: item.title };
    return { type: "note", note: item };
  });
}

export function stepCount(guide: Guide): number {
  return guide.items.filter((item) => item.type === "step").length;
}

/** The first step's image, for the library's thumbnails. */
export function coverImage(guide: Guide): string | null {
  const step = guide.items.find((item): item is Step => item.type === "step");
  return step ? shownImage(step) : null;
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
