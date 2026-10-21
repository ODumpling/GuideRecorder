import { describe, expect, it } from "vitest";
import { displayUrl, isWebAddress, MAX_SHOWN_URL_CHARS } from "./web";

describe("displayUrl", () => {
  it("drops the scheme, www and a lone trailing slash", () => {
    expect(displayUrl("https://www.example.com/")).toBe("example.com");
    expect(displayUrl("http://localhost:3000")).toBe("localhost:3000");
    expect(displayUrl("https://example.com/a/")).toBe("example.com/a/");
    expect(displayUrl("https://example.com/settings?tab=2")).toBe("example.com/settings?tab=2");
  });

  it("cuts long addresses", () => {
    const shown = displayUrl(`https://example.com/${"a".repeat(100)}`);
    expect(Array.from(shown)).toHaveLength(MAX_SHOWN_URL_CHARS);
    expect(shown.endsWith("…")).toBe(true);
  });
});

describe("isWebAddress", () => {
  it("takes http and https only", () => {
    expect(isWebAddress("https://example.com/settings")).toBe(true);
    expect(isWebAddress("http://localhost:3000/")).toBe(true);
    for (const url of ["javascript:alert(1)", "file:///etc/passwd", "example.com", "mailto:a@b.c", ""]) {
      expect(isWebAddress(url)).toBe(false);
    }
  });
});
