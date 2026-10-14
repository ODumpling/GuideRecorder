import type { Guide, Step } from "@/lib/types";

export function step(id: string, title = `Step ${id}`): Step {
  return {
    type: "step",
    id,
    title,
    description: "",
    kind: "click",
    image: { full: `steps/${id}.webp`, crop: `steps/${id}.crop.webp`, show: "crop" },
    annotations: [],
    aiState: { status: "template" },
  };
}

export function guide(): Guide {
  return {
    schemaVersion: 1,
    id: "G1",
    title: "Guide",
    summary: "",
    tags: [],
    intro: "",
    prerequisites: "",
    audience: "beginner",
    language: "en",
    createdAt: "2026-10-14T12:00:00Z",
    updatedAt: "2026-10-14T12:00:00Z",
    items: [
      { type: "section", id: "S1", title: "Start" },
      step("A"),
      step("B"),
      { type: "note", id: "N1", kind: "tip", body: "Tip" },
      step("C"),
    ],
  };
}
