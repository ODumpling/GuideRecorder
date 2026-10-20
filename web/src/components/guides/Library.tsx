import { Link, useNavigate, useRouter } from "@tanstack/react-router";
import { useEffect, useState, type FormEvent } from "react";
import { BookOpen, Camera, EllipsisVertical, FolderInput, LayoutGrid, List, Lock, Target, Trash2 } from "lucide-react";
import { Button } from "@guiderecorder/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@guiderecorder/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@guiderecorder/ui/dropdown-menu";
import { Input } from "@guiderecorder/ui/input";
import { cn } from "@guiderecorder/ui/utils";
import { ErrorText } from "@/components/ui";
import type { LibraryGuide } from "@/lib/library";
import { GUIDE_STATUSES, STATUS_LABEL, type LibrarySearch } from "@/lib/library-search";
import { setGuideFolder, setGuideTrashed, type getLibrary } from "@/server/guides";
import { Avatar, StatusPill, timeAgo } from "./bits";

// The cloud library: the workspace's synced guides, searched and filtered
// by folder, tag, status and review date (filters live in the URL; the
// sidebar sets most of them). Laid out like the desktop app's library.

type Data = Extract<Awaited<ReturnType<typeof getLibrary>>, { available: true }>;
type View = "grid" | "list";

const VIEW_KEY = "guiderecorder.libraryView";
const date = (iso: string) => new Date(iso).toLocaleDateString(undefined, { dateStyle: "medium" });

function heading(search: LibrarySearch) {
  if (search.status) return STATUS_LABEL[search.status];
  if (search.review) return "Review due";
  if (search.folder !== undefined) return search.folder || "Unfiled";
  if (search.tag) return `#${search.tag}`;
  return "All guides";
}

export function Library({ data, search }: { data: Data; search: LibrarySearch }) {
  const navigate = useNavigate({ from: "/library" });
  const set = (change: Partial<LibrarySearch>) => navigate({ search: (prev) => ({ ...prev, ...change }) });
  const [view, setView] = useState<View>("grid");
  const [moving, setMoving] = useState<LibraryGuide | null>(null);
  const [error, setError] = useState<string | null>(null);
  const router = useRouter();
  useEffect(() => {
    if (localStorage.getItem(VIEW_KEY) === "list") setView("list");
  }, []);
  const changeView = (v: View) => {
    setView(v);
    localStorage.setItem(VIEW_KEY, v);
  };
  const filtered = Boolean(search.q || search.folder !== undefined || search.tag || search.status || search.review);
  const overdue = data.guides.filter((g) => g.review === "overdue").length;
  const cover = (g: LibraryGuide) => (g.cover ? `/api/workspaces/${data.workspaceId}/assets/${g.cover}` : null);

  async function trash(g: LibraryGuide) {
    setError(null);
    const result = await setGuideTrashed({ data: { guideId: g.id, trashed: true } });
    if (!result.ok) setError(result.error);
    else await router.invalidate();
  }
  const actions = (g: LibraryGuide) =>
    data.canOrganize || data.canDelete ? (
      <GuideMenu guide={g} onMove={data.canOrganize ? () => setMoving(g) : undefined} onTrash={data.canDelete ? () => trash(g) : undefined} />
    ) : null;

  return (
    <div className="mx-auto flex w-full max-w-6xl flex-col gap-7 px-5 py-8 md:px-9">
      <div className="flex flex-wrap items-end gap-3">
        <div className="min-w-0 flex-1">
          <h1 className="truncate font-serif text-[40px] leading-none tracking-tight">{search.q ? "Search" : heading(search)}</h1>
          <p className="mt-2 text-sm text-muted-foreground">
            {search.q && <>Results for “{search.q}” · </>}
            {data.guides.length} {data.guides.length === 1 ? "guide" : "guides"}
            {overdue > 0 && <span className="text-[var(--status-danger)]"> · {overdue} due for review</span>}
            {filtered ? (
              <>
                {" · "}
                <Link to="/library" search={{}} className="hover:text-foreground hover:underline">
                  Clear filters
                </Link>
              </>
            ) : null}
          </p>
        </div>
        <ViewToggle view={view} onChange={changeView} />
      </div>

      <div className="flex flex-col gap-3">
        {data.counts.all > 0 ? (
          <div className="flex flex-wrap items-center gap-1">
            <Chip on={!search.status} onClick={() => set({ status: undefined })}>
              All
            </Chip>
            {GUIDE_STATUSES.map((s) => (
              <Chip key={s} on={search.status === s} onClick={() => set({ status: search.status === s ? undefined : s })}>
                {STATUS_LABEL[s]}
              </Chip>
            ))}
            {data.tags.length ? <span className="mx-1 h-4 w-px bg-border" /> : null}
            {data.tags.map((t) => (
              <Chip key={t.name} on={search.tag === t.name} onClick={() => set({ tag: search.tag === t.name ? undefined : t.name })}>
                #{t.name}
              </Chip>
            ))}
          </div>
        ) : null}
        <ErrorText>{error}</ErrorText>
        {data.guides.length === 0 ? (
          <p className="rounded-xl border border-dashed border-border px-6 py-14 text-center text-sm text-muted-foreground">
            {filtered
              ? "No matching guides"
              : "No guides yet. Record one in the desktop app, then turn on sync for it from Settings → Sync."}
          </p>
        ) : view === "grid" ? (
          <ul className="grid grid-cols-[repeat(auto-fill,minmax(15rem,1fr))] gap-x-5 gap-y-6">
            {data.guides.map((g) => (
              <GuideCard key={g.id} guide={g} cover={cover(g)} actions={actions(g)} />
            ))}
          </ul>
        ) : (
          <ul className="divide-y divide-border overflow-hidden rounded-xl border border-border bg-card">
            {data.guides.map((g) => (
              <GuideRow key={g.id} guide={g} cover={cover(g)} actions={actions(g)} />
            ))}
          </ul>
        )}
      </div>
      <MoveDialog guide={moving} folders={data.folders.map((f) => f.name)} onClose={() => setMoving(null)} />
    </div>
  );
}

