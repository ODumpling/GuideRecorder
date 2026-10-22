import { useEffect, useMemo, useState, type FormEvent } from "react";
import { Dialog } from "radix-ui";
import { RotateCcw, X } from "lucide-react";
import type { Guide, Step } from "@guiderecorder/guide-schema";
import { Button } from "@guiderecorder/ui/button";
import { Input } from "@guiderecorder/ui/input";
import { markdownToPlainText } from "@guiderecorder/ui/markdown";
import { editorStore, useEditor } from "@/editor/editor";
import { diffLines, diffStats, guideLines, versionName } from "@/editor/versions";
import { errorMessage, tauriHistoryApi } from "@/lib/ipc";
import type { VersionInfo } from "@/lib/types";
import { makeThumbnail } from "@/recording/thumbnails";
import { AI_ENABLED } from "@/lib/features";

// The open guide's version history: save a version, look through earlier
// ones (a text diff or a preview), and restore one as a new version.

type Compare = "current" | "previous";
type View = "changes" | "preview";

const when = (iso: string) => new Date(iso).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });

const TAB = "rounded-md px-2.5 py-1 text-xs font-medium aria-selected:bg-muted aria-selected:text-foreground text-muted-foreground";

/** A version's step image as a small blob URL; null while loading or if it fails. */
function useVersionImage(guideId: string, versionId: string, path: string | null): string | null {
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    setUrl(null);
    if (!path) return;
    let gone = false;
    let made: string | null = null;
    void tauriHistoryApi
      .image(guideId, versionId, path)
      .then((bytes) => makeThumbnail(bytes, 240))
      .then((u) => {
        made = u;
        if (!gone) setUrl(u);
        else if (u) URL.revokeObjectURL(u);
      })
      .catch(() => {});
    return () => {
      gone = true;
      if (made) URL.revokeObjectURL(made);
    };
  }, [guideId, versionId, path]);
  return url;
}

function PreviewStep({ guideId, versionId, step, number }: { guideId: string; versionId: string; step: Step; number: number }) {
  const path = step.image.show === "crop" && step.image.crop ? step.image.crop : step.image.full;
  const url = useVersionImage(guideId, versionId, path || null);
  return (
    <li className="flex gap-3 py-2">
      <div className="flex h-16 w-24 shrink-0 items-center justify-center overflow-hidden rounded border border-border bg-muted">
        {url && <img src={url} alt="" className="max-h-full max-w-full object-contain" />}
      </div>
      <div className="min-w-0 text-sm">
        <div className="font-medium">
          {number}. {markdownToPlainText(step.title) || "Untitled step"}
        </div>
        {step.description && (
          <p className="line-clamp-3 whitespace-pre-line text-xs text-muted-foreground">{markdownToPlainText(step.description)}</p>
        )}
      </div>
    </li>
  );
}

function Preview({ guideId, versionId, guide }: { guideId: string; versionId: string; guide: Guide }) {
  const steps = guide.items.filter((item): item is Step => item.type === "step");
  return (
    <div className="flex flex-col gap-2">
      <h3 className="text-base font-semibold">{guide.title || "Untitled"}</h3>
      {guide.summary && <p className="text-sm text-muted-foreground">{guide.summary}</p>}
      <ol className="divide-y divide-border">
        {steps.map((step, i) => (
          <PreviewStep key={step.id} guideId={guideId} versionId={versionId} step={step} number={i + 1} />
        ))}
      </ol>
    </div>
  );
}

