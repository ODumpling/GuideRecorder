import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./index.css";
import { installIpcDiagnostics } from "@/lib/ipc-diagnostics";

if (import.meta.env.DEV) installIpcDiagnostics();

// Follow the system appearance; the CSS variables switch on `.dark`.
const dark = window.matchMedia("(prefers-color-scheme: dark)");
const applyTheme = () => document.documentElement.classList.toggle("dark", dark.matches);
applyTheme();
dark.addEventListener("change", applyTheme);

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
