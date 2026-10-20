import { Link, useNavigate, useRouter, useRouterState } from "@tanstack/react-router";
import { useEffect, useRef, useState, type ReactNode } from "react";
import {
  BookOpen,
  CalendarClock,
  Check,
  ChevronsUpDown,
  EllipsisVertical,
  Folder,
  FolderOpen,
  LayoutDashboard,
  LogOut,
  Menu,
  ScrollText,
  Search,
  Settings,
  Shield,
  Sparkles,
  UserRound,
  X,
} from "lucide-react";
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
import { Avatar, STATUS_COLORS } from "@/components/guides/bits";
import { authClient } from "@/lib/auth-client";
import { GUIDE_STATUSES, STATUS_LABEL, type LibrarySearch } from "@/lib/library-search";
import type { getShell } from "@/server/shell";

// The signed-in pages' frame, laid out like the desktop app's library
// window: the library's sidebar (guides by status and folder) on the left,
// a search bar on top. Workspace settings and the account menu sit at the
// sidebar's foot.

export type Shell = Awaited<ReturnType<typeof getShell>>;

const PLAN_LABEL = { free: "Free", pro: "Pro", enterprise: "Enterprise" } as const;

export function AppShell({ shell, children }: { shell: Shell; children: ReactNode }) {
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
        <Sidebar shell={shell} />
      </aside>
      {open ? <div className="fixed inset-0 z-30 bg-black/30 md:hidden" onClick={() => setOpen(false)} /> : null}
      <div className="flex min-w-0 flex-1 flex-col">
        <header className="flex h-12 shrink-0 items-center gap-2 border-b border-border px-3">
          <Button variant="ghost" size="icon" className="md:hidden" aria-label={open ? "Close menu" : "Open menu"} onClick={() => setOpen(!open)}>
            {open ? <X /> : <Menu />}
          </Button>
          <div className="flex min-w-0 flex-1 justify-center">{shell.library ? <LibrarySearchBox /> : null}</div>
        </header>
        <main className="min-h-0 flex-1 overflow-y-auto">{children}</main>
      </div>
    </div>
  );
}

/** Searches the library; ⌘K (Ctrl+K) focuses it from anywhere. */
function LibrarySearchBox() {
  const navigate = useNavigate();
  const current = useRouterState({ select: (s) => (s.location.pathname === "/library" ? (s.location.search as LibrarySearch).q : undefined) });
  const [query, setQuery] = useState(current ?? "");
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => setQuery(current ?? ""), [current]);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        input.current?.focus();
        input.current?.select();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  return (
    <form
      role="search"
      className="w-full max-w-md"
      onSubmit={(e) => {
        e.preventDefault();
        void navigate({ to: "/library", search: query.trim() ? { q: query.trim() } : {} });
      }}
    >
      <label className="relative flex h-8 w-full items-center gap-2 rounded-lg border border-border/60 bg-foreground/[0.04] px-2.5 text-muted-foreground focus-within:border-ring focus-within:ring-2 focus-within:ring-ring/30">
        <Search className="size-4 shrink-0" />
        <input
          ref={input}
          type="search"
          aria-label="Search guides"
          placeholder="Search guides, steps and tags"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          className="h-full min-w-0 flex-1 bg-transparent text-sm text-foreground outline-none placeholder:text-muted-foreground"
        />
        <kbd className="rounded border border-border px-1 text-[10px] text-muted-foreground max-sm:hidden">⌘K</kbd>
      </label>
    </form>
  );
}

