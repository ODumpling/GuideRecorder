import { describe, expect, it } from "vitest";
import { isUlid, ulid } from "@/lib/ulid";

describe("ulid", () => {
  it("encodes the time like the ULID spec", () => {
    // The spec's example timestamp.
    expect(ulid(1469918176385, (b) => b.fill(0)).slice(0, 10)).toBe("01ARYZ6S41");
    expect(ulid(0, (b) => b.fill(0))).toBe("00000000000000000000000000");
    expect(ulid(0, (b) => b.fill(255))).toBe("0000000000ZZZZZZZZZZZZZZZZ");
  });

  it("makes valid, distinct, time-ordered ids", () => {
    const ids = Array.from({ length: 200 }, (_, i) => ulid(1_700_000_000_000 + i));
    for (const id of ids) expect(isUlid(id), id).toBe(true);
    expect(new Set(ids).size).toBe(ids.length);
    expect([...ids].sort()).toEqual(ids);
    expect(isUlid(ulid())).toBe(true);
    expect(isUlid("01ARYZ6S41TSV4RRFFQ69G5FAI")).toBe(false); // I is not Crockford
    expect(isUlid("81ARYZ6S41TSV4RRFFQ69G5FAV")).toBe(false); // overflows 128 bits
  });
});
