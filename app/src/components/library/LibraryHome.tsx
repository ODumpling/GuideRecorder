import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { AppWindow, LoaderCircle, Monitor, ScanLine, Search, Target } from "lucide-react";
import { GuideCard } from "@/components/library/GuideCard";
import { NameDialog } from "@/components/library/menus";
import { editorStore, useEditor } from "@/editor/editor";
import {
  folderName,
  folderPaths,
  GUIDE_KINDS,
  GUIDE_SORTS,
  GUIDE_STATUSES,
  hasOwner,
  hasTag,
  inFolder,
  isReviewOverdue,
  libraryOwners,
  localToday,
  matchesStatus,
  sortGuides,
  type GuideSort,
} from "@/editor/library";
import type { CaptureArea } from "@/lib/types";
import { startFlow, useRecording } from "@/recording/recording";
import { useSettings } from "@/settings/settings";
import { formatHotkey } from "@/settings/validate";
import { cn } from "@guiderecorder/ui/utils";

const SEARCH_DELAY_MS = 200;

/** The library search, in the title bar: filters the grid as you type; ⌘K focuses it. */
export function LibrarySearch() {
  const [query, setQuery] = useState(() => editorStore.getState().query);
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => {
    const timer = setTimeout(() => void editorStore.getState().setQuery(query), SEARCH_DELAY_MS);
    return () => clearTimeout(timer);
  }, [query]);
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key.toLowerCase() === "k" && (e.metaKey || e.ctrlKey) && !e.altKey && !e.shiftKey) {
        e.preventDefault();
        input.current?.focus();
        input.current?.select();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);
  return (
    <label className="relative flex h-8 w-full max-w-md items-center gap-2 rounded-lg border border-border/60 bg-foreground/[0.04] px-2.5 text-muted-foreground focus-within:border-ring focus-within:ring-2 focus-within:ring-ring/30">
      <Search className="size-4 shrink-0" />
      <input
        ref={input}
        aria-label="Search guides"
        placeholder="Search guides, steps and tags"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        onKeyDown={(e) => e.key === "Escape" && setQuery("")}
        className="h-full min-w-0 flex-1 bg-transparent text-sm text-foreground outline-none placeholder:text-muted-foreground"
      />
      <kbd className="rounded border border-border px-1 text-[10px] text-muted-foreground">⌘K</kbd>
    </label>
  );
}

/** One way to start a guide, above the grid. */
function StartTile({
  icon,
  title,
  note,
  hint,
  main,
  disabled,
  onClick,
}: {
  icon: ReactNode;
  title: string;
  note: string;
  hint?: string;
  main?: boolean;
  disabled: boolean;
  onClick(): void;
}) {
  return (
    <button
      type="button"
      disabled={disabled}
      onClick={onClick}
      className={cn(
        "flex items-center gap-3 rounded-xl border p-3.5 text-left transition duration-150 outline-none hover:-translate-y-px hover:shadow-md focus-visible:ring-2 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50",
        main ? "border-transparent bg-primary text-primary-foreground" : "border-border bg-card text-card-foreground",
      )}
    >
      <span
        className={cn(
          "flex size-9 shrink-0 items-center justify-center rounded-lg [&_svg]:size-[18px]",
          main ? "bg-brand text-brand-foreground" : "bg-foreground/[0.06] text-foreground/80",
        )}
      >
        {icon}
      </span>
      <span className="flex min-w-0 flex-col">
        <span className="truncate text-sm font-semibold">{title}</span>
        <span className={cn("truncate text-xs", main ? "text-primary-foreground/65" : "text-muted-foreground")}>
          {note}
        </span>
      </span>
      {hint && (
        <kbd className="ml-auto shrink-0 rounded bg-primary-foreground/15 px-1.5 py-px text-[11px] text-primary-foreground/80">
          {hint}
        </kbd>
      )}
    </button>
  );
}

function StartRow() {
  const idle = useRecording((s) => s.status.state === "idle" && s.pending === null);
  const starting = useRecording((s) => s.pending === "start");
  const creating = useEditor((s) => s.creating);
  const busy = useEditor((s) => s.busy || s.opening);
  const hotkey = useSettings((s) => s.settings?.recording.hotkeys.record);
  const record = (area: CaptureArea) => void startFlow.getState().begin(area);
  return (
    <div className="grid grid-cols-[1.3fr_1fr_1fr_1fr] gap-3 max-lg:grid-cols-2">
      <StartTile
        main
        icon={starting ? <LoaderCircle className="animate-spin" /> : <Monitor />}
        title="Record screen"
        note="Do the task once"
        hint={hotkey ? formatHotkey(hotkey) : undefined}
        disabled={!idle}
        onClick={() => record("display")}
      />
      <StartTile
        icon={<AppWindow />}
        title="Record a window"
        note="Just one app"
        disabled={!idle}
        onClick={() => record("window")}
      />
      <StartTile
        icon={<ScanLine />}
        title="Record a region"
        note="Draw an area"
        disabled={!idle}
        onClick={() => record("region")}
      />
      <StartTile
        icon={creating ? <LoaderCircle className="animate-spin" /> : <Target />}
        title="Annotation guide"
        note="From images or a PDF"
        disabled={creating || busy}
        onClick={() => void editorStore.getState().createAnnotatedGuide()}
      />
    </div>
  );
}

