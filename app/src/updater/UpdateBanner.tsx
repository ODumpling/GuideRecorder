import { CircleCheck, Download, LoaderCircle, RotateCw, TriangleAlert, X } from "lucide-react";
import { Button } from "@guiderecorder/ui/button";
import { bannerFor } from "@/updater/store";
import { updaterStore, useUpdater } from "@/updater/updater";

/** "45%", or the MB so far when the size is unknown. */
export function progressText(downloaded: number, total: number | null): string {
  if (total) return `${Math.min(100, Math.floor((downloaded / total) * 100))}%`;
  return `${(downloaded / (1024 * 1024)).toFixed(1)} MB`;
}

/** Update available, downloading, ready to restart, or a check's result. */
export function UpdateBanner() {
  const banner = useUpdater(bannerFor);
  const error = useUpdater((s) => s.error);
  if (!banner && !error) return null;
  const { install, restart, dismiss } = updaterStore.getState();
  const dismissButton = (
    <Button variant="ghost" size="icon" aria-label="Dismiss" onClick={dismiss}>
      <X />
    </Button>
  );
  if (error) {
    return (
      <div role="alert" className="flex items-center gap-2 border-b border-border bg-muted px-4 py-2 text-sm">
        <TriangleAlert className="size-4 shrink-0 text-[var(--status-danger)]" />
        <span className="flex-1">Update: {error}</span>
        {dismissButton}
      </div>
    );
  }
  if (!banner) return null;
  switch (banner.state) {
    case "available":
      return (
        <div role="status" aria-label="Update" className="flex items-center gap-2 border-b border-border bg-muted px-4 py-2 text-sm">
          <Download className="size-4 shrink-0 text-[var(--status-info)]" />
          <span className="flex-1">GuideRecorder {banner.version} is available.</span>
          <Button variant="outline" size="sm" onClick={() => void install()}>
            Install update
          </Button>
          <Button variant="ghost" size="sm" onClick={dismiss}>
            Later
          </Button>
        </div>
      );
    case "downloading":
      return (
        <div role="status" aria-label="Update" className="flex items-center gap-2 border-b border-border bg-muted px-4 py-2 text-sm">
          <LoaderCircle className="size-4 shrink-0 animate-spin" />
          <span className="flex-1">
            Downloading GuideRecorder {banner.version}… {progressText(banner.downloaded, banner.total)}
          </span>
        </div>
      );
    case "ready":
      return (
        <div role="status" aria-label="Update" className="flex items-center gap-2 border-b border-border bg-muted px-4 py-2 text-sm">
          <CircleCheck className="size-4 shrink-0 text-[var(--status-success)]" />
          <span className="flex-1">GuideRecorder {banner.version} is installed. Restart to use it; your guide is saved first.</span>
          <Button variant="outline" size="sm" onClick={() => void restart()}>
            <RotateCw /> Restart now
          </Button>
        </div>
      );
    case "checking":
      return (
        <div role="status" aria-label="Update" className="flex items-center gap-2 border-b border-border bg-muted px-4 py-2 text-sm">
          <LoaderCircle className="size-4 shrink-0 animate-spin" />
          <span className="flex-1">Checking for updates…</span>
        </div>
      );
    case "upToDate":
      return (
        <div role="status" aria-label="Update" className="flex items-center gap-2 border-b border-border bg-muted px-4 py-2 text-sm">
          <CircleCheck className="size-4 shrink-0 text-[var(--status-success)]" />
          <span className="flex-1">GuideRecorder is up to date.</span>
          {dismissButton}
        </div>
      );
    case "failed":
      return (
        <div role="alert" aria-label="Update" className="flex items-center gap-2 border-b border-border bg-muted px-4 py-2 text-sm">
          <TriangleAlert className="size-4 shrink-0 text-[var(--status-danger)]" />
          <span className="flex-1">{banner.message}</span>
          <Button variant="outline" size="sm" onClick={() => void updaterStore.getState().check()}>
            Try again
          </Button>
          {dismissButton}
        </div>
      );
    default:
      return null;
  }
}
