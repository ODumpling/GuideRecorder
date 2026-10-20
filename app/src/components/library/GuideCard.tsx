import type { ButtonHTMLAttributes } from "react";
import { BookOpen, Camera, Lock, Target } from "lucide-react";
import { Avatar, StatusPill } from "@/components/library/bits";
import { SyncBadge } from "@/components/library/SyncBadge";
import { askName, showGuideMenu } from "@/components/library/menus";
import { useGuideCover } from "@/components/library/useGuideCover";
import { editorStore, useEditor } from "@/editor/editor";
import { folderName, guideContents, isReviewOverdue, localToday, reviewDue, timeAgo } from "@/editor/library";
import type { GuideListItem } from "@/lib/types";
import { cn } from "@guiderecorder/ui/utils";

/**
 * A guide in the library grid: its first screenshot, title, status, folder
 * and owner. `itemProps` (selection, keys, dragging) go on its button and
 * replace its own click and context menu handlers.
 */
export function GuideCard({
  guide,
  folders,
  selected = false,
  itemProps,
}: {
  guide: GuideListItem;
  folders: string[];
  selected?: boolean;
  itemProps?: ButtonHTMLAttributes<HTMLButtonElement> & { "data-guide-path"?: string };
}) {
  const opening = useEditor((s) => s.opening);
  const cover = useGuideCover(guide.id, guide.updatedAt);
  const overdue = isReviewOverdue(guide, localToday());
  const KindIcon = guide.kind === "annotated" ? Target : Camera;
  const meta = [guide.folder && folderName(guide.folder), timeAgo(guide.updatedAt)].filter(Boolean).join(" · ");
  return (
    <li className={cn(guide.duplicateOf && "opacity-60")}>
      <button
        type="button"
        disabled={opening}
        title={guide.summary || undefined}
        onClick={() => void editorStore.getState().openGuide(guide.id)}
        onContextMenu={(e) => void showGuideMenu(e, guide, folders, () => askName({ kind: "rename", guide }))}
        aria-selected={selected}
        {...itemProps}
        className="group flex w-full flex-col rounded-xl text-left outline-none disabled:cursor-progress"
      >
        <span
          className={cn(
            "relative block aspect-[16/10] overflow-hidden rounded-xl border border-border bg-sidebar transition duration-200 group-hover:-translate-y-0.5 group-hover:shadow-lg group-focus-visible:ring-2 group-focus-visible:ring-ring",
            selected && "border-transparent ring-2 ring-[var(--status-info)] group-focus-visible:ring-[var(--status-info)]",
          )}
        >
          {cover ? (
            <img
              src={cover}
              alt=""
              className="absolute top-[11%] left-[8%] w-full rounded-tl-lg border-t border-l border-black/5 shadow-md"
            />
          ) : (
            <span className="flex size-full items-center justify-center text-muted-foreground/50">
              <BookOpen className="size-8" strokeWidth={1.5} />
            </span>
          )}
          <span className="absolute bottom-2.5 left-2.5 flex items-center gap-1 rounded-md bg-black/70 px-2 py-0.5 text-[11px] font-semibold text-white backdrop-blur">
            <KindIcon className="size-3" /> {guideContents(guide)}
          </span>
          {overdue && (
            <span
              title={`Review was due ${reviewDue(guide)}`}
              className="absolute top-2.5 right-2.5 rounded-md bg-[var(--status-danger)] px-2 py-0.5 text-[11px] font-semibold text-white"
            >
              Review overdue
            </span>
          )}
        </span>
        <span className="mt-2.5 flex items-center gap-1.5 px-0.5 text-sm font-semibold tracking-tight">
          <span className="truncate">{guide.title || "Untitled"}</span>
          {guide.readOnly && <Lock className="size-3 shrink-0 text-muted-foreground" aria-label="Read-only" />}
          <SyncBadge guideId={guide.id} />
          {guide.version && (
            <span className="shrink-0 text-xs font-normal text-muted-foreground">v{guide.version}</span>
          )}
        </span>
        <span className="mt-1 flex min-w-0 items-center gap-2 px-0.5 text-xs text-muted-foreground">
          {guide.status && <StatusPill status={guide.status} />}
          <span className="truncate">{meta}</span>
          {guide.owner && <Avatar name={guide.owner} className="ml-auto" />}
        </span>
      </button>
    </li>
  );
}
