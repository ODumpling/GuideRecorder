import type { Theme } from "@/lib/types";

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

/**
 * Where the last theme is kept, so a window shows it before the settings
 * load. The windows share it, so the toolbar follows the main window.
 */
export const THEME_STORAGE_KEY = "guiderecorder.theme";

/** The theme this window last used, or the default. */
export function storedTheme(): Theme {
  try {
    return parseTheme(localStorage.getItem(THEME_STORAGE_KEY));
  } catch {
    return DEFAULT_THEME;
  }
}

/** Shows `theme` in this window (index.css switches on `data-theme`) and remembers it. */
export function applyTheme(theme: Theme) {
  const root = document.documentElement;
  root.dataset.theme = theme;
  // For Tailwind's `dark:` variant.
  root.classList.toggle("dark", isDarkTheme(theme));
  try {
    localStorage.setItem(THEME_STORAGE_KEY, theme);
  } catch {
    // Private storage off: the theme still applies, it just isn't remembered.
  }
}
