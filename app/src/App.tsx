import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface AppInfo {
  version: string;
  schemaVersion: number;
  captureBackend: string;
}

export default function App() {
  const [info, setInfo] = useState<AppInfo | null>(null);

  useEffect(() => {
    invoke<AppInfo>("app_info").then(setInfo).catch(console.error);
  }, []);

  return (
    <main className="flex min-h-screen flex-col items-center justify-center gap-2 bg-background text-foreground">
      <h1 className="text-2xl font-semibold">GuidesYa</h1>
      <p className="text-sm text-muted-foreground">
        {info
          ? `v${info.version} · schema v${info.schemaVersion} · capture: ${info.captureBackend}`
          : "Loading…"}
      </p>
    </main>
  );
}
