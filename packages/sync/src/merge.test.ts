import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import fc from "fast-check";
import { describe, expect, it } from "vitest";
import type { Guide } from "@guiderecorder/guide-schema";
import { equal, mergeGuides } from "./merge.ts";
import { imagePaths } from "./protocol.ts";

type Item = { id: string; type: string; [key: string]: unknown };

const fixtures = join(import.meta.dirname, "../../../fixtures/guides");
const guides: Guide[] = readdirSync(fixtures).map((bundle) => JSON.parse(readFileSync(join(fixtures, bundle, "guide.json"), "utf8")));
const rotate = guides.find((g) => g.title === "Rotate an API key")!;

const clone = <T>(value: T): T => structuredClone(value);
const itemsOf = (guide: Guide) => (guide.items ?? []) as Item[];
const ids = (guide: Guide) => itemsOf(guide).map((item) => item.id);
const item = (guide: Guide, id: string) => itemsOf(guide).find((i) => i.id === id);

type Editable = Omit<Guide, "items"> & { items: Item[] };

function edit(guide: Guide, change: (g: Editable) => void): Guide {
  const copy = clone(guide) as unknown as Editable;
  change(copy);
  return copy as unknown as Guide;
}

describe("mergeGuides", () => {
  it("combines edits to different steps", () => {
    const [a, b] = itemsOf(rotate).filter((i) => i.type === "step");
    const ours = edit(rotate, (g) => (g.items.find((i) => i.id === a!.id)!.title = "Mine"));
    const theirs = edit(rotate, (g) => (g.items.find((i) => i.id === b!.id)!.description = "Theirs"));
    const { guide, conflicts } = mergeGuides(rotate, ours, theirs);
    expect(conflicts).toEqual([]);
    expect(item(guide, a!.id)!.title).toBe("Mine");
    expect(item(guide, b!.id)!.description).toBe("Theirs");
  });

  it("combines edits to different fields of one step", () => {
    const step = itemsOf(rotate).find((i) => i.type === "step")!;
    const ours = edit(rotate, (g) => (g.items.find((i) => i.id === step.id)!.title = "Mine"));
    const theirs = edit(rotate, (g) => (g.items.find((i) => i.id === step.id)!.description = "Theirs"));
    const { guide, conflicts } = mergeGuides(rotate, ours, theirs);
    expect(conflicts).toEqual([]);
    expect(item(guide, step.id)).toMatchObject({ title: "Mine", description: "Theirs" });
  });

  it("reports the same field edited on both sides, resolving to the preferred side", () => {
    const step = itemsOf(rotate).find((i) => i.type === "step")!;
    const ours = edit(rotate, (g) => (g.items.find((i) => i.id === step.id)!.title = "Mine"));
    const theirs = edit(rotate, (g) => (g.items.find((i) => i.id === step.id)!.title = "Theirs"));
    const mine = mergeGuides(rotate, ours, theirs, "ours");
    expect(mine.conflicts).toEqual([{ kind: "field", itemId: step.id, field: "title", base: step.title, ours: "Mine", theirs: "Theirs" }]);
    expect(item(mine.guide, step.id)!.title).toBe("Mine");
    expect(item(mergeGuides(rotate, ours, theirs, "theirs").guide, step.id)!.title).toBe("Theirs");
  });

  it("reports guide fields edited on both sides", () => {
    const ours = edit(rotate, (g) => (g.title = "Mine"));
    const theirs = edit(rotate, (g) => (g.title = "Theirs"));
    expect(mergeGuides(rotate, ours, theirs).conflicts).toMatchObject([{ kind: "field", itemId: null, field: "title" }]);
  });

  it("deletes an item the other side left alone", () => {
    const id = ids(rotate)[0]!;
    const ours = edit(rotate, (g) => (g.items = g.items.filter((i) => i.id !== id)));
    const theirs = edit(rotate, (g) => (g.title = "Theirs"));
    const { guide, conflicts } = mergeGuides(rotate, ours, theirs);
    expect(conflicts).toEqual([]);
    expect(ids(guide)).not.toContain(id);
    expect(guide.title).toBe("Theirs");
  });

  it("reports an item deleted on one side and changed on the other", () => {
    const id = ids(rotate)[1]!;
    const ours = edit(rotate, (g) => (g.items = g.items.filter((i) => i.id !== id)));
    const theirs = edit(rotate, (g) => (g.items.find((i) => i.id === id)!.title = "Theirs"));
    const mine = mergeGuides(rotate, ours, theirs, "ours");
    expect(mine.conflicts).toMatchObject([{ kind: "deleted", itemId: id, deletedBy: "ours" }]);
    expect(ids(mine.guide)).not.toContain(id);
    const kept = mergeGuides(rotate, ours, theirs, "theirs");
    expect(ids(kept.guide)).toEqual(ids(rotate));
  });

  it("keeps items added on both sides after the item they followed", () => {
    const [first, second] = ids(rotate);
    const note = (id: string) => ({ type: "note", id, kind: "info", body: id });
    const ours = edit(rotate, (g) => g.items.splice(1, 0, note("01M4WZCE2000000000000000A1")));
    const theirs = edit(rotate, (g) => g.items.splice(2, 0, note("01M4WZCE2000000000000000B1")));
    const { guide, conflicts } = mergeGuides(rotate, ours, theirs);
    expect(conflicts).toEqual([]);
    expect(ids(guide).slice(0, 4)).toEqual([first, "01M4WZCE2000000000000000A1", second, "01M4WZCE2000000000000000B1"]);
  });

  it("takes one side's reordering and the other's edits", () => {
    const ours = edit(rotate, (g) => g.items.reverse());
    const id = ids(rotate)[0]!;
    const theirs = edit(rotate, (g) => (g.items[0]!.body = "Theirs"));
    const { guide, conflicts } = mergeGuides(rotate, ours, theirs);
    expect(conflicts).toEqual([]);
    expect(ids(guide)).toEqual([...ids(rotate)].reverse());
    expect(item(guide, id)!.body).toBe("Theirs");
  });

  it("reports different reorderings", () => {
    const ours = edit(rotate, (g) => g.items.reverse());
    const theirs = edit(rotate, (g) => g.items.push(g.items.shift()!));
    const { guide, conflicts } = mergeGuides(rotate, ours, theirs, "theirs");
    expect(conflicts).toMatchObject([{ kind: "order" }]);
    expect(ids(guide)).toEqual(ids(theirs));
  });

  it("takes the later updatedAt and the newer schema version", () => {
    const ours = edit(rotate, (g) => (g.updatedAt = "2026-10-20T00:00:00Z"));
    const theirs = edit(rotate, (g) => ((g.updatedAt = "2026-10-21T00:00:00Z"), (g.schemaVersion = 3)));
    const { guide, conflicts } = mergeGuides(rotate, ours, theirs);
    expect(conflicts).toEqual([]);
    expect(guide).toMatchObject({ updatedAt: "2026-10-21T00:00:00Z", schemaVersion: 3 });
  });

  it("merges guides created on both sides without a base", () => {
    const theirs = edit(rotate, (g) => (g.summary = "Theirs"));
    const { conflicts } = mergeGuides(null, rotate, theirs);
    expect(conflicts).toMatchObject([{ kind: "field", itemId: null, field: "summary" }]);
  });
});

