import { useEffect, useState } from "react";
import { Dialog } from "radix-ui";
import {
  CircleAlert,
  CircleCheck,
  FileOutput,
  FolderOpen,
  ImageIcon,
  LoaderCircle,
  TriangleAlert,
  X,
} from "lucide-react";
import { create } from "zustand";
import { ExportPreview } from "@/components/ExportPreview";
import { Button } from "@guiderecorder/ui/button";
import { Input } from "@guiderecorder/ui/input";
import { editorStore, useEditor } from "@/editor/editor";
import { unclearSteps, unclearStepsText } from "@/editor/element";
import { inspectorStore } from "@/editor/inspector";
import {
  ASPECTS,
  baseName,
  exportName,
  FORMATS,
  initialMotion,
  initialPaper,
  initialReviewDetails,
  initialSkill,
  isAccent,
  isDocumentFormat,
  isFolderFormat,
  isMotionFormat,
  isPaperFormat,
  isSecondsPerStep,
  MAX_SECONDS_PER_STEP,
  MIN_SECONDS_PER_STEP,
  outPath,
  PAPERS,
  rememberMotion,
  rememberPaper,
  rememberReviewDetails,
  rememberSkill,
  runExport,
  unreviewedBlurs,
  unreviewedBlursText,
  warningLines,
  type Destination,
  type ExportOutcome,
} from "@/export/export";
import { errorMessage, tauriExportApi, type ExportApi } from "@/lib/ipc";
import { fileManager } from "@/lib/platform";
import type { ExportFormat, MotionOptions, PaperSize, SkillOptions } from "@/lib/types";
import { settingsStore } from "@/settings/settings";
import { cn } from "@guiderecorder/ui/utils";

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
    <span className={cn("min-w-0 flex-1 truncate text-sm", !path && "text-muted-foreground")} title={path ?? undefined}>
      {path ? baseName(path) : empty}
    </span>
  );
}

/** Each format's file badge in the format picker. */
const FORMAT_BADGES: Record<ExportFormat, string> = {
  md: "MD",
  html: "HTML",
  walkthrough: "TOUR",
  docx: "DOCX",
  pdf: "PDF",
  pptx: "PPTX",
  mp4: "MP4",
  gif: "GIF",
  skill: "SKILL",
};
const FORMAT_COLORS: Record<ExportFormat, string> = {
  md: "#4a4740",
  html: "#3a9a68",
  walkthrough: "#6b5be6",
  docx: "#2f6bff",
  pdf: "#e8543a",
  pptx: "#d35230",
  mp4: "#1d1c1a",
  gif: "#c2398f",
  skill: "#b8740f",
};

/** Whether the export dialog is open, and the format it opens on (null: the remembered one). */
export const exportDialogStore = create<{ open: boolean; format: ExportFormat | null }>(() => ({
  open: false,
  format: null,
}));

/** Opens the export dialog, from the toolbar or the command palette. */
export function openExport(format: ExportFormat | null = null) {
  exportDialogStore.setState({ open: true, format });
}

