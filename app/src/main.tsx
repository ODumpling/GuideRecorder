import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./index.css";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { installIpcDiagnostics } from "@/lib/ipc-diagnostics";
import { settingsStore } from "@/settings/settings";
import { appearanceFor, applyAppearance, isDarkTheme, storedAppearance, systemIsDark, watchSystem } from "@/settings/theme";

if (import.meta.env.DEV) installIpcDiagnostics();

// The appearance from Settings → Appearance; until the settings load, the last one used.
let windowTheme: "light" | "dark" | null | undefined;
const showAppearance = () => {
  const general = settingsStore.getState().settings?.general;
  const appearance = general ? appearanceFor(general, systemIsDark()) : storedAppearance();
  applyAppearance(appearance);
  // The title bar and native controls follow. With Match system they follow
  // the system, which keeps `prefers-color-scheme` telling us its mode.
  const next = general?.matchSystem ? null : isDarkTheme(appearance.theme) ? "dark" : "light";
  if (next === windowTheme) return;
  windowTheme = next;
  void getCurrentWindow()
    .setTheme(next)
    .catch(() => {});
};
showAppearance();
settingsStore.subscribe((s, prev) => s.settings?.general !== prev.settings?.general && showAppearance());
// The system's light or dark mode, and its motion and transparency preferences.
watchSystem(showAppearance);
void getCurrentWindow()
  .onThemeChanged(showAppearance)
  .catch(() => {});

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
