import { useMemo, useState, type DragEvent } from "react";
import { ArrowDown, ArrowLeft, ArrowUp, Heading, Info, MousePointerClick } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { dropTarget, moveItem, renameGuide } from "@/editor/commands";
import { editorStore, stepNumbers, useEditor } from "@/editor/editor";
import type { Item } from "@/lib/types";
import { cn } from "@/lib/utils";

function itemLabel(item: Item): string {
  switch (item.type) {
    case "step":
      return item.title || "Untitled step";
    case "section":
      return item.title || "Untitled section";
    case "note":
      return item.body || `Empty ${item.kind}`;
  }
}

function ItemIcon({ item }: { item: Item }) {
  if (item.type === "section") return <Heading className="size-3.5 text-muted-foreground" />;
  if (item.type === "note") return <Info className="size-3.5 text-[var(--status-info)]" />;
  return <MousePointerClick className="size-3.5 text-muted-foreground" />;
}

/** The open guide's title and its items, with selection and reordering. */
export function Outline() {
  const doc = useEditor((s) => s.doc)!;
  const selectedId = useEditor((s) => s.selectedId);
  const { guide, readOnly } = doc;
  const numbers = useMemo(() => stepNumbers(guide.items), [guide.items]);
  const [dragId, setDragId] = useState<string | null>(null);
  const [dropIndex, setDropIndex] = useState<number | null>(null);

  const { dispatch, select } = editorStore.getState();
  const move = (id: string, to: number) => dispatch(moveItem(editorStore.getState().doc!.guide, id, to));
  /** Drops the dragged item before position `before` (items.length: the end). */
  const drop = (before: number) => {
    const items = editorStore.getState().doc!.guide.items;
    const from = items.findIndex((item) => item.id === dragId);
    if (from >= 0) move(items[from].id, dropTarget(from, before));
    setDragId(null);
    setDropIndex(null);
  };
  const dropZone = (before: number) => ({
    onDragOver: (e: DragEvent) => {
      if (!dragId) return;
      e.preventDefault();
      setDropIndex(before);
    },
    onDrop: (e: DragEvent) => {
      e.preventDefault();
      drop(before);
    },
  });

  const back = async () => {
    if (await editorStore.getState().closeGuide()) await editorStore.getState().loadGuides();
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex flex-col gap-2 border-b border-sidebar-border p-3">
        <Button variant="ghost" size="sm" className="self-start" onClick={() => void back()}>
          <ArrowLeft /> Library
        </Button>
        <Input
          aria-label="Guide title"
          className="font-semibold"
          value={guide.title}
          disabled={readOnly}
          onChange={(e) => dispatch(renameGuide(editorStore.getState().doc!.guide, e.target.value))}
        />
      </div>
      <ol className="min-h-0 flex-1 overflow-y-auto p-2" onDragLeave={() => setDropIndex(null)}>
        {guide.items.map((item, index) => (
          <li
            key={item.id}
            draggable={!readOnly}
            onDragStart={(e) => {
              setDragId(item.id);
              e.dataTransfer.effectAllowed = "move";
            }}
            onDragEnd={() => {
              setDragId(null);
              setDropIndex(null);
            }}
            {...dropZone(index)}
            className={cn(
              "group flex items-center gap-1 rounded-md border-t-2 border-transparent",
              dropIndex === index && dragId !== item.id && "border-[var(--status-info)]",
              dragId === item.id && "opacity-50",
            )}
          >
            <button
              type="button"
              onClick={() => select(item.id)}
              className={cn(
                "flex min-w-0 flex-1 items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm hover:bg-sidebar-accent",
                item.type === "section" && "font-semibold",
                item.id === selectedId && "bg-sidebar-accent text-sidebar-accent-foreground",
              )}
            >
              {item.type === "step" ? (
                <span className="w-5 shrink-0 text-right text-xs text-muted-foreground tabular-nums">
                  {numbers.get(item.id)}
                </span>
              ) : (
                <span className="flex w-5 shrink-0 justify-end">
                  <ItemIcon item={item} />
                </span>
              )}
              <span className="truncate">{itemLabel(item)}</span>
            </button>
            {!readOnly && (
              <span className="flex opacity-0 group-focus-within:opacity-100 group-hover:opacity-100">
                <Button
                  variant="ghost"
                  size="icon"
                  aria-label="Move up"
                  disabled={index === 0}
                  onClick={() => move(item.id, index - 1)}
                >
                  <ArrowUp />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  aria-label="Move down"
                  disabled={index === guide.items.length - 1}
                  onClick={() => move(item.id, index + 1)}
                >
                  <ArrowDown />
                </Button>
              </span>
            )}
          </li>
        ))}
        {dragId && (
          // Drop target after the last item.
          <li
            aria-hidden
            {...dropZone(guide.items.length)}
            className={cn(
              "h-8 rounded-md border-t-2 border-transparent",
              dropIndex === guide.items.length && "border-[var(--status-info)]",
            )}
          />
        )}
      </ol>
    </div>
  );
}
