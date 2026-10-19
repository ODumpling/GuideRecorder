import { describe, expect, it } from "vitest";
import { securityHeaders, withHeaders } from "./security-headers";

describe("securityHeaders", () => {
  it("locks pages down in production and allows the object store", () => {
    const headers = securityHeaders({ baseURL: "https://app.guiderecorder.app", storageEndpoint: "https://acct.r2.cloudflarestorage.com/", production: true });
    expect(headers["strict-transport-security"]).toContain("max-age=");
    expect(headers["x-content-type-options"]).toBe("nosniff");
    const csp = headers["content-security-policy"]!;
    expect(csp).toContain("frame-ancestors 'none'");
    expect(csp).toContain("object-src 'none'");
    expect(csp).toContain("img-src 'self' data: blob: https://acct.r2.cloudflarestorage.com");
    expect(csp).toContain("upgrade-insecure-requests");
  });

  it("skips the CSP in development and HSTS without HTTPS", () => {
    const headers = securityHeaders({ baseURL: "http://localhost:3000", production: false });
    expect(headers["content-security-policy"]).toBeUndefined();
    expect(headers["strict-transport-security"]).toBeUndefined();
    expect(headers["x-frame-options"]).toBe("DENY");
  });
});

describe("withHeaders", () => {
  it("adds headers without overriding the response's own", () => {
    const response = withHeaders(new Response("ok", { headers: { "x-frame-options": "SAMEORIGIN" } }), { "x-frame-options": "DENY", "x-a": "1" });
    expect(response.headers.get("x-frame-options")).toBe("SAMEORIGIN");
    expect(response.headers.get("x-a")).toBe("1");
  });

  it("copies responses whose headers can't change", () => {
    const response = withHeaders(Response.redirect("http://localhost/elsewhere", 302), { "x-a": "1" });
    expect(response.status).toBe(302);
    expect(response.headers.get("x-a")).toBe("1");
  });
});