describe("imagePaths", () => {
  it("lists every image a guide names", () => {
    expect(imagePaths(rotate)).toContain("steps/01M4WZCE2000000000000002R2.crop.webp");
    expect(imagePaths(rotate)).toContain("steps/01M4WZCE2000000000000002R8.after.webp");
    expect(imagePaths(guides.find((g) => g.title === "Kurzanleitung Drucker")!)).toEqual(["assets/drucker-fach.webp"]);
  });
});

// Property tests: random edits to the fixture guides.

type Edit =
  | { op: "guideField"; field: "title" | "summary" | "intro"; value: string }
  | { op: "itemField"; index: number; value: string }
  | { op: "delete"; index: number }
  | { op: "insert"; index: number; n: number }
  | { op: "move"; from: number; to: number };

const editArb: fc.Arbitrary<Edit> = fc.oneof(
  fc.record({ op: fc.constant("guideField" as const), field: fc.constantFrom("title" as const, "summary" as const, "intro" as const), value: fc.string() }),
  fc.record({ op: fc.constant("itemField" as const), index: fc.nat(), value: fc.string() }),
  fc.record({ op: fc.constant("delete" as const), index: fc.nat() }),
  fc.record({ op: fc.constant("insert" as const), index: fc.nat(), n: fc.nat(9999) }),
  fc.record({ op: fc.constant("move" as const), from: fc.nat(), to: fc.nat() }),
);

/** The field an item edit changes: a note's body, a step's or section's title. */
const textField = (i: Item) => (i.type === "note" ? "body" : "title");

/**
 * Applies edits, each to an item `allowed` accepts (by its id), so two edit
 * lists can be kept to disjoint items. Inserted notes get ids unique to `side`.
 */
