import { describe, expect, it } from "vitest";
import { clientIp, rateLimit } from "./rate-limit";

describe("rateLimit", () => {
  it("allows the limit per window, per key", () => {
    const limit = rateLimit({ limit: 2, windowMs: 1000 });
    expect([limit.hit("a", 0), limit.hit("a", 10), limit.hit("a", 20)]).toEqual([true, true, false]);
    expect(limit.hit("b", 20)).toBe(true);
    expect(limit.hit("a", 1000)).toBe(true);
  });

  it("forgets old keys", () => {
    const limit = rateLimit({ limit: 1, windowMs: 1000, maxKeys: 2 });
    limit.hit("a", 0);
    limit.hit("b", 0);
    limit.hit("c", 0);
    // `a` was dropped to make room, so it starts a fresh window.
    expect(limit.hit("a", 1)).toBe(true);
  });
});

describe("clientIp", () => {
  it("takes the first forwarded hop", () => {
    expect(clientIp(new Headers({ "x-forwarded-for": "203.0.113.7, 10.0.0.1" }))).toBe("203.0.113.7");
    expect(clientIp(new Headers())).toBe("unknown");
  });
});
