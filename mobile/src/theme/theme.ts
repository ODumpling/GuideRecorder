import AsyncStorage from "@react-native-async-storage/async-storage";
import { useColorScheme } from "react-native";
import { create } from "zustand";
import { resolveTheme, themes, type Theme, type ThemeChoice } from "./themes";

const KEY = "guiderecorder.theme";

interface ThemeStore {
  choice: ThemeChoice;
  load(): Promise<void>;
  choose(choice: ThemeChoice): void;
}

/** The theme picked in Settings, kept on the device. */
export const themeStore = create<ThemeStore>((set) => ({
  choice: "system",
  async load() {
    const saved = await AsyncStorage.getItem(KEY);
    if (saved && (saved === "system" || saved in themes)) set({ choice: saved as ThemeChoice });
  },
  choose(choice) {
    set({ choice });
    void AsyncStorage.setItem(KEY, choice);
  },
}));

export function useTheme(): Theme {
  const choice = themeStore((s) => s.choice);
  return resolveTheme(choice, useColorScheme());
}
