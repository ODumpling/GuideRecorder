import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./index.css";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { installIpcDiagnostics } from "@/lib/ipc-diagnostics";
import { settingsStore } from "@/settings/settings";
import { applyTheme, isDarkTheme, storedTheme } from "@/settings/theme";
import type { Theme } from "@/lib/types";

if (import.meta.env.DEV) installIpcDiagnostics();

// The theme from Settings → Appearance; until the settings load, the last one used.
let theme: Theme | null = null;
const showTheme = (next: Theme) => {
  if (next === theme) return;
  theme = next;
  applyTheme(next);
  // The title bar and native controls follow.
  void getCurrentWindow()
    .setTheme(isDarkTheme(next) ? "dark" : "light")
    .catch(() => {});
};
showTheme(storedTheme());
settingsStore.subscribe((s) => s.settings && showTheme(s.settings.general.theme));

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
