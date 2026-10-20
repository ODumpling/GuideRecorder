import { Link, useNavigate, useRouter } from "@tanstack/react-router";
import { useState, type ReactNode } from "react";
import type { LibraryGuide } from "@/lib/library";
import { GUIDE_STATUSES, STATUS_LABEL, type LibrarySearch } from "@/lib/library-search";
import { setGuideFolder, type getLibrary } from "@/server/guides";
import { Button, Card, ErrorText, Input } from "../ui";

// The cloud library: the workspace's synced guides, searched and filtered
// by folder, tag, status and review date. Filters live in the URL.

type Data = Extract<Awaited<ReturnType<typeof getLibrary>>, { available: true }>;

const date = (iso: string) => new Date(iso).toLocaleDateString(undefined, { dateStyle: "medium" });

export function Library({ data, search }: { data: Data; search: LibrarySearch }) {
  const navigate = useNavigate({ from: "/library" });
  const [query, setQuery] = useState(search.q ?? "");
  const filtered = Boolean(search.q || search.folder !== undefined || search.tag || search.status || search.review);
  const set = (change: Partial<LibrarySearch>) => navigate({ search: (prev) => ({ ...prev, ...change }) });

  return (
    <div className="mx-auto grid w-full max-w-6xl gap-6 p-6">
      <header className="flex flex-wrap items-center justify-between gap-4">
        <div className="flex items-baseline gap-4">
          <Link to="/dashboard" className="text-sm text-muted-foreground hover:underline">
            ← Dashboard
          </Link>
          <h1 className="font-serif text-3xl">Library</h1>
        </div>
        <form
          role="search"
          className="flex w-full max-w-md gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            set({ q: query.trim() || undefined });
          }}
        >
          <Input type="search" aria-label="Search guides" placeholder="Search titles, steps, notes and tags" value={query} onChange={(e) => setQuery(e.target.value)} />
          <Button type="submit">Search</Button>
        </form>
      </header>

      <div className="grid gap-6 md:grid-cols-[14rem_minmax(0,1fr)]">
        <aside className="grid content-start gap-5 text-sm">
          <Facet title="Folders">
            <FacetLink active={search.folder === undefined} onClick={() => set({ folder: undefined })}>
              All guides
            </FacetLink>
            {data.folders.map((f) => (
              <FacetLink key={f.name} active={search.folder === f.name} count={f.count} onClick={() => set({ folder: f.name })}>
                {f.name}
              </FacetLink>
            ))}
            {data.folders.length ? (
              <FacetLink active={search.folder === ""} onClick={() => set({ folder: "" })}>
                Unfiled
              </FacetLink>
            ) : null}
          </Facet>
          <Facet title="Status">
            {GUIDE_STATUSES.map((s) => (
              <FacetLink key={s} active={search.status === s} onClick={() => set({ status: search.status === s ? undefined : s })}>
                {STATUS_LABEL[s]}
              </FacetLink>
            ))}
            <FacetLink active={search.review === "due"} onClick={() => set({ review: search.review ? undefined : "due" })}>
              Review due
            </FacetLink>
          </Facet>
          {data.tags.length ? (
            <Facet title="Tags">
              <div className="flex flex-wrap gap-1.5">
                {data.tags.map((t) => (
                  <button
                    key={t.name}
                    type="button"
                    aria-pressed={search.tag === t.name}
                    onClick={() => set({ tag: search.tag === t.name ? undefined : t.name })}
                    className={`rounded-full border px-2.5 py-0.5 text-xs ${search.tag === t.name ? "border-brand bg-brand text-brand-foreground" : "border-border bg-card hover:bg-muted"}`}
                  >
                    {t.name} <span className="opacity-70">{t.count}</span>
                  </button>
                ))}
              </div>
            </Facet>
          ) : null}
        </aside>

        <Card className="p-0">
          <div className="flex items-center justify-between gap-4 border-b border-border px-6 py-3 text-sm text-muted-foreground">
            <span>
              {data.guides.length} {data.guides.length === 1 ? "guide" : "guides"}
              {search.q ? ` matching “${search.q}”` : ""}
            </span>
            {filtered ? (
              <Link to="/library" search={{}} onClick={() => setQuery("")} className="hover:underline">
                Clear filters
              </Link>
            ) : null}
          </div>
          {data.guides.length ? (
            <ul className="divide-y divide-border">
              {data.guides.map((g) => (
                <GuideRow key={g.id} guide={g} folders={data.folders.map((f) => f.name)} canOrganize={data.canOrganize} onTag={(tag) => set({ tag })} />
              ))}
            </ul>
          ) : (
            <p className="p-6 text-sm text-muted-foreground">
              {filtered ? "No guides match." : "No synced guides yet. In the desktop app, sign in from Settings → Sync and turn sync on for a guide."}
            </p>
          )}
        </Card>
      </div>
    </div>
  );
}

