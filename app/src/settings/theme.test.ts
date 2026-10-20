import { describe, expect, it } from "vitest";
import type { Settings } from "@/lib/types";
import {
  accentForeground,
  adjustAccent,
  appearanceFor,
  contrastRatio,
  DEFAULT_APPEARANCE,
  DEFAULT_THEME,
  isDarkTheme,
  MIN_ACCENT_CONTRAST,
  normalHex,
  parseAppearance,
  parseHex,
  parseTheme,
  themeBackground,
  themeFor,
  THEMES,
} from "@/settings/theme";

const general = {
  theme: "steel",
  matchSystem: false,
  lightTheme: "paper",
  darkTheme: "paperDark",
  accent: null,
  density: "comfortable",
  reduceEffects: false,
} as Settings["general"];

describe("match system", () => {
  it("uses the chosen theme when off", () => {
    expect(themeFor(general, true)).toBe("steel");
    expect(themeFor(general, false)).toBe("steel");
  });

  it("switches between the light and dark pair when on", () => {
    const matching = { ...general, matchSystem: true };
    expect(themeFor(matching, false)).toBe("paper");
    expect(themeFor(matching, true)).toBe("paperDark");
    expect(appearanceFor({ ...matching, darkTheme: "nebula", density: "compact" }, true)).toEqual({
      theme: "nebula",
      accent: null,
      density: "compact",
      reduceEffects: false,
    });
  });
});

describe("accent colours", () => {
  it("reads #rgb and #rrggbb", () => {
    expect(parseHex("#0a84FF")).toEqual([10, 132, 255]);
    expect(parseHex("#fff")).toEqual([255, 255, 255]);
    expect(parseHex("blue")).toBeNull();
    expect(normalHex("#ABC")).toBe("#aabbcc");
  });

  it("measures contrast like WCAG", () => {
    expect(contrastRatio("#000000", "#ffffff")).toBeCloseTo(21);
    expect(contrastRatio("#777", "#777")).toBeCloseTo(1);
  });

  it("keeps an accent that stands out", () => {
    expect(adjustAccent("#2563EB", "#f3f6fa")).toEqual({ color: "#2563eb", adjusted: false });
  });

  it("darkens a pale accent on a light theme and lightens a dark one on a dark theme", () => {
    const light = adjustAccent("#ffe066", themeBackground("paper"));
    expect(light.adjusted).toBe(true);
    expect(contrastRatio(light.color, themeBackground("paper"))).toBeGreaterThanOrEqual(MIN_ACCENT_CONTRAST);

    const dark = adjustAccent("#1a1a40", themeBackground("nebula"));
    expect(dark.adjusted).toBe(true);
    expect(contrastRatio(dark.color, themeBackground("nebula"))).toBeGreaterThanOrEqual(MIN_ACCENT_CONTRAST);
  });

  it("puts readable text on the accent", () => {
    expect(accentForeground("#2563eb")).toBe("#ffffff");
    expect(accentForeground("#ffe066")).toBe("#111111");
  });
});

describe("stored appearance", () => {
  it("round-trips, with defaults for anything missing or wrong", () => {
    const appearance = { theme: "sage", accent: "#0a84ff", density: "compact", reduceEffects: true } as const;
    expect(parseAppearance(JSON.stringify(appearance))).toEqual(appearance);
    expect(parseAppearance(null)).toEqual(DEFAULT_APPEARANCE);
    expect(parseAppearance("not json")).toEqual(DEFAULT_APPEARANCE);
    expect(parseAppearance(JSON.stringify({ theme: "graphite", accent: "red", density: "tiny" }))).toEqual(
      DEFAULT_APPEARANCE,
    );
  });
});

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