/** The export dialog for the open guide; `format` overrides the starting format. */
export function ExportDialog({
  open,
  onOpenChange,
  format: startFormat = null,
  api = tauriExportApi,
}: {
  open: boolean;
  onOpenChange(open: boolean): void;
  format?: ExportFormat | null;
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
  const [motion, setMotion] = useState<MotionOptions>(() => initialMotion());
  const [skill, setSkill] = useState<SkillOptions>(() => initialSkill());
  // The seconds field as typed, so it can be cleared while editing.
  const [seconds, setSeconds] = useState(() => String(motion.secondsPerStep));
  const [busy, setBusy] = useState(false);
  const [outcome, setOutcome] = useState<ExportOutcome | null>(null);
  const [revealError, setRevealError] = useState<string | null>(null);
  // Exporting with unreviewed blur suggestions takes a second look.
  const [skipReview, setSkipReview] = useState(false);

  // The chosen logo itself, read (and checked) by the Rust side.
  const [logoImage, setLogoImage] = useState<{ src: string | null; error: string | null }>({ src: null, error: null });
  useEffect(() => {
    setLogoImage({ src: null, error: null });
    if (!open || !logo) return;
    let gone = false;
    api.logoPreview(logo).then(
      (src) => !gone && setLogoImage({ src, error: null }),
      (error) => !gone && setLogoImage({ src: null, error: errorMessage(error) }),
    );
    return () => {
      gone = true;
    };
  }, [open, logo, api]);

  // A new guide starts from its own name and a fresh file.
  useEffect(() => {
    setDestination((d) => ({ ...d, name: exportName(title), file: null }));
    setOutcome(null);
    // Only when the guide changes, not on every title edit.
  }, [guideId]);

  useEffect(() => {
    if (open) {
      const start = initialChoices();
      setFormat(startFormat ?? start.format);
      setDestination((d) => ({ ...d, folder: start.folder }));
      setLogo(start.logo);
      setAccent(start.accent);
      setPaper(start.paper);
      setReviewDetails(initialReviewDetails());
      const startMotion = initialMotion();
      setMotion(startMotion);
      setSeconds(String(startMotion.secondsPerStep));
      setSkill(initialSkill());
      setOutcome(null);
      setRevealError(null);
      setSkipReview(false);
    }
  }, [open, startFormat]);

  const out = outPath(format, destination);
  // "Replace" covers one destination only: a new guide, format or target,
  // or reopening the dialog, asks again.
  useEffect(() => {
    setOverwrite(false);
  }, [open, guideId, out]);

  const isDocument = isDocumentFormat(format);
  // Branding only shows in the document formats.
  const accentOk = !isDocument || isAccent(accent);
  const secondsOk = !isMotionFormat(format) || isSecondsPerStep(Number(seconds));
  const unreviewed = unreviewedBlurs(doc?.guide.items ?? []);
  const reviewText = unreviewedBlursText(unreviewed);
  // Only skills find elements again; the other formats show screenshots.
  const unclear = format === "skill" ? unclearSteps(doc?.guide.items ?? []) : { stepIds: [], steps: [] };
  const unclearText = unclearStepsText(unclear.steps);
  const canExport = !!guideId && !!out && accentOk && secondsOk && !busy && (!reviewText || skipReview);

  /** Closes the dialog on the privacy review of the first step with suggestions left. */
  const review = () => {
    const first = unreviewed.stepIds[0];
    if (first) {
      editorStore.getState().select(first);
      inspectorStore.getState().setTab("privacy");
    }
    onOpenChange(false);
  };

  /**
   * Closes the dialog on the Step tab of the first step whose element is
   * unclear; it opens on the skill again afterwards.
   */
  const reviewElements = () => {
    remembered.format = format;
    const first = unclear.stepIds[0];
    if (first) {
      editorStore.getState().select(first);
      inspectorStore.getState().setTab("step");
    }
    onOpenChange(false);
  };

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
    const motionChoice = { ...motion, secondsPerStep: Number(seconds) };
    if (isMotionFormat(format)) rememberMotion(motionChoice);
    if (format === "skill") rememberSkill(skill);
    setBusy(true);
    setOutcome(null);
    setRevealError(null);
    const result = await runExport(
      { flush: () => editorStore.getState().flush(), api },
      {
        guideId,
        format,
        out,
        overwrite,
        branding: isDocument ? { logo, accent } : { logo: null, accent: "" },
        paper,
        reviewDetails: isDocument && reviewDetails,
        motion: motionChoice,
        skill,
      },
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
  const stepCount = doc?.guide.items.filter((i) => i.type === "step").length ?? 0;
  const summary = `${stepCount} ${doc?.guide.kind === "annotated" ? "page" : "step"}${stepCount === 1 ? "" : "s"}`;

  return (
    <Dialog.Root open={open} onOpenChange={(next) => !busy && onOpenChange(next)}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-black/40 backdrop-blur-[2px]" />
        <Dialog.Content className="fixed top-1/2 left-1/2 z-50 flex h-[min(44rem,92vh)] w-[min(62rem,96vw)] -translate-x-1/2 -translate-y-1/2 overflow-hidden rounded-2xl border border-border bg-card text-card-foreground shadow-2xl">
          <div className="flex w-[27rem] shrink-0 flex-col gap-4 overflow-y-auto p-6">
            <div className="flex items-start justify-between">
              <div>
                <Dialog.Title className="font-serif text-3xl leading-none tracking-tight">Export guide</Dialog.Title>
                <p className="mt-1.5 text-sm text-muted-foreground">{summary}</p>
              </div>
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
                  Exported to{" "}
                  <span className="truncate font-medium" title={done.files[0]}>
                    {done.files[0] && baseName(done.files[0])}
                  </span>
                </p>
                {done.files.length > 1 && (
                  <p className="text-xs text-muted-foreground">
                    {done.files.length - 1} more file{done.files.length > 2 ? "s" : ""} (
                    {format === "skill" ? "steps.json and images" : "images"}) next to it.
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
                <div className="sticky bottom-0 -mx-6 mt-auto -mb-6 flex justify-end gap-2 border-t border-border bg-card px-6 py-4">
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
                    {FORMATS.map((f) => (
                      <button
                        key={f.format}
                        type="button"
                        role="radio"
                        aria-checked={format === f.format}
                        disabled={busy}
                        onClick={() => setFormat(f.format)}
                        title={f.detail}
                        className={cn(
                          "flex items-center gap-2.5 rounded-xl border border-border p-2.5 text-left outline-none focus-visible:ring-2 focus-visible:ring-ring",
                          format === f.format
                            ? "border-foreground ring-1 ring-foreground"
                            : "hover:bg-foreground/[0.04]",
                        )}
                      >
                        <span
                          aria-hidden
                          className="flex h-9 w-7 shrink-0 items-end justify-center rounded-[4px] pb-1 text-[7.5px] font-bold text-white"
                          style={{ background: FORMAT_COLORS[f.format] }}
                        >
                          {FORMAT_BADGES[f.format]}
                        </span>
                        <span className="flex min-w-0 flex-col">
                          <span className="truncate text-sm font-semibold">{f.label}</span>
                          <span className="truncate text-xs text-muted-foreground">{f.detail}</span>
                        </span>
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
                      <PathLabel path={out} empty={`No .${format} file chosen`} />
                      <Button variant="outline" size="sm" disabled={busy} onClick={() => void chooseDestination()}>
                        Choose…
                      </Button>
                    </div>
                  </Field>
                )}

                {isMotionFormat(format) && (
                  <>
                    <Field label="Frame">
                      <div role="radiogroup" aria-label="Frame" className="grid grid-cols-3 gap-2">
                        {ASPECTS.map((a) => (
                          <button
                            key={a.aspect}
                            type="button"
                            role="radio"
                            aria-checked={motion.aspect === a.aspect}
                            disabled={busy}
                            onClick={() => setMotion((m) => ({ ...m, aspect: a.aspect }))}
                            className={cn(
                              "flex items-baseline justify-between gap-2 rounded-xl border border-border px-3 py-2 text-left outline-none focus-visible:ring-2 focus-visible:ring-ring",
                              motion.aspect === a.aspect
                                ? "border-foreground ring-1 ring-foreground"
                                : "hover:bg-foreground/[0.04]",
                            )}
                          >
                            <span className="text-sm font-medium">{a.label}</span>
                            <span className="text-xs text-muted-foreground">{a.detail}</span>
                          </button>
                        ))}
                      </div>
                    </Field>
                    <Field label="Seconds per step">
                      <Input
                        type="number"
                        inputMode="decimal"
                        min={MIN_SECONDS_PER_STEP}
                        max={MAX_SECONDS_PER_STEP}
                        step={0.5}
                        value={seconds}
                        disabled={busy}
                        aria-invalid={!secondsOk}
                        className={cn("w-28", !secondsOk && "border-[var(--status-danger)]")}
                        onChange={(e) => setSeconds(e.target.value)}
                      />
                      {!secondsOk && (
                        <span className="text-xs text-[var(--status-danger)]">
                          From {MIN_SECONDS_PER_STEP} to {MAX_SECONDS_PER_STEP} seconds.
                        </span>
                      )}
                    </Field>
                    <label className="flex items-center gap-2 text-sm">
                      <input
                        type="checkbox"
                        checked={motion.zoom}
                        disabled={busy}
                        onChange={(e) => setMotion((m) => ({ ...m, zoom: e.target.checked }))}
                      />
                      Zoom in on each click
                    </label>
                    <label className="flex items-center gap-2 text-sm">
                      <input
                        type="checkbox"
                        checked={motion.captions}
                        disabled={busy}
                        onChange={(e) => setMotion((m) => ({ ...m, captions: e.target.checked }))}
                      />
                      Show a title card and step captions
                    </label>
                    {format === "mp4" && busy && (
                      <span className="text-xs text-muted-foreground">Encoding the video can take a little while.</span>
                    )}
                  </>
                )}

                {format === "skill" && (
                  <label className="flex items-start gap-2 text-sm">
                    <input
                      type="checkbox"
                      className="mt-1"
                      checked={skill.replay}
                      disabled={busy}
                      onChange={(e) => setSkill({ replay: e.target.checked })}
                    />
                    <span>
                      Include a replay script
                      <span className="block text-xs text-muted-foreground">
                        replay.js repeats the steps on a Mac through the accessibility API. Run it with --dry-run first.
                      </span>
                    </span>
                  </label>
                )}

                {isPaperFormat(format) && (
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
                            paper === p.paper
                              ? "border-foreground ring-1 ring-foreground"
                              : "hover:bg-foreground/[0.04]",
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

                {isDocument && (
                  <label className="flex items-center gap-2 text-sm">
                    <input
                      type="checkbox"
                      checked={reviewDetails}
                      disabled={busy}
                      onChange={(e) => setReviewDetails(e.target.checked)}
                    />
                    Show the version, owner and last review date
                  </label>
                )}

                {isDocument && (
                  <Field label="Branding (optional)">
                    <div className="flex items-center gap-2">
                      {logo && logoImage.src ? (
                        <img src={logoImage.src} alt="" className="size-6 shrink-0 rounded-[3px] object-contain" />
                      ) : (
                        <ImageIcon className="size-4 shrink-0 text-muted-foreground" />
                      )}
                      <PathLabel path={logo} empty="No logo" />
                      {logo && (
                        <Button variant="ghost" size="sm" disabled={busy} onClick={() => setLogo(null)}>
                          Remove
                        </Button>
                      )}
                      <Button
                        variant="outline"
                        size="sm"
                        disabled={busy}
                        onClick={() => void choose(api.chooseLogo, setLogo)}
                      >
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
                    {logo && logoImage.error && (
                      <span role="alert" className="text-xs text-[var(--status-danger)]">
                        {logoImage.error}
                      </span>
                    )}
                    <span className="text-xs text-muted-foreground">
                      PNG or JPEG logo, up to 2 MB. Markdown shows the logo but not the accent colour.
                    </span>
                  </Field>
                )}

                {reviewText && (
                  <div
                    role="status"
                    className="flex flex-col gap-2 rounded-md border border-[var(--status-warning)] bg-muted p-3 text-xs"
                  >
                    <p className="flex items-start gap-2">
                      <TriangleAlert className="mt-0.5 size-3.5 shrink-0 text-[var(--status-warning)]" />
                      <span>
                        <span className="font-medium">Review blurs. </span>
                        {reviewText}
                      </span>
                    </p>
                    <div className="flex items-center justify-between gap-2">
                      <label className="flex items-center gap-2">
                        <input
                          type="checkbox"
                          checked={skipReview}
                          disabled={busy}
                          onChange={(e) => setSkipReview(e.target.checked)}
                        />
                        Export without them
                      </label>
                      <Button variant="outline" size="sm" disabled={busy} onClick={review}>
                        Review
                      </Button>
                    </div>
                  </div>
                )}

                {unclearText && (
                  <div
                    role="status"
                    aria-label="Unclear elements"
                    className="flex flex-col gap-2 rounded-md border border-[var(--status-warning)] bg-muted p-3 text-xs"
                  >
                    <p className="flex items-start gap-2">
                      <TriangleAlert className="mt-0.5 size-3.5 shrink-0 text-[var(--status-warning)]" />
                      <span>
                        <span className="font-medium">Unclear elements. </span>
                        {unclearText}
                      </span>
                    </p>
                    <div className="flex justify-end">
                      <Button variant="outline" size="sm" disabled={busy} onClick={reviewElements}>
                        Show step
                      </Button>
                    </div>
                  </div>
                )}

                {outcome && !outcome.ok && (
                  <p role="alert" className="flex items-start gap-2 text-sm text-[var(--status-danger)]">
                    <CircleAlert className="mt-0.5 size-4 shrink-0" />
                    <span>
                      {outcome.error}
                      {outcome.exists && !overwrite && " Turn on “Replace an earlier export there” to replace it."}
                    </span>
                  </p>
                )}

                <div className="sticky bottom-0 -mx-6 mt-auto -mb-6 flex justify-end gap-2 border-t border-border bg-card px-6 py-4">
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
          </div>
          {doc && (
            <div className="relative flex flex-1 justify-center overflow-hidden border-l border-border bg-sidebar px-8 pt-10 pb-0 max-md:hidden">
              <span className="absolute top-3.5 left-4 text-[11px] font-medium text-muted-foreground">
                Preview · {FORMATS.find((f) => f.format === format)?.label}
              </span>
              <ExportPreview
                guide={doc.guide}
                format={format}
                paper={paper}
                logo={logo}
                accent={accent}
                reviewDetails={reviewDetails}
                motion={motion}
                skill={skill}
                api={api}
                flush={() => editorStore.getState().flush()}
              />
            </div>
          )}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
