import { useMemo, useState } from "react";
import { ArrowDown, ArrowUp, BookCopy, FileOutput, Pilcrow, SquarePen, Trash2, TriangleAlert, X } from "lucide-react";
import { Button } from "@guiderecorder/ui/button";
import { NativeSelect, NativeSelectOption } from "@guiderecorder/ui/native-select";
import { Textarea } from "@guiderecorder/ui/textarea";
import { CollectionExportDialog } from "@/components/collections/CollectionExportDialog";
import { collectionsStore, useCollections } from "@/collections/collections";
import { collectionSummary, findCollection, moved, withGuides, withText, without } from "@/collections/store";
import { editorStore, useEditor } from "@/editor/editor";
import type { Collection, CollectionEntry, CollectionNumbering, GuideListItem } from "@/lib/types";
import { cn } from "@guiderecorder/ui/utils";

const NUMBERINGS: { value: CollectionNumbering; label: string; detail: string }[] = [
  { value: "continuous", label: "Continuous", detail: "1, 2, 3 … through the whole document" },
  { value: "per-chapter", label: "Per guide", detail: "Every guide starts again at 1" },
];

const iconButton =
  "flex size-7 items-center justify-center rounded-md text-muted-foreground outline-none hover:bg-foreground/[0.06] hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-30 [&_svg]:size-4";

/** Move up, move down and remove, at the end of an entry. */
function EntryActions({ entry, index, count, label }: { entry: CollectionEntry; index: number; count: number; label: string }) {
  const edit = (change: (entries: CollectionEntry[]) => CollectionEntry[]) =>
    collectionsStore.getState().edit(collectionsStore.getState().openId!, (c) => ({ ...c, entries: change(c.entries) }));
  return (
    <div className="flex shrink-0 items-center gap-0.5">
      <button type="button" aria-label={`Move ${label} up`} title="Move up" disabled={index === 0} onClick={() => edit((e) => moved(e, entry.id, -1))} className={iconButton}>
        <ArrowUp />
      </button>
      <button type="button" aria-label={`Move ${label} down`} title="Move down" disabled={index === count - 1} onClick={() => edit((e) => moved(e, entry.id, 1))} className={iconButton}>
        <ArrowDown />
      </button>
      <button type="button" aria-label={`Remove ${label}`} title="Remove from the collection" onClick={() => edit((e) => without(e, entry.id))} className={iconButton}>
        <X />
      </button>
    </div>
  );
}

function GuideEntry({ entry, chapter, guide, index, count }: { entry: CollectionEntry; chapter: number; guide: GuideListItem | null; index: number; count: number }) {
  const label = guide ? `“${guide.title || "Untitled guide"}”` : "missing guide";
  return (
    <li className="flex items-center gap-3 rounded-xl border border-border bg-card px-3 py-2.5">
      <span className="flex size-7 shrink-0 items-center justify-center rounded-full bg-foreground/[0.06] text-xs font-semibold tabular-nums">
        {chapter}
      </span>
      {guide ? (
        <div className="flex min-w-0 flex-1 flex-col">
          <span className="truncate text-sm font-medium">{guide.title || "Untitled guide"}</span>
          <span className="truncate text-xs text-muted-foreground">
            {guide.stepCount} {guide.kind === "annotated" ? (guide.stepCount === 1 ? "page" : "pages") : guide.stepCount === 1 ? "step" : "steps"}
            {guide.folder && <> · {guide.folder.split("/").join(" › ")}</>}
          </span>
        </div>
      ) : (
        <div className="flex min-w-0 flex-1 items-center gap-2 text-sm text-[var(--status-danger)]">
          <TriangleAlert className="size-4 shrink-0" />
          This guide is no longer in the library and is left out of exports.
        </div>
      )}
      {guide && (
        <button type="button" aria-label={`Open ${label}`} title="Open the guide" onClick={() => void editorStore.getState().openGuide(guide.id)} className={iconButton}>
          <SquarePen />
        </button>
      )}
      <EntryActions entry={entry} index={index} count={count} label={label} />
    </li>
  );
}

function TextEntry({ entry, index, count }: { entry: Extract<CollectionEntry, { type: "text" }>; index: number; count: number }) {
  const id = useCollections((s) => s.openId)!;
  return (
    <li className="flex items-start gap-3 rounded-xl border border-dashed border-border px-3 py-2.5">
      <span className="flex size-7 shrink-0 items-center justify-center text-muted-foreground">
        <Pilcrow className="size-4" />
      </span>
      <Textarea
        aria-label="Text between guides"
        placeholder="Text between guides, such as a short introduction to the next part"
        value={entry.body}
        rows={2}
        onChange={(e) => {
          const body = e.target.value;
          collectionsStore.getState().edit(id, (c) => ({
            ...c,
            entries: c.entries.map((x) => (x.id === entry.id ? { ...x, body } : x)),
          }));
        }}
        className="min-h-14 flex-1 resize-y"
      />
      <EntryActions entry={entry} index={index} count={count} label="text" />
    </li>
  );
}

/** "Add guide…": the library's guides by title. */
function AddGuide({ collection, guides }: { collection: Collection; guides: GuideListItem[] }) {
  const sorted = useMemo(() => [...guides].sort((a, b) => a.title.localeCompare(b.title)), [guides]);
  const included = new Set(collection.entries.flatMap((e) => (e.type === "guide" ? [e.guideId] : [])));
  return (
    <NativeSelect
      aria-label="Add a guide"
      value=""
      disabled={guides.length === 0}
      onChange={(e) => {
        const guideId = e.target.value;
        if (guideId) collectionsStore.getState().edit(collection.id, (c) => ({ ...c, entries: withGuides(c.entries, [guideId]) }));
      }}
      className="w-60"
    >
      <NativeSelectOption value="">{guides.length === 0 ? "No guides in the library" : "Add a guide…"}</NativeSelectOption>
      {sorted.map((g) => (
        <NativeSelectOption key={g.id} value={g.id}>
          {(g.title || "Untitled guide") + (included.has(g.id) ? " (already in)" : "")}
        </NativeSelectOption>
      ))}
    </NativeSelect>
  );
}

