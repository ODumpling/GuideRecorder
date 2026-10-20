import { useEffect, useMemo, useRef, useState, type KeyboardEvent, type MouseEvent } from "react";
import { BulkBar, BulkDialog, showSelectionMenu } from "@/components/library/bulk";
import { GuideCard } from "@/components/library/GuideCard";
import { LibraryList, type RowProps } from "@/components/library/LibraryList";
import { askName, showGuideMenu } from "@/components/library/menus";
import { clearSelection, guidesAt, selectionStore, startGuideDrag } from "@/components/library/selection";
import { editorStore } from "@/editor/editor";
import {
  arrowStep,
  initialListLayout,
  pruneSelection,
  rememberListLayout,
  selectGuide,
  sortRows,
  stepFocus,
  type LibraryView,
  type ListLayout,
  type SelectMode,
} from "@/editor/libraryList";
import type { GuideListItem } from "@/lib/types";

/** Whether keys typed at `target` are text (so ⌘A selects text, not guides). */
function isTextTarget(target: EventTarget | null): boolean {
  return (
    target instanceof HTMLElement &&
    (target.isContentEditable || !!target.closest("input:not([type=checkbox]), textarea, select, [contenteditable]"))
  );
}

/**
 * The guides shown in the library, as cards or a list, with multi-select
 * (⌘-click, ⇧-click, ⌘A, arrow keys) and the bulk actions on the selection.
 * A plain click opens a guide; Enter opens the focused one and ⌫ deletes
 * the selection after confirming.
 */
export function LibraryGuides({
  shown,
  folders,
  view,
}: {
  shown: GuideListItem[];
  /** Every folder, for "Move to". */
  folders: string[];
  view: LibraryView;
}) {
  const [layout, setLayout] = useState<ListLayout>(() => initialListLayout());
  const selection = selectionStore();
  const container = useRef<HTMLDivElement>(null);
  const grid = useRef<HTMLUListElement>(null);

  const rows = useMemo(() => (view === "list" ? sortRows(shown, layout.sort) : shown), [view, shown, layout.sort]);
  const order = useMemo(() => rows.map((g) => g.path), [rows]);
  const selected = useMemo(() => new Set(selection.paths), [selection.paths]);
  const selectedGuides = useMemo(() => guidesAt(rows, selection.paths), [rows, selection.paths]);

  // Guides filtered out, moved or deleted leave the selection.
  useEffect(() => {
    const current = selectionStore.getState();
    const pruned = pruneSelection(current, order);
    if (pruned !== current) selectionStore.setState(pruned, true);
  }, [order]);

  // ⌘A selects every guide shown, unless typing.
  useEffect(() => {
    const onKeyDown = (e: globalThis.KeyboardEvent) => {
      if (e.key.toLowerCase() !== "a" || !(e.metaKey || e.ctrlKey) || e.altKey || e.shiftKey) return;
      if (isTextTarget(e.target) || document.querySelector("[role=dialog]")) return;
      e.preventDefault();
      selectionStore.setState({ paths: order, anchor: order[0] ?? null }, true);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [order]);

  const changeLayout = (next: ListLayout) => {
    setLayout(next);
    rememberListLayout(next);
  };

  const select = (path: string, mode: SelectMode) =>
    selectionStore.setState(selectGuide(selectionStore.getState(), order, path, mode), true);

  const focusGuide = (path: string) =>
    container.current?.querySelector<HTMLElement>(`[data-guide-path="${CSS.escape(path)}"]`)?.focus();

  const open = (guide: GuideListItem) => {
    clearSelection();
    void editorStore.getState().openGuide(guide.id);
  };

  const onClick = (e: MouseEvent, guide: GuideListItem) => {
    if (e.metaKey || e.ctrlKey) select(guide.path, "toggle");
    else if (e.shiftKey) select(guide.path, "range");
    else open(guide);
  };

  const onContextMenu = (e: MouseEvent, guide: GuideListItem) => {
    const { paths } = selectionStore.getState();
    if (paths.length > 1 && paths.includes(guide.path)) {
      void showSelectionMenu(e, guidesAt(rows, paths), folders);
    } else {
      void showGuideMenu(e, guide, folders, () => askName({ kind: "rename", guide }));
    }
  };

  const itemProps = (guide: GuideListItem): RowProps => ({
    "data-guide-path": guide.path,
    tabIndex: 0,
    draggable: true,
    onDragStart: (e) => startGuideDrag(e, guide),
    onClick: (e) => onClick(e, guide),
    onContextMenu: (e) => onContextMenu(e, guide),
  });

  /** Cards per row in the grid, from its laid-out columns. */
  const gridColumns = () =>
    grid.current ? getComputedStyle(grid.current).gridTemplateColumns.split(" ").filter(Boolean).length : 1;

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (isTextTarget(e.target)) return;
    const item = (e.target as HTMLElement).closest<HTMLElement>("[data-guide-path]");
    const path = item?.dataset.guidePath ?? null;
    if (e.key === "Escape" && selection.paths.length > 0) {
      e.preventDefault();
      clearSelection();
      return;
    }
    // The rest is for a focused guide (not the bulk bar's buttons).
    if (!path) return;
    const step = arrowStep(e.key, view, gridColumns());
    if (step !== null && !e.metaKey && !e.ctrlKey && !e.altKey) {
      const next = stepFocus(order, path, step);
      if (!next) return;
      e.preventDefault();
      if (e.shiftKey) {
        const current = selectionStore.getState();
        // A range starts where the keyboard was, if nothing anchors it.
        const anchored = current.anchor ? current : { paths: current.paths, anchor: path };
        selectionStore.setState(selectGuide(anchored, order, next, "range"), true);
      } else {
        select(next, "only");
      }
      focusGuide(next);
      return;
    }
    if (e.key === "Enter" && !e.metaKey && !e.shiftKey) {
      const guide = rows.find((g) => g.path === path);
      if (!guide) return;
      e.preventDefault();
      open(guide);
    } else if ((e.key === "Backspace" || e.key === "Delete") && !e.altKey && !e.shiftKey) {
      const doomed = selectedGuides.length > 0 ? selectedGuides : rows.filter((g) => g.path === path);
      if (doomed.length === 0) return;
      e.preventDefault();
      void editorStore
        .getState()
        .deleteGuides(doomed)
        .then((deleted) => deleted && clearSelection());
    }
  };

  return (
    <div ref={container} onKeyDown={onKeyDown} className="flex flex-col gap-3">
      {selectedGuides.length > 0 && (
        <BulkBar
          guides={selectedGuides}
          folders={folders}
          total={rows.length}
          onSelectAll={() => selectionStore.setState({ paths: order, anchor: order[0] ?? null }, true)}
        />
      )}
      {view === "list" ? (
        <LibraryList
          guides={rows}
          layout={layout}
          onLayout={changeLayout}
          selected={selected}
          rowProps={itemProps}
          onToggle={(guide) => select(guide.path, "toggle")}
          onToggleAll={(all) =>
            all ? selectionStore.setState({ paths: order, anchor: order[0] ?? null }, true) : clearSelection()
          }
        />
      ) : (
        <ul
          ref={grid}
          aria-label="Guides"
          aria-multiselectable
          className="grid grid-cols-[repeat(auto-fill,minmax(15rem,1fr))] gap-x-5 gap-y-6"
        >
          {rows.map((guide) => (
            <GuideCard
              key={guide.path}
              guide={guide}
              folders={folders}
              selected={selected.has(guide.path)}
              itemProps={itemProps(guide)}
            />
          ))}
        </ul>
      )}
      <BulkDialog />
    </div>
  );
}
