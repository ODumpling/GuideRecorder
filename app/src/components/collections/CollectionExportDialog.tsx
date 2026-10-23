import { useEffect, useState } from "react";
import { Dialog } from "radix-ui";
import { CircleCheck, FolderOpen, LoaderCircle, X } from "lucide-react";
import { Button } from "@guiderecorder/ui/button";
import { Input } from "@guiderecorder/ui/input";
import { NativeSelect, NativeSelectOption } from "@guiderecorder/ui/native-select";
import { collectionsStore } from "@/collections/collections";
import { editorStore } from "@/editor/editor";
import {
  baseName,
  exportBranding,
  exportName,
  fileExtension,
  initialPaper,
  initialPreset,
  isFolderFormat,
  outPath,
  PAPERS,
  rememberPaper,
  rememberPreset,
  type Destination,
} from "@/export/export";
import {
  errorMessage,
  tauriCollectionApi,
  tauriExportApi,
  tauriPresetApi,
  type CollectionApi,
  type ExportApi,
  type PresetApi,
} from "@/lib/ipc";
import { fileManager } from "@/lib/platform";
import type { AppError, Collection, CollectionFormat, ExportResult, PaperSize, PresetList } from "@/lib/types";
import { cn } from "@guiderecorder/ui/utils";

export const COLLECTION_FORMATS: { format: CollectionFormat; label: string; detail: string }[] = [
  { format: "pdf", label: "PDF", detail: "Contents with page numbers" },
  { format: "docx", label: "Word", detail: "Edit it further in Word" },
  { format: "html", label: "HTML", detail: "One self-contained page" },
  { format: "md", label: "Markdown", detail: "For wikis and Git" },
];

type Outcome = { ok: true; result: ExportResult } | { ok: false; error: string; exists: boolean };

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex flex-col gap-1.5">
      <span className="text-xs font-medium text-muted-foreground">{label}</span>
      {children}
    </div>
  );
}

function PathLabel({ path, empty }: { path: string | null; empty: string }) {
  return (
    <span className={cn("min-w-0 flex-1 truncate text-sm", !path && "text-muted-foreground")} title={path ?? undefined}>
      {path ? baseName(path) : empty}
    </span>
  );
}

