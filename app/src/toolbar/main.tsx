import React from "react";
import ReactDOM from "react-dom/client";
import { listen } from "@tauri-apps/api/event";
import "../index.css";
import "./toolbar.css";
import { installIpcDiagnostics } from "@/lib/ipc-diagnostics";
import { addManualStep, tauriRecordingApi } from "@/lib/ipc";
import { createRecordingStore, subscribe } from "@/recording/store";
import { FloatingToolbar } from "@/toolbar/FloatingToolbar";
import { createManualStepStore } from "@/toolbar/toolbar";

if (import.meta.env.DEV) installIpcDiagnostics();

document.documentElement.classList.add("toolbar-window");

// Follow the system appearance, as the main window does.
const dark = window.matchMedia("(prefers-color-scheme: dark)");
const applyTheme = () => document.documentElement.classList.toggle("dark", dark.matches);
applyTheme();
dark.addEventListener("change", applyTheme);

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
