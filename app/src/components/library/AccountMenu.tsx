import type { ReactNode } from "react";
import { EllipsisVertical, ExternalLink, LogIn, LogOut, RefreshCw, UserRound } from "lucide-react";
import { Button } from "@guiderecorder/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@guiderecorder/ui/dropdown-menu";
import { Avatar } from "@/components/library/bits";
import { tauriSyncApi } from "@/lib/ipc";
import { syncStore, useSync } from "@/sync/sync";

/**
 * Bottom of the sidebar: sign in to the web app (in the sign-in dialog), or,
 * signed in, who's signed in and their account menu, like the web app's.
 * The sync status sits in the toolbar.
 */
export function AccountMenu({ onSettings }: { onSettings(): void }) {
  const account = useSync((s) => s.account);
  const user = useSync((s) => s.user);
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

  const host = account.server?.replace(/^https?:\/\//, "") ?? "";
  const name = user?.name || user?.email || "Signed in";
  const detail = user?.email ?? host;
  const openWeb = (path: string) => void tauriSyncApi.openWeb(path).catch(() => {});

  return (
    <div className="p-2">
      <DropdownMenu>
        <DropdownMenuTrigger
          aria-label="Account menu"
          className="flex h-11 w-full items-center gap-2.5 rounded-lg px-2 text-left outline-none hover:bg-sidebar-foreground/5 focus-visible:ring-2 focus-visible:ring-sidebar-ring data-[state=open]:bg-sidebar-foreground/5"
        >
          <Avatar name={name} label={detail} className="size-7 rounded-md text-[11px]" />
          <span className="flex min-w-0 flex-1 flex-col">
            <span className="truncate text-[13px] font-semibold">{name}</span>
            <span className="truncate text-[11px] text-muted-foreground">{detail}</span>
          </span>
          <EllipsisVertical className="size-4 shrink-0 text-muted-foreground" />
        </DropdownMenuTrigger>
        <DropdownMenuContent side="right" align="end" sideOffset={8} className="w-64">
          <DropdownMenuLabel className="flex items-center gap-2 font-normal">
            <Avatar name={name} label={detail} className="size-8 text-[11px]" />
            <span className="flex min-w-0 flex-col">
              <span className="truncate text-sm font-medium">{name}</span>
              {user ? <span className="truncate text-xs text-muted-foreground">{user.email}</span> : null}
              <span className="truncate text-xs text-muted-foreground">{host}</span>
            </span>
          </DropdownMenuLabel>
          <DropdownMenuSeparator />
          <DropdownMenuItem onSelect={() => openWeb("/profile")}>
            <UserRound /> Profile
          </DropdownMenuItem>
          <DropdownMenuItem onSelect={() => openWeb("/library")}>
            <ExternalLink /> Open the web app
          </DropdownMenuItem>
          <DropdownMenuItem onSelect={onSettings}>
            <RefreshCw /> Sync settings…
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem onSelect={() => void store.signOut()}>
            <LogOut /> Sign out
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
}

function Card({ children }: { children: ReactNode }) {
  return <div className="m-2 flex flex-col items-start rounded-lg bg-sidebar-foreground/5 p-2.5 text-[11px] text-muted-foreground">{children}</div>;
}
