import type { Density, Settings, Theme } from "@/lib/types";

export const DEFAULT_THEME: Theme = "arctic";

/** The themes in Settings → Appearance; `swatch` is background, sidebar and accent, for the picker. */
export const THEMES: { value: Theme; label: string; dark: boolean; swatch: [string, string, string] }[] = [
  { value: "arctic", label: "Arctic", dark: false, swatch: ["#f3f6fa", "#e8eef5", "#2563eb"] },
  { value: "paper", label: "Paper", dark: false, swatch: ["#f5f3ee", "#eeebe4", "#e8543a"] },
  { value: "sage", label: "Sage & Linen", dark: false, swatch: ["#f1f0e8", "#e6e8dc", "#3f7d5c"] },
  { value: "paperDark", label: "Paper Dark", dark: true, swatch: ["#161615", "#121211", "#f0644a"] },
  { value: "steel", label: "Steel & Brass", dark: true, swatch: ["#1a2129", "#161c23", "#c9a35b"] },
  { value: "nebula", label: "Nebula", dark: true, swatch: ["#0d0b1c", "#16112e", "#ff4fb8"] },
];

/** `value` if it names a theme, else the default. */
export function parseTheme(value: unknown): Theme {
  return THEMES.some((t) => t.value === value) ? (value as Theme) : DEFAULT_THEME;
}

export function isDarkTheme(theme: Theme): boolean {
  return THEMES.find((t) => t.value === theme)?.dark ?? false;
}

/** The theme's background colour, for checking the accent against. */
export function themeBackground(theme: Theme): string {
  return (THEMES.find((t) => t.value === theme) ?? THEMES[0]).swatch[0];
}

/** What a window shows: the theme in use and the other appearance options. */
export interface Appearance {
  theme: Theme;
  /** The custom accent as chosen, or null for the theme's own. */
  accent: string | null;
  density: Density;
  reduceEffects: boolean;
}

export const DEFAULT_APPEARANCE: Appearance = {
  theme: DEFAULT_THEME,
  accent: null,
  density: "comfortable",
  reduceEffects: false,
};

/** The theme the settings ask for: with Match system, the light or dark one of the pair. */
export function themeFor(general: Settings["general"], systemDark: boolean): Theme {
  if (!general.matchSystem) return general.theme;
  return systemDark ? general.darkTheme : general.lightTheme;
}

export function appearanceFor(general: Settings["general"], systemDark: boolean): Appearance {
  return {
    theme: themeFor(general, systemDark),
    accent: general.accent,
    density: general.density,
    reduceEffects: general.reduceEffects,
  };
}

/** `#rgb` or `#rrggbb` as red, green and blue (0–255), or null. */
export function parseHex(value: string): [number, number, number] | null {
  const hex = /^#([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(value.trim())?.[1];
  if (!hex) return null;
  const full = hex.length === 3 ? [...hex].map((c) => c + c).join("") : hex;
  return [0, 2, 4].map((i) => parseInt(full.slice(i, i + 2), 16)) as [number, number, number];
}

function toHex(rgb: number[]): string {
  return "#" + rgb.map((c) => Math.round(c).toString(16).padStart(2, "0")).join("");
}

/** `#rgb` or `#rrggbb` as lowercase `#rrggbb` (as a colour input wants), or null. */
export function normalHex(value: string): string | null {
  const rgb = parseHex(value);
  return rgb && toHex(rgb);
}

/** WCAG relative luminance. */
function luminance([r, g, b]: number[]): number {
  const linear = (c: number) => {
    const s = c / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b);
}

function contrast(a: number[], b: number[]): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

/** The WCAG contrast ratio of two hex colours (1–21); 1 if either isn't one. */
export function contrastRatio(a: string, b: string): number {
  const ca = parseHex(a);
  const cb = parseHex(b);
  return ca && cb ? contrast(ca, cb) : 1;
}

/** The least contrast an accent needs against the background (WCAG's for buttons and outlines). */
export const MIN_ACCENT_CONTRAST = 3;

/**
 * `accent` as shown on `background`: darkened (on a light background) or
 * lightened (on a dark one) until it has MIN_ACCENT_CONTRAST, as `#rrggbb`.
 * `adjusted` says whether it had to change.
 */
export function adjustAccent(accent: string, background: string): { color: string; adjusted: boolean } {
  const color = parseHex(accent);
  const bg = parseHex(background);
  if (!color || !bg) return { color: accent, adjusted: false };
  if (contrast(color, bg) >= MIN_ACCENT_CONTRAST) return { color: toHex(color), adjusted: false };
  const target = luminance(bg) > 0.18 ? 0 : 255;
  let mixed: number[] = color;
  for (let step = 1; step <= 20; step++) {
    mixed = color.map((c) => c + (target - c) * (step / 20));
    if (contrast(mixed, bg) >= MIN_ACCENT_CONTRAST) break;
  }
  return { color: toHex(mixed), adjusted: true };
}

