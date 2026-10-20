import { useEffect, useRef, useState } from "react";
import type { ExportFormat } from "@/lib/exports";
import { requestGuideExport } from "@/server/guides";
import { ErrorText } from "../ui";

// Downloads of the revision on screen. The export worker makes each file
// once per revision; until it's ready, this polls and then starts the
// download.

const POLL_MS = 1500;

interface Props {
  guideId: string;
  revisionId: string;
  formats: { id: ExportFormat; label: string }[];
}

export function ExportMenu({ guideId, revisionId, formats }: Props) {
  const [working, setWorking] = useState<ExportFormat | null>(null);
  const [error, setError] = useState<string | null>(null);
  const cancelled = useRef(false);
  useEffect(
    () => () => {
      cancelled.current = true;
    },
    [],
  );

  async function download(format: ExportFormat) {
    setWorking(format);
    setError(null);
    let retry = true;
    while (!cancelled.current) {
      const result = await requestGuideExport({ data: { guideId, revisionId, format, retry } }).catch(() => ({
        status: "failed" as const,
        error: "Couldn't reach the server.",
      }));
      retry = false;
      if (result.status === "done") {
        window.location.assign(result.url);
        break;
      }
      if (result.status === "failed") {
        setError(result.error);
        break;
      }
      await new Promise((resolve) => setTimeout(resolve, POLL_MS));
    }
    setWorking(null);
  }

  if (!formats.length) return null;
  return (
    <details className="relative">
      <summary className="inline-flex h-9 cursor-pointer list-none items-center rounded-lg border border-border bg-card px-4 text-sm font-medium hover:bg-muted">
        {working ? "Preparing…" : "Download"}
      </summary>
      <div className="absolute right-0 z-20 mt-1 grid w-56 gap-1 rounded-lg border border-border bg-card p-1 shadow-lg">
        {formats.map((f) => (
          <button
            key={f.id}
            type="button"
            disabled={working !== null}
            onClick={() => download(f.id)}
            className="rounded-md px-3 py-2 text-left text-sm hover:bg-muted disabled:opacity-50"
          >
            {working === f.id ? `Preparing ${f.label}…` : f.label}
          </button>
        ))}
        <div className="px-3">
          <ErrorText>{error}</ErrorText>
        </div>
      </div>
    </details>
  );
}
