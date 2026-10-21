import { lazy, Suspense, useEffect, useMemo, useState, type DragEvent, type KeyboardEvent, type MouseEvent } from "react";
import { Menu } from "@tauri-apps/api/menu";
import {
  ArrowDown,
  ArrowLeft,
  ArrowUp,
  ChevronRight,
  FileText,
  GitBranch,
  GripVertical,
  Heading,
  ImageIcon,
  Info,
  Lightbulb,
  Merge,
  MousePointerClick,
  Plus,
  Sparkles,
  Trash2,
  TriangleAlert,
  X,
} from "lucide-react";
import { aiStore, useAi } from "@/ai/ai";
import { BUSY_HINT } from "@/ai/controller";
import { StepStatusBadge, useAiBlocked } from "@/ai/WriteWithAi";
import { Button } from "@guiderecorder/ui/button";
import { changeCover, toggleCover } from "@/editor/actions";
import { Input } from "@guiderecorder/ui/input";
import { useStepThumbnail } from "@/components/useStepThumbnail";
import { coverOptions, coverStepId, deleteItems, mergeSteps, moveItems, renameGuide } from "@/editor/commands";
import { editorStore, stepNoun, stepNumbers, useEditor } from "@/editor/editor";
import { startInsert, startRecapture } from "@/editor/recapture";
import { recordingStore, startFlow } from "@/recording/recording";
import {
  blockDropTarget,
  canMergeRange,
  moveAnnouncement,
  outlineKeyFor,
  rangeIds,
  sectionOf,
  sectionStepCount,
  selectionAnnouncement,
  shiftTarget,
  visibleItems,
  withCollapsed,
} from "@/editor/outline";
import { navigationFor } from "@/editor/shortcuts";
import { isWindows, shortcutLabel } from "@/lib/platform";
import type { Item, Step } from "@guiderecorder/guide-schema";
import { cn } from "@guiderecorder/ui/utils";
import { markdownToPlainText } from "@guiderecorder/ui/markdown";

// Plain-text labels by Markdown source. The outline re-renders on every
// keystroke, but only the edited item's text changes.
const plainLabels = new Map<string, string>();

function plainLabel(markdown: string): string {
  let label = plainLabels.get(markdown);
  if (label === undefined) {
    if (plainLabels.size >= 1000) plainLabels.clear();
    label = markdownToPlainText(markdown).replace(/\n/g, " ");
    plainLabels.set(markdown, label);
  }
  return label;
}

// The menu (Radix and its positioning) loads with the first opened guide.
const InsertMenu = lazy(() => import("@/components/InsertMenu"));

export function itemLabel(item: Item, annotated: boolean): string {
  switch (item.type) {
    case "step":
      return plainLabel(item.title) || (annotated ? "Untitled page" : "Untitled step");
    case "section":
      return plainLabel(item.title) || "Untitled section";
    case "note":
      return plainLabel(item.body) || `Empty ${item.kind}`;
    case "decision":
      return plainLabel(item.question) || "Untitled decision";
  }
}

function ItemIcon({ item }: { item: Item }) {
  if (item.type === "section") return <Heading className="size-3.5 text-muted-foreground" />;
  if (item.type === "decision") return <GitBranch className="size-3.5 text-primary" />;
  if (item.type === "note") {
    if (item.kind === "tip") return <Lightbulb className="size-3.5 text-[var(--status-success)]" />;
    if (item.kind === "warning") return <TriangleAlert className="size-3.5 text-[var(--status-warning)]" />;
    return <Info className="size-3.5 text-[var(--status-info)]" />;
  }
  if (item.kind === "manual") return <ImageIcon className="size-3.5 text-muted-foreground" />;
  return <MousePointerClick className="size-3.5 text-muted-foreground" />;
}

/** A step's screenshot, small: the close-up if it has one. */
function StepThumb({ guideId, step }: { guideId: string; step: Step }) {
  const url = useStepThumbnail(guideId, step.image.crop || step.image.full || null);
  return (
    <span className="flex h-8 w-12 shrink-0 items-center justify-center overflow-hidden rounded-[5px] border border-sidebar-border bg-background compact:h-6 compact:w-9">
      {url ? <img src={url} alt="" className="size-full object-cover" /> : <ItemIcon item={step} />}
    </span>
  );
}

