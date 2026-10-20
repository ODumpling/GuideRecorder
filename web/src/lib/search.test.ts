import { readFileSync } from "node:fs";
import { expect, it } from "vitest";
import type { Guide } from "@guiderecorder/guide-schema";
import { prefixQuery, searchText } from "./search";

const floorPlan: Guide = JSON.parse(
  readFileSync(new URL("../../../fixtures/guides/Office floor plan.guiderecorder/guide.json", import.meta.url), "utf8"),
);

it("weights titles and tags over step titles over the rest", () => {
  const { a, b, c } = searchText(floorPlan);
  expect(a).toBe("Office floor plan office");
  expect(b).toBe("Find your way around the third floor. Third floor Fire exits");
  expect(c).toContain("Ask at **Reception**.");
  expect(c).toContain("Kitchen & café");
});

it("matches every word as a prefix, dropping tsquery syntax", () => {
  expect(prefixQuery("Rotate API")).toBe("rotate:* & api:*");
  expect(prefixQuery("café & 'drop' | !x:*")).toBe("café:* & drop:* & x:*");
  expect(prefixQuery("  !!  ")).toBeNull();
});
