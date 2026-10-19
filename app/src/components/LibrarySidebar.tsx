import { useEffect, useMemo, useState, type FormEvent, type MouseEvent } from "react";
import { Dialog } from "radix-ui";
import { BookOpen, ChevronDown, ChevronRight, Folder, FolderPlus, Lock, Search, X } from "lucide-react";
import { Menu } from "@tauri-apps/api/menu";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { editorStore, useEditor } from "@/editor/editor";
import {
  buildTree,
  folderName,
  folderPaths,
  GUIDE_SORTS,
  hasTag,
  libraryTags,
  sortGuides,
  type FolderNode,
  type GuideSort,
} from "@/editor/library";
import type { GuideListItem } from "@/lib/types";
import { cn } from "@/lib/utils";

const SEARCH_DELAY_MS = 200;
/** Indent per folder level, in pixels. */
const INDENT_PX = 12;

/** What the name dialog is asking for. */
type Naming = { kind: "rename"; guide: GuideListItem } | { kind: "folder"; parent: string };

/** The native context menu of a guide in the list. */
async function showGuideMenu(
  event: MouseEvent,
  guide: GuideListItem,
  folders: string[],
  onRename: () => void,
) {
  event.preventDefault();
  const editor = editorStore.getState();
  const enabled = !editor.busy;
  const moveTo = (folder: string, text: string) => ({
    id: `move-to:${folder}`,
    text,
    enabled: enabled && folder !== guide.folder,
    action: () => void editorStore.getState().moveGuide(guide, folder),
  });
  try {
    const menu = await Menu.new({
      items: [
        { id: "rename-guide", text: "Rename…", enabled, action: onRename },
        {
          id: "duplicate-guide",
          text: "Duplicate",
          enabled,
          action: () => void editorStore.getState().duplicateGuide(guide),
        },
        {
          id: "move-guide",
          text: "Move to",
          enabled,
          items: [
            moveTo("", "Library (top level)"),
            ...(folders.length > 0 ? [{ item: "Separator" as const }] : []),
            ...folders.map((folder) => moveTo(folder, folder.split("/").join(" › "))),
          ],
        },
        { item: "Separator" },
        {
          id: "reveal-guide",
          text: "Reveal in Finder",
          action: () => void editorStore.getState().reveal({ path: guide.path }),
        },
        { item: "Separator" },
        {
          id: "delete-guide",
          text: "Delete Guide…",
          enabled,
          action: () => void editorStore.getState().deleteGuide(guide),
        },
      ],
    });
    await menu.popup();
  } catch (error) {
    console.error("guide menu failed:", error);
  }
}

/** The native context menu of a folder in the list. */
async function showFolderMenu(event: MouseEvent, folder: string, onNewFolder: () => void) {
  event.preventDefault();
  try {
    const menu = await Menu.new({
      items: [
        { id: "new-subfolder", text: "New Folder Inside…", action: onNewFolder },
        {
          id: "reveal-folder",
          text: "Reveal in Finder",
          action: () => void editorStore.getState().reveal({ folder }),
        },
      ],
    });
    await menu.popup();
  } catch (error) {
    console.error("folder menu failed:", error);
  }
}