/** Exports a collection as one document: format, paper, brand preset and where. */
export function CollectionExportDialog({
  collection,
  open,
  onOpenChange,
  api = tauriExportApi,
  collectionApi = tauriCollectionApi,
  presetApi = tauriPresetApi,
}: {
  collection: Collection;
  open: boolean;
  onOpenChange(open: boolean): void;
  api?: ExportApi;
  collectionApi?: CollectionApi;
  presetApi?: PresetApi;
}) {
  const [format, setFormat] = useState<CollectionFormat>("pdf");
  const [paper, setPaper] = useState<PaperSize>(() => initialPaper());
  const [destination, setDestination] = useState<Destination>({ folder: null, name: "", file: null });
  const [overwrite, setOverwrite] = useState(false);
  const [presets, setPresets] = useState<PresetList | null>(null);
  const [preset, setPreset] = useState("");
  const [busy, setBusy] = useState(false);
  const [outcome, setOutcome] = useState<Outcome | null>(null);
  const [revealError, setRevealError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    setOutcome(null);
    setRevealError(null);
    setDestination((d) => ({ ...d, name: exportName(collection.title || "Collection") }));
    let gone = false;
    presetApi.listPresets().then(
      (list) => {
        if (gone) return;
        setPresets(list);
        setPreset(initialPreset(list));
      },
      () => {
        if (!gone) setPresets({ presets: [], defaultId: null });
      },
    );
    return () => {
      gone = true;
    };
    // Only when it opens: the title may change while it is open.
  }, [open, presetApi]);

  const out = outPath(format, destination);
  const folderFormat = isFolderFormat(format);
  const paperFormat = format === "pdf" || format === "docx";

  const choose = async () => {
    try {
      if (folderFormat) {
        const folder = await api.chooseFolder(destination.folder);
        if (folder) setDestination((d) => ({ ...d, folder }));
      } else {
        const name = exportName(collection.title || "Collection");
        const file = await api.chooseFile(format, name, destination.folder);
        if (file) setDestination((d) => ({ ...d, file }));
      }
    } catch (error) {
      setOutcome({ ok: false, exists: false, error: errorMessage(error) });
    }
  };

  const start = async () => {
    if (!out || busy) return;
    rememberPaper(paper);
    if (presets && presets.presets.length > 0) rememberPreset(preset);
    setBusy(true);
    setOutcome(null);
    // The collection and an open guide of it are exported as saved.
    const saved = (await collectionsStore.getState().flush()) && (await editorStore.getState().flush());
    if (!saved) {
      setBusy(false);
      setOutcome({ ok: false, exists: false, error: "Your latest changes could not be saved, so they would be missing from the export." });
      return;
    }
    try {
      const branding = exportBranding(format, { logo: null, accent: "", preset });
      const result = await collectionApi.exportCollection(collection.id, format, out, overwrite, branding, paper);
      setOutcome({ ok: true, result });
    } catch (error) {
      const exists = !!error && typeof error === "object" && (error as AppError).kind === "exists";
      setOutcome({ ok: false, exists, error: errorMessage(error) });
    }
    setBusy(false);
  };

  const reveal = async (path: string) => {
    try {
      await api.reveal(path);
    } catch (error) {
      setRevealError(errorMessage(error));
    }
  };

  const done = outcome?.ok ? outcome.result : null;
  const failed = outcome && !outcome.ok ? outcome : null;

  return (
    <Dialog.Root open={open} onOpenChange={(next) => !busy && onOpenChange(next)}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-black/40 backdrop-blur-[2px]" />
        <Dialog.Content className="fixed top-1/2 left-1/2 z-50 flex max-h-[92vh] w-[min(30rem,96vw)] -translate-x-1/2 -translate-y-1/2 flex-col gap-4 overflow-y-auto rounded-2xl border border-border bg-card p-6 text-card-foreground shadow-2xl">
          <div className="flex items-start justify-between">
            <div className="min-w-0">
              <Dialog.Title className="font-serif text-3xl leading-none tracking-tight">Export collection</Dialog.Title>
              <p className="mt-1.5 truncate text-sm text-muted-foreground">{collection.title || "Untitled collection"}</p>
            </div>
            <Dialog.Close asChild>
              <Button variant="ghost" size="icon" aria-label="Close" disabled={busy}>
                <X />
              </Button>
            </Dialog.Close>
          </div>
          <Dialog.Description className="sr-only">
            One document with a table of contents, each guide a chapter.
          </Dialog.Description>

          {done ? (
            <div className="flex flex-col gap-3">
              <p className="flex items-center gap-2 text-sm">
                <CircleCheck className="size-4 shrink-0 text-[var(--status-success)]" />
                Exported to{" "}
                <span className="truncate font-medium" title={done.files[0]}>
                  {done.files[0] && baseName(done.files[0])}
                </span>
              </p>
              {(done.warnings.length > 0 || done.notes.length > 0) && (
                <div className="flex flex-col gap-1 rounded-md border border-border bg-muted p-3 text-xs">
                  <span className="font-medium">Exported with warnings</span>
                  <ul className="list-disc space-y-0.5 pl-4">
                    {done.notes.map((note, i) => (
                      <li key={`n${i}`}>{note}</li>
                    ))}
                    {done.warnings.length > 0 && (
                      <li>
                        {done.warnings.length} step image{done.warnings.length === 1 ? "" : "s"} with warnings; export the
                        guide on its own to see which.
                      </li>
                    )}
                  </ul>
                </div>
              )}
              {revealError && <p className="text-xs text-[var(--status-danger)]">{revealError}</p>}
              <div className="flex justify-end gap-2">
                <Button variant="outline" onClick={() => setOutcome(null)}>
                  Export again
                </Button>
                {done.files[0] && (
                  <Button variant="outline" onClick={() => void reveal(done.files[0]!)}>
                    <FolderOpen /> Reveal in {fileManager()}
                  </Button>
                )}
                <Button onClick={() => onOpenChange(false)}>Done</Button>
              </div>
            </div>
          ) : (
            <>
              <Field label="Format">
                <div role="radiogroup" aria-label="Format" className="grid grid-cols-2 gap-2">
                  {COLLECTION_FORMATS.map((f) => (
                    <button
                      key={f.format}
                      type="button"
                      role="radio"
                      aria-checked={format === f.format}
                      disabled={busy}
                      onClick={() => setFormat(f.format)}
                      className={cn(
                        "flex flex-col rounded-xl border border-border px-3 py-2 text-left outline-none focus-visible:ring-2 focus-visible:ring-ring",
                        format === f.format ? "border-foreground ring-1 ring-foreground" : "hover:bg-foreground/[0.04]",
                      )}
                    >
                      <span className="text-sm font-medium">{f.label}</span>
                      <span className="text-xs text-muted-foreground">{f.detail}</span>
                    </button>
                  ))}
                </div>
              </Field>

              {folderFormat ? (
                <>
                  <Field label="Save into folder">
                    <div className="flex items-center gap-2">
                      <PathLabel path={destination.folder} empty="No folder chosen" />
                      <Button variant="outline" size="sm" disabled={busy} onClick={() => void choose()}>
                        Choose…
                      </Button>
                    </div>
                  </Field>
                  <Field label="Export folder name">
                    <Input
                      aria-label="Export folder name"
                      value={destination.name}
                      disabled={busy}
                      aria-invalid={!outPath(format, { ...destination, folder: "/" })}
                      onChange={(e) => setDestination((d) => ({ ...d, name: e.target.value }))}
                    />
                  </Field>
                </>
              ) : (
                <Field label="Save as">
                  <div className="flex items-center gap-2">
                    <PathLabel path={out} empty={`No .${fileExtension(format)} file chosen`} />
                    <Button variant="outline" size="sm" disabled={busy} onClick={() => void choose()}>
                      Choose…
                    </Button>
                  </div>
                </Field>
              )}

              {paperFormat && (
                <Field label="Paper size">
                  <div role="radiogroup" aria-label="Paper size" className="grid grid-cols-2 gap-2">
                    {PAPERS.map((p) => (
                      <button
                        key={p.paper}
                        type="button"
                        role="radio"
                        aria-checked={paper === p.paper}
                        disabled={busy}
                        onClick={() => setPaper(p.paper)}
                        className={cn(
                          "flex items-baseline justify-between gap-2 rounded-xl border border-border px-3 py-2 text-left outline-none focus-visible:ring-2 focus-visible:ring-ring",
                          paper === p.paper ? "border-foreground ring-1 ring-foreground" : "hover:bg-foreground/[0.04]",
                        )}
                      >
                        <span className="text-sm font-medium">{p.label}</span>
                        <span className="text-xs text-muted-foreground">{p.detail}</span>
                      </button>
                    ))}
                  </div>
                </Field>
              )}

              {presets && presets.presets.length > 0 && (
                <Field label="Brand preset">
                  <NativeSelect aria-label="Brand preset" value={preset} disabled={busy} onChange={(e) => setPreset(e.target.value)}>
                    <NativeSelectOption value="">None</NativeSelectOption>
                    {presets.presets.map((p) => (
                      <NativeSelectOption key={p.id} value={p.id} disabled={!p.preset || !!p.error}>
                        {p.preset?.name ?? p.id}
                        {p.id === presets.defaultId ? " (library default)" : ""}
                      </NativeSelectOption>
                    ))}
                  </NativeSelect>
                </Field>
              )}

              <label className="flex items-center gap-2 text-sm">
                <input type="checkbox" checked={overwrite} disabled={busy} onChange={(e) => setOverwrite(e.target.checked)} />
                Replace an earlier export there
              </label>

              {failed && (
                <p role="alert" className="text-sm text-[var(--status-danger)]">
                  {failed.error}
                  {failed.exists && !overwrite && " Turn on “Replace an earlier export there” to replace it."}
                </p>
              )}

              <div className="flex justify-end gap-2">
                <Dialog.Close asChild>
                  <Button variant="ghost" disabled={busy}>
                    Cancel
                  </Button>
                </Dialog.Close>
                <Button variant="brand" disabled={!out || busy} onClick={() => void start()}>
                  {busy && <LoaderCircle className="animate-spin" />} Export
                </Button>
              </div>
            </>
          )}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
