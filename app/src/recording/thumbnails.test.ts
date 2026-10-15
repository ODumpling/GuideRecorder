import { describe, expect, it } from "vitest";
import { thumbnailSize } from "@/recording/thumbnails";

describe("thumbnailSize", () => {
  it("fits the longer side, keeping the shape, and never scales up", () => {
    expect(thumbnailSize(3456, 2234, 160)).toEqual({ width: 160, height: 103 });
    expect(thumbnailSize(1080, 1920, 160)).toEqual({ width: 90, height: 160 });
    expect(thumbnailSize(100, 50, 160)).toEqual({ width: 100, height: 50 });
    expect(thumbnailSize(0, 0, 160)).toEqual({ width: 1, height: 1 });
  });
});
