import { describe, expect, it } from "vitest";
import { difference, pickSteps, sampleTimes, signature, type Signature } from "./frames";

/** A flat frame of one shade, with an optional bright square (a screen's content). */
function frame(shade: number, square?: number): Signature {
  const width = 10;
  const height = 10;
  const data = new Uint8Array(width * height).fill(shade);
  if (square !== undefined) for (let y = 2; y < 6; y++) for (let x = square; x < square + 4; x++) data[y * width + x] = 255;
  return { width, height, data };
}

describe("signature", () => {
  it("drops the status bar and home indicator and keeps luma", () => {
    const width = 2;
    const height = 100;
    const rgba = new Uint8Array(width * height * 4);
    // A white status bar; the rest mid grey.
    for (let y = 0; y < height; y++)
      for (let x = 0; x < width; x++) {
        const v = y < 7 ? 255 : 128;
        rgba.set([v, v, v, 255], (y * width + x) * 4);
      }
    const sig = signature(rgba, width, height);
    expect(sig.height).toBe(89);
    expect([...sig.data].every((v) => v === 128)).toBe(true);
  });
});

describe("difference", () => {
  it("is 0 for the same frame and grows with change", () => {
    expect(difference(frame(10), frame(10))).toBe(0);
    expect(difference(frame(10), frame(10, 0))).toBeGreaterThan(30);
  });
});

describe("pickSteps", () => {
  it("takes each settled screen at its last moment", () => {
    const frames = [frame(0, 0), frame(0, 0), frame(0, 0), frame(0, 5), frame(0, 5), frame(200), frame(200), frame(200)];
    expect(pickSteps(frames)).toEqual([2, 4, 7]);
  });

  it("skips screens that flash past, like transitions", () => {
    const frames = [frame(0, 0), frame(0, 0), frame(90), frame(0, 5), frame(0, 5)];
    expect(pickSteps(frames)).toEqual([1, 4]);
  });

  it("merges a screen that comes back unchanged after a blip", () => {
    const frames = [frame(0, 0), frame(0, 0), frame(250), frame(0, 0), frame(0, 0)];
    expect(pickSteps(frames)).toEqual([1]);
  });

  it("keeps the final screen even if brief", () => {
    expect(pickSteps([frame(0, 0), frame(0, 0), frame(0, 5)])).toEqual([1, 2]);
    expect(pickSteps([])).toEqual([]);
  });
});

describe("sampleTimes", () => {
  it("samples twice a second, ending near the end", () => {
    expect(sampleTimes(2)).toEqual([0, 0.5, 1, 1.5, 1.95]);
  });

  it("caps long recordings", () => {
    expect(sampleTimes(1200, 0.5, 600)).toHaveLength(600);
    expect(sampleTimes(0)).toEqual([0]);
  });
});