function Sidebar({ shell }: { shell: Shell }) {
  const location = useRouterState({ select: (s) => s.location });
  const search = (location.pathname === "/library" ? location.search : null) as LibrarySearch | null;
  // The library entry for exactly these filters is the current one.
  const at = (filter: LibrarySearch) =>
    search !== null &&
    !search.q &&
    !search.tag &&
    search.folder === filter.folder &&
    search.status === filter.status &&
    search.review === filter.review;
  const lib = shell.library;

  return (
    <>
      <div className="p-2.5 pb-0">
        <WorkspaceMenu shell={shell} />
      </div>
      <nav aria-label="Library" className="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto p-2.5 pt-3">
        <div className="flex flex-col gap-px">
          <NavItem to="/library" search={{}} icon={<BookOpen />} label="All guides" count={lib?.counts.all} current={at({})} />
          {lib ? (
            <NavItem
              to="/library"
              search={{ review: "due" }}
              icon={<CalendarClock />}
              label="Review due"
              count={lib.counts.reviewDue}
              warn
              current={at({ review: "due" })}
            />
          ) : null}
        </div>
        {lib ? (
          <>
            <div className="flex flex-col gap-px">
              <NavHeading>Status</NavHeading>
              {GUIDE_STATUSES.map((s) => (
                <NavItem
                  key={s}
                  to="/library"
                  search={{ status: s }}
                  icon={<span className="size-2 rounded-full" style={{ background: STATUS_COLORS[s] }} />}
                  label={STATUS_LABEL[s]}
                  count={lib.counts.statuses[s]}
                  current={at({ status: s })}
                />
              ))}
            </div>
            <div className="flex flex-col gap-px">
              <NavHeading>Folders</NavHeading>
              {lib.folders.length ? (
                <>
                  {lib.folders.map((f) => (
                    <NavItem
                      key={f.name}
                      to="/library"
                      search={{ folder: f.name }}
                      icon={search?.folder === f.name ? <FolderOpen /> : <Folder />}
                      label={f.name}
                      count={f.count}
                      current={at({ folder: f.name })}
                    />
                  ))}
                  {lib.counts.unfiled ? (
                    <NavItem to="/library" search={{ folder: "" }} icon={<Folder />} label="Unfiled" count={lib.counts.unfiled} current={at({ folder: "" })} />
                  ) : null}
                </>
              ) : (
                <p className="px-2 text-xs text-muted-foreground">No folders yet</p>
              )}
            </div>
          </>
        ) : null}
      </nav>
      <div className="flex flex-col gap-px border-t border-sidebar-border p-2.5">
        <NavItem to="/dashboard" icon={<Settings />} label="Workspace" current={location.pathname === "/dashboard"} />
        {shell.can.readAudit ? <NavItem to="/audit" icon={<ScrollText />} label="Audit log" current={location.pathname === "/audit"} /> : null}
        {shell.can.admin ? <NavItem to="/admin" icon={<Shield />} label="Admin" current={location.pathname === "/admin"} /> : null}
      </div>
      <div className="p-2.5 pt-0">
        <ProfileMenu shell={shell} />
      </div>
    </>
  );
}