function Chip({ on, onClick, children }: { on: boolean; onClick: () => void; children: React.ReactNode }) {
  return (
    <button
      type="button"
      aria-pressed={on}
      onClick={onClick}
      className={cn(
        "rounded-full px-2.5 py-1 text-xs outline-none focus-visible:ring-2 focus-visible:ring-ring",
        on ? "bg-primary text-primary-foreground" : "text-foreground/80 hover:bg-foreground/[0.06]",
      )}
    >
      {children}
    </button>
  );
}

function ViewToggle({ view, onChange }: { view: View; onChange: (view: View) => void }) {
  const options = [
    { value: "grid" as const, label: "Grid", icon: <LayoutGrid /> },
    { value: "list" as const, label: "List", icon: <List /> },
  ];
  return (
    <div role="radiogroup" aria-label="View" className="inline-flex rounded-lg bg-foreground/[0.06] p-0.5">
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          role="radio"
          aria-checked={view === o.value}
          aria-label={o.label}
          title={o.label}
          onClick={() => onChange(o.value)}
          className={cn(
            "flex h-7 w-8 items-center justify-center rounded-md text-muted-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring [&_svg]:size-4",
            view === o.value && "bg-card text-foreground shadow-sm",
          )}
        >
          {o.icon}
        </button>
      ))}
    </div>
  );
}

const contents = (g: LibraryGuide) => `${g.steps} ${g.steps === 1 ? "step" : "steps"}`;

/** A guide as a card: its first screenshot, title, status, folder and owner (the desktop app's GuideCard). */
function GuideCard({ guide: g, cover, actions }: { guide: LibraryGuide; cover: string | null; actions: React.ReactNode }) {
  const meta = [g.folder, timeAgo(g.updatedAt)].filter(Boolean).join(" · ");
  return (
    <li className="group relative">
      <Link to="/guides/$guideId" params={{ guideId: g.id }} title={g.summary || undefined} className="flex flex-col rounded-xl outline-none">
        <span className="relative block aspect-[16/10] overflow-hidden rounded-xl border border-border bg-sidebar transition duration-200 group-hover:-translate-y-0.5 group-hover:shadow-lg group-has-[a:focus-visible]:ring-2 group-has-[a:focus-visible]:ring-ring">
          {cover ? (
            <img src={cover} alt="" loading="lazy" className="absolute top-[11%] left-[8%] w-full rounded-tl-lg border-t border-l border-black/5 shadow-md" />
          ) : (
            <span className="flex size-full items-center justify-center text-muted-foreground/50">
              <BookOpen className="size-8" strokeWidth={1.5} />
            </span>
          )}
          <span className="absolute bottom-2.5 left-2.5 flex items-center gap-1 rounded-md bg-black/70 px-2 py-0.5 text-[11px] font-semibold text-white backdrop-blur">
            <Camera className="size-3" /> {contents(g)}
          </span>
          {g.review === "overdue" ? (
            <span
              title={`Review was due ${date(g.reviewDue!)}`}
              className="absolute top-2.5 right-2.5 rounded-md bg-[var(--status-danger)] px-2 py-0.5 text-[11px] font-semibold text-white"
            >
              Review overdue
            </span>
          ) : null}
        </span>
        <span className="mt-2.5 flex items-center gap-1.5 px-0.5 pr-8 text-sm font-semibold tracking-tight">
          <span className="truncate">{g.title}</span>
          {g.private ? <Lock className="size-3 shrink-0 text-muted-foreground" aria-label="Private" /> : null}
        </span>
        <span className="mt-1 flex min-w-0 items-center gap-2 px-0.5 text-xs text-muted-foreground">
          {g.status ? <StatusPill status={g.status} /> : null}
          <span className="truncate">{meta}</span>
          {g.owner ? <Avatar name={g.owner} label={`Owner ${g.owner}`} className="ml-auto" /> : null}
        </span>
      </Link>
      {actions ? <div className="absolute right-0 bottom-6">{actions}</div> : null}
    </li>
  );
}