function Changes({ before, after }: { before: Guide; after: Guide }) {
  const diff = useMemo(() => diffLines(guideLines(before), guideLines(after)), [before, after]);
  const { added, removed } = diffStats(diff);
  if (added === 0 && removed === 0) return <p className="text-sm text-muted-foreground">No text changes.</p>;
  return (
    <div className="flex flex-col gap-2">
      <p className="text-xs text-muted-foreground">
        <span className="text-[var(--status-success)]">+{added}</span> <span className="text-[var(--status-danger)]">−{removed}</span> lines
      </p>
      <pre aria-label="Changes" className="overflow-x-auto rounded-md border border-border font-mono text-xs leading-5">
        {diff.map((line, i) => (
          <div
            key={i}
            data-kind={line.kind}
            className={
              line.kind === "added"
                ? "bg-[color-mix(in_oklab,var(--status-success)_14%,transparent)] px-2"
                : line.kind === "removed"
                  ? "bg-[color-mix(in_oklab,var(--status-danger)_14%,transparent)] px-2 line-through decoration-1 opacity-80"
                  : "px-2 text-muted-foreground"
            }
          >
            {line.kind === "added" ? "+ " : line.kind === "removed" ? "− " : "  "}
            {line.text || " "}
          </div>
        ))}
      </pre>
    </div>
  );
}

