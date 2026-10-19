import { describe, expect, it } from "vitest";
import { addTags, removeTag } from "@/text/tags";

describe("tags", () => {
  it("adds comma-separated tags, trimmed, skipping empty ones and duplicates", () => {
    expect(addTags(["calendar"], " sharing,,  team  events , Calendar,sharing")).toEqual([
      "calendar",
      "sharing",
      "team events",
    ]);
    expect(addTags([], "a\nb")).toEqual(["a", "b"]);
  });

  it("returns the same list when nothing is added", () => {
    const tags = ["a"];
    expect(addTags(tags, " , A ")).toBe(tags);
  });

  it("removes a tag by index", () => {
    expect(removeTag(["a", "b", "c"], 1)).toEqual(["a", "c"]);
    expect(removeTag(["a"], 3)).toEqual(["a"]);
  });
});
