import { execFile } from "node:child_process";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { promisify } from "node:util";
import { and, eq, isNotNull, sql } from "drizzle-orm";
import { z } from "zod";
import type { Database } from "@/db";
import { exportJob, guide, revision, revisionAsset } from "@/db/schema";
import { assetKey } from "./assets";
import type { BlobStore } from "./storage";
import { visibleTo, type SyncError } from "./sync";

// Downloads of a guide as PDF, DOCX, Markdown or HTML. The web app queues a
// job per revision and format; the export worker (`worker/export.ts`, in its
// own container with the Rust CLI) claims it, rebuilds the bundle from the
// revision and its assets, runs `guiderecorder export`, and stores the file.
// Revisions never change, so a finished export is served again as is.

export const EXPORT_FORMATS = {
  pdf: { label: "PDF", extension: "pdf", contentType: "application/pdf" },
  docx: {
    label: "Word",
    extension: "docx",
    contentType: "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
  },
  // `guide.md` plus `images/`, zipped.
  md: { label: "Markdown", extension: "zip", contentType: "application/zip" },
  // One self-contained `guide.html`.
  html: { label: "HTML", extension: "html", contentType: "text/html; charset=utf-8" },
} as const;

export type ExportFormat = keyof typeof EXPORT_FORMATS;
export const exportFormat = z.enum(Object.keys(EXPORT_FORMATS) as [ExportFormat, ...ExportFormat[]]);

/** A running job not finished after this long is taken to have died with its worker. */
export const STALE_AFTER_MS = 10 * 60 * 1000;
/** Tries before a job that keeps dying is given up. */
export const MAX_ATTEMPTS = 3;
const MAX_ERROR_LENGTH = 500;

export type ExportJob = typeof exportJob.$inferSelect;

export type ExportStatus =
  | { status: "pending" | "running" }
  | { status: "done"; key: string }
  | { status: "failed"; error: string };

export function exportKey(organizationId: string, revisionId: string, format: ExportFormat) {
  return `orgs/${organizationId}/exports/${revisionId}/${format}.${EXPORT_FORMATS[format].extension}`;
}