/**
 * The native context menu of a step in the outline: Re-capture Step and
 * Insert Recorded Steps Here (not for pages), Set as Cover (or Remove as
 * Cover) and, on the cover, how it is drawn. Each is one undoable change.
 */
async function showStepMenu(event: MouseEvent, step: Step, page: boolean) {
  event.preventDefault();
  const { doc } = editorStore.getState();
  if (!doc || doc.readOnly) return;
  const isCover = coverStepId(doc.guide) === step.id;
  const options = coverOptions(doc.guide);
  const noun = page ? "Page" : "Step";
  const { status, pending } = recordingStore.getState();
  const idle = status.state === "idle" && pending === null;
  try {
    const menu = await Menu.new({
      items: [
        ...(page
          ? []
          : [
              {
                id: "recapture",
                text: "Re-capture Step",
                enabled: idle,
                action: () => void startRecapture(editorStore, startFlow, step.id),
              },
              {
                id: "insert-recorded",
                text: "Insert Recorded Steps Here",
                enabled: idle,
                action: () => void startInsert(editorStore, startFlow, step.id),
              },
              { item: "Separator" as const },
            ]),
        {
          id: "toggle-cover",
          text: isCover ? "Remove as Cover" : "Set as Cover",
          enabled: !!step.image.full,
          action: () => toggleCover(editorStore, step.id),
        },
        ...(isCover
          ? [
              { item: "Separator" as const },
              {
                id: "cover-close-up",
                text: "Cover Shows the Close-up",
                checked: options.image === "crop",
                enabled: !!step.image.crop,
                action: () => changeCover(editorStore, { image: "crop" }),
              },
              {
                id: "cover-full",
                text: "Cover Shows the Full Screenshot",
                checked: options.image === "full",
                action: () => changeCover(editorStore, { image: "full" }),
              },
              {
                id: "cover-annotations",
                text: `Draw the ${noun}'s Annotations on the Cover`,
                checked: options.annotations,
                action: () => changeCover(editorStore, { annotations: !options.annotations }),
              },
            ]
          : []),
      ],
    });
    await menu.popup();
  } catch (error) {
    console.error("step menu failed:", error);
  }
}

/**
 * Regenerates the guide details (title, intro, prerequisites, summary,
 * tags), edited or not, as one undoable change.
 */
function RegenerateGuide({ readOnly }: { readOnly: boolean }) {
  const { busy, background } = useAiBlocked();
  const writing = useAi((s) => s.explicit?.kind === "guide");
  return (
    <Button
      variant="ghost"
      size="icon"
      className={cn(writing && "animate-pulse")}
      aria-label={writing ? "Writing the guide details" : "Regenerate guide details"}
      title={background ? BUSY_HINT : "Regenerate the title, introduction, prerequisites, summary and tags with AI"}
      disabled={readOnly || busy}
      onClick={() => void aiStore.getState().regenerateGuide()}
    >
      <Sparkles />
    </Button>
  );
}

/** Whether keys typed at `target` are text, so J and K type rather than navigate. */
function isTextTarget(target: EventTarget | null): boolean {
  return (
    target instanceof HTMLElement &&
    (target.isContentEditable || !!target.closest("input, textarea, select, [contenteditable]"))
  );
}

/** Focuses the outline entry of item `id` (null: Guide details) once it has rendered. */
function focusOutlineItem(id: string | null) {
  requestAnimationFrame(() =>
    document
      .querySelector<HTMLElement>(id === null ? "[data-outline-details]" : `[data-outline-item="${id}"]`)
      ?.focus(),
  );
}

/** Focuses item `id`'s first text field (a step's title) once its editor has loaded. */
function focusItemText(id: string, frames = 30) {
  requestAnimationFrame(() => {
    const field = document.querySelector<HTMLElement>(`main [data-item="${id}"] [role="textbox"][contenteditable="true"]`);
    if (field) field.focus();
    else if (frames > 0) focusItemText(id, frames - 1);
  });
}

