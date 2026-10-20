import { useState, type HTMLAttributes, type MouseEvent, type PointerEvent } from "react";
import { ArrowDown, ArrowUp, Columns3, Lock } from "lucide-react";
import { Menu } from "@tauri-apps/api/menu";
import { Avatar, StatusPill } from "@/components/library/bits";
import { SyncBadge } from "@/components/library/SyncBadge";
import { isReviewOverdue, localToday, reviewDue, timeAgo } from "@/editor/library";
import {
  clampWidth,
  DEFAULT_LIST_LAYOUT,
  LIST_COLUMNS,
  nextSort,
  type ListColumn,
  type ListColumnSpec,
  type ListLayout,
} from "@/editor/libraryList";
import type { GuideListItem } from "@/lib/types";
import { cn } from "@guiderecorder/ui/utils";

/** What the container puts on each row: selection, keys and dragging. */
export type RowProps = HTMLAttributes<HTMLElement> & { draggable?: boolean; "data-guide-path"?: string };

/** The width of the selection checkbox column, in pixels. */
const CHECK_PX = 36;

function Cell({ guide, column, today }: { guide: GuideListItem; column: ListColumn; today: string }) {
  switch (column) {
    case "title":
      return (
        <span className="flex min-w-0 items-center gap-1.5 font-medium">
          <span className="truncate">{guide.title || "Untitled"}</span>
          {guide.readOnly && <Lock className="size-3 shrink-0 text-muted-foreground" aria-label="Read-only" />}
          <SyncBadge guideId={guide.id} />
          {guide.version && (
            <span className="shrink-0 text-xs font-normal text-muted-foreground">v{guide.version}</span>
          )}
        </span>
      );
    case "folder":
      return (
        <span className="truncate text-muted-foreground" title={guide.folder || undefined}>
          {guide.folder ? guide.folder.split("/").join(" › ") : "—"}
        </span>
      );
    case "status":
      return guide.status ? <StatusPill status={guide.status} /> : <span className="text-muted-foreground">—</span>;
    case "owner":
      return guide.owner ? (
        <span className="flex min-w-0 items-center gap-1.5">
          <Avatar name={guide.owner} />
          <span className="truncate">{guide.owner}</span>
        </span>
      ) : (
        <span className="text-muted-foreground">—</span>
      );
    case "steps":
      return <span className="text-muted-foreground tabular-nums">{guide.stepCount}</span>;
    case "updated":
      return (
        <span className="truncate text-muted-foreground" title={new Date(guide.updatedAt).toLocaleString()}>
          {timeAgo(guide.updatedAt)}
        </span>
      );
    case "reviewDue": {
      const due = reviewDue(guide);
      if (!due) return <span className="text-muted-foreground">—</span>;
      const overdue = isReviewOverdue(guide, today);
      return (
        <span
          title={overdue ? "Review overdue" : undefined}
          className={cn("tabular-nums", overdue ? "font-medium text-[var(--status-danger)]" : "text-muted-foreground")}
        >
          {due}
        </span>
      );
    }
  }
}

/** The columns menu: which columns show. */
async function showColumnsMenu(e: MouseEvent, layout: ListLayout, onLayout: (layout: ListLayout) => void) {
  e.preventDefault();
  try {
    const menu = await Menu.new({
      items: [
        ...LIST_COLUMNS.filter((c) => c.column !== "title").map((c) => {
          const shown = !layout.hidden.includes(c.column);
          return {
            id: `list-column:${c.column}`,
            text: c.label,
            checked: shown,
            action: () =>
              onLayout({
                ...layout,
                hidden: shown ? [...layout.hidden, c.column] : layout.hidden.filter((h) => h !== c.column),
              }),
          };
        }),
        { item: "Separator" as const },
        {
          id: "list-columns-reset",
          text: "Reset Columns",
          action: () => onLayout({ ...DEFAULT_LIST_LAYOUT, sort: layout.sort }),
        },
      ],
    });
    await menu.popup();
  } catch (error) {
    console.error("columns menu failed:", error);
  }
}

function HeaderCell({
  spec,
  layout,
  width,
  onResize,
  onLayout,
}: {
  spec: ListColumnSpec;
  layout: ListLayout;
  width: number;
  onResize(width: number | null): void;
  onLayout(layout: ListLayout): void;
}) {
  const sorted = layout.sort?.column === spec.column ? layout.sort : null;
  const startResize = (e: PointerEvent<HTMLSpanElement>) => {
    e.preventDefault();
    e.stopPropagation();
    const handle = e.currentTarget;
    handle.setPointerCapture(e.pointerId);
    const startX = e.clientX;
    const resized = (ev: globalThis.PointerEvent) => clampWidth(width + ev.clientX - startX);
    const move = (ev: globalThis.PointerEvent) => onResize(resized(ev));
    const up = (ev: globalThis.PointerEvent) => {
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", up);
      onResize(null);
      onLayout({ ...layout, widths: { ...layout.widths, [spec.column]: resized(ev) } });
    };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", up);
  };
  return (
    <div
      role="columnheader"
      aria-sort={sorted ? (sorted.descending ? "descending" : "ascending") : "none"}
      className="relative flex min-w-0 items-center"
    >
      <button
        type="button"
        onClick={() => onLayout({ ...layout, sort: nextSort(layout.sort, spec.column) })}
        className={cn(
          "flex h-7 min-w-0 flex-1 items-center gap-1 rounded px-1 text-left outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring",
          spec.align === "end" && "justify-end",
          sorted && "text-foreground",
        )}
      >
        <span className="truncate">{spec.label}</span>
        {sorted &&
          (sorted.descending ? <ArrowDown className="size-3 shrink-0" /> : <ArrowUp className="size-3 shrink-0" />)}
      </button>
      <span
        role="separator"
        aria-orientation="vertical"
        aria-label={`Resize ${spec.label}`}
        onPointerDown={startResize}
        onDoubleClick={() => {
          const { [spec.column]: _, ...widths } = layout.widths;
          onLayout({ ...layout, widths });
        }}
        className="absolute top-1 -right-1.5 bottom-1 z-10 w-3 cursor-col-resize after:absolute after:inset-y-0 after:left-1.5 after:w-px after:bg-border hover:after:bg-ring"
      />
    </div>
  );
}

