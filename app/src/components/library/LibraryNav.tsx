import { useEffect, useMemo, useState, type ReactNode } from "react";
import { BookOpen, Camera, ChevronRight, Folder, FolderPlus, HardDrive, Target, TriangleAlert } from "lucide-react";
import { NavHeading, STATUS_COLORS } from "@/components/library/bits";
import { askName, showFolderMenu } from "@/components/library/menus";
import { editorStore, useEditor } from "@/editor/editor";
import {
  buildTree,
  GUIDE_KINDS,
  GUIDE_STATUSES,
  isReviewOverdue,
  libraryTags,
  localToday,
  type FolderNode,
} from "@/editor/library";
import { tauriSettingsApi, type SettingsApi } from "@/lib/ipc";
import { cn } from "@guiderecorder/ui/utils";

/** Indent per folder level, in pixels. */
const INDENT_PX = 12;

function NavItem({
  icon,
  label,
  count,
  warn,
  current,
  pad = 8,
  title,
  onClick,
  onContextMenu,
  children,
}: {
  icon: ReactNode;
  label: string;
  count?: number;
  warn?: boolean;
  current: boolean;
  /** Left padding, in pixels (folders indent by depth). */
  pad?: number;
  title?: string;
  onClick(): void;
  onContextMenu?(e: React.MouseEvent): void;
  children?: ReactNode;
}) {
  return (
    <div className="relative">
      {children}
      <button
        type="button"
        aria-current={current || undefined}
        title={title}
        onClick={onClick}
        onContextMenu={onContextMenu}
        style={{ paddingLeft: pad }}
        className={cn(
          "flex h-7 w-full items-center gap-2 rounded-md pr-2 text-left text-[13px] text-sidebar-foreground/80 outline-none hover:bg-sidebar-foreground/5 focus-visible:ring-2 focus-visible:ring-sidebar-ring",
          current && "bg-sidebar-accent font-medium text-sidebar-accent-foreground shadow-sm hover:bg-sidebar-accent",
        )}
      >
        <span className="flex size-4 shrink-0 items-center justify-center text-muted-foreground [&_svg]:size-4">
          {icon}
        </span>
        <span className="min-w-0 flex-1 truncate">{label}</span>
        {count !== undefined && (
          <span
            className={cn(
              "text-[11px] text-muted-foreground tabular-nums",
              warn && count > 0 && "font-semibold text-[var(--status-danger)]",
            )}
          >
            {count}
          </span>
        )}
      </button>
    </div>
  );
}

function countGuides(node: FolderNode): number {
  return node.guides.length + node.folders.reduce((n, f) => n + countGuides(f), 0);
}

function FolderItems({
  nodes,
  depth,
  current,
  collapsed,
  onToggle,
}: {
  nodes: FolderNode[];
  depth: number;
  current: string | null;
  collapsed: Set<string>;
  onToggle(path: string): void;
}) {
  return nodes.map((node) => {
    const open = !collapsed.has(node.path);
    const hasChildren = node.folders.length > 0;
    return (
      <div key={node.path}>
        <NavItem
          icon={<Folder />}
          label={node.name}
          count={countGuides(node)}
          pad={22 + depth * INDENT_PX}
          title={node.path}
          current={current === node.path}
          onClick={() => editorStore.getState().setFolder(node.path)}
          onContextMenu={(e) => void showFolderMenu(e, node.path, () => askName({ kind: "folder", parent: node.path }))}
        >
          {hasChildren && (
            <button
              type="button"
              aria-expanded={open}
              aria-label={open ? `Collapse ${node.name}` : `Expand ${node.name}`}
              onClick={() => onToggle(node.path)}
              style={{ left: 4 + depth * INDENT_PX }}
              className="absolute top-1.5 z-10 flex size-4 items-center justify-center rounded text-muted-foreground hover:text-foreground"
            >
              <ChevronRight className={cn("size-3 transition-transform", open && "rotate-90")} />
            </button>
          )}
        </NavItem>
        {open && hasChildren && (
          <FolderItems
            nodes={node.folders}
            depth={depth + 1}
            current={current}
            collapsed={collapsed}
            onToggle={onToggle}
          />
        )}
      </div>
    );
  });
}

/** Where the library is, at the bottom of the sidebar. */
function LibraryPlace({ api }: { api: SettingsApi }) {
  const [path, setPath] = useState<string | null>(null);
  useEffect(() => {
    api.libraryInfo().then(
      (info) => setPath(info.path),
      () => setPath(null),
    );
  }, [api]);
  if (!path) return null;
  return (
    <div className="m-2 flex items-start gap-2 rounded-lg bg-sidebar-foreground/5 p-2.5 text-[11px] text-muted-foreground">
      <HardDrive className="mt-px size-3.5 shrink-0" />
      <span className="min-w-0">
        <span className="block font-medium text-sidebar-foreground/80">Stored on this Mac</span>
        <span className="block truncate" title={path}>
          {path.replace(/^\/Users\/[^/]+/, "~")}
        </span>
      </span>
    </div>
  );
}