/** A file name for the download: the guide's title without characters file systems refuse. */
export function exportFilename(title: string, format: ExportFormat) {
  const name = title.replace(/[\u0000-\u001f\u007f/\\:*?"<>|]/g, " ").replace(/\s+/g, " ").trim().slice(0, 120) || "Guide";
  return `${name}.${EXPORT_FORMATS[format].extension}`;
}

// Timestamps are stored as UTC without a zone, as Drizzle writes them.
const stale = (now: Date) => sql`${exportJob.startedAt} < ${new Date(now.getTime() - STALE_AFTER_MS).toISOString()}::timestamp`;

/**
 * The export of a revision of a guide the user can see, queued if there is
 * none yet. `retry` queues a failed export again.
 */
export async function requestExport(
  db: Database,
  organizationId: string,
  userId: string,
  request: { guideId: string; revisionId: string; format: ExportFormat; retry?: boolean },
  now = new Date(),
): Promise<ExportStatus | SyncError> {
  const [found] = await db
    .select({ id: revision.id })
    .from(revision)
    .innerJoin(guide, and(eq(guide.organizationId, revision.organizationId), eq(guide.id, revision.guideId)))
    .where(
      and(
        eq(revision.id, request.revisionId),
        eq(revision.organizationId, organizationId),
        eq(revision.guideId, request.guideId),
        visibleTo(userId),
        isNotNull(guide.headRevisionId),
      ),
    );
  if (!found) return { status: 404, error: "Guide not found." };

  await db
    .insert(exportJob)
    .values({
      id: crypto.randomUUID(),
      organizationId,
      guideId: request.guideId,
      revisionId: request.revisionId,
      format: request.format,
      requestedBy: userId,
    })
    .onConflictDoNothing({ target: [exportJob.revisionId, exportJob.format] });
  const [job] = await db
    .select()
    .from(exportJob)
    .where(and(eq(exportJob.revisionId, request.revisionId), eq(exportJob.format, request.format)));
  if (!job) return { status: 404, error: "Guide not found." };

  const gaveUp = job.status === "running" && job.attempts >= MAX_ATTEMPTS && job.startedAt && now.getTime() - job.startedAt.getTime() > STALE_AFTER_MS;
  if ((job.status === "failed" || gaveUp) && request.retry) {
    await db
      .update(exportJob)
      .set({ status: "pending", error: null, attempts: 0, startedAt: null, finishedAt: null, requestedBy: userId })
      .where(eq(exportJob.id, job.id));
    return { status: "pending" };
  }
  if (gaveUp) return { status: "failed", error: "The export took too long." };
  if (job.status === "done" && job.key) return { status: "done", key: job.key };
  if (job.status === "failed") return { status: "failed", error: job.error ?? "The export failed." };
  return { status: job.status === "running" ? "running" : "pending" };
}

/**
 * Takes the oldest queued job (or one whose worker died) for this worker.
 * `skip locked` lets several workers claim side by side.
 */
export async function claimExport(db: Database, now = new Date()): Promise<ExportJob | null> {
  const [job] = await db
    .update(exportJob)
    .set({ status: "running", startedAt: now, attempts: sql`${exportJob.attempts} + 1` })
    .where(
      eq(
        exportJob.id,
        sql`(select ${exportJob.id} from ${exportJob}
          where ${exportJob.status} = 'pending'
             or (${exportJob.status} = 'running' and ${stale(now)} and ${exportJob.attempts} < ${MAX_ATTEMPTS})
          order by ${exportJob.createdAt}
          limit 1
          for update skip locked)`,
      ),
    )
    .returning();
  return job ?? null;
}

/** Records how a claimed job ended. */
export async function finishExport(db: Database, id: string, result: { key: string } | { error: string }, now = new Date()) {
  await db
    .update(exportJob)
    .set(
      "key" in result
        ? { status: "done", key: result.key, error: null, finishedAt: now }
        : { status: "failed", error: result.error.slice(0, MAX_ERROR_LENGTH), finishedAt: now },
    )
    .where(eq(exportJob.id, id));
}

export interface ExportDeps {
  db: Database;
  store: BlobStore;
  /** Runs `guiderecorder` with these arguments; rejects on failure. */
  cli: (args: string[]) => Promise<void>;
  /** Zips the contents of `dir` into `file`. */
  zip: (dir: string, file: string) => Promise<void>;
}

/**
 * Makes a claimed job's file in `workDir` (an empty scratch folder) and
 * stores it; returns its key. The bundle is the revision's guide.json with
 * each image at its path; images the store lacks are left out, and the
 * CLI draws placeholders for them.
 */
export async function runExport(deps: ExportDeps, job: ExportJob, workDir: string): Promise<string> {
  const format = exportFormat.parse(job.format);
  const [found] = await deps.db.select().from(revision).where(eq(revision.id, job.revisionId));
  if (!found) throw new Error("The revision no longer exists.");
  const assets = await deps.db.select().from(revisionAsset).where(eq(revisionAsset.revisionId, job.revisionId));

  const bundle = path.join(workDir, "guide.guiderecorder");
  await mkdir(bundle, { recursive: true });
  await writeFile(path.join(bundle, "guide.json"), JSON.stringify(found.guideJson, null, 2));
  for (const a of assets) {
    const target = path.resolve(bundle, a.path);
    // Paths come from guide.json: never write outside the bundle.
    if (!target.startsWith(bundle + path.sep)) throw new Error(`Image path outside the bundle: ${a.path}`);
    const bytes = await deps.store.get(assetKey(job.organizationId, a.sha256));
    if (!bytes) continue;
    await mkdir(path.dirname(target), { recursive: true });
    await writeFile(target, bytes);
  }

  // Markdown and HTML write a folder; the other formats one file.
  const folder = format === "md" || format === "html";
  const out = path.join(workDir, folder ? "out" : `out.${EXPORT_FORMATS[format].extension}`);
  await deps.cli(["export", bundle, "--format", format, "--out", out]);
  let file = out;
  if (format === "md") {
    file = path.join(workDir, "out.zip");
    await deps.zip(out, file);
  } else if (format === "html") {
    file = path.join(out, "guide.html");
  }

  const key = exportKey(job.organizationId, job.revisionId, format);
  await deps.store.put(key, new Uint8Array(await readFile(file)), EXPORT_FORMATS[format].contentType);
  return key;
}

const run = promisify(execFile);

/** `guiderecorder` at `binary`, with a time limit. */
export function cliRunner(binary: string, timeoutMs = 5 * 60 * 1000): ExportDeps["cli"] {
  return async (args) => {
    try {
      await run(binary, args, { timeout: timeoutMs, maxBuffer: 16 * 1024 * 1024 });
    } catch (error) {
      const stderr = (error as { stderr?: string }).stderr?.trim();
      throw new Error(stderr || (error as Error).message);
    }
  };
}

/** The `zip` command. */
export const zipFolder: ExportDeps["zip"] = async (dir, file) => {
  await run("zip", ["-qr", file, "."], { cwd: dir });
};
