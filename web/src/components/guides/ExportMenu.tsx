import { useEffect, useRef, useState } from "react";
import type { ExportFormat } from "@/lib/exports";
import { requestGuideExport } from "@/server/guides";
import { Download } from "lucide-react";
import { Button } from "@guiderecorder/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@guiderecorder/ui/dropdown-menu";
import { ErrorText } from "@/components/ui";

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
    <div className="grid justify-items-end gap-1">
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button variant="ghost" disabled={working !== null}>
            <Download />
            {working ? `Preparing ${formats.find((f) => f.id === working)?.label ?? ""}…` : "Download"}
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-56">
          {formats.map((f) => (
            <DropdownMenuItem key={f.id} onSelect={() => void download(f.id)}>
              {f.label}
            </DropdownMenuItem>
          ))}
        </DropdownMenuContent>
      </DropdownMenu>
      <ErrorText>{error}</ErrorText>
    </div>
  );
}
