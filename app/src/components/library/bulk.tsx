// Bulk actions on the guides selected in the library: the bar above the
// grid or list, the selection's context menu, and the dialog asking for
// an owner or tags.

import { useEffect, useState, type FormEvent, type MouseEvent } from "react";
import { Dialog } from "radix-ui";
import { CheckCheck, CircleDot, Copy, FolderInput, Tag, Trash2, UserRound, X } from "lucide-react";
import { Menu } from "@tauri-apps/api/menu";
import { create } from "zustand";
import { Button } from "@guiderecorder/ui/button";
import { Input } from "@guiderecorder/ui/input";
import { clearSelection } from "@/components/library/selection";
import { editorStore, useEditor } from "@/editor/editor";
import { GUIDE_STATUSES, libraryOwners, libraryTags } from "@/editor/library";
import type { GuideListItem } from "@/lib/types";

/** What the bulk dialog asks for, and for which guides. */
type Asking = { kind: "owner" | "addTags"; guides: GuideListItem[] };

const askingStore = create<{ asking: Asking | null }>(() => ({ asking: null }));

function ask(asking: Asking) {
  askingStore.setState({ asking });
}

type MenuItems = NonNullable<Parameters<typeof Menu.new>[0]>["items"];

function moveItems(guides: GuideListItem[], folders: string[]): MenuItems {
  const moveTo = (folder: string, text: string) => ({
    id: `bulk-move-to:${folder}`,
    text,
    enabled: guides.some((g) => g.folder !== folder),
    action: () =>
      void editorStore.getState().moveGuides(
        guides.filter((g) => g.folder !== folder),
        folder,
      ),
  });
  return [
    moveTo("", "Library (top level)"),
    ...(folders.length > 0 ? [{ item: "Separator" as const }] : []),
    ...folders.map((folder) => moveTo(folder, folder.split("/").join(" › "))),
  ];
}

function statusItems(guides: GuideListItem[]): MenuItems {
  return GUIDE_STATUSES.map((s) => ({
    id: `bulk-status:${s.value}`,
    text: s.label,
    checked: guides.every((g) => g.status === s.value),
    action: () => void editorStore.getState().editGuides(guides, { status: s.value }),
  }));
}

function tagItems(guides: GuideListItem[]): MenuItems {
  const tags = libraryTags(guides);
  return [
    { id: "bulk-add-tags", text: "Add Tags…", action: () => ask({ kind: "addTags", guides }) },
    {
      id: "bulk-remove-tags",
      text: "Remove Tag",
      enabled: tags.length > 0,
      items: tags.map((tag) => ({
        id: `bulk-remove-tag:${tag}`,
        text: tag,
        action: () => void editorStore.getState().editGuides(guides, { removeTags: [tag] }),
      })),
    },
  ];
}

async function popup(event: MouseEvent, items: MenuItems) {
  event.preventDefault();
  try {
    const menu = await Menu.new({ items });
    await menu.popup();
  } catch (error) {
    console.error("library menu failed:", error);
  }
}

/** Deletes the guides after one confirmation; the selection goes with them. */
async function deleteAll(guides: GuideListItem[]) {
  if (await editorStore.getState().deleteGuides(guides)) clearSelection();
}

/** The context menu of several selected guides. */
export function showSelectionMenu(event: MouseEvent, guides: GuideListItem[], folders: string[]) {
  const enabled = !editorStore.getState().busy;
  const editor = () => editorStore.getState();
  return popup(event, [
    { id: "bulk-move", text: "Move to", enabled, items: moveItems(guides, folders) },
    { id: "bulk-status", text: "Set Status", enabled, items: statusItems(guides) },
    { id: "bulk-owner", text: "Set Owner…", enabled, action: () => ask({ kind: "owner", guides }) },
    { id: "bulk-tags", text: "Tags", enabled, items: tagItems(guides) },
    { id: "bulk-reviewed", text: "Mark Reviewed", enabled, action: () => void editor().markGuidesReviewed(guides) },
    { id: "bulk-duplicate", text: "Duplicate", enabled, action: () => void editor().duplicateGuides(guides) },
    { item: "Separator" },
    { id: "bulk-delete", text: `Delete ${guides.length} Guides…`, enabled, action: () => void deleteAll(guides) },
  ]);
}

function BarButton({
  icon,
  label,
  disabled,
  onClick,
}: {
  icon: React.ReactNode;
  label: string;
  disabled: boolean;
  onClick(e: MouseEvent): void;
}) {
  return (
    <Button variant="ghost" size="sm" disabled={disabled} onClick={onClick}>
      {icon}
      {label}
    </Button>
  );
}

