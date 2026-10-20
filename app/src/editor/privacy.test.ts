import { describe, expect, it } from "vitest";
import { tabCounts } from "@/editor/inspector";
import { findings, kindKey, kindLabel, pendingOf, scanStatus, sourceLabel } from "@/editor/privacy";
import { guide } from "@/editor/test-fixtures";
import type { Guide, RedactionSuggestion, Step } from "@/lib/types";

const rect = { x: 1, y: 2, width: 3, height: 4 };
const email: RedactionSuggestion = { rect, reason: "email address", source: "onDevice", preview: "•••@acme.co" };
const card: RedactionSuggestion = { rect, reason: "card number", source: "onDevice", review: "blurred" };
const name: RedactionSuggestion = { rect, reason: "a name" };

/** The fixture guide (steps A, B and C) with these suggestions and scans. */
function withFindings(): Guide {
  const g = guide();
  const [a, b, c] = g.items.filter((i): i is Step => i.type === "step");
  a.aiState = { status: "ai", scanned: true, redactionSuggestions: [email, card] };
  b.aiState = { status: "ai", scanned: true, redactionSuggestions: [] };
  c.aiState = { status: "ai", redactionSuggestions: [name, { ...email, review: "kept" }] };
  return g;
}

describe("privacy review", () => {
  it("lists every finding with its step and index", () => {
    expect(findings(withFindings().items).map(({ stepId, index, step }) => [stepId, index, step])).toEqual([
      ["A", 0, 1],
      ["A", 1, 1],
      ["C", 0, 3],
      ["C", 1, 3],
    ]);
  });

  it("leaves reviewed ones out of what is pending", () => {
    const [a, , c] = withFindings().items.filter((i): i is Step => i.type === "step");
    expect(pendingOf(a).map((f) => f.index)).toEqual([0]);
    expect(pendingOf(c).map((f) => f.index)).toEqual([0]);
  });

  it("names the kind and who found it", () => {
    expect(kindLabel(email)).toBe("Email address");
    expect(kindLabel({ rect, reason: " " })).toBe("May be sensitive");
    expect(sourceLabel(email)).toBe("on this Mac");
    expect(sourceLabel(name)).toBe("by AI");
    expect(kindKey(email)).toBe(kindKey({ ...email, rect: { ...rect, x: 9 } }));
    expect(kindKey(email)).not.toBe(kindKey({ ...email, source: "ai" }));
  });

  it("says which steps were scanned on this Mac", () => {
    expect(scanStatus(withFindings().items)).toBe("Scanned on this Mac. Step 3 wasn't scanned.");
    const g = withFindings();
    for (const item of g.items) if (item.type === "step") item.aiState.scanned = true;
    expect(scanStatus(g.items)).toBe("Scanned on this Mac.");
    expect(scanStatus(guide().items)).toMatch(/^Not scanned on this Mac\./);
    const many = guide();
    (many.items[1] as Step).aiState.scanned = true;
    expect(scanStatus(many.items)).toBe("Scanned on this Mac. Steps 2 and 3 weren't scanned.");
    expect(scanStatus([])).toBeNull();
  });

  it("counts what needs attention on each inspector tab", () => {
    const a = withFindings().items[1] as Step;
    a.annotations = [
      { id: "H1", type: "hotspot", geometry: { ...rect, title: "", description: "" } },
      { id: "H2", type: "hotspot", geometry: { ...rect, title: "Kitchen", description: "" } },
    ];
    expect(tabCounts(a)).toEqual({ step: 0, privacy: 1, hotspots: 1 });
  });
});
