import { describe, expect, it } from "vitest";
import { diffLines, diffStats, guideLines, versionName } from "@/editor/versions";
import { guide, step } from "@/editor/test-fixtures";

describe("guideLines", () => {
  it("lists the guide's fields, then each item, numbering steps", () => {
    const g = guide();
    g.intro = "Read this\nfirst";
    g.tags = ["pdf", "q3"];
    g.items[1] = { ...step("A", "Click **Save**"), description: "Then wait.\n\nDone." };
    expect(guideLines(g)).toEqual([
      "Title: Guide",
      "Intro: Read this",
      "first",
      "Tags: pdf, q3",
      "",
      "## Start",
      "",
      "1. Click **Save**",
      "   Then wait.",
      "   Done.",
      "",
      "2. Step B",
      "",
      "Tip: Tip",
      "",
      "3. Step C",
    ]);
  });
});

describe("diffLines", () => {
  it("keeps common lines and marks the rest", () => {
    const diff = diffLines(["a", "b", "c", "d"], ["a", "c", "x", "d"]);
    expect(diff).toEqual([
      { kind: "same", text: "a" },
      { kind: "removed", text: "b" },
      { kind: "same", text: "c" },
      { kind: "added", text: "x" },
      { kind: "same", text: "d" },
    ]);
    expect(diffStats(diff)).toEqual({ added: 1, removed: 1 });
  });

  it("puts removed lines before the lines added in their place", () => {
    expect(diffLines(["a", "old", "z"], ["a", "new", "z"]).map((l) => l.kind)).toEqual([
      "same",
      "removed",
      "added",
      "same",
    ]);
  });

  it("handles empty sides and identical input", () => {
    expect(diffLines([], ["a"])).toEqual([{ kind: "added", text: "a" }]);
    expect(diffLines(["a"], [])).toEqual([{ kind: "removed", text: "a" }]);
    expect(diffStats(diffLines(["a", "b"], ["a", "b"]))).toEqual({ added: 0, removed: 0 });
  });

  it("falls back to a block replace for huge changes", () => {
    const before = Array.from({ length: 2500 }, (_, i) => `old ${i}`);
    const after = Array.from({ length: 2500 }, (_, i) => `new ${i}`);
    const diff = diffLines(["same", ...before], ["same", ...after]);
    expect(diff[0]).toEqual({ kind: "same", text: "same" });
    expect(diffStats(diff)).toEqual({ added: 2500, removed: 2500 });
  });
});

describe("versionName", () => {
  it("prefers the label", () => {
    const base = { id: "V", createdAt: "", title: "", stepCount: 0, updatedAt: "" };
    expect(versionName({ ...base, reason: "beforeAi" })).toBe("Before Write with AI");
    expect(versionName({ ...base, reason: "manual", label: "Audit" })).toBe("Audit");
  });
});
