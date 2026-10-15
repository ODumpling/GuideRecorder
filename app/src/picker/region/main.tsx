import React from "react";
import ReactDOM from "react-dom/client";
import "../../index.css";
import "./region.css";
import { installIpcDiagnostics } from "@/lib/ipc-diagnostics";
import { finishRegionPick, regionPickId } from "@/lib/ipc";
import { RegionOverlay } from "@/picker/region/RegionOverlay";

if (import.meta.env.DEV) installIpcDiagnostics();

document.documentElement.classList.add("region-window");

// Which pick this overlay answers; Rust ignores a stale overlay's answer.
const pick = regionPickId(location.search);

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <RegionOverlay onFinish={(result) => finishRegionPick(result, pick)} />
  </React.StrictMode>,
);