/** Text on the accent: white or near-black, whichever reads better. */
export function accentForeground(accent: string): string {
  return contrastRatio(accent, "#ffffff") >= contrastRatio(accent, "#111111") ? "#ffffff" : "#111111";
}

/** The theme tokens a custom accent replaces: main buttons, selection, focus rings and annotation handles. */
const ACCENT_TOKENS = ["--primary", "--brand", "--ring", "--status-info", "--sidebar-primary", "--sidebar-ring", "--chart-1"];
const ACCENT_FOREGROUND_TOKENS = ["--primary-foreground", "--brand-foreground", "--sidebar-primary-foreground"];

/**
 * Where the last appearance is kept, so a window shows it before the
 * settings load. The windows share it, so the toolbar and the region
 * overlay follow the main window.
 */
export const APPEARANCE_STORAGE_KEY = "guiderecorder.appearance";

/** `value` (stored JSON) as an appearance, with defaults for anything missing or wrong. */
export function parseAppearance(value: string | null): Appearance {
  let raw: Partial<Record<keyof Appearance, unknown>> = {};
  try {
    const parsed: unknown = value ? JSON.parse(value) : null;
    if (parsed && typeof parsed === "object") raw = parsed;
  } catch {
    // Not JSON: the defaults.
  }
  return {
    theme: parseTheme(raw.theme),
    accent: typeof raw.accent === "string" && parseHex(raw.accent) ? raw.accent : null,
    density: raw.density === "compact" ? "compact" : "comfortable",
    reduceEffects: raw.reduceEffects === true,
  };
}

/** The appearance this window last used, or the default. */
export function storedAppearance(): Appearance {
  try {
    return parseAppearance(localStorage.getItem(APPEARANCE_STORAGE_KEY));
  } catch {
    return DEFAULT_APPEARANCE;
  }
}

function media(query: string): MediaQueryList | null {
  return typeof window !== "undefined" && window.matchMedia ? window.matchMedia(query) : null;
}

const DARK = "(prefers-color-scheme: dark)";
const REDUCE = ["(prefers-reduced-motion: reduce)", "(prefers-reduced-transparency: reduce)"];

/** Whether the system appearance is dark. */
export function systemIsDark(): boolean {
  return media(DARK)?.matches ?? false;
}

/** Whether the system asks for less motion or transparency. */
function systemReducesEffects(): boolean {
  return REDUCE.some((q) => media(q)?.matches ?? false);
}

/** Calls `onChange` when the system's appearance, motion or transparency preference changes. */
export function watchSystem(onChange: () => void): () => void {
  const lists = [DARK, ...REDUCE].map(media).filter((m): m is MediaQueryList => !!m);
  lists.forEach((m) => m.addEventListener("change", onChange));
  return () => lists.forEach((m) => m.removeEventListener("change", onChange));
}

/**
 * Shows `appearance` in this window and remembers it: index.css switches
 * on `data-theme`, `data-density` and `data-reduce-effects`; a custom
 * accent replaces the theme's accent tokens.
 */
export function applyAppearance(appearance: Appearance) {
  const root = document.documentElement;
  root.dataset.theme = appearance.theme;
  // For Tailwind's `dark:` variant.
  root.classList.toggle("dark", isDarkTheme(appearance.theme));
  root.dataset.density = appearance.density;
  root.toggleAttribute("data-reduce-effects", appearance.reduceEffects || systemReducesEffects());
  const accent = appearance.accent && adjustAccent(appearance.accent, themeBackground(appearance.theme)).color;
  for (const token of ACCENT_TOKENS) {
    if (accent) root.style.setProperty(token, accent);
    else root.style.removeProperty(token);
  }
  for (const token of ACCENT_FOREGROUND_TOKENS) {
    if (accent) root.style.setProperty(token, accentForeground(accent));
    else root.style.removeProperty(token);
  }
  try {
    localStorage.setItem(APPEARANCE_STORAGE_KEY, JSON.stringify(appearance));
  } catch {
    // Private storage off: the appearance still applies, it just isn't remembered.
  }
}

/**
 * For a window without the settings (the toolbar, the region overlay):
 * shows the main window's last appearance and follows its changes and the
 * system's.
 */
export function followStoredAppearance() {
  applyAppearance(storedAppearance());
  window.addEventListener("storage", (e) => {
    if (e.key === APPEARANCE_STORAGE_KEY) applyAppearance(parseAppearance(e.newValue));
  });
  watchSystem(() => applyAppearance(storedAppearance()));
}