/** The active workspace; switches workspace. */
function WorkspaceMenu({ shell }: { shell: Shell }) {
  const router = useRouter();
  const name = shell.active?.name ?? "No workspace";

  async function switchTo(organizationId: string) {
    await authClient.organization.setActive({ organizationId });
    await router.navigate({ to: "/library", search: {} });
    await router.invalidate();
  }

  return (
    <DropdownMenu>
      <DropdownMenuTrigger className="flex h-11 w-full items-center gap-2.5 rounded-lg px-2 text-left outline-none hover:bg-sidebar-foreground/5 focus-visible:ring-2 focus-visible:ring-sidebar-ring data-[state=open]:bg-sidebar-foreground/5">
        <span className="flex size-7 shrink-0 items-center justify-center rounded-md bg-brand text-xs font-semibold text-brand-foreground">
          {name.slice(0, 1).toUpperCase()}
        </span>
        <span className="flex min-w-0 flex-1 flex-col">
          <span className="truncate text-[13px] font-semibold">{name}</span>
          <span className="truncate text-[11px] text-muted-foreground">
            {shell.active ? `${PLAN_LABEL[shell.active.plan]} plan` : shell.user.email}
          </span>
        </span>
        <ChevronsUpDown className="size-4 shrink-0 text-muted-foreground" />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-64">
        {shell.workspaces.length > 1 ? (
          <>
            <DropdownMenuLabel className="text-xs font-normal text-muted-foreground">Workspaces</DropdownMenuLabel>
            {shell.workspaces.map((w) => (
              <DropdownMenuItem key={w.id} onSelect={() => w.id !== shell.active?.id && void switchTo(w.id)}>
                <span className="flex-1 truncate">{w.name}</span>
                {w.id === shell.active?.id ? <Check /> : null}
              </DropdownMenuItem>
            ))}
            <DropdownMenuSeparator />
          </>
        ) : null}
        <DropdownMenuItem asChild>
          <Link to="/dashboard">
            <Settings /> Workspace settings
          </Link>
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** The signed-in user: their profile, the workspace's plan, signing out. */
function ProfileMenu({ shell }: { shell: Shell }) {
  const router = useRouter();
  const name = shell.user.name || shell.user.email;

  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        aria-label="Account menu"
        className="flex h-11 w-full items-center gap-2.5 rounded-lg px-2 text-left outline-none hover:bg-sidebar-foreground/5 focus-visible:ring-2 focus-visible:ring-sidebar-ring data-[state=open]:bg-sidebar-foreground/5"
      >
        <Avatar name={name} label={shell.user.email} className="size-7 rounded-md text-[11px]" />
        <span className="flex min-w-0 flex-1 flex-col">
          <span className="truncate text-[13px] font-semibold">{shell.user.name || shell.user.email}</span>
          <span className="truncate text-[11px] text-muted-foreground">{shell.user.email}</span>
        </span>
        <EllipsisVertical className="size-4 shrink-0 text-muted-foreground" />
      </DropdownMenuTrigger>
      <DropdownMenuContent side="right" align="end" sideOffset={8} className="w-64">
        <DropdownMenuLabel className="flex items-center gap-2 font-normal">
          <Avatar name={name} label={shell.user.email} className="size-8 text-[11px]" />
          <span className="flex min-w-0 flex-col">
            <span className="truncate text-sm font-medium">{shell.user.name}</span>
            <span className="truncate text-xs text-muted-foreground">{shell.user.email}</span>
          </span>
        </DropdownMenuLabel>
        <DropdownMenuSeparator />
        <DropdownMenuItem asChild>
          <Link to="/profile">
            <UserRound /> Profile
          </Link>
        </DropdownMenuItem>
        {shell.staff ? (
          <DropdownMenuItem asChild>
            <Link to="/staff">
              <LayoutDashboard /> Admin dashboard
            </Link>
          </DropdownMenuItem>
        ) : null}
        {shell.active ? (
          <DropdownMenuItem asChild>
            <Link to="/dashboard" hash="plan">
              <Sparkles />
              <span className="flex-1">{shell.active.plan === "enterprise" ? "Plan and billing" : "Upgrade plan"}</span>
              <span className="text-xs text-muted-foreground">{PLAN_LABEL[shell.active.plan]}</span>
            </Link>
          </DropdownMenuItem>
        ) : null}
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

/** A small uppercase heading over a group of sidebar items. */
function NavHeading({ children }: { children: ReactNode }) {
  return <h2 className="flex h-6 items-center px-2 text-[11px] font-semibold tracking-wide text-muted-foreground uppercase">{children}</h2>;
}

function NavItem({
  to,
  search,
  icon,
  label,
  count,
  warn,
  current,
}: {
  to: "/library" | "/dashboard" | "/audit" | "/admin";
  search?: LibrarySearch;
  icon: ReactNode;
  label: string;
  count?: number;
  warn?: boolean;
  current: boolean;
}) {
  return (
    <Link
      to={to}
      search={search as never}
      aria-current={current ? "page" : undefined}
      className={cn(
        "flex h-7 w-full items-center gap-2 rounded-md px-2 text-left text-[13px] text-sidebar-foreground/80 outline-none hover:bg-sidebar-foreground/5 focus-visible:ring-2 focus-visible:ring-sidebar-ring",
        current && "bg-sidebar-accent font-medium text-sidebar-accent-foreground shadow-sm hover:bg-sidebar-accent",
      )}
    >
      <span className="flex size-4 shrink-0 items-center justify-center text-muted-foreground [&_svg]:size-4">{icon}</span>
      <span className="min-w-0 flex-1 truncate">{label}</span>
      {count !== undefined ? (
        <span className={cn("text-[11px] text-muted-foreground tabular-nums", warn && count > 0 && "font-semibold text-[var(--status-danger)]")}>
          {count}
        </span>
      ) : null}
    </Link>
  );
}
