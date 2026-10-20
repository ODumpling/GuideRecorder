import { describe, expect, it } from "vitest";
import { apiRules, clientIp, clientKey, limitApiRequest, rateLimit } from "./rate-limit";

describe("rateLimit", () => {
  it("allows `limit` hits per window, per key", () => {
    const limit = rateLimit({ limit: 2, windowMs: 1000 });
    expect([limit.hit("a", 0), limit.hit("a", 10), limit.hit("a", 20)]).toEqual([true, true, false]);
    expect(limit.hit("b", 20)).toBe(true);
    expect(limit.retryAfter("a", 200)).toBe(1);
    expect(limit.hit("a", 1000)).toBe(true);
  });

  it("forgets old windows instead of growing without bound", () => {
    const limit = rateLimit({ limit: 1, windowMs: 1000, maxKeys: 2 });
    limit.hit("a", 0);
    limit.hit("b", 0);
    limit.hit("c", 2000);
    // "a" was dropped, so it starts a fresh window.
    expect(limit.hit("a", 2000)).toBe(true);
  });
});

describe("clients", () => {
  it("are known by their token, else their address", async () => {
    const ip = new Headers({ "x-forwarded-for": "203.0.113.7, 10.0.0.1" });
    expect(clientIp(ip)).toBe("203.0.113.7");
    expect(await clientKey(ip)).toBe("ip:203.0.113.7");
    const bearer = await clientKey(new Headers({ authorization: "Bearer secret-token", "x-forwarded-for": "203.0.113.7" }));
    expect(bearer).toMatch(/^u:[0-9a-f]{32}$/);
    expect(bearer).not.toContain("secret");
    const cookie = await clientKey(new Headers({ cookie: "theme=dark; __Secure-better-auth.session_token=secret-token" }));
    expect(cookie).toBe(bearer);
  });
});

describe("limitApiRequest", () => {
  const request = (path: string, method = "GET", token = "t") =>
    new Request(`http://localhost:3000${path}`, { method, headers: { authorization: `Bearer ${token}` } });

  it("answers 429 with Retry-After once a rule's limit is used up", async () => {
    const rules = apiRules();
    let last: Response | null = null;
    for (let i = 0; i < 121; i++) last = await limitApiRequest(request("/api/workspaces/w/assets/uploads", "POST"), rules, 0);
    expect(last?.status).toBe(429);
    expect(last?.headers.get("retry-after")).toBe("60");
    // Other rules and other clients are unaffected.
    expect(await limitApiRequest(request("/api/workspaces/w/sync/changes"), rules, 0)).toBeNull();
    expect(await limitApiRequest(request("/api/workspaces/w/assets/uploads", "POST", "other"), rules, 0)).toBeNull();
    // A minute later it's fine again.
    expect(await limitApiRequest(request("/api/workspaces/w/assets/uploads", "POST"), rules, 60_000)).toBeNull();
  });

  it("leaves pages, Better Auth and health checks alone", async () => {
    const rules = apiRules().map((r) => ({ ...r, limit: rateLimit({ limit: 0, windowMs: 1000 }) }));
    expect(await limitApiRequest(request("/dashboard"), rules)).toBeNull();
    expect(await limitApiRequest(request("/api/auth/sign-in/email", "POST"), rules)).toBeNull();
    expect(await limitApiRequest(request("/api/health"), rules)).toBeNull();
    expect((await limitApiRequest(request("/api/workspaces/w/sync/changes"), rules))?.status).toBe(429);
  });
});
