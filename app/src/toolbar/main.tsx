import React from "react";
import ReactDOM from "react-dom/client";
import { listen } from "@tauri-apps/api/event";
import "../index.css";
import "./toolbar.css";
import { installIpcDiagnostics } from "@/lib/ipc-diagnostics";
import { addManualStep, tauriRecordingApi } from "@/lib/ipc";
import { createRecordingStore, subscribe } from "@/recording/store";
import { applyTheme, parseTheme, storedTheme, THEME_STORAGE_KEY } from "@/settings/theme";
import { FloatingToolbar } from "@/toolbar/FloatingToolbar";
import { createManualStepStore } from "@/toolbar/toolbar";

if (import.meta.env.DEV) installIpcDiagnostics();

document.documentElement.classList.add("toolbar-window");

// The main window's theme, which it keeps in the windows' shared storage.
applyTheme(storedTheme());
window.addEventListener("storage", (e) => {
  if (e.key === THEME_STORAGE_KEY) applyTheme(parseTheme(e.newValue));
});

// This window's own recording store. No `onSaved`: the main window handles
// `recording://saved` and opens the guide; Rust closes this window.
const recording = createRecordingStore({ api: tauriRecordingApi });
const manual = createManualStepStore({ addManualStep, recording });
// Subscribed before loading, so no event between the read and the listen is lost.
subscribe(recording, (event, handler) => listen(event, (e) => handler(e.payload)));
void recording.getState().load();

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <FloatingToolbar recording={recording} manual={manual} />
  </React.StrictMode>,
);