function Facet({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="grid gap-1">
      <h2 className="px-2 text-xs font-semibold tracking-wide text-muted-foreground uppercase">{title}</h2>
      {children}
    </section>
  );
}

function FacetLink({ active, count, onClick, children }: { active: boolean; count?: number; onClick: () => void; children: ReactNode }) {
  return (
    <button
      type="button"
      aria-pressed={active}
      onClick={onClick}
      className={`flex items-center justify-between gap-2 rounded-md px-2 py-1 text-left ${active ? "bg-muted font-medium" : "text-muted-foreground hover:bg-muted hover:text-foreground"}`}
    >
      <span className="truncate">{children}</span>
      {count !== undefined ? <span className="tabular-nums text-xs">{count}</span> : null}
    </button>
  );
}

const STATUS_STYLE = {
  draft: "bg-muted text-muted-foreground",
  inReview: "bg-brand/10 text-brand",
  published: "bg-success/10 text-success",
  needsUpdate: "bg-destructive/10 text-destructive",
} as const;

function Badge({ className, children }: { className: string; children: ReactNode }) {
  return <span className={`rounded-full px-2 py-0.5 text-xs font-medium whitespace-nowrap ${className}`}>{children}</span>;
}

function GuideRow({ guide: g, folders, canOrganize, onTag }: { guide: LibraryGuide; folders: string[]; canOrganize: boolean; onTag: (tag: string) => void }) {
  const router = useRouter();
  const [moving, setMoving] = useState(false);
  const [folder, setFolder] = useState(g.folder ?? "");
  const [error, setError] = useState<string | null>(null);

  async function move(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    const result = await setGuideFolder({ data: { guideId: g.id, folder: folder.trim() || null } });
    if (!result.ok) return setError(result.error);
    setMoving(false);
    await router.invalidate();
  }

  return (
    <li className="grid gap-1.5 px-6 py-4">
      <div className="flex flex-wrap items-center gap-2">
        <Link to="/guides/$guideId" params={{ guideId: g.id }} className="font-medium hover:underline">
          {g.title}
        </Link>
        {g.status ? <Badge className={STATUS_STYLE[g.status]}>{STATUS_LABEL[g.status]}</Badge> : null}
        {g.review === "overdue" ? (
          <Badge className="bg-destructive/10 text-destructive">Review overdue since {date(g.reviewDue!)}</Badge>
        ) : g.review === "soon" ? (
          <Badge className="bg-amber-500/15 text-amber-700">Review due {date(g.reviewDue!)}</Badge>
        ) : null}
        {g.private ? <Badge className="bg-muted text-muted-foreground">Private</Badge> : null}
      </div>
      {g.summary ? <p className="text-sm text-muted-foreground">{g.summary}</p> : null}
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted-foreground">
        {g.folder ? <span>Folder: {g.folder}</span> : null}
        {g.owner ? <span>Owner: {g.owner}</span> : null}
        <span>
          {g.steps} {g.steps === 1 ? "step" : "steps"}
        </span>
        <span>
          Updated {date(g.updatedAt)} {g.source === "web" ? "on the web" : "in the app"}
        </span>
        {g.tags.map((t) => (
          <button key={t} type="button" onClick={() => onTag(t)} className="rounded-full border border-border px-2 py-px hover:bg-muted">
            {t}
          </button>
        ))}
        {canOrganize && !moving ? (
          <button type="button" onClick={() => setMoving(true)} className="hover:underline">
            {g.folder ? "Move" : "Add to folder"}
          </button>
        ) : null}
      </div>
      {moving ? (
        <form onSubmit={move} className="flex max-w-md items-center gap-2">
          <Input aria-label="Folder" list={`folders-${g.id}`} placeholder="Folder (empty: none)" value={folder} onChange={(e) => setFolder(e.target.value)} autoFocus />
          <datalist id={`folders-${g.id}`}>
            {folders.map((f) => (
              <option key={f} value={f} />
            ))}
          </datalist>
          <Button type="submit" className="h-8 px-3">
            Save
          </Button>
          <Button type="button" variant="secondary" className="h-8 px-3" onClick={() => setMoving(false)}>
            Cancel
          </Button>
        </form>
      ) : null}
      <ErrorText>{error}</ErrorText>
    </li>
  );
}