/** The open collection: its title, intro, numbering and guides, in the main pane. */
export function CollectionView() {
  const id = useCollections((s) => s.openId);
  const view = useCollections((s) => s.collections.find((c) => c.id === s.openId) ?? null);
  const collection = useCollections((s) => findCollection(s, s.openId));
  const error = useCollections((s) => s.error);
  const guides = useEditor((s) => s.guides);
  const [exporting, setExporting] = useState(false);
  const [deleting, setDeleting] = useState(false);

  if (!id || !view) return null;
  if (!collection) {
    return (
      <div className="mx-auto flex w-full max-w-3xl flex-col gap-4 px-9 py-8">
        <h1 className="font-serif text-[40px] leading-none tracking-tight">Unreadable collection</h1>
        <p role="alert" className="text-sm text-[var(--status-danger)]">
          {view.error}
        </p>
      </div>
    );
  }

  const edit = (change: (c: Collection) => Collection) => collectionsStore.getState().edit(collection.id, change);
  const byId = new Map(guides.map((g) => [g.id, g]));
  let chapter = 0;

  return (
    <div className="mx-auto flex w-full max-w-3xl flex-col gap-6 px-9 py-8">
      <div className="flex items-start gap-3">
        <BookCopy className="mt-2 size-6 shrink-0 text-muted-foreground" />
        <div className="min-w-0 flex-1">
          <input
            aria-label="Collection title"
            placeholder="Untitled collection"
            value={collection.title}
            maxLength={200}
            onChange={(e) => {
              const title = e.target.value;
              edit((c) => ({ ...c, title }));
            }}
            className="w-full bg-transparent font-serif text-[40px] leading-none tracking-tight outline-none placeholder:text-muted-foreground"
          />
          <p className="mt-2 text-sm text-muted-foreground">{collectionSummary(collection, guides)}</p>
        </div>
        <Button
          variant="brand"
          disabled={!collection.entries.some((e) => e.type === "guide" && byId.has(e.guideId))}
          onClick={() => setExporting(true)}
        >
          <FileOutput /> Export…
        </Button>
        {deleting ? (
          <div className="flex items-center gap-1">
            <Button variant="ghost" onClick={() => setDeleting(false)}>
              Cancel
            </Button>
            <Button
              variant="destructive"
              onClick={() => void collectionsStore.getState().remove(collection.id).then(() => setDeleting(false))}
            >
              Delete collection
            </Button>
          </div>
        ) : (
          <Button variant="ghost" size="icon" aria-label="Delete collection" title="Delete collection (its guides stay)" onClick={() => setDeleting(true)}>
            <Trash2 />
          </Button>
        )}
      </div>

      {error && (
        <p role="alert" className="text-sm text-[var(--status-danger)]">
          {error}
        </p>
      )}

      <label className="flex flex-col gap-1.5">
        <span className="text-xs font-medium text-muted-foreground">Introduction</span>
        <Textarea
          placeholder="What the manual is for and who it is for. **Bold**, *italic*, links and lists work."
          value={collection.intro}
          rows={3}
          onChange={(e) => {
            const intro = e.target.value;
            edit((c) => ({ ...c, intro }));
          }}
          className="resize-y"
        />
      </label>

      <div role="radiogroup" aria-label="Step numbers" className="flex flex-col gap-1.5">
        <span className="text-xs font-medium text-muted-foreground">Step numbers</span>
        <div className="grid grid-cols-2 gap-2">
          {NUMBERINGS.map((n) => (
            <button
              key={n.value}
              type="button"
              role="radio"
              aria-checked={collection.numbering === n.value}
              onClick={() => edit((c) => ({ ...c, numbering: n.value }))}
              className={cn(
                "flex flex-col rounded-lg border px-3 py-2 text-left outline-none focus-visible:ring-2 focus-visible:ring-ring",
                collection.numbering === n.value ? "border-ring bg-ring/5" : "border-border hover:bg-foreground/[0.03]",
              )}
            >
              <span className="text-sm font-medium">{n.label}</span>
              <span className="text-xs text-muted-foreground">{n.detail}</span>
            </button>
          ))}
        </div>
      </div>

      <div className="flex flex-col gap-2">
        <span className="text-xs font-medium text-muted-foreground">Contents</span>
        {collection.entries.length === 0 ? (
          <p className="rounded-xl border border-dashed border-border px-6 py-10 text-center text-sm text-muted-foreground">
            Add guides below, or drag them from the library onto the collection in the sidebar.
          </p>
        ) : (
          <ol aria-label="Contents" className="flex flex-col gap-2">
            {collection.entries.map((entry, index) =>
              entry.type === "guide" ? (
                <GuideEntry
                  key={entry.id}
                  entry={entry}
                  chapter={++chapter}
                  guide={byId.get(entry.guideId) ?? null}
                  index={index}
                  count={collection.entries.length}
                />
              ) : (
                <TextEntry key={entry.id} entry={entry} index={index} count={collection.entries.length} />
              ),
            )}
          </ol>
        )}
        <div className="flex items-center gap-2">
          <AddGuide collection={collection} guides={guides} />
          <Button variant="outline" onClick={() => edit((c) => ({ ...c, entries: withText(c.entries) }))}>
            <Pilcrow /> Add text
          </Button>
        </div>
      </div>

      <CollectionExportDialog collection={collection} open={exporting} onOpenChange={setExporting} />
    </div>
  );
}