function apply(guide: Guide, edits: Edit[], side: string, allowed: (id: string) => boolean = () => true): Guide {
  return edit(guide, (g) => {
    for (const e of edits) {
      const choices = g.items.map((i, at) => [i, at] as const).filter(([i]) => allowed(i.id));
      const pick = (n: number) => choices[n % choices.length];
      if (e.op === "guideField") g[e.field] = `${side}:${e.value}`;
      else if (e.op === "insert") {
        const id = `01M4WZCE20000000000${side === "ours" ? "A" : "B"}${String(e.n).padStart(6, "0")}`;
        if (!g.items.some((i) => i.id === id)) g.items.splice(e.index % (g.items.length + 1), 0, { type: "note", id, kind: "info", body: side });
      } else if (choices.length) {
        if (e.op === "itemField") {
          const [i] = pick(e.index)!;
          i[textField(i)] = `${side}:${e.value}`;
        } else if (e.op === "delete") g.items.splice(pick(e.index)![1], 1);
        else if (e.op === "move") {
          const [moved] = g.items.splice(pick(e.from)![1], 1);
          g.items.splice(e.to % (g.items.length + 1), 0, moved!);
        }
      }
    }
  });
}

const guideArb = fc.constantFrom(...guides);
const editsArb = fc.array(editArb, { maxLength: 8 });

describe("mergeGuides properties", () => {
  it("changes on one side come through untouched", () => {
    fc.assert(
      fc.property(guideArb, editsArb, (base, edits) => {
        const changed = apply(base, edits, "ours");
        for (const [o, t] of [[changed, base], [base, changed], [changed, changed]] as const) {
          const { guide, conflicts } = mergeGuides(base, o, t);
          expect(conflicts).toEqual([]);
          expect(equal(guide, changed)).toBe(true);
        }
      }),
    );
  });

  it("edits to different items merge without conflicts, keeping both", () => {
    fc.assert(
      fc.property(guideArb, editsArb, editsArb, (base, oursEdits, theirsEdits) => {
        // Ours edits even-indexed base items and may reorder; theirs edits odd ones and doesn't.
        const baseIds = ids(base);
        const even = (id: string) => baseIds.indexOf(id) % 2 === 0;
        const odd = (id: string) => baseIds.indexOf(id) % 2 === 1;
        const guideFields = new Set(oursEdits.flatMap((e) => (e.op === "guideField" ? [e.field] : [])));
        const ours = apply(base, oursEdits, "ours", even);
        const theirs = apply(
          base,
          theirsEdits.filter((e) => e.op !== "move" && !(e.op === "guideField" && guideFields.has(e.field))),
          "theirs",
          odd,
        );
        const { guide, conflicts } = mergeGuides(base, ours, theirs);
        expect(conflicts).toEqual([]);

        for (const [side, from] of [[ours, even], [theirs, odd]] as const) {
          for (const id of baseIds.filter(from)) {
            if (item(side, id)) expect(item(guide, id)).toEqual(item(side, id));
            else expect(item(guide, id)).toBeUndefined();
          }
          for (const added of itemsOf(side).filter((i) => !baseIds.includes(i.id))) expect(item(guide, added.id)).toEqual(added);
        }
        for (const field of ["title", "summary", "intro"] as const) {
          const changed = [ours, theirs].find((side) => side[field] !== base[field]);
          expect(guide[field]).toEqual(changed ? changed[field] : base[field]);
        }
      }),
    );
  });

  it("is symmetric: swapping sides and preference gives the same items", () => {
    fc.assert(
      fc.property(guideArb, editsArb, editsArb, (base, oursEdits, theirsEdits) => {
        const ours = apply(base, oursEdits, "ours");
        const theirs = apply(base, theirsEdits, "theirs");
        const a = mergeGuides(base, ours, theirs, "ours");
        const b = mergeGuides(base, theirs, ours, "theirs");
        expect(a.conflicts.length).toBe(b.conflicts.length);
        const sorted = (g: Guide) => [...itemsOf(g)].sort((x, y) => x.id.localeCompare(y.id));
        expect(sorted(a.guide)).toEqual(sorted(b.guide));
        expect({ ...a.guide, items: [] }).toEqual({ ...b.guide, items: [] });
      }),
    );
  });

  it("never loses or duplicates an item", () => {
    fc.assert(
      fc.property(guideArb, editsArb, editsArb, fc.constantFrom("ours" as const, "theirs" as const), (base, oursEdits, theirsEdits, prefer) => {
        const ours = apply(base, oursEdits, "ours");
        const theirs = apply(base, theirsEdits, "theirs");
        const { guide, conflicts } = mergeGuides(base, ours, theirs, prefer);
        const merged = ids(guide);
        expect(new Set(merged).size).toBe(merged.length);
        const deletedConflicts = new Set(conflicts.flatMap((c) => (c.kind === "deleted" ? [c.itemId] : [])));
        for (const id of new Set([...ids(ours), ...ids(theirs)])) {
          const inBoth = item(ours, id) && item(theirs, id);
          const added = !ids(base).includes(id);
          if (inBoth || added) expect(merged).toContain(id);
          else if (!deletedConflicts.has(id)) {
            // Kept on one side only: deleted unless it was changed there.
            const kept = item(ours, id) ?? item(theirs, id);
            expect(merged.includes(id)).toBe(!equal(kept, item(base, id)));
          }
        }
      }),
    );
  });
});
