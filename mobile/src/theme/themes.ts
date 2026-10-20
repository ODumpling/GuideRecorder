// GuideRecorder's colour themes for the mobile app: the same tokens as
// packages/ui/src/theme.css (the desktop app's Settings → Appearance and the
// web app). Keep the two in step.

export type ThemeId = "arctic" | "paper" | "sage" | "paperDark" | "steel" | "nebula";

export interface Theme {
  id: ThemeId;
  name: string;
  dark: boolean;
  background: string;
  foreground: string;
  card: string;
  cardForeground: string;
  primary: string;
  primaryForeground: string;
  secondary: string;
  secondaryForeground: string;
  muted: string;
  mutedForeground: string;
  accent: string;
  accentForeground: string;
  destructive: string;
  success: string;
  warning: string;
  info: string;
  danger: string;
  neutral: string;
  border: string;
  input: string;
  ring: string;
  brand: string;
  brandForeground: string;
  ai: string;
}

export const themes: Record<ThemeId, Theme> = {
  arctic: {
    id: "arctic",
    name: "Arctic",
    dark: false,
    background: "#f3f6fa",
    foreground: "#0f1b2d",
    card: "#ffffff",
    cardForeground: "#0f1b2d",
    primary: "#0f1b2d",
    primaryForeground: "#ffffff",
    secondary: "#e6ecf3",
    secondaryForeground: "#0f1b2d",
    muted: "#e9eef4",
    mutedForeground: "#5f6f84",
    accent: "#e1e8f1",
    accentForeground: "#0f1b2d",
    destructive: "#d93b3b",
    success: "#11885f",
    warning: "#b7790c",
    info: "#2563eb",
    danger: "#d93b3b",
    neutral: "#5b6b80",
    border: "#d4dde8",
    input: "#cbd5e2",
    ring: "#2563eb",
    brand: "#2563eb",
    brandForeground: "#ffffff",
    ai: "#0e9fb3",
  },
  paper: {
    id: "paper",
    name: "Paper",
    dark: false,
    background: "#f5f3ee",
    foreground: "#1d1c1a",
    card: "#ffffff",
    cardForeground: "#1d1c1a",
    primary: "#1d1c1a",
    primaryForeground: "#ffffff",
    secondary: "#ece9e2",
    secondaryForeground: "#1d1c1a",
    muted: "#ece9e2",
    mutedForeground: "#74706a",
    accent: "#e7e3da",
    accentForeground: "#1d1c1a",
    destructive: "#c4321f",
    success: "#2f8457",
    warning: "#b0740c",
    info: "#d9482c",
    danger: "#c4321f",
    neutral: "#6b675f",
    border: "#e3dfd6",
    input: "#d9d4c9",
    ring: "#e8543a",
    brand: "#e8543a",
    brandForeground: "#ffffff",
    ai: "#6b5be6",
  },
  sage: {
    id: "sage",
    name: "Sage & Linen",
    dark: false,
    background: "#f1f0e8",
    foreground: "#1f2a22",
    card: "#ffffff",
    cardForeground: "#1f2a22",
    primary: "#2c4a3a",
    primaryForeground: "#f4f3ec",
    secondary: "#e4e6da",
    secondaryForeground: "#1f2a22",
    muted: "#e6e8dd",
    mutedForeground: "#66726a",
    accent: "#e1e4d6",
    accentForeground: "#1f2a22",
    destructive: "#b84a31",
    success: "#3f7d5c",
    warning: "#a3741a",
    info: "#3f7d5c",
    danger: "#b84a31",
    neutral: "#5e6a60",
    border: "#d8dcce",
    input: "#cdd2c2",
    ring: "#3f7d5c",
    brand: "#3f7d5c",
    brandForeground: "#ffffff",
    ai: "#b0643c",
  },
  paperDark: {
    id: "paperDark",
    name: "Paper Dark",
    dark: true,
    background: "#161615",
    foreground: "#eeece7",
    card: "#222120",
    cardForeground: "#eeece7",
    primary: "#eeece7",
    primaryForeground: "#1b1a19",
    secondary: "#2a2826",
    secondaryForeground: "#eeece7",
    muted: "#2a2826",
    mutedForeground: "#9a958c",
    accent: "#2c2a27",
    accentForeground: "#eeece7",
    destructive: "#ff6b57",
    success: "#5cc48e",
    warning: "#e9b44c",
    info: "#f0644a",
    danger: "#ff6b57",
    neutral: "#9a958c",
    border: "#2e2c29",
    input: "#3a3835",
    ring: "#f0644a",
    brand: "#f0644a",
    brandForeground: "#ffffff",
    ai: "#a397ff",
  },
  steel: {
    id: "steel",
    name: "Steel & Brass",
    dark: true,
    background: "#1a2129",
    foreground: "#e6eaee",
    card: "#232c36",
    cardForeground: "#e6eaee",
    primary: "#c9a35b",
    primaryForeground: "#1a1f26",
    secondary: "#2a3440",
    secondaryForeground: "#e6eaee",
    muted: "#26303b",
    mutedForeground: "#8c99a6",
    accent: "#2d3947",
    accentForeground: "#e6eaee",
    destructive: "#e07a5f",
    success: "#6fb58e",
    warning: "#d9a441",
    info: "#c9a35b",
    danger: "#e07a5f",
    neutral: "#8c99a6",
    border: "#33404d",
    input: "#3c4a58",
    ring: "#c9a35b",
    brand: "#c9a35b",
    brandForeground: "#1a1f26",
    ai: "#8fb3d9",
  },
  nebula: {
    id: "nebula",
    name: "Nebula",
    dark: true,
    background: "#0d0b1c",
    foreground: "#eeeaff",
    card: "#1a1538",
    cardForeground: "#eeeaff",
    primary: "#8b6cff",
    primaryForeground: "#ffffff",
    secondary: "#221c47",
    secondaryForeground: "#eeeaff",
    muted: "#1e1840",
    mutedForeground: "#9a92c8",
    accent: "#2a2257",
    accentForeground: "#eeeaff",
    destructive: "#ff5e8a",
    success: "#4be3a8",
    warning: "#ffc857",
    info: "#6fe3ff",
    danger: "#ff5e8a",
    neutral: "#9a92c8",
    border: "rgba(170, 150, 255, 0.16)",
    input: "rgba(170, 150, 255, 0.24)",
    ring: "#8b6cff",
    brand: "#ff4fb8",
    brandForeground: "#ffffff",
    ai: "#6fe3ff",
  },
};

/** Arctic in light mode, Paper Dark in dark mode. */
export type ThemeChoice = ThemeId | "system";

export function resolveTheme(choice: ThemeChoice, scheme: string | null | undefined): Theme {
  if (choice === "system") return scheme === "dark" ? themes.paperDark : themes.arctic;
  return themes[choice] ?? themes.arctic;
}

/** The shared radius (`--radius`, 0.625rem) and its steps, in points. */
export const radius = { sm: 6, md: 8, lg: 10, xl: 14, full: 999 } as const;

export const fonts = {
  sans: "Inter_400Regular",
  medium: "Inter_500Medium",
  semibold: "Inter_600SemiBold",
  bold: "Inter_700Bold",
  serif: "InstrumentSerif_400Regular",
  mono: "Menlo",
} as const;