/**
 * The library as a dense list: a row per guide in the columns the user
 * keeps, which sort, resize and hide (right-click the header).
 */
export function LibraryList({
  guides,
  layout,
  onLayout,
  selected,
  rowProps,
  onToggle,
  onToggleAll,
}: {
  guides: GuideListItem[];
  layout: ListLayout;
  onLayout(layout: ListLayout): void;
  selected: Set<string>;
  rowProps(guide: GuideListItem): RowProps;
  onToggle(guide: GuideListItem): void;
  onToggleAll(select: boolean): void;
}) {
  const [resizing, setResizing] = useState<{ column: ListColumn; width: number } | null>(null);
  const today = localToday();
  const columns = LIST_COLUMNS.filter((c) => !layout.hidden.includes(c.column));
  const widthOf = (c: ListColumnSpec) =>
    resizing?.column === c.column ? resizing.width : (layout.widths[c.column] ?? c.width);
  const template = [
    `${CHECK_PX}px`,
    ...columns.map((c) => (c.column === "title" ? `minmax(${widthOf(c)}px, 1fr)` : `${widthOf(c)}px`)),
    // The columns menu button.
    "24px",
  ].join(" ");
  const allSelected = guides.length > 0 && guides.every((g) => selected.has(g.path));

  return (
    <div
      role="grid"
      aria-label="Guides"
      aria-multiselectable
      className="overflow-x-auto rounded-xl border border-border bg-card"
    >
      <div
        role="row"
        style={{ gridTemplateColumns: template }}
        onContextMenu={(e) => void showColumnsMenu(e, layout, onLayout)}
        className="grid min-w-fit items-center gap-x-3 border-b border-border px-2 py-1 text-xs font-medium text-muted-foreground"
      >
        <div role="columnheader" className="flex items-center justify-center">
          <input
            type="checkbox"
            aria-label={allSelected ? "Deselect all" : "Select all"}
            checked={allSelected}
            onChange={() => onToggleAll(!allSelected)}
            className="size-3.5 accent-[var(--status-info)]"
          />
        </div>
        {columns.map((spec) => (
          <HeaderCell
            key={spec.column}
            spec={spec}
            layout={layout}
            width={widthOf(spec)}
            onResize={(width) => setResizing(width === null ? null : { column: spec.column, width })}
            onLayout={onLayout}
          />
        ))}
        <button
          type="button"
          aria-label="Columns"
          title="Columns"
          onClick={(e) => void showColumnsMenu(e, layout, onLayout)}
          className="flex size-6 items-center justify-center rounded text-muted-foreground hover:bg-foreground/[0.06] hover:text-foreground"
        >
          <Columns3 className="size-3.5" />
        </button>
      </div>
      {guides.map((guide) => {
        const on = selected.has(guide.path);
        return (
          <div
            key={guide.path}
            role="row"
            aria-selected={on}
            style={{ gridTemplateColumns: template }}
            {...rowProps(guide)}
            className={cn(
              "grid min-h-10 min-w-fit cursor-default items-center gap-x-3 border-b border-border/60 px-2 text-[13px] outline-none last:border-b-0 hover:bg-foreground/[0.03] focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset",
              on &&
                "bg-[color-mix(in_oklab,var(--status-info)_10%,transparent)] hover:bg-[color-mix(in_oklab,var(--status-info)_14%,transparent)]",
              guide.duplicateOf && "opacity-60",
            )}
          >
            <div role="gridcell" className="flex items-center justify-center">
              <input
                type="checkbox"
                tabIndex={-1}
                aria-label={`Select ${guide.title || "Untitled"}`}
                checked={on}
                onClick={(e) => e.stopPropagation()}
                onChange={() => onToggle(guide)}
                className="size-3.5 accent-[var(--status-info)]"
              />
            </div>
            {columns.map((spec) => (
              <div
                key={spec.column}
                role="gridcell"
                className={cn("flex min-w-0 items-center", spec.align === "end" && "justify-end")}
              >
                <Cell guide={guide} column={spec.column} today={today} />
              </div>
            ))}
            <div />
          </div>
        );
      })}
    </div>
  );
}
