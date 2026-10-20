import { Link, useRouter, useRouterState } from "@tanstack/react-router";
import { useEffect, useState, type ReactNode } from "react";
import { ArrowLeft, EllipsisVertical, LogOut, Menu, Users, X } from "lucide-react";
import { Button } from "@guiderecorder/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@guiderecorder/ui/dropdown-menu";
import { cn } from "@guiderecorder/ui/utils";
import { Avatar } from "@/components/guides/bits";
import { authClient } from "@/lib/auth-client";

// The staff console's frame, matching the signed-in pages' (AppShell): a
// sidebar with the console's sections, the way back to the app and the
// account menu at its foot.

interface Viewer {
  email: string;
  name: string;
}

export function StaffShell({ viewer, children }: { viewer: Viewer; children: ReactNode }) {
  const [open, setOpen] = useState(false);
  const href = useRouterState({ select: (s) => s.location.href });
  // The sidebar is a drawer on small screens; close it on every navigation.
  useEffect(() => setOpen(false), [href]);

  return (
    <div className="flex h-dvh">
      <aside
        className={cn(
          "z-40 w-60 shrink-0 flex-col border-r border-sidebar-border bg-sidebar text-sidebar-foreground max-md:fixed max-md:inset-y-0 max-md:left-0 max-md:shadow-2xl",
          open ? "flex" : "hidden md:flex",
        )}
      >
        <Sidebar viewer={viewer} />
      </aside>
      {open ? <div className="fixed inset-0 z-30 bg-black/30 md:hidden" onClick={() => setOpen(false)} /> : null}
      <div className="flex min-w-0 flex-1 flex-col">
        <header className="flex h-12 shrink-0 items-center gap-2 border-b border-border px-3 md:hidden">
          <Button variant="ghost" size="icon" aria-label={open ? "Close menu" : "Open menu"} onClick={() => setOpen(!open)}>
            {open ? <X /> : <Menu />}
          </Button>
        </header>
        <main className="min-h-0 flex-1 overflow-y-auto">
          <div className="mx-auto flex w-full max-w-5xl flex-col gap-7 px-5 py-8 md:px-9">{children}</div>
        </main>
      </div>
    </div>
  );
}

function Sidebar({ viewer }: { viewer: Viewer }) {
  const pathname = useRouterState({ select: (s) => s.location.pathname });

  return (
    <>
      <div className="flex h-[52px] items-center gap-2.5 px-4.5">
        <Link to="/staff" className="flex items-center gap-2 text-[13px] font-semibold">
          GuideRecorder
          <span className="rounded bg-status-warning/15 px-1.5 py-0.5 text-[11px] font-medium text-status-warning">Staff</span>
        </Link>
      </div>
      <nav aria-label="Staff console" className="flex min-h-0 flex-1 flex-col gap-px overflow-y-auto p-2.5 pt-1">
        <NavItem to="/staff" icon={<Users />} label="Users" current={pathname === "/staff" || pathname.startsWith("/staff/users")} />
      </nav>
      <div className="flex flex-col gap-px border-t border-sidebar-border p-2.5">
        <NavItem to="/library" icon={<ArrowLeft />} label="Back to app" current={false} />
      </div>
      <div className="p-2.5 pt-0">
        <AccountMenu viewer={viewer} />
      </div>
    </>
  );
}

function AccountMenu({ viewer }: { viewer: Viewer }) {
  const router = useRouter();
  const name = viewer.name || viewer.email;

  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        aria-label="Account menu"
        className="flex h-11 w-full items-center gap-2.5 rounded-lg px-2 text-left outline-none hover:bg-sidebar-foreground/5 focus-visible:ring-2 focus-visible:ring-sidebar-ring data-[state=open]:bg-sidebar-foreground/5"
      >
        <Avatar name={name} label={viewer.email} className="size-7 rounded-md text-[11px]" />
        <span className="flex min-w-0 flex-1 flex-col">
          <span className="truncate text-[13px] font-semibold">{name}</span>
          <span className="truncate text-[11px] text-muted-foreground">{viewer.email}</span>
        </span>
        <EllipsisVertical className="size-4 shrink-0 text-muted-foreground" />
      </DropdownMenuTrigger>
      <DropdownMenuContent side="right" align="end" sideOffset={8} className="w-64">
        <DropdownMenuLabel className="flex items-center gap-2 font-normal">
          <Avatar name={name} label={viewer.email} className="size-8 text-[11px]" />
          <span className="flex min-w-0 flex-col">
            <span className="truncate text-sm font-medium">{viewer.name}</span>
            <span className="truncate text-xs text-muted-foreground">{viewer.email}</span>
          </span>
        </DropdownMenuLabel>
        <DropdownMenuSeparator />
        <DropdownMenuItem
          onSelect={async () => {
            await authClient.signOut();
            await router.navigate({ to: "/sign-in" });
          }}
        >
          <LogOut /> Sign out
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function NavItem({ to, icon, label, current }: { to: "/staff" | "/library"; icon: ReactNode; label: string; current: boolean }) {
  return (
    <Link
      to={to}
      aria-current={current ? "page" : undefined}
      className={cn(
        "flex h-7 w-full items-center gap-2 rounded-md px-2 text-left text-[13px] text-sidebar-foreground/80 outline-none hover:bg-sidebar-foreground/5 focus-visible:ring-2 focus-visible:ring-sidebar-ring",
        current && "bg-sidebar-accent font-medium text-sidebar-accent-foreground shadow-sm hover:bg-sidebar-accent",
      )}
    >
      <span className="flex size-4 shrink-0 items-center justify-center text-muted-foreground [&_svg]:size-4">{icon}</span>
      <span className="min-w-0 flex-1 truncate">{label}</span>
    </Link>
  );
}