function Kbd({ children }: { children: string }) {
  return <kbd className="rounded border border-sidebar-border px-1 font-sans text-[10px]">{children}</kbd>;
}

/** Where dragged items will land: a line with a dot, above the item. */
function DropLine() {
  return (
    <span
      aria-hidden
      className="pointer-events-none absolute -top-[2px] right-1 left-2 z-10 h-0.5 rounded-full bg-[var(--status-info)]"
    >
      <span className="absolute top-1/2 -left-1.5 size-2 -translate-y-1/2 rounded-full border-2 border-[var(--status-info)] bg-sidebar" />
    </span>
  );
}

/**
 * The open guide's title and its items, with selection (⇧-click or ⇧↑/⇧↓
 * for a range), keyboard navigation, folding sections and reordering.
 */
export function Outline() {
  const doc = useEditor((s) => s.doc)!;
  const selectedId = useEditor((s) => s.selectedId);
  const { guide, readOnly } = doc;
  // Annotation guides have pages, which AI doesn't write.
  const annotated = guide.kind === "annotated";
  const noun = stepNoun(annotated);
  const numbers = useMemo(() => stepNumbers(guide.items), [guide.items]);
  const cover = coverStepId(guide);
  // The dragged items: one, a range, or a collapsed section with its items.
  const [dragIds, setDragIds] = useState<string[]>([]);
  const [dropIndex, setDropIndex] = useState<number | null>(null);
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(new Set());
  // A range runs from its anchor to the selected item; selecting anything
  // else (a click, J/K, undo) ends it.
  const [range, setRange] = useState<{ anchor: string; focus: string } | null>(null);
  // Read out by screen readers when it changes.
  const [announcement, setAnnouncement] = useState("");

  const ranged = range && range.focus === selectedId ? rangeIds(guide.items, range.anchor, range.focus) : [];
  const selection = ranged.length > 1 ? ranged : [];
  const visible = useMemo(() => visibleItems(guide.items, collapsed), [guide.items, collapsed]);
  const shown = new Set(visible.map((item) => item.id));

  const { dispatch, select } = editorStore.getState();
  const items = () => editorStore.getState().doc!.guide.items;
  /** What an action on item `id` applies to: the range it is in, or itself. */
  const targets = (id: string) => (selection.includes(id) ? selection : [id]);

  const setFolded = (id: string, folded: boolean) =>
    setCollapsed((current) => {
      const next = new Set(current);
      if (folded) next.add(id);
      else next.delete(id);
      return next;
    });
  const toggle = (id: string) => {
    const folding = !collapsed.has(id);
    // The selection can't hide: it moves up to the section.
    if (folding && selectedId && sectionOf(items(), selectedId) === id) select(id);
    setFolded(id, folding);
  };
  /** Opens the collapsed section the item `id` landed in. */
  const reveal = (id: string) => {
    const section = sectionOf(items(), id);
    if (section && collapsed.has(section)) setFolded(section, false);
  };

  /** Moves the items `ids` and announces where they went. */
  const moveTo = (ids: string[], to: number | null) => {
    if (to === null || !dispatch(moveItems(editorStore.getState().doc!.guide, ids, to))) return;
    reveal(ids[0]);
    setAnnouncement(moveAnnouncement(items(), ids));
  };
  /** Moves the items `ids` past the item shown above (-1) or below (1). */
  const shift = (ids: string[], by: -1 | 1) => {
    const block = withCollapsed(items(), ids, collapsed);
    moveTo(block, shiftTarget(items(), block, by, collapsed));
  };
  const remove = (ids: string[]) => {
    if (!dispatch(deleteItems(editorStore.getState().doc!.guide, ids))) return;
    setRange(null);
    // Keep the keyboard in the outline, on the newly selected item.
    focusOutlineItem(editorStore.getState().selectedId);
  };
  const merge = (ids: string[]) => {
    if (!dispatch(mergeSteps(editorStore.getState().doc!.guide, ids))) return;
    setRange(null);
    focusOutlineItem(editorStore.getState().selectedId);
  };

  /** Drops the dragged items before position `before` (items.length: the end). */
  const drop = (before: number) => {
    if (dragIds.length > 0) moveTo(dragIds, blockDropTarget(items(), dragIds, before));
    setDragIds([]);
    setDropIndex(null);
  };
  const dropZone = (before: number) => ({
    onDragOver: (e: DragEvent) => {
      if (dragIds.length === 0) return;
      e.preventDefault();
      setDropIndex(before);
    },
    onDrop: (e: DragEvent) => {
      e.preventDefault();
      drop(before);
    },
  });
  /** Whether dropping before `index` would leave the dragged items where they are. */
  const dropsInPlace = (index: number) =>
    dragIds.includes(guide.items[index]?.id) || dragIds.includes(guide.items[index - 1]?.id);

  const onItemKeyDown = (e: KeyboardEvent<HTMLButtonElement>, item: Item) => {
    if (e.key === "Escape" && selection.length > 0) {
      e.preventDefault();
      setRange(null);
      return;
    }
    const key = outlineKeyFor(e);
    if (!key) return;
    e.preventDefault();
    const at = visible.findIndex((other) => other.id === item.id);
    /** Selects `target` (with `extend`, the range up to it) and keeps the keyboard on it. */
    const go = (target: Item | undefined, extend = false) => {
      if (!target) return;
      setRange(extend ? { anchor: selection.length > 0 ? range!.anchor : item.id, focus: target.id } : null);
      select(target.id);
      focusOutlineItem(target.id);
    };
    switch (key) {
      case "previous":
      case "extendUp":
        return go(visible[at - 1], key === "extendUp");
      case "next":
      case "extendDown":
        return go(visible[at + 1], key === "extendDown");
      case "first":
        return go(visible[0]);
      case "last":
        return go(visible[visible.length - 1]);
      case "moveUp":
      case "moveDown":
        if (readOnly) return;
        shift(targets(item.id), key === "moveUp" ? -1 : 1);
        // Moving the button can drop its focus.
        return focusOutlineItem(item.id);
      case "collapse": {
        if (item.type === "section" && !collapsed.has(item.id)) return toggle(item.id);
        const section = sectionOf(guide.items, item.id);
        return go(guide.items.find((other) => other.id === section));
      }
      case "expand":
        if (collapsed.has(item.id)) toggle(item.id);
        return;
      case "edit":
        setRange(null);
        select(item.id);
        return focusItemText(item.id);
      case "delete":
        if (!readOnly) remove(targets(item.id));
        return;
    }
  };

  // A newly opened guide starts unfolded, with no range.
  useEffect(() => {
    setCollapsed(new Set());
    setRange(null);
  }, [guide.id]);

  // An item selected inside a collapsed section (J/K, ‹ ›, undo) unfolds it.
  useEffect(() => {
    const section = selectedId && sectionOf(guide.items, selectedId);
    if (section && collapsed.has(section)) setFolded(section, false);
  }, [selectedId, guide.items, collapsed]);

  useEffect(() => {
    const { doc, selectedId } = editorStore.getState();
    if (doc) setAnnouncement(selectionAnnouncement(doc.guide.items, selectedId, selection.length, annotated));
  }, [selectedId, selection.length, annotated]);

  // Editor-wide keys outside text fields: J/K and ⌘[ ⌘] for the previous
  // and next item, ⌘D for the guide details. Esc in the item's text goes
  // back to the outline.
  useEffect(() => {
    const onKeyDown = (e: globalThis.KeyboardEvent) => {
      const target = e.target instanceof HTMLElement ? e.target : null;
      // Dialogs and menus keep their keys.
      if (!target || target.closest('[role="dialog"], [role="menu"]')) return;
      const { doc, selectedId, select } = editorStore.getState();
      if (!doc) return;
      if (isTextTarget(target)) {
        // ProseMirror marks every Esc handled; other fields (the link
        // popover) only when they used it.
        if (e.key !== "Escape" || e.metaKey || e.ctrlKey || e.altKey || e.shiftKey || !target.closest("main")) return;
        if (e.defaultPrevented && !target.isContentEditable) return;
        e.preventDefault();
        focusOutlineItem(selectedId);
        return;
      }
      if (e.defaultPrevented) return;
      const action = navigationFor(e);
      if (!action) return;
      e.preventDefault();
      const items = doc.guide.items;
      const index = items.findIndex((item) => item.id === selectedId);
      let next: string | null = null;
      if (action !== "details") {
        // From the guide details (index -1), next is the first item.
        const item = action === "next" ? items[index + 1] : index > 0 ? items[index - 1] : undefined;
        if (!item) return;
        next = item.id;
      }
      select(next);
      // Keep the keyboard in the outline if it was there.
      if (target.closest("[data-outline-item], [data-outline-details]")) focusOutlineItem(next);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  const back = async () => {
    if (await editorStore.getState().closeGuide()) await editorStore.getState().loadGuides();
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex flex-col gap-1.5 border-b border-sidebar-border p-2.5">
        <div className="flex items-center justify-between">
          <Button variant="ghost" size="sm" title="Close this guide and go back to the library" onClick={() => void back()}>
            <ArrowLeft /> Library
          </Button>
          <Suspense
            fallback={
              <Button variant="outline" size="sm" disabled>
                <Plus /> Insert
              </Button>
            }
          >
            <InsertMenu disabled={readOnly} />
          </Suspense>
        </div>
        <div className="flex items-center gap-1">
          <Input
            aria-label="Guide title"
            className="h-9 border-transparent bg-transparent px-2 text-[15px] font-semibold hover:border-sidebar-border focus-visible:bg-background"
            value={guide.title}
            disabled={readOnly}
            onChange={(e) => dispatch(renameGuide(editorStore.getState().doc!.guide, e.target.value))}
          />
          {!annotated && <RegenerateGuide readOnly={readOnly} />}
        </div>
        <button
          type="button"
          aria-current={selectedId === null || undefined}
          title={`Edit the guide's introduction, prerequisites, summary and tags (${shortcutLabel("⌘D")})`}
          data-outline-details
          onClick={() => select(null)}
          onKeyDown={(e) => {
            if (e.key !== "ArrowDown" || e.altKey || e.shiftKey || e.metaKey || e.ctrlKey || !visible[0]) return;
            e.preventDefault();
            select(visible[0].id);
            focusOutlineItem(visible[0].id);
          }}
          className={cn(
            "flex items-center gap-2 rounded-lg px-2 py-1.5 text-left text-[13px] outline-none hover:bg-sidebar-foreground/5 focus-visible:ring-2 focus-visible:ring-sidebar-ring",
            selectedId === null && "bg-sidebar-accent text-sidebar-accent-foreground shadow-sm hover:bg-sidebar-accent",
          )}
        >
          <FileText className="size-3.5 text-muted-foreground" /> Guide details
        </button>
      </div>
      <ol
        aria-label="Outline"
        className="flex min-h-0 flex-1 flex-col gap-px overflow-y-auto p-2"
        onDragLeave={() => setDropIndex(null)}
      >
        {guide.items.map((item, index) => {
          if (!shown.has(item.id)) return null;
          const folded = item.type === "section" && collapsed.has(item.id);
          const count = folded ? sectionStepCount(guide.items, item.id) : 0;
          return (
            <li
              key={item.id}
              draggable={!readOnly}
              onDragStart={(e) => {
                setDragIds(withCollapsed(guide.items, targets(item.id), collapsed));
                e.dataTransfer.effectAllowed = "move";
              }}
              onDragEnd={() => {
                setDragIds([]);
                setDropIndex(null);
              }}
              {...dropZone(index)}
              className={cn(
                "group relative flex items-center gap-0.5 rounded-lg",
                item.type === "section" && "mt-2 first:mt-0",
                dragIds.includes(item.id) && "opacity-50",
              )}
            >
              {dropIndex === index && !dropsInPlace(index) && <DropLine />}
              {item.type === "section" && (
                <button
                  type="button"
                  // The keyboard folds with ← and → on the item.
                  tabIndex={-1}
                  aria-label={folded ? "Expand section" : "Collapse section"}
                  title={folded ? "Expand section (→)" : "Collapse section (←)"}
                  onClick={() => toggle(item.id)}
                  className="flex size-6 shrink-0 items-center justify-center rounded-md text-muted-foreground hover:bg-sidebar-foreground/5"
                >
                  <ChevronRight className={cn("size-3.5 transition-transform", !folded && "rotate-90")} />
                </button>
              )}
              <button
                type="button"
                aria-current={item.id === selectedId || undefined}
                aria-expanded={item.type === "section" ? !folded : undefined}
                // ⇧-click selects a range, not text.
                onMouseDown={(e) => e.shiftKey && e.preventDefault()}
                onClick={(e) => {
                  if (e.shiftKey && selectedId !== null && selectedId !== item.id) {
                    setRange({ anchor: selection.length > 0 ? range!.anchor : selectedId, focus: item.id });
                  } else {
                    setRange(null);
                  }
                  select(item.id);
                }}
                onContextMenu={item.type === "step" && !readOnly ? (e) => void showStepMenu(e, item, annotated) : undefined}
                onKeyDown={(e) => onItemKeyDown(e, item)}
                data-outline-item={item.id}
                className={cn(
                  "flex min-w-0 flex-1 items-center gap-2 rounded-lg px-1.5 py-1.5 text-left text-[13px] compact:py-1 text-sidebar-foreground/85 outline-none hover:bg-sidebar-foreground/5 focus-visible:ring-2 focus-visible:ring-sidebar-ring",
                  item.type === "section" && "py-1 compact:py-0.5 pl-0.5 text-[11px] font-semibold tracking-wide text-muted-foreground uppercase",
                  item.type === "note" && "bg-sidebar-foreground/[0.04] text-xs",
                  selection.includes(item.id) && "bg-sidebar-accent/60 text-sidebar-accent-foreground",
                  item.id === selectedId &&
                    "bg-sidebar-accent text-sidebar-accent-foreground shadow-sm hover:bg-sidebar-accent",
                )}
              >
                {item.type === "step" ? (
                  <>
                    <span
                      className={cn(
                        "w-4 shrink-0 text-right text-[11px] text-muted-foreground tabular-nums",
                        item.id === selectedId && "font-semibold text-brand",
                      )}
                    >
                      {numbers.get(item.id)}
                    </span>
                    <StepThumb guideId={guide.id} step={item} />
                  </>
                ) : (
                  (item.type === "note" || item.type === "decision") && (
                    <span className="flex w-4 shrink-0 justify-end">
                      <ItemIcon item={item} />
                    </span>
                  )
                )}
                <span className={cn(item.type === "step" ? "line-clamp-2 leading-snug" : "truncate")}>
                  {itemLabel(item, annotated)}
                </span>
                {item.type === "section" && !!item.audiences?.length && (
                  <span
                    className="shrink-0 truncate rounded-full bg-sidebar-foreground/[0.06] px-1.5 py-px text-[10px] font-medium tracking-normal normal-case"
                    title="Who this section is for"
                    data-testid="section-audiences"
                  >
                    {item.audiences.join(", ")}
                  </span>
                )}
                {item.id === cover && (
                  <span
                    className="shrink-0 rounded-full bg-foreground/[0.06] px-1.5 py-0.5 text-[10px] text-muted-foreground"
                    title={`This ${annotated ? "page" : "step"}'s screenshot is the guide's cover`}
                  >
                    Cover
                  </span>
                )}
                {folded && (
                  <span className="ml-auto shrink-0 rounded-full bg-sidebar-foreground/[0.06] px-1.5 py-px text-[10px] font-medium tracking-normal normal-case tabular-nums">
                    {count} {count === 1 ? noun : `${noun}s`}
                  </span>
                )}
                {item.type === "step" && !annotated && <StepStatusBadge stepId={item.id} aiState={item.aiState} />}
              </button>
              {!readOnly && (
                <span className="flex items-center opacity-0 group-focus-within:opacity-100 group-hover:opacity-100">
                  <span
                    aria-hidden
                    title="Drag to reorder"
                    className="flex h-7 w-4 cursor-grab items-center justify-center text-muted-foreground active:cursor-grabbing"
                  >
                    <GripVertical className="size-3.5" />
                  </span>
                  <Button
                    variant="ghost"
                    size="icon"
                    aria-label="Move up"
                    title={`Move up (${shortcutLabel("⌥↑")})`}
                    disabled={index === 0}
                    onClick={() => shift(targets(item.id), -1)}
                  >
                    <ArrowUp />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon"
                    aria-label="Move down"
                    title={`Move down (${shortcutLabel("⌥↓")})`}
                    disabled={index === guide.items.length - 1}
                    onClick={() => shift(targets(item.id), 1)}
                  >
                    <ArrowDown />
                  </Button>
                </span>
              )}
            </li>
          );
        })}
        {dragIds.length > 0 && (
          // Drop target after the last item.
          <li aria-hidden {...dropZone(guide.items.length)} className="relative h-8">
            {dropIndex === guide.items.length && !dropsInPlace(guide.items.length) && <DropLine />}
          </li>
        )}
      </ol>
      {selection.length > 0 && (
        <div role="toolbar" aria-label="Selected items" className="flex items-center gap-0.5 border-t border-sidebar-border px-2 py-1.5 text-xs">
          <span className="flex-1 pl-1 font-medium tabular-nums">{selection.length} items selected</span>
          {!readOnly && (
            <>
              <Button variant="ghost" size="icon" aria-label="Move selected items up" title={`Move up (${shortcutLabel("⌥↑")})`} onClick={() => shift(selection, -1)}>
                <ArrowUp />
              </Button>
              <Button variant="ghost" size="icon" aria-label="Move selected items down" title={`Move down (${shortcutLabel("⌥↓")})`} onClick={() => shift(selection, 1)}>
                <ArrowDown />
              </Button>
              {canMergeRange(guide.items, selection) && (
                <Button
                  variant="ghost"
                  size="icon"
                  aria-label={`Merge ${selection.length} ${noun}s`}
                  title={`Merge these ${noun}s into the first`}
                  onClick={() => merge(selection)}
                >
                  <Merge />
                </Button>
              )}
              <Button variant="ghost" size="icon" aria-label="Delete selected items" title="Delete (⌫)" onClick={() => remove(selection)}>
                <Trash2 />
              </Button>
            </>
          )}
          <Button variant="ghost" size="icon" aria-label="Clear the selection" title="Clear the selection (Esc)" onClick={() => setRange(null)}>
            <X />
          </Button>
        </div>
      )}
      <p
        className="border-t border-sidebar-border px-3 py-2 text-[11px] leading-relaxed text-muted-foreground"
        title={[
          `J / K or ${shortcutLabel("⌘[")} / ${shortcutLabel("⌘]")}: previous and next, anywhere`,
          `${shortcutLabel("⌘D")}: guide details`,
          "Home / End: first and last",
          "Enter: edit the title; Esc: back to the outline",
          "← / →: collapse and expand a section",
          `${isWindows() ? "Shift" : "⇧"}-click or ${shortcutLabel("⇧↑")} / ${shortcutLabel("⇧↓")}: select a range`,
        ].join("\n")}
      >
        <Kbd>↑</Kbd> <Kbd>↓</Kbd> to move between {noun}s
        {!readOnly && (
          <>
            , <Kbd>{shortcutLabel("⌥↑")}</Kbd> <Kbd>{shortcutLabel("⌥↓")}</Kbd> to reorder
          </>
        )}
      </p>
      <div aria-live="polite" aria-atomic className="sr-only" data-testid="outline-announcement">
        {announcement}
      </div>
    </div>
  );
}

