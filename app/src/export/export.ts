import { stepNumbers } from "@/editor/editor";
import { errorMessage, type ExportApi } from "@/lib/ipc";
import type { AppError, ExportBranding, ExportFormat, ExportResult, Item } from "@/lib/types";

export const FORMATS: { format: ExportFormat; label: string; detail: string }[] = [
  { format: "md", label: "Markdown", detail: "A folder with guide.md and its images" },
  { format: "html", label: "HTML", detail: "A folder with one self-contained guide.html" },
  { format: "docx", label: "Word", detail: "One .docx file" },
  { format: "pdf", label: "PDF", detail: "One .pdf file" },
];

/** Markdown and HTML export into a folder; DOCX and PDF into one file. */
export function isFolderFormat(format: ExportFormat): boolean {
  return format === "md" || format === "html";
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
  request: { guideId: string; format: ExportFormat; out: string; overwrite: boolean; branding: ExportBranding },
): Promise<ExportOutcome> {
  if (!(await deps.flush())) {
    return {
      ok: false,
      exists: false,
      error: "Your latest changes could not be saved, so they would be missing from the export. Try again once the guide is saved.",
    };
  }
  try {
    const { guideId, format, out, overwrite, branding } = request;
    const accent = branding.accent?.trim() || null;
    const result = await deps.api.exportGuide(guideId, format, out, overwrite, { logo: branding.logo, accent });
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
