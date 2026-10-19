// The export worker: makes the PDF, DOCX, Markdown and HTML downloads the
// web app queues (see src/lib/exports.ts). Runs beside the Rust CLI in its
// own container (worker/Dockerfile): `pnpm worker`. Any number can run; each
// claims one job at a time.

import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { drizzle } from "drizzle-orm/postgres-js";
import postgres from "postgres";
import { z } from "zod";
import type { Database } from "@/db";
import * as schema from "@/db/schema";
import { claimExport, cliRunner, finishExport, runExport, zipFolder, type ExportDeps } from "@/lib/exports";
import { s3Store } from "@/lib/storage";

const POLL_MS = 2000;

const env = z
  .object({
    DATABASE_URL: z.url(),
    S3_ENDPOINT: z.url(),
    S3_REGION: z.string().default("auto"),
    S3_BUCKET: z.string().min(1),
    S3_ACCESS_KEY_ID: z.string().min(1),
    S3_SECRET_ACCESS_KEY: z.string().min(1),
    /** The `guiderecorder` binary. */
    GUIDERECORDER_CLI: z.string().default("guiderecorder"),
  })
  .parse(process.env);

const deps: ExportDeps = {
  db: drizzle(postgres(env.DATABASE_URL, { max: 2 }), { schema }) as unknown as Database,
  store: s3Store({
    endpoint: env.S3_ENDPOINT,
    region: env.S3_REGION,
    bucket: env.S3_BUCKET,
    accessKeyId: env.S3_ACCESS_KEY_ID,
    secretAccessKey: env.S3_SECRET_ACCESS_KEY,
  }),
  cli: cliRunner(env.GUIDERECORDER_CLI),
  zip: zipFolder,
};

let stopping = false;
for (const signal of ["SIGINT", "SIGTERM"] as const) process.on(signal, () => (stopping = true));

/** Runs one queued job, if there is one; returns whether there was. */
async function next(): Promise<boolean> {
  const job = await claimExport(deps.db);
  if (!job) return false;
  const started = Date.now();
  const workDir = await mkdtemp(path.join(tmpdir(), "guiderecorder-export-"));
  try {
    const key = await runExport(deps, job, workDir);
    await finishExport(deps.db, job.id, { key });
    console.log(`export ${job.id}: ${job.format} of revision ${job.revisionId} in ${Date.now() - started} ms`);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    await finishExport(deps.db, job.id, { error: message });
    console.error(`export ${job.id} failed: ${message}`);
  } finally {
    await rm(workDir, { recursive: true, force: true });
  }
  return true;
}

console.log("Export worker ready.");
while (!stopping) {
  try {
    if (!(await next())) await new Promise((resolve) => setTimeout(resolve, POLL_MS));
  } catch (error) {
    // Database or storage down: wait and try again.
    console.error(error);
    await new Promise((resolve) => setTimeout(resolve, POLL_MS * 5));
  }
}
process.exit(0);
