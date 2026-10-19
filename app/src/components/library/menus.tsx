// The library's context menus and its name dialog (rename a guide, name a
// new folder), shared by the sidebar and the guide grid.

import { useEffect, useState, type FormEvent, type MouseEvent } from "react";
import { Dialog } from "radix-ui";
import { X } from "lucide-react";
import { Menu } from "@tauri-apps/api/menu";
import { create } from "zustand";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { editorStore } from "@/editor/editor";
import { folderName, localToday } from "@/editor/library";
import type { GuideListItem } from "@/lib/types";
import { guideSyncStatus } from "@/sync/store";
import { shareStore, syncStore } from "@/sync/sync";

/** What the name dialog is asking for. */
export type Naming = { kind: "rename"; guide: GuideListItem } | { kind: "folder"; parent: string };

/** The open name dialog, if any; set it to ask for a name. */
export const namingStore = create<{ naming: Naming | null }>(() => ({ naming: null }));

export function askName(naming: Naming) {
  namingStore.setState({ naming });
}

/** The native context menu of a guide in the list. */
export async function showGuideMenu(event: MouseEvent, guide: GuideListItem, folders: string[], onRename: () => void) {
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
          id: "mark-guide-reviewed",
          text: "Mark Reviewed",
          enabled: enabled && guide.lastReviewed !== localToday(),
          action: () => void editorStore.getState().markReviewed(guide),
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
        ...syncItems(guide),
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

/** Sync to Web, Resolve Sync Conflict while there is one, and Share… once synced; none until signed in. */
function syncItems(guide: GuideListItem) {
  const sync = syncStore.getState();
  if (!sync.account?.signedIn || !sync.state.workspaceId) return [];
  const status = guideSyncStatus(sync, guide.id);
  const items: ({ item: "Separator" } | { id: string; text: string; enabled?: boolean; action: () => void })[] = [
    { item: "Separator" },
  ];
  if (!sync.state.library) {
    const on = Boolean(sync.state.guides[guide.id]?.enabled);
    items.push({
      id: "sync-guide",
      text: on ? "Stop Syncing to Web" : "Sync to Web",
      enabled: !guide.readOnly,
      action: () => void syncStore.getState().setGuideEnabled(guide.id, !on),
    });
  }
  if (status === "conflict") {
    items.push({ id: "resolve-sync-conflict", text: "Resolve Sync Conflict…", action: () => syncStore.getState().openConflict(guide.id) });
  }
  // Share links need the guide on the server.
  const revisionId = sync.state.guides[guide.id]?.revisionId ?? null;
  if (revisionId && status !== "deleted") {
    items.push({ id: "share-guide", text: "Share…", action: () => void shareStore.getState().open(guide.id, revisionId) });
  }
  return items.length > 1 ? items : [];
}

/** The native context menu of a folder in the list. */
export async function showFolderMenu(event: MouseEvent, folder: string, onNewFolder: () => void) {
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
export function NameDialog() {
  const naming = namingStore((s) => s.naming);
  const onClose = () => namingStore.setState({ naming: null });
  const [name, setName] = useState("");
  const [saving, setSaving] = useState(false);
  useEffect(() => {
    if (naming) setName(naming.kind === "rename" ? naming.guide.title : "");
  }, [naming]);
  const rename = naming?.kind === "rename";
  const title = rename
    ? "Rename guide"
    : naming?.parent
      ? `New folder in “${folderName(naming.parent)}”`
      : "New folder";

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
        <Dialog.Overlay className="fixed inset-0 z-50 bg-black/40 backdrop-blur-[2px]" />
        <Dialog.Content className="fixed top-1/2 left-1/2 z-50 flex w-96 -translate-x-1/2 -translate-y-1/2 flex-col gap-4 rounded-2xl border border-border bg-card p-5 text-card-foreground shadow-2xl">
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