const selectClass =
  "h-8 rounded-lg border border-border bg-card px-2 text-xs text-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50";

/** The library: ways to start a guide, then the guides as cards. */
export function LibraryHome() {
  const guides = useEditor((s) => s.guides);
  const folders = useEditor((s) => s.folders);
  const sort = useEditor((s) => s.sort);
  const tag = useEditor((s) => s.tag);
  const statusFilter = useEditor((s) => s.statusFilter);
  const owner = useEditor((s) => s.owner);
  const kind = useEditor((s) => s.kind);
  const folder = useEditor((s) => s.folder);
  const query = useEditor((s) => s.query.trim());
  const listError = useEditor((s) => s.listError);
  const openError = useEditor((s) => s.openError);
  const today = localToday();

  const owners = useMemo(() => libraryOwners(guides), [guides]);
  // A tag or owner that no listed guide has any more (deleted, retagged) filters nothing out.
  const activeTag = tag && guides.some((g) => hasTag(g, tag)) ? tag : null;
  const activeOwner = owner && owners.some((o) => o.toLowerCase() === owner.toLowerCase()) ? owner : null;
  const shown = useMemo(() => {
    const filtered = guides.filter(
      (g) =>
        (folder === null || inFolder(g, folder)) &&
        (!activeTag || hasTag(g, activeTag)) &&
        (!activeOwner || hasOwner(g, activeOwner)) &&
        (!statusFilter || matchesStatus(g, statusFilter, today)) &&
        (!kind || g.kind === kind),
    );
    // A search lists best matches first; the sort applies to browsing.
    return query ? filtered : sortGuides(filtered, sort);
  }, [guides, folder, activeTag, activeOwner, statusFilter, kind, today, query, sort]);
  const moveTargets = useMemo(() => folderPaths(folders, guides), [folders, guides]);
  const overdue = shown.filter((g) => isReviewOverdue(g, today)).length;

  const title = query
    ? "Search"
    : folder
      ? folderName(folder)
      : statusFilter === "overdue"
        ? "Review overdue"
        : statusFilter
          ? GUIDE_STATUSES.find((s) => s.value === statusFilter)!.label
          : activeTag
            ? `#${activeTag}`
            : kind
              ? GUIDE_KINDS.find((k) => k.value === kind)!.label
              : "All guides";
  const filtered = !!(query || folder !== null || statusFilter || activeTag || activeOwner || kind);

  return (
    <div className="mx-auto flex w-full max-w-6xl flex-col gap-7 px-9 py-8">
      <div className="flex items-end gap-3">
        <div className="min-w-0 flex-1">
          <h1 className="truncate font-serif text-[40px] leading-none tracking-tight">{title}</h1>
          <p className="mt-2 text-sm text-muted-foreground">
            {query && <>Results for “{query}” · </>}
            {shown.length} {shown.length === 1 ? "guide" : "guides"}
            {overdue > 0 && <span className="text-[var(--status-danger)]"> · {overdue} due for review</span>}
          </p>
        </div>
        {owners.length > 0 && (
          <select
            aria-label="Filter by owner"
            value={activeOwner ?? ""}
            onChange={(e) => editorStore.getState().setOwner(e.target.value || null)}
            className={selectClass}
          >
            <option value="">Any owner</option>
            {owners.map((o) => (
              <option key={o} value={o}>
                {o}
              </option>
            ))}
          </select>
        )}
        <select
          aria-label="Sort guides"
          title={query ? "Search results are listed best match first" : "Sort guides"}
          disabled={!!query}
          value={sort}
          onChange={(e) => editorStore.getState().setSort(e.target.value as GuideSort)}
          className={selectClass}
        >
          {GUIDE_SORTS.map((o) => (
            <option key={o.value} value={o.value}>
              {o.label}
            </option>
          ))}
        </select>
      </div>

      {!filtered && <StartRow />}

      {(listError || openError) && (
        <p role="alert" className="text-sm text-[var(--status-danger)]">
          {listError ?? openError}
        </p>
      )}

      {shown.length > 0 ? (
        <ul aria-label="Guides" className="grid grid-cols-[repeat(auto-fill,minmax(15rem,1fr))] gap-x-5 gap-y-6">
          {shown.map((guide) => (
            <GuideCard key={guide.path} guide={guide} folders={moveTargets} />
          ))}
        </ul>
      ) : (
        !listError && (
          <p className="rounded-xl border border-dashed border-border px-6 py-14 text-center text-sm text-muted-foreground">
            {filtered ? "No matching guides" : "No guides yet. Record one to get started."}
          </p>
        )
      )}
      <NameDialog />
    </div>
  );
}
