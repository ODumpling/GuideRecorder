import React from "react";
import ReactDOM from "react-dom/client";
import { listen } from "@tauri-apps/api/event";
import "../index.css";
import "./toolbar.css";
import { installIpcDiagnostics } from "@/lib/ipc-diagnostics";
import { addManualStep, tauriFeedApi, tauriNarrationApi, tauriRecordingApi } from "@/lib/ipc";
import { createRecordingStore, subscribe } from "@/recording/store";
import { followStoredAppearance } from "@/settings/theme";
import { createFeedStore } from "@/toolbar/feed";
import { FloatingToolbar } from "@/toolbar/FloatingToolbar";
import { createMicStore, subscribeMic } from "@/toolbar/narration";
import { createManualStepStore } from "@/toolbar/toolbar";

if (import.meta.env.DEV) installIpcDiagnostics();

document.documentElement.classList.add("toolbar-window");

// The main window's appearance, which it keeps in the windows' shared storage.
followStoredAppearance();

// This window's own recording store. No `onSaved`: the main window handles
// `recording://saved` and opens the guide; Rust closes this window.
const recording = createRecordingStore({ api: tauriRecordingApi });
const manual = createManualStepStore({ addManualStep, recording });
const feed = createFeedStore({ api: tauriFeedApi, recording });
// Subscribed before loading, so no event between the read and the listen is lost.
subscribe(recording, (event, handler) => listen(event, (e) => handler(e.payload)));
void recording.getState().load();
const mic = createMicStore({ api: tauriNarrationApi });
void subscribeMic(mic, (event, handler) => listen(event, (e) => handler(e.payload as Parameters<typeof handler>[0])));
void mic.getState().load();

// The window follows the content's height (the bar, and the feed panel when it shows).
let height = 0;
const resize = (next: number) => {
  if (next === height) return;
  height = next;
  tauriFeedApi.resizeToolbar(next).catch((error) => console.error("resizing the toolbar failed:", error));
};

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <FloatingToolbar recording={recording} manual={manual} feed={feed} mic={mic} resize={resize} />
  </React.StrictMode>,
);
