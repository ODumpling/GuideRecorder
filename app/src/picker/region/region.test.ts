import { describe, expect, it } from "vitest";
import { dragRect, isPickable, MIN_REGION_SIDE } from "@/picker/region/region";

const SCREEN = { width: 1440, height: 900 };

describe("region drawing", () => {
  it("makes the same rect whichever way the drag goes", () => {
    const expected = { x: 100, y: 50, width: 300, height: 200 };
    expect(dragRect({ x: 100, y: 50 }, { x: 400, y: 250 }, SCREEN)).toEqual(expected);
    expect(dragRect({ x: 400, y: 250 }, { x: 100, y: 50 }, SCREEN)).toEqual(expected);
    expect(dragRect({ x: 400, y: 50 }, { x: 100, y: 250 }, SCREEN)).toEqual(expected);
    expect(dragRect({ x: 100, y: 250 }, { x: 400, y: 50 }, SCREEN)).toEqual(expected);
  });

  it("stays on the screen and rounds to whole points", () => {
    expect(dragRect({ x: 1400, y: 880 }, { x: 1600, y: 1000 }, SCREEN)).toEqual({ x: 1400, y: 880, width: 40, height: 20 });
    expect(dragRect({ x: 20, y: 20 }, { x: -50, y: -10 }, SCREEN)).toEqual({ x: 0, y: 0, width: 20, height: 20 });
    expect(dragRect({ x: 10.4, y: 10.6 }, { x: 60.5, y: 70.2 }, SCREEN)).toEqual({ x: 10, y: 11, width: 51, height: 59 });
  });

  it("needs both sides at least 40", () => {
    expect(MIN_REGION_SIDE).toBe(40);
    expect(isPickable({ x: 0, y: 0, width: 40, height: 40 })).toBe(true);
    expect(isPickable({ x: 0, y: 0, width: 39, height: 400 })).toBe(false);
    expect(isPickable({ x: 0, y: 0, width: 400, height: 39 })).toBe(false);
    expect(isPickable(null)).toBe(false);
  });
});
