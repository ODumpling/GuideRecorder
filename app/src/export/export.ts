import { stepNumbers } from "@/editor/editor";
import { errorMessage, type ExportApi } from "@/lib/ipc";
import type { AppError, ExportBranding, ExportFormat, ExportResult, Item, PaperSize } from "@/lib/types";

export const FORMATS: { format: ExportFormat; label: string; detail: string }[] = [
  { format: "md", label: "Markdown", detail: "A folder with guide.md and its images" },
  { format: "html", label: "HTML", detail: "A folder with one self-contained guide.html" },
  {
    format: "walkthrough",
    label: "Interactive walkthrough",
    detail: "A click-through guide.html, one step at a time",
  },
  { format: "docx", label: "Word", detail: "One .docx file" },
  { format: "pdf", label: "PDF", detail: "One .pdf file" },
];

export const PAPERS: { paper: PaperSize; label: string; detail: string }[] = [
  { paper: "a4", label: "A4", detail: "210 × 297 mm" },
  { paper: "letter", label: "Letter", detail: "8.5 × 11 in" },
];

/** Regions that use US Letter (CLDR `paperSize`); everywhere else uses A4. */
const LETTER_REGIONS = new Set(["BZ", "CA", "CL", "CO", "CR", "GT", "MX", "NI", "PA", "PH", "PR", "SV", "US", "VE"]);

/**
 * The paper size usual where `locales` (BCP 47 tags, most preferred
 * first) point: Letter in the US, Canada, Mexico, the Philippines and the
 * other Letter regions, A4 elsewhere. A tag without a region takes its
 * likely one ("en" → US).
 */
export function localePaper(locales: readonly string[]): PaperSize {
  for (const tag of locales) {
    try {
      const region = new Intl.Locale(tag).maximize().region;
      if (region) return LETTER_REGIONS.has(region.toUpperCase()) ? "letter" : "a4";
    } catch {
      // Not a valid tag: try the next.
    }
  }
  return "a4";
}

/** The last paper chosen in the Export dialog (on this Mac). */
const PAPER_KEY = "guiderecorder.exportPaper";

/** The paper chosen last time, else the system locale's. */
export function initialPaper(
  storage: Pick<Storage, "getItem"> = localStorage,
  locales: readonly string[] = navigator.languages,
): PaperSize {
  let stored: string | null = null;
  try {
    stored = storage.getItem(PAPER_KEY);
  } catch {
    // Storage unavailable: fall back to the locale.
  }
  return stored === "a4" || stored === "letter" ? stored : localePaper(locales);
}

/** Remembers `paper` for the next time the dialog opens, also after a restart. */
export function rememberPaper(paper: PaperSize, storage: Pick<Storage, "setItem"> = localStorage) {
  try {
    storage.setItem(PAPER_KEY, paper);
  } catch {
    // Not remembered; the locale default applies next time.
  }
}

/** Whether the Export dialog last showed the review details (on this Mac). */
const REVIEW_DETAILS_KEY = "guiderecorder.exportReviewDetails";

/** Whether to show the version, owner and review day, as chosen last time; off at first. */
export function initialReviewDetails(storage: Pick<Storage, "getItem"> = localStorage): boolean {
  try {
    return storage.getItem(REVIEW_DETAILS_KEY) === "true";
  } catch {
    return false;
  }
}

/** Remembers the review details choice, also after a restart. */
export function rememberReviewDetails(on: boolean, storage: Pick<Storage, "setItem"> = localStorage) {
  try {
    storage.setItem(REVIEW_DETAILS_KEY, String(on));
  } catch {
    // Not remembered; off next time.
  }
}

/** Markdown and HTML (either kind) export into a folder; DOCX and PDF into one file. */
export function isFolderFormat(format: ExportFormat): boolean {
  return format === "md" || format === "html" || format === "walkthrough";
}

/**
 * A file or folder name for a guide title: no path separators, colons or
 * control characters, no leading dots, at most 100 characters.
 */
export function exportName(title: string): string {
  const name = title
    .replace(/[/\\:\u0000-\u001f\u007f]/g, "-")
    .replace(/\s+/g, " ")
    .trim()
    .replace(/^\.+/, "")
    .slice(0, 100)
    .trim();
  return name || "Guide";
}

/** Whether a folder name typed by the user is one plain name (the Rust side checks too). */
export function isPlainName(name: string): boolean {
  return name.trim() !== "" && name !== "." && name !== ".." && !/[/\u0000]/.test(name);
}

/** An accent the exporters accept: `#rgb` or `#rrggbb`; empty means none. */
export function isAccent(value: string): boolean {
  return value.trim() === "" || /^#([0-9a-f]{3}|[0-9a-f]{6})$/i.test(value.trim());
}

/** `name` inside `folder` (macOS paths). */
export function joinPath(folder: string, name: string): string {
  return `${folder.replace(/\/+$/, "")}/${name}`;
}

/** The last part of a path, for display. */
export function baseName(path: string): string {
  return path.replace(/\/+$/, "").split("/").pop() || path;
}

/** Where an export goes, as chosen in the dialog. */
export interface Destination {
  /** For Markdown and HTML: the folder chosen, and the export folder's name in it. */
  folder: string | null;
  name: string;
  /** For DOCX and PDF: the file chosen in the save dialog. */
  file: string | null;
}

/** The `out` path for `format`, or null while the destination is incomplete. */
export function outPath(format: ExportFormat, destination: Destination): string | null {
  if (isFolderFormat(format)) {
    const name = destination.name.trim();
    return destination.folder && isPlainName(name) ? joinPath(destination.folder, name) : null;
  }
  const file = destination.file;
  return file && file.toLowerCase().endsWith(`.${format}`) ? file : null;
}

export type ExportOutcome =
  | { ok: true; result: ExportResult }
  | { ok: false; error: string; exists: boolean };

/**
 * Saves the editor's changes, then exports from the saved bundle. The
 * export is refused when the save fails, as it would miss the latest
 * changes.
 */
export async function runExport(
  deps: { flush(): Promise<boolean>; api: ExportApi },
  request: {
    guideId: string;
    format: ExportFormat;
    out: string;
    overwrite: boolean;
    branding: ExportBranding;
    paper: PaperSize;
    reviewDetails: boolean;
  },
): Promise<ExportOutcome> {
  if (!(await deps.flush())) {
    return {
      ok: false,
      exists: false,
      error: "Your latest changes could not be saved, so they would be missing from the export. Try again once the guide is saved.",
    };
  }
  try {
    const { guideId, format, out, overwrite, branding, paper, reviewDetails } = request;
    const accent = branding.accent?.trim() || null;
    const result = await deps.api.exportGuide(
      guideId,
      format,
      out,
      overwrite,
      { logo: branding.logo, accent },
      paper,
      reviewDetails,
    );
    return { ok: true, result };
  } catch (error) {
    const exists = !!error && typeof error === "object" && (error as AppError).kind === "exists";
    return { ok: false, exists, error: errorMessage(error) };
  }
}

/** The export's step warnings as lines naming the step by its number. */
export function warningLines(result: ExportResult, items: Item[]): string[] {
  const numbers = stepNumbers(items);
  return result.warnings.map(({ stepId, message }) => {
    const n = numbers.get(stepId);
    return `${n ? `Step ${n}` : "A removed step"}: ${message}`;
  });
}
