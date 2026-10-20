import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import type { Guide, Item, Step } from "@guiderecorder/guide-schema";
import { imagePaths } from "@guiderecorder/sync";
import { diffGuides } from "./diff";

const fixture: Guide = JSON.parse(
  readFileSync(new URL("../../../fixtures/guides/Rotate an API key.guiderecorder/guide.json", import.meta.url), "utf8"),
);
const assetsFor = (guide: Guide) => Object.fromEntries(imagePaths(guide).map((p) => [p, `hash-of-${p}`]));
const side = (guide: Guide, assets = assetsFor(guide)) => ({ guide, assets });
const ids = fixture.items.map((i) => i.id);
const step = (guide: Guide, id: string) => guide.items.find((i) => i.id === id) as Step;
const withItems = (items: Item[], extra: Partial<Guide> = {}): Guide => ({ ...structuredClone(fixture), ...extra, items });

describe("diffGuides", () => {
  it("finds nothing between a guide and itself", () => {
    const diff = diffGuides(side(fixture), side(structuredClone(fixture)));
    expect(diff.changed).toBe(false);
    expect(diff.fields).toEqual([]);
    expect(diff.items.map((i) => i.change)).toEqual(ids.map(() => "unchanged"));
  });

  it("lists changed guide fields, leaving out bookkeeping", () => {
    const head = { ...structuredClone(fixture), title: "Rotate a key", updatedAt: "2027-01-01T00:00:00Z", status: "draft" as const };
    expect(diffGuides(side(fixture), side(head)).fields).toEqual([
      { field: "title", before: "Rotate an API key", after: "Rotate a key" },
      { field: "status", before: null, after: "draft" },
    ]);
  });

  it("matches items by ULID and names the fields that changed", () => {
    const head = structuredClone(fixture);
    step(head, ids[1]!).title = "Open the keys page";
    step(head, ids[1]!).aiState = { status: "edited" };
    const diff = diffGuides(side(fixture), side(head));
    expect(diff.items[1]).toMatchObject({ id: ids[1], change: "changed", fields: ["title"] });
    expect(diff.items.filter((i) => i.change !== "unchanged")).toHaveLength(1);
  });

  it("shows added items in place and removed items where they were", () => {
    const added: Item = { type: "note", id: "01M4WZCE20000000000000NEW0", kind: "tip", body: "New" };
    const head = withItems([fixture.items[0]!, added, ...fixture.items.slice(2)]);
    const diff = diffGuides(side(fixture), side(head));
    expect(diff.items.map((i) => [i.id, i.change])).toEqual([
      [ids[0], "unchanged"],
      [added.id, "added"],
      [ids[1], "removed"],
      ...ids.slice(2).map((id) => [id, "unchanged"]),
    ]);
    expect(diff.items[1]!.before).toBeNull();
    expect(diff.items[2]!.after).toBeNull();
  });

  it("removed items at the end stay at the end", () => {
    const diff = diffGuides(side(fixture), side(withItems(fixture.items.slice(0, -1))));
    expect(diff.items.at(-1)).toMatchObject({ id: ids.at(-1), change: "removed" });
  });

  it("marks moved items, not the ones that kept their order", () => {
    const [a, b, ...rest] = fixture.items;
    const head = withItems([...rest, a!, b!]);
    const diff = diffGuides(side(fixture), side(head));
    const moved = diff.items.filter((i) => i.change === "moved").map((i) => i.id);
    expect(moved).toEqual([ids[0], ids[1]]);
  });

  it("sees a new screenshot under the same path, and gives both images", () => {
    const id = ids[1]!;
    const s = step(fixture, id);
    const path = s.image.show === "full" || !s.image.crop ? s.image.full : s.image.crop;
    const assets = { ...assetsFor(fixture), [path]: "new-hash" };
    const diff = diffGuides(side(fixture), side(structuredClone(fixture), assets));
    expect(diff.items[1]).toMatchObject({ change: "changed", fields: ["image"], beforeImage: `hash-of-${path}`, afterImage: "new-hash" });
  });

  it("treats every item as added for a guide never published", () => {
    const diff = diffGuides(null, side(fixture));
    expect(diff.items.every((i) => i.change === "added")).toBe(true);
    expect(diff.fields.map((f) => f.field)).toContain("title");
  });
});
