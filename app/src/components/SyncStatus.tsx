import { RotateCw } from "lucide-react";
import { Button } from "@guiderecorder/ui/button";
import { timeAgo } from "@/editor/library";
import { syncStore, useSync } from "@/sync/sync";

/** The toolbar's sync status, signed in: when it last synced, and sync now. */
export function SyncStatus({ onSettings }: { onSettings(): void }) {
  const signedIn = useSync((s) => s.account?.signedIn ?? false);
  const running = useSync((s) => s.running);
  const error = useSync((s) => s.error);
  const lastSynced = useSync((s) => s.lastSynced);
  if (!signedIn) return null;

  const label = running ? "Syncing…" : error ? "Sync failed" : lastSynced ? `Synced ${timeAgo(lastSynced)}` : "Not synced yet";
  return (
    <div className="flex shrink-0 items-center">
      <button
        type="button"
        title={error ?? "Sync settings"}
        onClick={onSettings}
        className={`rounded-md px-1.5 py-1 text-xs whitespace-nowrap outline-none hover:bg-foreground/5 focus-visible:ring-2 focus-visible:ring-ring ${
          error && !running ? "text-[var(--status-danger)]" : "text-muted-foreground"
        }`}
      >
        {label}
      </button>
      <Button
        variant="ghost"
        size="icon"
        aria-label="Sync now"
        title="Sync now"
        disabled={running}
        onClick={() => void syncStore.getState().syncNow()}
      >
        <RotateCw className={running ? "animate-spin" : undefined} />
      </Button>
    </div>
  );
}
