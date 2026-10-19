import type { GuideListItem } from "@/lib/types";

/** How the library sidebar orders guides (a search orders by best match instead). */
export type GuideSort = "updated" | "created" | "title";

export const GUIDE_SORTS: { value: GuideSort; label: string }[] = [
  { value: "updated", label: "Last updated" },
  { value: "created", label: "Date created" },
  { value: "title", label: "Title" },
];

const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });

function time(iso: string): number {
  const t = Date.parse(iso);
  return Number.isNaN(t) ? 0 : t;
}

/** Guides in `sort` order: newest first by date, A–Z by title. Ties go by title, then path. */
export function sortGuides(guides: GuideListItem[], sort: GuideSort): GuideListItem[] {
  const byTitle = (a: GuideListItem, b: GuideListItem) =>
    collator.compare(a.title || "Untitled", b.title || "Untitled") || (a.path < b.path ? -1 : a.path > b.path ? 1 : 0);
  const key = (g: GuideListItem) => (sort === "created" ? time(g.createdAt) : time(g.updatedAt));
  return [...guides].sort((a, b) => (sort === "title" ? byTitle(a, b) : key(b) - key(a) || byTitle(a, b)));
}

/** Every tag in `guides`, once each ignoring case (first spelling wins), A–Z. */
export function libraryTags(guides: GuideListItem[]): string[] {
  const tags = new Map<string, string>();
  for (const guide of guides) {
    for (const tag of guide.tags) {
      const key = tag.trim().toLowerCase();
      if (key && !tags.has(key)) tags.set(key, tag.trim());
    }
  }
  return [...tags.values()].sort(collator.compare);
}

/** Whether a guide has `tag`, ignoring case (as the CLI and MCP filter). */
export function hasTag(guide: GuideListItem, tag: string): boolean {
  const key = tag.trim().toLowerCase();
  return guide.tags.some((t) => t.trim().toLowerCase() === key);
}

/** The last part of a folder path (`Work/Team` → `Team`). */
export function folderName(folder: string): string {
  return folder.slice(folder.lastIndexOf("/") + 1);
}

/** The folder holding `folder` (`""` for a top-level folder). */
export function parentFolder(folder: string): string {
  const i = folder.lastIndexOf("/");
  return i < 0 ? "" : folder.slice(0, i);
}

/** A library folder with what is directly in it. */
export interface FolderNode {
  /** Relative path, `/` separated; `""` for the library itself. */
  path: string;
  name: string;
  folders: FolderNode[];
  guides: GuideListItem[];
}

/**
 * Groups guides (in the order given) into their folders. `folders` adds
 * folders that hold no listed guide; with `keepEmpty` false (a search or
 * tag filter is on) folders without a matching guide below them are left
 * out. Subfolders are A–Z.
 */
export function buildTree(guides: GuideListItem[], folders: string[], keepEmpty: boolean): FolderNode {
  const root: FolderNode = { path: "", name: "", folders: [], guides: [] };
  const nodes = new Map<string, FolderNode>([["", root]]);
  const node = (path: string): FolderNode => {
    const found = nodes.get(path);
    if (found) return found;
    const created: FolderNode = { path, name: folderName(path), folders: [], guides: [] };
    nodes.set(path, created);
    node(parentFolder(path)).folders.push(created);
    return created;
  };
  if (keepEmpty) for (const folder of folders) if (folder) node(folder);
  for (const guide of guides) node(guide.folder).guides.push(guide);
  const sortFolders = (n: FolderNode) => {
    n.folders.sort((a, b) => collator.compare(a.name, b.name));
    n.folders.forEach(sortFolders);
  };
  sortFolders(root);
  return root;
}

/** Every folder path from the top down, for a "Move to" menu. */
export function folderPaths(folders: string[], guides: GuideListItem[]): string[] {
  const all = new Set(folders.filter(Boolean));
  for (const guide of guides) {
    for (let f = guide.folder; f; f = parentFolder(f)) all.add(f);
  }
  return [...all].sort((a, b) => collator.compare(a, b));
}

/** The title of a copy: `Title copy`, then `Title copy 2`, ... avoiding `taken` titles. */
export function copyTitle(title: string, taken: string[]): string {
  const base = `${title.trim() || "Untitled"} copy`;
  const used = new Set(taken.map((t) => t.toLowerCase()));
  if (!used.has(base.toLowerCase())) return base;
  for (let n = 2; ; n++) {
    const candidate = `${base} ${n}`;
    if (!used.has(candidate.toLowerCase())) return candidate;
  }
}