/** Asks for a guide's new title or a new folder's name. */
function NameDialog({ naming, onClose }: { naming: Naming | null; onClose(): void }) {
  const [name, setName] = useState("");
  const [saving, setSaving] = useState(false);
  useEffect(() => {
    if (naming) setName(naming.kind === "rename" ? naming.guide.title : "");
  }, [naming]);
  const rename = naming?.kind === "rename";
  const title = rename ? "Rename guide" : naming?.parent ? `New folder in “${folderName(naming.parent)}”` : "New folder";

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!naming || !name.trim() || saving) return;
    setSaving(true);
    const editor = editorStore.getState();
    const done =
      naming.kind === "rename"
        ? name.trim() === naming.guide.title || (await editor.renameGuide(naming.guide, name))
        : await editor.createFolder(naming.parent, name.trim());
    setSaving(false);
    if (done) onClose();
  }

  return (
    <Dialog.Root open={naming !== null} onOpenChange={(open) => !open && !saving && onClose()}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-black/40" />
        <Dialog.Content className="fixed top-1/2 left-1/2 z-50 flex w-96 -translate-x-1/2 -translate-y-1/2 flex-col gap-4 rounded-lg border border-border bg-background p-5 text-foreground shadow-lg">
          <div className="flex items-center justify-between">
            <Dialog.Title className="text-base font-semibold">{title}</Dialog.Title>
            <Dialog.Close asChild>
              <Button variant="ghost" size="icon" aria-label="Close">
                <X />
              </Button>
            </Dialog.Close>
          </div>
          <Dialog.Description className="text-sm text-muted-foreground">
            {rename
              ? "The guide's folder on disk is renamed to match."
              : "Folders are plain folders in your library, so Finder and sync tools see them too."}
          </Dialog.Description>
          <form className="flex flex-col gap-4" onSubmit={(e) => void submit(e)}>
            <Input
              autoFocus
              aria-label={rename ? "Title" : "Folder name"}
              placeholder={rename ? "Title" : "Folder name"}
              value={name}
              onChange={(e) => setName(e.target.value)}
            />
            <div className="flex justify-end gap-2">
              <Dialog.Close asChild>
                <Button type="button" variant="ghost" size="sm" disabled={saving}>
                  Cancel
                </Button>
              </Dialog.Close>
              <Button type="submit" size="sm" disabled={!name.trim() || saving}>
                {rename ? "Rename" : "Create"}
              </Button>
            </div>
          </form>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

function GuideRow({
  guide,
  depth,
  folders,
  onRename,
}: {
  guide: GuideListItem;
  depth: number;
  folders: string[];
  onRename(guide: GuideListItem): void;
}) {
  const opening = useEditor((s) => s.opening);
  return (
    <li>
      <button
        type="button"
        disabled={opening}
        onClick={() => void editorStore.getState().openGuide(guide.id)}
        onContextMenu={(e) => void showGuideMenu(e, guide, folders, () => onRename(guide))}
        style={{ paddingLeft: 8 + depth * INDENT_PX }}
        className={cn(
          "flex w-full flex-col gap-0.5 rounded-md py-1.5 pr-2 text-left hover:bg-sidebar-accent",
          guide.duplicateOf && "opacity-60",
        )}
      >
        <span className="flex items-center gap-1.5 text-sm font-medium">
          <BookOpen className="size-3.5 shrink-0 text-muted-foreground" />
          <span className="truncate">{guide.title || "Untitled"}</span>
          {guide.readOnly && <Lock className="size-3 shrink-0 text-muted-foreground" />}
        </span>
        <span className="truncate pl-5 text-xs text-muted-foreground">
          {guide.stepCount} {guide.stepCount === 1 ? "step" : "steps"}
          {guide.tags.length > 0 && ` · ${guide.tags.join(", ")}`}
          {guide.summary && ` · ${guide.summary}`}
        </span>
      </button>
    </li>
  );
}

/** Guide count in a folder and everything below it. */
function countGuides(node: FolderNode): number {
  return node.guides.length + node.folders.reduce((n, f) => n + countGuides(f), 0);
}

/** A folder's contents: its subfolders (collapsible), then its guides. */
function FolderContents({
  node,
  depth,
  folders,
  collapsed,
  onToggle,
  onRename,
  onNewFolder,
}: {
  node: FolderNode;
  depth: number;
  folders: string[];
  collapsed: Set<string>;
  onToggle(path: string): void;
  onRename(guide: GuideListItem): void;
  onNewFolder(parent: string): void;
}) {
  return (
    <>
      {node.folders.map((folder) => {
        const open = !collapsed.has(folder.path);
        const Chevron = open ? ChevronDown : ChevronRight;
        return (
          <li key={folder.path}>
            <button
              type="button"
              aria-expanded={open}
              title={folder.path}
              onClick={() => onToggle(folder.path)}
              onContextMenu={(e) => void showFolderMenu(e, folder.path, () => onNewFolder(folder.path))}
              style={{ paddingLeft: 4 + depth * INDENT_PX }}
              className="flex w-full items-center gap-1 rounded-md py-1 pr-2 text-left text-sm hover:bg-sidebar-accent"
            >
              <Chevron className="size-3.5 shrink-0 text-muted-foreground" />
              <Folder className="size-3.5 shrink-0 text-muted-foreground" />
              <span className="truncate font-medium">{folder.name}</span>
              <span className="ml-auto pl-2 text-xs text-muted-foreground">{countGuides(folder)}</span>
            </button>
            {open && (
              <ul>
                <FolderContents
                  node={folder}
                  depth={depth + 1}
                  folders={folders}
                  collapsed={collapsed}
                  onToggle={onToggle}
                  onRename={onRename}
                  onNewFolder={onNewFolder}
                />
              </ul>
            )}
          </li>
        );
      })}
      {node.guides.map((guide) => (
        <GuideRow key={guide.path} guide={guide} depth={depth} folders={folders} onRename={onRename} />
      ))}
    </>
  );
}

const selectClass =
  "h-7 min-w-0 flex-1 rounded-md border border-input bg-transparent px-1.5 text-xs outline-none focus-visible:ring-2 focus-visible:ring-ring";

export function LibrarySidebar() {
  const guides = useEditor((s) => s.guides);
  const folders = useEditor((s) => s.folders);
  const sort = useEditor((s) => s.sort);
  const tag = useEditor((s) => s.tag);
  const listError = useEditor((s) => s.listError);
  const openError = useEditor((s) => s.openError);
  const [query, setQuery] = useState(() => editorStore.getState().query);
  const [naming, setNaming] = useState<Naming | null>(null);
  const [collapsed, setCollapsed] = useState<Set<string>>(() => new Set());

  useEffect(() => {
    const timer = setTimeout(() => void editorStore.getState().setQuery(query), SEARCH_DELAY_MS);
    return () => clearTimeout(timer);
  }, [query]);

  const tags = useMemo(() => libraryTags(guides), [guides]);
  // A tag that no listed guide has any more (deleted, retagged) filters nothing out.
  const activeTag = tag && tags.some((t) => t.toLowerCase() === tag.toLowerCase()) ? tag : null;
  // The query the list was made for (the input is debounced).
  const searching = useEditor((s) => s.query.trim() !== "");
  const tree = useMemo(() => {
    const shown = activeTag ? guides.filter((g) => hasTag(g, activeTag)) : guides;
    // A search lists best matches first; the sort applies to browsing.
    const ordered = searching ? shown : sortGuides(shown, sort);
    return buildTree(ordered, folders, !searching && !activeTag);
  }, [guides, folders, sort, activeTag, searching]);
  const moveTargets = useMemo(() => folderPaths(folders, guides), [folders, guides]);
  const empty = tree.guides.length === 0 && tree.folders.length === 0;

  const toggle = (path: string) =>
    setCollapsed((current) => {
      const next = new Set(current);
      if (!next.delete(path)) next.add(path);
      return next;
    });

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex flex-col gap-1.5 p-3">
        <div className="flex items-center gap-1.5">
          <div className="relative min-w-0 flex-1">
            <Search className="pointer-events-none absolute top-2 left-2.5 size-4 text-muted-foreground" />
            <Input
              className="pl-8"
              placeholder="Search guides"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
            />
          </div>
          <Button
            variant="ghost"
            size="icon"
            aria-label="New folder"
            title="New folder"
            onClick={() => setNaming({ kind: "folder", parent: "" })}
          >
            <FolderPlus />
          </Button>
        </div>
        <div className="flex items-center gap-1.5">
          <select
            aria-label="Sort guides"
            title={searching ? "Search results are listed best match first" : "Sort guides"}
            disabled={searching}
            value={sort}
            onChange={(e) => editorStore.getState().setSort(e.target.value as GuideSort)}
            className={cn(selectClass, "disabled:opacity-50")}
          >
            {GUIDE_SORTS.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </select>
          {tags.length > 0 && (
            <select
              aria-label="Filter by tag"
              value={activeTag ?? ""}
              onChange={(e) => editorStore.getState().setTag(e.target.value || null)}
              className={selectClass}
            >
              <option value="">All tags</option>
              {tags.map((t) => (
                <option key={t} value={t}>
                  {t}
                </option>
              ))}
            </select>
          )}
        </div>
      </div>
      {(listError || openError) && (
        <p className="px-3 pb-2 text-xs text-[var(--status-danger)]">{listError ?? openError}</p>
      )}
      <ul className="min-h-0 flex-1 overflow-y-auto px-2 pb-3">
        <FolderContents
          node={tree}
          depth={0}
          folders={moveTargets}
          collapsed={collapsed}
          onToggle={toggle}
          onRename={(guide) => setNaming({ kind: "rename", guide })}
          onNewFolder={(parent) => setNaming({ kind: "folder", parent })}
        />
        {empty && !listError && (
          <li className="px-2 py-6 text-center text-sm text-muted-foreground">
            {searching || activeTag ? "No matching guides" : "No guides yet"}
          </li>
        )}
      </ul>
      <NameDialog naming={naming} onClose={() => setNaming(null)} />
    </div>
  );
}
