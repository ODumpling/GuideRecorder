import { describe, expect, it } from "vitest";
import type { GuideListItem } from "@/lib/types";
import {
  buildTree,
  copyTitle,
  folderName,
  folderPaths,
  addMonths,
  hasOwner,
  hasTag,
  isReviewOverdue,
  libraryOwners,
  libraryTags,
  localToday,
  matchesStatus,
  parentFolder,
  reviewDue,
  sortGuides,
} from "@/editor/library";

function item(title: string, extra: Partial<GuideListItem> = {}): GuideListItem {
  return {
    id: title,
    path: `/lib/${extra.folder ? `${extra.folder}/` : ""}${title}.guiderecorder`,
    title,
    summary: "",
    tags: [],
    stepCount: 0,
    createdAt: "2026-01-01T00:00:00Z",
    updatedAt: "2026-01-01T00:00:00Z",
    folder: "",
    readOnly: false,
    duplicateOf: null,
    ...extra,
  };
}

const titles = (guides: GuideListItem[]) => guides.map((g) => g.title);

describe("sorting", () => {
  const a = item("b Guide", { createdAt: "2026-03-01T00:00:00Z", updatedAt: "2026-04-01T00:00:00Z" });
  const b = item("A guide", { createdAt: "2026-05-01T00:00:00Z", updatedAt: "2026-02-01T00:00:00Z" });
  const c = item("Guide 10", { createdAt: "2026-01-01T00:00:00Z", updatedAt: "2026-06-01T00:00:00Z" });
  const d = item("Guide 9", { createdAt: "2026-01-01T00:00:00Z", updatedAt: "2026-06-01T00:00:00Z" });

  it("orders by update or creation, newest first, then by title", () => {
    expect(titles(sortGuides([a, b, c, d], "updated"))).toEqual(["Guide 9", "Guide 10", "b Guide", "A guide"]);
    expect(titles(sortGuides([a, b, c, d], "created"))).toEqual(["A guide", "b Guide", "Guide 9", "Guide 10"]);
  });

  it("orders titles A–Z ignoring case, numbers by value", () => {
    expect(titles(sortGuides([a, b, c, d], "title"))).toEqual(["A guide", "b Guide", "Guide 9", "Guide 10"]);
  });

  it("does not change its input", () => {
    const input = [a, b];
    sortGuides(input, "title");
    expect(input).toEqual([a, b]);
  });
});

describe("tags", () => {
  const guides = [item("One", { tags: ["Security", "api"] }), item("Two", { tags: ["security", " ", "macOS"] })];

  it("lists each tag once, ignoring case", () => {
    expect(libraryTags(guides)).toEqual(["api", "macOS", "Security"]);
  });

  it("matches tags ignoring case", () => {
    expect(guides.filter((g) => hasTag(g, "SECURITY")).map((g) => g.id)).toEqual(["One", "Two"]);
    expect(guides.filter((g) => hasTag(g, "api")).map((g) => g.id)).toEqual(["One"]);
  });
});

describe("folders", () => {
  it("names and parents", () => {
    expect(folderName("Work/Team")).toBe("Team");
    expect(folderName("Work")).toBe("Work");
    expect(parentFolder("Work/Team")).toBe("Work");
    expect(parentFolder("Work")).toBe("");
  });

  it("groups guides into nested folders, keeping empty ones unless filtering", () => {
    const guides = [item("Top"), item("Deep", { folder: "Work/Team" }), item("Mid", { folder: "Work" })];
    const tree = buildTree(guides, ["Archive", "Work", "Work/Team", "Empty"], true);
    expect(titles(tree.guides)).toEqual(["Top"]);
    expect(tree.folders.map((f) => f.path)).toEqual(["Archive", "Empty", "Work"]);
    const work = tree.folders[2];
    expect(titles(work.guides)).toEqual(["Mid"]);
    expect(work.folders.map((f) => [f.path, f.name])).toEqual([["Work/Team", "Team"]]);
    expect(titles(work.folders[0].guides)).toEqual(["Deep"]);

    const filtered = buildTree([guides[1]], ["Archive", "Work", "Work/Team"], false);
    expect(filtered.folders.map((f) => f.path)).toEqual(["Work"]);
    expect(filtered.folders[0].folders[0].guides.map((g) => g.title)).toEqual(["Deep"]);
  });

  it("lists every folder for moving, including ones only guides name", () => {
    expect(folderPaths(["Archive"], [item("Deep", { folder: "Work/Team" })])).toEqual([
      "Archive",
      "Work",
      "Work/Team",
    ]);
  });
});

describe("copy titles", () => {
  it("adds copy, then a number", () => {
    expect(copyTitle("Guide", ["Guide"])).toBe("Guide copy");
    expect(copyTitle("Guide", ["Guide", "guide copy"])).toBe("Guide copy 2");
    expect(copyTitle("Guide", ["Guide copy", "Guide copy 2"])).toBe("Guide copy 3");
    expect(copyTitle("  ", [])).toBe("Untitled copy");
  });
});

describe("review", () => {
  it("is due an interval after the last review, or after creation", () => {
    expect(reviewDue(item("A"))).toBeNull();
    expect(reviewDue(item("A", { reviewIntervalMonths: 0, lastReviewed: "2026-01-01" }))).toBeNull();
    expect(reviewDue(item("A", { reviewIntervalMonths: 3 }))).toBe("2026-04-01");
    expect(reviewDue(item("A", { reviewIntervalMonths: 3, lastReviewed: "2026-08-31" }))).toBe("2026-11-30");
    expect(reviewDue(item("A", { reviewIntervalMonths: 1, lastReviewed: "2000-01-31" }))).toBe("2000-02-29");
    expect(reviewDue(item("A", { reviewIntervalMonths: 12, lastReviewed: "2026-12-15" }))).toBe("2027-12-15");
    expect(reviewDue(item("A", { reviewIntervalMonths: 1, createdAt: "garbage" }))).toBeNull();
    expect(addMonths("not a day", 1)).toBeNull();
  });

  it("is overdue only after its due day", () => {
    const guide = item("A", { reviewIntervalMonths: 6, lastReviewed: "2026-04-19" });
    expect(isReviewOverdue(guide, "2026-10-19")).toBe(false);
    expect(isReviewOverdue(guide, "2026-10-20")).toBe(true);
    expect(isReviewOverdue(item("B"), "2999-01-01")).toBe(false);
    expect(matchesStatus(guide, "overdue", "2026-10-20")).toBe(true);
    expect(matchesStatus(item("C", { status: "needsUpdate" }), "needsUpdate", "2026-10-20")).toBe(true);
    expect(matchesStatus(item("D"), "published", "2026-10-20")).toBe(false);
  });

  it("formats today in local time", () => {
    expect(localToday(new Date(2026, 0, 5, 23, 59))).toBe("2026-01-05");
  });

  it("collects owners once each, ignoring case", () => {
    const guides = [item("A", { owner: "IT" }), item("B", { owner: " it " }), item("C", { owner: "Finance" }), item("D")];
    expect(libraryOwners(guides)).toEqual(["Finance", "IT"]);
    expect(hasOwner(guides[1], "IT")).toBe(true);
    expect(hasOwner(guides[3], "IT")).toBe(false);
  });
});
