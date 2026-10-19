import { describe, expect, it } from "vitest";
import { DEFAULT_THEME, isDarkTheme, parseTheme, THEMES } from "@/settings/theme";

describe("themes", () => {
  it("defaults to Arctic", () => {
    expect(DEFAULT_THEME).toBe("arctic");
    expect(THEMES.map((t) => t.value)).toContain(DEFAULT_THEME);
  });

  it("lists each theme once", () => {
    const values = THEMES.map((t) => t.value);
    expect(new Set(values).size).toBe(values.length);
  });

  it("falls back to the default for anything that isn't a theme", () => {
    expect(parseTheme("nebula")).toBe("nebula");
    expect(parseTheme("graphite")).toBe("arctic");
    expect(parseTheme(null)).toBe("arctic");
  });

  it("knows which themes are dark", () => {
    expect(isDarkTheme("arctic")).toBe(false);
    expect(isDarkTheme("sage")).toBe(false);
    expect(isDarkTheme("paperDark")).toBe(true);
    expect(isDarkTheme("steel")).toBe(true);
    expect(isDarkTheme("nebula")).toBe(true);
  });
});