/** The History dialog of the open guide. */
export function HistoryDialog({ open, onOpenChange }: { open: boolean; onOpenChange(open: boolean): void }) {
  const doc = useEditor((s) => s.doc);
  const busy = useEditor((s) => s.busy);
  const guideId = doc?.guide.id ?? null;
  const [versions, setVersions] = useState<VersionInfo[] | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [guides, setGuides] = useState<Map<string, Guide>>(new Map());
  const [error, setError] = useState<string | null>(null);
  const [label, setLabel] = useState("");
  const [view, setView] = useState<View>("changes");
  const [compare, setCompare] = useState<Compare>("current");

  async function reload(select?: string) {
    if (!guideId) return;
    try {
      const listed = await tauriHistoryApi.list(guideId);
      setVersions(listed);
      setSelected((current) => select ?? (listed.some((v) => v.id === current) ? current : (listed[0]?.id ?? null)));
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  useEffect(() => {
    if (!open) return;
    setVersions(null);
    setGuides(new Map());
    setError(null);
    setLabel("");
    void reload();
  }, [open, guideId]);

  const index = versions?.findIndex((v) => v.id === selected) ?? -1;
  const version = index >= 0 ? versions![index]! : null;
  const previous = index >= 0 ? (versions![index + 1] ?? null) : null;
  const wanted = [version?.id, compare === "previous" ? previous?.id : undefined].filter((id): id is string => !!id);

  useEffect(() => {
    if (!guideId) return;
    for (const id of wanted) {
      if (guides.has(id)) continue;
      tauriHistoryApi
        .read(guideId, id)
        .then((guide) => setGuides((m) => new Map(m).set(id, guide)))
        .catch((e: unknown) => setError(errorMessage(e)));
    }
  }, [guideId, wanted.join()]);

  async function save(e: FormEvent) {
    e.preventDefault();
    const info = await editorStore.getState().saveVersion(label);
    if (!info) {
      setError(editorStore.getState().notice);
      return;
    }
    setLabel("");
    setError(null);
    await reload(info.id);
  }

  async function restore() {
    if (!version) return;
    if (await editorStore.getState().restoreVersion(version.id)) {
      onOpenChange(false);
    } else {
      setError(editorStore.getState().notice);
    }
  }

  const versionGuide = version ? guides.get(version.id) : undefined;
  const base = compare === "previous" ? (previous ? guides.get(previous.id) : undefined) : doc?.guide;
  const readOnly = doc?.readOnly ?? true;

  return (
    <Dialog.Root open={open && !!doc} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-black/30" />
        <Dialog.Content className="fixed top-1/2 left-1/2 z-50 flex h-[80vh] w-[900px] max-w-[95vw] -translate-x-1/2 -translate-y-1/2 flex-col rounded-xl border border-border bg-background shadow-xl">
          <div className="flex items-start justify-between gap-4 border-b border-border p-5 pb-4">
            <div>
              <Dialog.Title className="text-base font-semibold">Version history</Dialog.Title>
              <Dialog.Description className="mt-1 text-xs text-muted-foreground">
                A version is saved when you close the guide, {AI_ENABLED && "before Write with AI, "}and when you save one here. They are kept in the guide's folder.
              </Dialog.Description>
            </div>
            <Dialog.Close asChild>
              <Button variant="ghost" size="icon" aria-label="Close">
                <X />
              </Button>
            </Dialog.Close>
          </div>
          <div className="flex min-h-0 flex-1">
            <div className="flex w-72 shrink-0 flex-col border-r border-border">
              {!readOnly && (
                <form className="flex gap-2 border-b border-border p-3" onSubmit={(e) => void save(e)}>
                  <Input
                    aria-label="Version name"
                    placeholder="Name (optional)"
                    maxLength={200}
                    value={label}
                    onChange={(e) => setLabel(e.target.value)}
                  />
                  <Button type="submit" variant="outline" disabled={busy}>
                    Save version
                  </Button>
                </form>
              )}
              <ul aria-label="Versions" className="flex-1 overflow-y-auto p-2">
                {versions === null && !error && <li className="p-2 text-sm text-muted-foreground">Loading…</li>}
                {versions?.length === 0 && (
                  <li className="p-2 text-sm text-muted-foreground">No versions yet. One is saved when you close the guide.</li>
                )}
                {versions?.map((v) => (
                  <li key={v.id}>
                    <button
                      type="button"
                      aria-current={v.id === selected}
                      onClick={() => setSelected(v.id)}
                      className="flex w-full flex-col items-start gap-0.5 rounded-md px-2.5 py-2 text-left text-sm hover:bg-muted aria-[current=true]:bg-muted"
                    >
                      <span className={v.label ? "font-medium" : ""}>{versionName(v)}</span>
                      <span className="text-xs text-muted-foreground">
                        {when(v.createdAt)} · {v.stepCount} {v.stepCount === 1 ? "step" : "steps"}
                      </span>
                    </button>
                  </li>
                ))}
              </ul>
            </div>
            <div className="flex min-w-0 flex-1 flex-col">
              {version ? (
                <>
                  <div className="flex items-center gap-2 border-b border-border px-4 py-2">
                    <div role="tablist" className="flex gap-1">
                      <button type="button" role="tab" aria-selected={view === "changes"} className={TAB} onClick={() => setView("changes")}>
                        Changes
                      </button>
                      <button type="button" role="tab" aria-selected={view === "preview"} className={TAB} onClick={() => setView("preview")}>
                        Preview
                      </button>
                    </div>
                    {view === "changes" && (
                      <select
                        aria-label="Compare with"
                        className="ml-auto h-7 rounded-md border border-input bg-transparent px-2 text-xs"
                        value={compare}
                        onChange={(e) => setCompare(e.target.value as Compare)}
                      >
                        <option value="current">Compared with the guide now</option>
                        <option value="previous" disabled={!previous}>
                          Compared with the version before
                        </option>
                      </select>
                    )}
                  </div>
                  <div className="min-h-0 flex-1 overflow-y-auto p-4">
                    {!versionGuide ? (
                      <p className="text-sm text-muted-foreground">Loading…</p>
                    ) : view === "preview" ? (
                      <Preview guideId={guideId!} versionId={version.id} guide={versionGuide} />
                    ) : compare === "previous" && !previous ? (
                      <p className="text-sm text-muted-foreground">This is the oldest version.</p>
                    ) : !base ? (
                      <p className="text-sm text-muted-foreground">Loading…</p>
                    ) : (
                      // From the base to this version: against the guide now, what a restore changes.
                      <Changes before={base} after={versionGuide} />
                    )}
                  </div>
                  <div className="flex items-center gap-3 border-t border-border px-4 py-3">
                    <p className="flex-1 text-xs text-muted-foreground">
                      {compare === "current" && view === "changes"
                        ? "Green lines come back if you restore; struck-out lines go. "
                        : ""}
                      The guide as it is now is kept as a version first.
                    </p>
                    <Button disabled={readOnly || busy} onClick={() => void restore()}>
                      <RotateCcw /> Restore this version
                    </Button>
                  </div>
                </>
              ) : (
                <div className="flex flex-1 items-center justify-center text-sm text-muted-foreground">
                  {versions?.length ? "Choose a version." : ""}
                </div>
              )}
            </div>
          </div>
          {error && (
            <p role="alert" className="border-t border-border px-5 py-2 text-sm text-[var(--status-danger)]">
              {error}
            </p>
          )}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