/** The library's sidebar: what to show in the grid (everything, a status, a folder or a tag). */
export function LibraryNav({ api = tauriSettingsApi }: { api?: SettingsApi }) {
  const guides = useEditor((s) => s.guides);
  const folders = useEditor((s) => s.folders);
  const statusFilter = useEditor((s) => s.statusFilter);
  const folder = useEditor((s) => s.folder);
  const tag = useEditor((s) => s.tag);
  const kind = useEditor((s) => s.kind);
  const [collapsed, setCollapsed] = useState<Set<string>>(() => new Set());
  const today = localToday();

  const tree = useMemo(() => buildTree(guides, folders, true), [guides, folders]);
  const tags = useMemo(() => libraryTags(guides), [guides]);
  const overdue = guides.filter((g) => isReviewOverdue(g, today)).length;
  const { setFolder, setStatusFilter, setTag, setKind } = editorStore.getState();
  const everything = !statusFilter && folder === null && !tag && !kind;

  const toggle = (path: string) =>
    setCollapsed((current) => {
      const next = new Set(current);
      if (!next.delete(path)) next.add(path);
      return next;
    });

  return (
    <nav aria-label="Library" className="flex min-h-0 flex-1 flex-col">
      <div className="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto p-2.5 pt-3">
        <div className="flex flex-col gap-px">
          <NavItem
            icon={<BookOpen />}
            label="All guides"
            count={guides.length}
            current={everything}
            onClick={() => {
              setFolder(null);
              setStatusFilter(null);
              setTag(null);
              setKind(null);
            }}
          />
          <NavItem
            icon={<TriangleAlert />}
            label="Review overdue"
            count={overdue}
            warn
            current={statusFilter === "overdue"}
            onClick={() => setStatusFilter(statusFilter === "overdue" ? null : "overdue")}
          />
        </div>

        <div className="flex flex-col gap-px">
          <NavHeading>Status</NavHeading>
          {GUIDE_STATUSES.map((s) => (
            <NavItem
              key={s.value}
              icon={<span className="size-2 rounded-full" style={{ background: STATUS_COLORS[s.value] }} />}
              label={s.label}
              count={guides.filter((g) => g.status === s.value).length}
              current={statusFilter === s.value}
              onClick={() => setStatusFilter(statusFilter === s.value ? null : s.value)}
            />
          ))}
        </div>

        <div className="flex flex-col gap-px">
          <NavHeading>Kind</NavHeading>
          {GUIDE_KINDS.map((k) => (
            <NavItem
              key={k.value}
              icon={k.value === "annotated" ? <Target /> : <Camera />}
              label={k.label}
              count={guides.filter((g) => g.kind === k.value).length}
              current={kind === k.value}
              onClick={() => setKind(kind === k.value ? null : k.value)}
            />
          ))}
        </div>

        <div className="flex flex-col gap-px">
          <NavHeading
            action={
              <button
                type="button"
                aria-label="New folder"
                title="New folder"
                onClick={() => askName({ kind: "folder", parent: folder ?? "" })}
                className="flex size-5 items-center justify-center rounded text-muted-foreground hover:bg-sidebar-foreground/10 hover:text-foreground"
              >
                <FolderPlus className="size-3.5" />
              </button>
            }
          >
            Folders
          </NavHeading>
          {tree.folders.length === 0 ? (
            <p className="px-2 text-xs text-muted-foreground">No folders yet</p>
          ) : (
            <FolderItems nodes={tree.folders} depth={0} current={folder} collapsed={collapsed} onToggle={toggle} />
          )}
        </div>

        {tags.length > 0 && (
          <div className="flex flex-col gap-1.5">
            <NavHeading>Tags</NavHeading>
            <div className="flex flex-wrap gap-1 px-2">
              {tags.map((t) => {
                const on = tag?.toLowerCase() === t.toLowerCase();
                return (
                  <button
                    key={t}
                    type="button"
                    aria-pressed={on}
                    onClick={() => setTag(on ? null : t)}
                    className={cn(
                      "rounded-md border px-2 py-0.5 text-[11px] outline-none focus-visible:ring-2 focus-visible:ring-sidebar-ring",
                      on
                        ? "border-transparent bg-primary text-primary-foreground"
                        : "border-sidebar-border bg-sidebar-foreground/5 text-sidebar-foreground/80 hover:bg-sidebar-foreground/10",
                    )}
                  >
                    {t}
                  </button>
                );
              })}
            </div>
          </div>
        )}
      </div>
      <LibraryPlace api={api} />
    </nav>
  );
}
