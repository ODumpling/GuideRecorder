import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import type { Guide, Step } from "@guiderecorder/guide-schema";
import { deleteItem, moveItem, setGuideText, setItemText, textOnlyProblem } from "./draft";

const fixture: Guide = JSON.parse(
  readFileSync(new URL("../../../fixtures/guides/Rotate an API key.guiderecorder/guide.json", import.meta.url), "utf8"),
);
const steps = (guide: Guide) => guide.items.filter((i): i is Step => i.type === "step");

describe("setGuideText", () => {
  it("sets the field and marks it edited", () => {
    const guide = setGuideText({ ...fixture, editedFields: undefined, aiState: { status: "ai" } }, "intro", "New intro");
    expect(guide).toMatchObject({ intro: "New intro", editedFields: ["intro"], aiState: { status: "edited" } });
    expect(setGuideText(guide, "title", "New title").editedFields).toEqual(["title", "intro"]);
  });

  it("keeps an older guide's edited aiState covering every field", () => {
    const guide = setGuideText({ ...fixture, editedFields: undefined, aiState: { status: "edited" } }, "title", "T");
    expect(guide.editedFields).toEqual(["title", "intro", "prerequisites", "summary", "tags"]);
  });

  it("changes nothing when the value is the same", () => {
    expect(setGuideText(fixture, "title", fixture.title)).toBe(fixture);
  });
});

describe("setItemText", () => {
  it("marks a step's text edited", () => {
    const step = steps(fixture)[0]!;
    const guide = setItemText(fixture, step.id, "description", "Do this");
    expect(steps(guide)[0]).toMatchObject({ description: "Do this", aiState: { status: "edited" } });
  });

  it("leaves fields an item doesn't have", () => {
    const step = steps(fixture)[0]!;
    expect(setItemText(fixture, step.id, "body", "x").items).toEqual(fixture.items);
  });
});

describe("moveItem and deleteItem", () => {
  it("moves an item within the list", () => {
    const [a, b] = fixture.items;
    expect(moveItem(fixture, a!.id, 1).items.slice(0, 2).map((i) => i.id)).toEqual([b!.id, a!.id]);
    expect(moveItem(fixture, a!.id, -1)).toBe(fixture);
  });

  it("deletes an item", () => {
    const [a] = fixture.items;
    expect(deleteItem(fixture, a!.id).items).toHaveLength(fixture.items.length - 1);
  });
});

describe("textOnlyProblem", () => {
  const step = steps(fixture)[0]!;

  it("accepts text edits, reordering and deleting", () => {
    let guide = setGuideText(fixture, "prerequisites", "Admin access");
    guide = setItemText(guide, step.id, "title", "Open settings");
    guide = moveItem(guide, step.id, 1);
    guide = deleteItem(guide, steps(fixture)[1]!.id);
    expect(textOnlyProblem(fixture, guide)).toBeNull();
  });

  it("rejects other changes", () => {
    const repointed = { ...fixture, items: fixture.items.map((i) => (i.id === step.id ? { ...step, point: { x: -1, y: -1 } } : i)) };
    expect(textOnlyProblem(fixture, repointed)).toMatch(/Only the text/);
    expect(textOnlyProblem(fixture, { ...fixture, language: "de" })).toMatch(/Only the guide's/);
    const added = { ...fixture, items: [...fixture.items, { type: "section" as const, id: "01M4WZCE2000000000000009ZZ", title: "New" }] };
    expect(textOnlyProblem(fixture, added)).toMatch(/can't be added/);
    const suggested = setItemText(fixture, step.id, "title", "x");
    const withSuggestion = {
      ...suggested,
      items: suggested.items.map((i) => (i.id === step.id && i.type === "step" ? { ...i, aiState: { ...i.aiState, model: "other" } } : i)),
    };
    expect(textOnlyProblem(fixture, withSuggestion)).toMatch(/Only the text/);
  });
});
