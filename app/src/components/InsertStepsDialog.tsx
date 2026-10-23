import { useEffect, useMemo, useState } from "react";
import { Dialog } from "radix-ui";
import { LoaderCircle, X } from "lucide-react";
import { create } from "zustand";
import { Button } from "@guiderecorder/ui/button";
import { NativeSelect, NativeSelectOption } from "@guiderecorder/ui/native-select";
import { markdownToPlainText } from "@guiderecorder/ui/markdown";
import { insertStepsFrom } from "@/editor/actions";
import { editorStore, stepNoun, useAnnotated, useEditor } from "@/editor/editor";
import { errorMessage, sourceSteps } from "@/lib/ipc";
import type { Step } from "@guiderecorder/guide-schema";

/** Where the dialog inserts: below the selection, or after the last item. */
export const insertStepsStore = create<{ open: boolean; atEnd: boolean }>(() => ({ open: false, atEnd: false }));

export function openInsertSteps(atEnd: boolean) {
  insertStepsStore.setState({ open: true, atEnd });
}

/** Picks a guide of the library and some of its steps to copy into the open guide. */
export function InsertStepsDialog() {
  const { open, atEnd } = insertStepsStore();
  const close = () => insertStepsStore.setState({ open: false });
  const guides = useEditor((s) => s.guides);
  const current = useEditor((s) => s.doc?.guide);
  const annotated = useAnnotated();
  const noun = stepNoun(annotated);
  // Guides of the same kind: steps into a walkthrough, pages into an annotation guide.
  const sources = useMemo(
    () =>
      guides
        .filter((g) => g.id !== current?.id && g.kind === (current?.kind ?? "walkthrough") && g.stepCount > 0)
        .sort((a, b) => a.title.localeCompare(b.title)),
    [guides, current?.id, current?.kind],
  );
  const [sourceId, setSourceId] = useState("");
  const [steps, setSteps] = useState<Step[] | null>(null);
  const [chosen, setChosen] = useState<Set<string>>(new Set());
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!open) return;
    setSourceId("");
    setSteps(null);
    setError(null);
  }, [open]);

  useEffect(() => {
    if (!sourceId) return;
    let gone = false;
    setSteps(null);
    setError(null);
    sourceSteps(sourceId).then(
      (loaded) => {
        if (gone) return;
        setSteps(loaded);
        setChosen(new Set(loaded.map((s) => s.id)));
      },
      (e: unknown) => !gone && setError(errorMessage(e)),
    );
    return () => {
      gone = true;
    };
  }, [sourceId]);

  const toggle = (id: string) =>
    setChosen((now) => {
      const next = new Set(now);
      if (!next.delete(id)) next.add(id);
      return next;
    });

  const insert = async () => {
    if (!steps || chosen.size === 0) return;
    setBusy(true);
    const ids = steps.filter((s) => chosen.has(s.id)).map((s) => s.id);
    const added = await insertStepsFrom(editorStore, sourceId, ids, atEnd);
    setBusy(false);
    if (added > 0) close();
    else setError(editorStore.getState().notice ?? `Could not insert the ${noun}s.`);
  };

  const all = !!steps && chosen.size === steps.length;
  return (
    <Dialog.Root open={open} onOpenChange={(next) => !next && !busy && close()}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-black/40 backdrop-blur-[2px]" />
        <Dialog.Content className="fixed top-1/2 left-1/2 z-50 flex max-h-[85vh] w-[min(30rem,96vw)] -translate-x-1/2 -translate-y-1/2 flex-col gap-4 rounded-2xl border border-border bg-card p-5 text-card-foreground shadow-2xl">
          <div className="flex items-center justify-between">
            <Dialog.Title className="text-base font-semibold">Insert {noun}s from another guide</Dialog.Title>
            <Dialog.Close asChild>
              <Button variant="ghost" size="icon" aria-label="Close" disabled={busy}>
                <X />
              </Button>
            </Dialog.Close>
          </div>
          <Dialog.Description className="text-sm text-muted-foreground">
            The {noun}s are copied with their screenshots and annotations. Later changes to the other guide don't change them.
          </Dialog.Description>
          <NativeSelect aria-label="Guide" value={sourceId} disabled={busy} onChange={(e) => setSourceId(e.target.value)}>
            <NativeSelectOption value="">{sources.length === 0 ? "No other guides with steps" : "Choose a guide…"}</NativeSelectOption>
            {sources.map((g) => (
              <NativeSelectOption key={g.id} value={g.id}>
                {g.title || "Untitled guide"}
              </NativeSelectOption>
            ))}
          </NativeSelect>

          {sourceId && !steps && !error && <LoaderCircle className="size-4 animate-spin text-muted-foreground" />}
          {steps && (
            <div className="flex min-h-0 flex-col gap-2">
              <label className="flex items-center gap-2 text-xs text-muted-foreground">
                <input
                  type="checkbox"
                  checked={all}
                  onChange={() => setChosen(all ? new Set() : new Set(steps.map((s) => s.id)))}
                />
                All {steps.length} {noun}s
              </label>
              <ul aria-label={`${noun[0]!.toUpperCase()}${noun.slice(1)}s`} className="min-h-0 overflow-y-auto rounded-lg border border-border">
                {steps.map((s, i) => {
                  const title = markdownToPlainText(s.title).replace(/\n/g, " ") || `Untitled ${noun}`;
                  return (
                    <li key={s.id} className="border-b border-border last:border-b-0">
                      <label className="flex items-center gap-2 px-3 py-1.5 text-sm">
                        <input type="checkbox" checked={chosen.has(s.id)} onChange={() => toggle(s.id)} />
                        <span className="w-6 shrink-0 text-right text-xs text-muted-foreground tabular-nums">{i + 1}.</span>
                        <span className="truncate">{title}</span>
                      </label>
                    </li>
                  );
                })}
              </ul>
            </div>
          )}
          {error && (
            <p role="alert" className="text-sm text-[var(--status-danger)]">
              {error}
            </p>
          )}

          <div className="flex justify-end gap-2">
            <Dialog.Close asChild>
              <Button type="button" variant="ghost" size="sm" disabled={busy}>
                Cancel
              </Button>
            </Dialog.Close>
            <Button size="sm" disabled={!steps || chosen.size === 0 || busy} onClick={() => void insert()}>
              {busy && <LoaderCircle className="animate-spin" />}
              Insert {chosen.size > 0 && steps ? chosen.size : ""} {chosen.size === 1 ? noun : `${noun}s`}
            </Button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
