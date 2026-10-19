import { useEffect, useState } from "react";
import { Dialog } from "radix-ui";
import { CircleAlert, CircleCheck, FileOutput, FolderOpen, ImageIcon, LoaderCircle, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { editorStore, useEditor } from "@/editor/editor";
import {
  baseName,
  exportName,
  FORMATS,
  initialPaper,
  initialReviewDetails,
  isAccent,
  isFolderFormat,
  outPath,
  PAPERS,
  rememberPaper,
  rememberReviewDetails,
  runExport,
  warningLines,
  type Destination,
  type ExportOutcome,
} from "@/export/export";
import { errorMessage, tauriExportApi, type ExportApi } from "@/lib/ipc";
import type { ExportFormat, PaperSize } from "@/lib/types";
import { settingsStore } from "@/settings/settings";
import { cn } from "@/lib/utils";

/**
 * Choices made in this dialog while the app runs; until then the dialog
 * starts from the Export defaults in Settings. Not "Replace": that is
 * asked afresh for every destination. The paper size and the review
 * details choice are kept across restarts too, see `initialPaper`.
 */
const remembered: {
  format?: ExportFormat;
  folder?: string | null;
  logo?: string | null;
  accent?: string;
  paper?: PaperSize;
} = {};

/** The dialog's starting choices: this session's, else the Settings defaults. */
function initialChoices() {
  const defaults = settingsStore.getState().settings?.export;
  return {
    format: remembered.format ?? defaults?.format ?? "pdf",
    folder: remembered.folder !== undefined ? remembered.folder : (defaults?.folder ?? null),
    logo: remembered.logo !== undefined ? remembered.logo : (defaults?.logo ?? null),
    accent: remembered.accent ?? defaults?.accent ?? "",
    paper: remembered.paper ?? initialPaper(),
  };
}

/** `#rgb` as `#rrggbb`, for the colour well. */
function wellColor(accent: string): string {
  const value = accent.trim();
  if (/^#[0-9a-f]{6}$/i.test(value)) return value.toLowerCase();
  if (/^#[0-9a-f]{3}$/i.test(value)) {
    return `#${[...value.slice(1)].map((c) => c + c).join("")}`.toLowerCase();
  }
  return "#3b82f6";
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex flex-col gap-1.5">
      <span className="text-xs font-medium text-muted-foreground">{label}</span>
      {children}
    </div>
  );
}

/** A chosen path: its last part, with the full path on hover. */
function PathLabel({ path, empty }: { path: string | null; empty: string }) {
  return (
    <span
      className={cn("min-w-0 flex-1 truncate text-sm", !path && "text-muted-foreground")}
      title={path ?? undefined}
    >
      {path ? baseName(path) : empty}
    </span>
  );
}

/** The export dialog for the open guide. */
export function ExportDialog({
  open,
  onOpenChange,
  api = tauriExportApi,
}: {
  open: boolean;
  onOpenChange(open: boolean): void;
  api?: ExportApi;
}) {
  const doc = useEditor((s) => s.doc);
  const guideId = doc?.guide.id ?? null;
  const title = doc?.guide.title ?? "";

  const [format, setFormat] = useState<ExportFormat>(() => initialChoices().format);
  const [destination, setDestination] = useState<Destination>(() => ({
    folder: initialChoices().folder,
    name: exportName(title),
    file: null,
  }));
  const [overwrite, setOverwrite] = useState(false);
  const [logo, setLogo] = useState<string | null>(() => initialChoices().logo);
  const [accent, setAccent] = useState(() => initialChoices().accent);
  const [paper, setPaper] = useState<PaperSize>(() => initialChoices().paper);
  const [reviewDetails, setReviewDetails] = useState(() => initialReviewDetails());
  const [busy, setBusy] = useState(false);
  const [outcome, setOutcome] = useState<ExportOutcome | null>(null);
  const [revealError, setRevealError] = useState<string | null>(null);

  // A new guide starts from its own name and a fresh file.
  useEffect(() => {
    setDestination((d) => ({ ...d, name: exportName(title), file: null }));
    setOutcome(null);
    // Only when the guide changes, not on every title edit.
  }, [guideId]);

  useEffect(() => {
    if (open) {
      const start = initialChoices();
      setFormat(start.format);
      setDestination((d) => ({ ...d, folder: start.folder }));
      setLogo(start.logo);
      setAccent(start.accent);
      setPaper(start.paper);
      setReviewDetails(initialReviewDetails());
      setOutcome(null);
      setRevealError(null);
    }
  }, [open]);

  const out = outPath(format, destination);
  // "Replace" covers one destination only: a new guide, format or target,
  // or reopening the dialog, asks again.
  useEffect(() => {
    setOverwrite(false);
  }, [open, guideId, out]);

  const accentOk = isAccent(accent);
  const canExport = !!guideId && !!out && accentOk && !busy;

  const choose = async (pick: () => Promise<string | null>, apply: (path: string) => void) => {
    try {
      const path = await pick();
      if (path) apply(path);
    } catch (error) {
      setOutcome({ ok: false, exists: false, error: errorMessage(error) });
    }
  };

  const chooseDestination = () =>
    isFolderFormat(format)
      ? choose(
          () => api.chooseFolder(destination.folder),
          (folder) => setDestination((d) => ({ ...d, folder })),
        )
      : choose(
          () => api.chooseFile(format, destination.name.trim() || exportName(title), destination.folder),
          (file) => setDestination((d) => ({ ...d, file })),
        );

  const start = async () => {
    if (!guideId || !out || !canExport) return;
    Object.assign(remembered, { format, folder: destination.folder, logo, accent, paper });
    rememberPaper(paper);
    rememberReviewDetails(reviewDetails);
    setBusy(true);
    setOutcome(null);
    setRevealError(null);
    const result = await runExport(
      { flush: () => editorStore.getState().flush(), api },
      { guideId, format, out, overwrite, branding: { logo, accent }, paper, reviewDetails },
    );
    setBusy(false);
    setOutcome(result);
  };

  const reveal = async (path: string) => {
    try {
      await api.reveal(path);
    } catch (error) {
      setRevealError(errorMessage(error));
    }
  };

  const done = outcome?.ok ? outcome.result : null;

  return (
    <Dialog.Root open={open} onOpenChange={(next) => !busy && onOpenChange(next)}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-black/40" />
        <Dialog.Content className="fixed top-1/2 left-1/2 z-50 flex max-h-[90vh] w-[30rem] -translate-x-1/2 -translate-y-1/2 flex-col gap-4 overflow-y-auto rounded-lg border border-border bg-background p-5 text-foreground shadow-lg">
          <div className="flex items-center justify-between">
            <Dialog.Title className="text-base font-semibold">Export guide</Dialog.Title>
            <Dialog.Close asChild>
              <Button variant="ghost" size="icon" aria-label="Close" disabled={busy}>
                <X />
              </Button>
            </Dialog.Close>
          </div>
          <Dialog.Description className="sr-only">
            Choose a format, where to save it, and optional branding.
          </Dialog.Description>

          {done ? (
            <div className="flex flex-col gap-3">
              <p className="flex items-center gap-2 text-sm">
                <CircleCheck className="size-4 shrink-0 text-[var(--status-success)]" />
                Exported to <span className="truncate font-medium" title={done.files[0]}>{done.files[0] && baseName(done.files[0])}</span>
              </p>
              {done.files.length > 1 && (
                <p className="text-xs text-muted-foreground">
                  {done.files.length - 1} more file{done.files.length > 2 ? "s" : ""} (images) next to it.
                </p>
              )}
              {(done.warnings.length > 0 || done.notes.length > 0) && (
                <div className="flex flex-col gap-1 rounded-md border border-border bg-muted p-3 text-xs">
                  <span className="font-medium">Exported with warnings</span>
                  <ul className="list-disc space-y-0.5 pl-4">
                    {doc && warningLines(done, doc.guide.items).map((line, i) => <li key={`w${i}`}>{line}</li>)}
                    {done.notes.map((note, i) => (
                      <li key={`n${i}`}>{note}</li>
                    ))}
                  </ul>
                </div>
              )}
              {revealError && <p className="text-xs text-[var(--status-danger)]">{revealError}</p>}
              <div className="mt-1 flex justify-end gap-2">
                <Button variant="outline" onClick={() => setOutcome(null)}>
                  Export again
                </Button>
                {done.files[0] && (
                  <Button variant="outline" onClick={() => void reveal(done.files[0]!)}>
                    <FolderOpen /> Reveal in Finder
                  </Button>
                )}
                <Button onClick={() => onOpenChange(false)}>Done</Button>
              </div>
            </div>
          ) : (
            <>
              <Field label="Format">
                <div role="radiogroup" aria-label="Format" className="grid grid-cols-2 gap-2">
                  {FORMATS.map((f) => (
                    <button
                      key={f.format}
                      type="button"
                      role="radio"
                      aria-checked={format === f.format}
                      disabled={busy}
                      onClick={() => setFormat(f.format)}
                      className={cn(
                        "flex flex-col items-start gap-0.5 rounded-md border border-border px-3 py-2 text-left outline-none focus-visible:ring-2 focus-visible:ring-ring",
                        format === f.format ? "border-[var(--status-info)] bg-muted" : "hover:bg-muted/60",
                      )}
                    >
                      <span className="text-sm font-medium">{f.label}</span>
                      <span className="text-xs text-muted-foreground">{f.detail}</span>
                    </button>
                  ))}
                </div>
              </Field>

              {isFolderFormat(format) ? (
                <>
                  <Field label="Save into folder">
                    <div className="flex items-center gap-2">
                      <PathLabel path={destination.folder} empty="No folder chosen" />
                      <Button variant="outline" size="sm" disabled={busy} onClick={() => void chooseDestination()}>
                        Choose…
                      </Button>
                    </div>
                  </Field>
                  <Field label="Export folder name">
                    <Input
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
                    <PathLabel
                      path={out}
                      empty={`No .${format} file chosen`}
                    />
                    <Button variant="outline" size="sm" disabled={busy} onClick={() => void chooseDestination()}>
                      Choose…
                    </Button>
                  </div>
                </Field>
              )}

              {!isFolderFormat(format) && (
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
                          "flex items-baseline justify-between gap-2 rounded-md border border-border px-3 py-2 text-left outline-none focus-visible:ring-2 focus-visible:ring-ring",
                          paper === p.paper ? "border-[var(--status-info)] bg-muted" : "hover:bg-muted/60",
                        )}
                      >
                        <span className="text-sm font-medium">{p.label}</span>
                        <span className="text-xs text-muted-foreground">{p.detail}</span>
                      </button>
                    ))}
                  </div>
                </Field>
              )}

              <label className="flex items-center gap-2 text-sm">
                <input
                  type="checkbox"
                  checked={overwrite}
                  disabled={busy}
                  onChange={(e) => setOverwrite(e.target.checked)}
                />
                Replace an earlier export there
              </label>

              <label className="flex items-center gap-2 text-sm">
                <input
                  type="checkbox"
                  checked={reviewDetails}
                  disabled={busy}
                  onChange={(e) => setReviewDetails(e.target.checked)}
                />
                Show the version, owner and last review date
              </label>

              <Field label="Branding (optional)">
                <div className="flex items-center gap-2">
                  <ImageIcon className="size-4 shrink-0 text-muted-foreground" />
                  <PathLabel path={logo} empty="No logo" />
                  {logo && (
                    <Button variant="ghost" size="sm" disabled={busy} onClick={() => setLogo(null)}>
                      Remove
                    </Button>
                  )}
                  <Button variant="outline" size="sm" disabled={busy} onClick={() => void choose(api.chooseLogo, setLogo)}>
                    Choose logo…
                  </Button>
                </div>
                <div className="flex items-center gap-2">
                  <input
                    type="color"
                    aria-label="Accent colour"
                    className="h-8 w-10 shrink-0 cursor-pointer rounded-md border border-input bg-transparent p-0.5"
                    value={wellColor(accent)}
                    disabled={busy}
                    onChange={(e) => setAccent(e.target.value)}
                  />
                  <Input
                    placeholder="Accent colour, e.g. #3b82f6"
                    value={accent}
                    disabled={busy}
                    aria-invalid={!accentOk}
                    className={cn(!accentOk && "border-[var(--status-danger)]")}
                    onChange={(e) => setAccent(e.target.value)}
                  />
                  {accent && (
                    <Button variant="ghost" size="sm" disabled={busy} onClick={() => setAccent("")}>
                      Clear
                    </Button>
                  )}
                </div>
                <span className="text-xs text-muted-foreground">
                  PNG or JPEG logo, up to 2 MB. Markdown shows the logo but not the accent colour.
                </span>
              </Field>

              {outcome && !outcome.ok && (
                <p role="alert" className="flex items-start gap-2 text-sm text-[var(--status-danger)]">
                  <CircleAlert className="mt-0.5 size-4 shrink-0" />
                  <span>
                    {outcome.error}
                    {outcome.exists && !overwrite && " Turn on “Replace an earlier export there” to replace it."}
                  </span>
                </p>
              )}

              <div className="flex justify-end gap-2">
                <Dialog.Close asChild>
                  <Button variant="outline" disabled={busy}>
                    Cancel
                  </Button>
                </Dialog.Close>
                <Button disabled={!canExport} onClick={() => void start()}>
                  {busy ? <LoaderCircle className="animate-spin" /> : <FileOutput />}
                  {busy ? "Exporting…" : "Export"}
                </Button>
              </div>
            </>
          )}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