/** The actions on the selected guides, above the grid or list. */
export function BulkBar({
  guides,
  folders,
  total,
  onSelectAll,
}: {
  guides: GuideListItem[];
  folders: string[];
  /** How many guides are shown, for Select All. */
  total: number;
  onSelectAll(): void;
}) {
  const busy = useEditor((s) => s.busy);
  const editor = () => editorStore.getState();
  return (
    <div
      role="toolbar"
      aria-label="Selected guides"
      className="sticky top-2 z-20 flex flex-wrap items-center gap-1 rounded-xl border border-border bg-card/95 p-1.5 pl-3 text-sm shadow-md backdrop-blur"
    >
      <span className="mr-1 font-medium tabular-nums">{guides.length} selected</span>
      {guides.length < total && (
        <Button variant="ghost" size="sm" onClick={onSelectAll}>
          Select all {total}
        </Button>
      )}
      <span className="mx-1 h-4 w-px bg-border" />
      <BarButton
        icon={<FolderInput />}
        label="Move to"
        disabled={busy}
        onClick={(e) => void popup(e, moveItems(guides, folders))}
      />
      <BarButton
        icon={<CircleDot />}
        label="Status"
        disabled={busy}
        onClick={(e) => void popup(e, statusItems(guides))}
      />
      <BarButton icon={<UserRound />} label="Owner" disabled={busy} onClick={() => ask({ kind: "owner", guides })} />
      <BarButton icon={<Tag />} label="Tags" disabled={busy} onClick={(e) => void popup(e, tagItems(guides))} />
      <BarButton
        icon={<CheckCheck />}
        label="Mark reviewed"
        disabled={busy}
        onClick={() => void editor().markGuidesReviewed(guides)}
      />
      <BarButton
        icon={<Copy />}
        label="Duplicate"
        disabled={busy}
        onClick={() => void editor().duplicateGuides(guides)}
      />
      <BarButton icon={<Trash2 />} label="Delete" disabled={busy} onClick={() => void deleteAll(guides)} />
      <Button variant="ghost" size="icon" aria-label="Clear selection" className="ml-auto" onClick={clearSelection}>
        <X />
      </Button>
    </div>
  );
}

/** Asks for the owner, or the tags to add, of the selected guides. */
export function BulkDialog() {
  const asking = askingStore((s) => s.asking);
  const allGuides = useEditor((s) => s.guides);
  const [value, setValue] = useState("");
  const [saving, setSaving] = useState(false);
  const onClose = () => askingStore.setState({ asking: null });
  const owner = asking?.kind === "owner";
  useEffect(() => {
    if (!asking) return;
    const owners = libraryOwners(asking.guides);
    // One owner between them: start from it.
    setValue(asking.kind === "owner" && owners.length === 1 && asking.guides.every((g) => g.owner) ? owners[0]! : "");
  }, [asking]);
  const suggestions = owner ? libraryOwners(allGuides) : libraryTags(allGuides);
  const count = asking?.guides.length ?? 0;
  const noun = count === 1 ? "guide" : `${count} guides`;

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!asking || saving || (!owner && !value.trim())) return;
    setSaving(true);
    const edit = owner ? { owner: value.trim() } : { addTags: [value] };
    const done = await editorStore.getState().editGuides(asking.guides, edit);
    setSaving(false);
    if (done) onClose();
  }

  return (
    <Dialog.Root open={asking !== null} onOpenChange={(open) => !open && !saving && onClose()}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-black/40 backdrop-blur-[2px]" />
        <Dialog.Content className="fixed top-1/2 left-1/2 z-50 flex w-96 -translate-x-1/2 -translate-y-1/2 flex-col gap-4 rounded-2xl border border-border bg-card p-5 text-card-foreground shadow-2xl">
          <div className="flex items-center justify-between">
            <Dialog.Title className="text-base font-semibold">
              {owner ? `Set the owner of ${noun}` : `Add tags to ${noun}`}
            </Dialog.Title>
            <Dialog.Close asChild>
              <Button variant="ghost" size="icon" aria-label="Close">
                <X />
              </Button>
            </Dialog.Close>
          </div>
          <Dialog.Description className="text-sm text-muted-foreground">
            {owner
              ? "Who keeps them current. Leave it empty to clear the owner."
              : "Separate tags with commas. Tags the guides already have are kept."}
          </Dialog.Description>
          <form className="flex flex-col gap-4" onSubmit={(e) => void submit(e)}>
            <Input
              autoFocus
              aria-label={owner ? "Owner" : "Tags"}
              placeholder={owner ? "Owner" : "Tags"}
              list="bulk-suggestions"
              value={value}
              onChange={(e) => setValue(e.target.value)}
            />
            <datalist id="bulk-suggestions">
              {suggestions.map((s) => (
                <option key={s} value={s} />
              ))}
            </datalist>
            <div className="flex justify-end gap-2">
              <Dialog.Close asChild>
                <Button type="button" variant="ghost" size="sm" disabled={saving}>
                  Cancel
                </Button>
              </Dialog.Close>
              <Button type="submit" size="sm" disabled={saving || (!owner && !value.trim())}>
                {owner ? "Set owner" : "Add tags"}
              </Button>
            </div>
          </form>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
