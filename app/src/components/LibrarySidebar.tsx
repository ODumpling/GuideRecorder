import { useEffect, useState, type MouseEvent } from "react";
import { BookOpen, Lock, Search } from "lucide-react";
import { Menu } from "@tauri-apps/api/menu";
import { Input } from "@/components/ui/input";
import { editorStore, useEditor } from "@/editor/editor";
import type { GuideListItem } from "@/lib/types";
import { cn } from "@/lib/utils";

const SEARCH_DELAY_MS = 200;

/** The native context menu of a guide in the list. */
async function showGuideMenu(event: MouseEvent, guide: GuideListItem) {
  event.preventDefault();
  try {
    const menu = await Menu.new({
      items: [
        {
          id: "delete-guide",
          text: "Delete Guide…",
          enabled: !editorStore.getState().busy,
          action: () => void editorStore.getState().deleteGuide(guide),
        },
      ],
    });
    await menu.popup();
  } catch (error) {
    console.error("guide menu failed:", error);
  }
}

export function LibrarySidebar() {
  const guides = useEditor((s) => s.guides);
  const listError = useEditor((s) => s.listError);
  const opening = useEditor((s) => s.opening);
  const openError = useEditor((s) => s.openError);
  const [query, setQuery] = useState(() => editorStore.getState().query);

  useEffect(() => {
    const timer = setTimeout(() => void editorStore.getState().setQuery(query), SEARCH_DELAY_MS);
    return () => clearTimeout(timer);
  }, [query]);

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="p-3">
        <div className="relative">
          <Search className="pointer-events-none absolute top-2 left-2.5 size-4 text-muted-foreground" />
          <Input
            className="pl-8"
            placeholder="Search guides"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
        </div>
      </div>
      {(listError || openError) && (
        <p className="px-3 pb-2 text-xs text-[var(--status-danger)]">{listError ?? openError}</p>
      )}
      <ul className="min-h-0 flex-1 overflow-y-auto px-2 pb-3">
        {guides.map((guide) => (
          <li key={guide.path}>
            <button
              type="button"
              disabled={opening}
              onClick={() => void editorStore.getState().openGuide(guide.id)}
              onContextMenu={(e) => void showGuideMenu(e, guide)}
              className={cn(
                "flex w-full flex-col gap-0.5 rounded-md px-2 py-1.5 text-left hover:bg-sidebar-accent",
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
                {guide.summary && ` · ${guide.summary}`}
              </span>
            </button>
          </li>
        ))}
        {guides.length === 0 && !listError && (
          <li className="px-2 py-6 text-center text-sm text-muted-foreground">
            {query ? "No matching guides" : "No guides yet"}
          </li>
        )}
      </ul>
    </div>
  );
}