/** A guide as a list row (the desktop app's list view). */
function GuideRow({ guide: g, cover, actions }: { guide: LibraryGuide; cover: string | null; actions: React.ReactNode }) {
  return (
    <li className="relative flex items-center gap-3 px-3 py-2.5 hover:bg-muted/50">
      <span className="relative h-10 w-16 shrink-0 overflow-hidden rounded-md border border-border bg-sidebar">
        {cover ? (
          <img src={cover} alt="" loading="lazy" className="absolute top-[12%] left-[10%] w-full rounded-tl-sm" />
        ) : (
          <Target className="m-auto mt-2.5 size-4 text-muted-foreground/50" />
        )}
      </span>
      <span className="flex min-w-0 flex-1 flex-col">
        <Link to="/guides/$guideId" params={{ guideId: g.id }} className="truncate text-sm font-semibold after:absolute after:inset-0">
          {g.title}
          {g.private ? <Lock className="ml-1.5 inline size-3 text-muted-foreground" aria-label="Private" /> : null}
        </Link>
        <span className="truncate text-xs text-muted-foreground">{g.summary || contents(g)}</span>
      </span>
      <span className="hidden w-28 shrink-0 md:block">{g.status ? <StatusPill status={g.status} /> : null}</span>
      <span className="hidden w-32 shrink-0 truncate text-xs text-muted-foreground lg:block">{g.folder ?? "—"}</span>
      <span
        className={cn("hidden w-28 shrink-0 text-xs text-muted-foreground sm:block", g.review === "overdue" && "font-medium text-[var(--status-danger)]")}
        title={g.reviewDue ? `Review due ${date(g.reviewDue)}` : undefined}
      >
        {g.review === "overdue" ? "Review overdue" : timeAgo(g.updatedAt)}
      </span>
      <span className="w-5 shrink-0">{g.owner ? <Avatar name={g.owner} label={`Owner ${g.owner}`} /> : null}</span>
      <span className="relative w-7 shrink-0">{actions}</span>
    </li>
  );
}

function GuideMenu({ guide, onMove, onTrash }: { guide: LibraryGuide; onMove?: () => void; onTrash?: () => void }) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" size="icon" aria-label={`Actions for ${guide.title}`} className="relative text-muted-foreground">
          <EllipsisVertical />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        {onMove ? (
          <DropdownMenuItem onSelect={onMove}>
            <FolderInput /> {guide.folder ? "Move to folder…" : "Add to folder…"}
          </DropdownMenuItem>
        ) : null}
        {onMove && onTrash ? <DropdownMenuSeparator /> : null}
        {onTrash ? (
          <DropdownMenuItem variant="destructive" onSelect={onTrash}>
            <Trash2 /> Move to trash
          </DropdownMenuItem>
        ) : null}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** Files a guide in a folder (a new name makes one), or takes it out of its folder. */
function MoveDialog({ guide, folders, onClose }: { guide: LibraryGuide | null; folders: string[]; onClose: () => void }) {
  const router = useRouter();
  const [folder, setFolder] = useState("");
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    setFolder(guide?.folder ?? "");
    setError(null);
  }, [guide]);

  async function save(e: FormEvent) {
    e.preventDefault();
    if (!guide) return;
    setError(null);
    const result = await setGuideFolder({ data: { guideId: guide.id, folder: folder.trim() || null } });
    if (!result.ok) return setError(result.error);
    onClose();
    await router.invalidate();
  }

  return (
    <Dialog open={guide !== null} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="sm:max-w-md">
        <form onSubmit={save} className="grid gap-4">
          <DialogHeader>
            <DialogTitle>Move “{guide?.title}”</DialogTitle>
            <DialogDescription>Pick a folder or type a new one. Leave it empty to unfile the guide.</DialogDescription>
          </DialogHeader>
          <Input aria-label="Folder" list="library-folders" placeholder="Folder" value={folder} onChange={(e) => setFolder(e.target.value)} autoFocus />
          <datalist id="library-folders">
            {folders.map((f) => (
              <option key={f} value={f} />
            ))}
          </datalist>
          <ErrorText>{error}</ErrorText>
          <DialogFooter>
            <Button variant="outline" size="lg" onClick={onClose}>
              Cancel
            </Button>
            <Button variant="brand" size="lg" type="submit">
              Move
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
