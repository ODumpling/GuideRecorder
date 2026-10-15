import { describe, expect, it } from "vitest";
import { regionPickId, toArrayBuffer } from "@/lib/ipc";

describe("toArrayBuffer", () => {
  it("passes ArrayBuffers through", () => {
    const buffer = new Uint8Array([1, 2, 3]).buffer;
    expect(toArrayBuffer(buffer)).toBe(buffer);
  });

  it("turns postMessage number arrays into bytes", () => {
    expect([...new Uint8Array(toArrayBuffer([82, 73, 70, 70, 255, 0]))]).toEqual([82, 73, 70, 70, 255, 0]);
    expect(toArrayBuffer([]).byteLength).toBe(0);
  });

  it("copies just the viewed bytes of a typed array", () => {
    const whole = new Uint8Array([9, 1, 2, 9]);
    expect([...new Uint8Array(toArrayBuffer(whole.subarray(1, 3)))]).toEqual([1, 2]);
  });
});

describe("describeError", () => {
  it("includes the name, cause and stack", async () => {
    const { describeError } = await import("@/lib/ipc-diagnostics");
    const error = new TypeError("Load failed", { cause: new Error("cancelled") });
    const text = describeError(error);
    expect(text).toMatch(/^TypeError: Load failed cause=Error: cancelled/);
    expect(text).toContain("ipc.test.ts");
    expect(describeError("plain")).toBe("plain");
  });
});

describe("regionPickId", () => {
  it("reads the pick id from the overlay's URL", () => {
    expect(regionPickId("?pick=12")).toBe(12);
    expect(regionPickId("?page=region&pick=0")).toBe(0);
  });

  it("is undefined when absent or not a whole number", () => {
    expect(regionPickId("")).toBeUndefined();
    expect(regionPickId("?page=region")).toBeUndefined();
    expect(regionPickId("?pick=")).toBeUndefined();
    expect(regionPickId("?pick=abc")).toBeUndefined();
    expect(regionPickId("?pick=-1")).toBeUndefined();
  });
});
