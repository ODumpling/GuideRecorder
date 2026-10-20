import type { ReactNode } from "react";
import { Cloud, CloudAlert, LogIn, RotateCw } from "lucide-react";
import { Button } from "@guiderecorder/ui/button";
import { timeAgo } from "@/editor/library";
import { syncStore, useSync } from "@/sync/sync";

/**
 * Bottom of the sidebar: sign in to the web app (in the sign-in dialog), or,
 * signed in, the sync status. Settings → Sync has the rest.
 */
export function SyncCard({ onSettings }: { onSettings(): void }) {
  const account = useSync((s) => s.account);
  const running = useSync((s) => s.running);
  const error = useSync((s) => s.error);
  const lastSynced = useSync((s) => s.lastSynced);
  const store = syncStore.getState();

  if (!account?.signedIn) {
    return (
      <Card>
        <span className="block font-medium text-sidebar-foreground/80">Sync with the web</span>
        <span className="block">Sign in to sync guides with your workspace.</span>
        <Button size="sm" className="mt-2 w-full" onClick={() => store.openSignIn(true)}>
          <LogIn /> Sign in to sync
        </Button>
      </Card>
    );
  }

  const Icon = error ? CloudAlert : Cloud;
  const label = error ? "Sync failed" : running ? "Syncing…" : lastSynced ? `Synced ${timeAgo(lastSynced)}` : "Signed in";
  return (
    <div className="m-2 flex items-center gap-1 rounded-lg bg-sidebar-foreground/5 p-1 text-[11px] text-muted-foreground">
      <button
        type="button"
        title={error ?? undefined}
        className="flex min-w-0 flex-1 items-start gap-2 rounded-md p-1.5 text-left hover:bg-sidebar-foreground/5"
        onClick={onSettings}
      >
        <Icon className={`mt-px size-3.5 shrink-0 ${error ? "text-[var(--status-danger)]" : ""}`} />
        <span className="min-w-0">
          <span className="block font-medium text-sidebar-foreground/80">{label}</span>
          <span className="block truncate">{account.server?.replace(/^https?:\/\//, "")}</span>
        </span>
      </button>
      <Button variant="ghost" size="icon" aria-label="Sync now" title="Sync now" disabled={running} onClick={() => void store.syncNow()}>
        <RotateCw className={running ? "animate-spin" : undefined} />
      </Button>
    </div>
  );
}

function Card({ children }: { children: ReactNode }) {
  return <div className="m-2 flex flex-col items-start rounded-lg bg-sidebar-foreground/5 p-2.5 text-[11px] text-muted-foreground">{children}</div>;
}
