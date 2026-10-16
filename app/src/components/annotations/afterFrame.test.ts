import { describe, expect, it } from "vitest";
import { afterFrame } from "@/components/annotations/afterFrame";

const crop = { x: 10, y: 20, width: 32, height: 20 };
const size = { width: 64, height: 40 };

describe("afterFrame", () => {
  it("waits for the after frame's size", () => {
    expect(afterFrame(null, null, null)).toBeNull();
    expect(afterFrame(crop, null, size)).toBeNull();
  });

  it("shows the whole after frame without a crop", () => {
    expect(afterFrame(null, size, null)).toEqual({ x: 0, y: 0, width: 64, height: 40 });
  });

  it("shows the crop's region when the sizes match", () => {
    expect(afterFrame(crop, size, null)).toBeNull();
    expect(afterFrame(crop, size, { ...size })).toEqual(crop);
  });

  it("falls back to the whole after frame when the sizes differ", () => {
    expect(afterFrame(crop, size, { width: 128, height: 80 })).toEqual({ x: 0, y: 0, width: 64, height: 40 });
    expect(afterFrame(crop, size, { width: 64, height: 41 })).toEqual({ x: 0, y: 0, width: 64, height: 40 });
  });
});
