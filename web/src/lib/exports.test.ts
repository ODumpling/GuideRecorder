import { createHash } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { eq } from "drizzle-orm";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { Guide } from "@guiderecorder/guide-schema";
import { imagePaths, pushRequest } from "@guiderecorder/sync";
import { testDb } from "../../test/db";
import type { Database } from "@/db";
import { asset, exportJob, guide, organization, user } from "@/db/schema";
import { assetKey } from "./assets";
import {
  claimExport,
  cliRunner,
  exportFilename,
  exportKey,
  finishExport,
  MAX_ATTEMPTS,
  requestExport,
  runExport,
  STALE_AFTER_MS,
  zipFolder,
  type ExportDeps,
} from "./exports";
import type { BlobStore } from "./storage";
import { push, type Actor } from "./sync";

const bundleDir = new URL("../../../fixtures/guides/Rotate an API key.guiderecorder/", import.meta.url);
const fixture: Guide = JSON.parse(readFileSync(new URL("guide.json", bundleDir), "utf8"));
const hash = (text: string) => createHash("sha256").update(text).digest("hex");
const assetsFor = (g: Guide) => Object.fromEntries(imagePaths(g).map((p) => [p, hash(p)]));

let db: Database;
let work: string;
const ada: Actor = { userId: "ada", role: "owner" };

/** In-memory objects; the fixture's images under their keys where the bundle has them. */
function memoryStore() {
  const objects = new Map<string, { body: Uint8Array; contentType: string }>();
  for (const p of imagePaths(fixture)) {
    const file = new URL(p, bundleDir);
    if (existsSync(file)) objects.set(assetKey("org", hash(p)), { body: readFileSync(file), contentType: "image/webp" });
  }
  const store: BlobStore = {
    async get(key) {
      return objects.get(key)?.body ?? null;
    },
    async put(key, body, contentType) {
      objects.set(key, { body, contentType });
    },
  };
  return { store, objects };
}

async function pushed(g: Guide) {
  const result = await push(db, "org", ada, pushRequest.parse({ guideId: g.id, baseRevision: null, guide: g, assets: assetsFor(g) }), "app");
  if (result.status !== 200) throw new Error(`push failed: ${JSON.stringify(result)}`);
  return result.revision;
}

beforeEach(async () => {
  db = await testDb();
  work = await mkdtemp(path.join(tmpdir(), "exports-test-"));
  await db.insert(organization).values({ id: "org", name: "Acme", slug: "acme", createdAt: new Date() });
  await db.insert(user).values(["ada", "vic"].map((id) => ({ id, name: id, email: `${id}@example.com` })));
  await db
    .insert(asset)
    .values(imagePaths(fixture).map((p) => ({ organizationId: "org", sha256: hash(p), size: 10, contentType: "image/webp" })));
});

afterEach(async () => {
  await rm(work, { recursive: true, force: true });
});

describe("requestExport", () => {
  it("queues one job per revision and format, and serves it once done", async () => {
    const revision = await pushed(fixture);
    const request = { guideId: fixture.id, revisionId: revision.id, format: "pdf" as const };
    expect(await requestExport(db, "org", "vic", request)).toEqual({ status: "pending" });
    expect(await requestExport(db, "org", "ada", request)).toEqual({ status: "pending" });
    expect(await db.select().from(exportJob)).toHaveLength(1);

    const job = (await claimExport(db))!;
    expect(job).toMatchObject({ revisionId: revision.id, format: "pdf", status: "running", attempts: 1 });
    expect(await claimExport(db)).toBeNull();
    expect(await requestExport(db, "org", "vic", request)).toEqual({ status: "running" });

    await finishExport(db, job.id, { key: exportKey("org", revision.id, "pdf") });
    expect(await requestExport(db, "org", "vic", { ...request, retry: true })).toEqual({
      status: "done",
      key: `orgs/org/exports/${revision.id}/pdf.pdf`,
    });
  });

  it("refuses guides the user can't see and revisions of other guides", async () => {
    const revision = await pushed(fixture);
    await db.update(guide).set({ visibility: "private" }).where(eq(guide.id, fixture.id));
    const request = { guideId: fixture.id, revisionId: revision.id, format: "md" as const };
    expect(await requestExport(db, "org", "vic", request)).toMatchObject({ status: 404 });
    expect(await requestExport(db, "org", "ada", { ...request, guideId: "01M4WZCE2000000000000009ZZ" })).toMatchObject({ status: 404 });
    expect(await requestExport(db, "other", "ada", request)).toMatchObject({ status: 404 });
    expect(await requestExport(db, "org", "ada", request)).toEqual({ status: "pending" });
  });

  it("reports failures, and queues them again on retry", async () => {
    const revision = await pushed(fixture);
    const request = { guideId: fixture.id, revisionId: revision.id, format: "docx" as const };
    await requestExport(db, "org", "ada", request);
    await finishExport(db, (await claimExport(db))!.id, { error: "boom" });
    expect(await requestExport(db, "org", "ada", request)).toEqual({ status: "failed", error: "boom" });
    expect(await requestExport(db, "org", "ada", { ...request, retry: true })).toEqual({ status: "pending" });
    expect(await claimExport(db)).toMatchObject({ attempts: 1 });
  });

  it("takes back jobs whose worker died, a few times", async () => {
    const revision = await pushed(fixture);
    const request = { guideId: fixture.id, revisionId: revision.id, format: "html" as const };
    await requestExport(db, "org", "ada", request);
    let now = new Date("2026-10-19T12:00:00Z");
    const later = () => (now = new Date(now.getTime() + STALE_AFTER_MS + 1000));
    expect(await claimExport(db, now)).not.toBeNull();
    expect(await claimExport(db, now)).toBeNull();
    for (let attempt = 2; attempt <= MAX_ATTEMPTS; attempt++) expect(await claimExport(db, later())).toMatchObject({ attempts: attempt });
    expect(await claimExport(db, later())).toBeNull();
    expect(await requestExport(db, "org", "ada", request, now)).toEqual({ status: "failed", error: "The export took too long." });
    expect(await requestExport(db, "org", "ada", { ...request, retry: true }, now)).toEqual({ status: "pending" });
  });
});

describe("runExport", () => {
  /** A stand-in CLI that records its arguments and writes what `export` would. */
  function fakeCli(calls: string[][]): ExportDeps["cli"] {
    return async (args) => {
      calls.push(args);
      const out = args[args.indexOf("--out") + 1]!;
      const format = args[args.indexOf("--format") + 1];
      if (format === "md" || format === "html") {
        await mkdir(out, { recursive: true });
        await writeFile(path.join(out, format === "md" ? "guide.md" : "guide.html"), `${format} export`);
      } else {
        await writeFile(out, `${format} export`);
      }
    };
  }

  it("rebuilds the bundle from the revision, runs the CLI and stores the file", async () => {
    const revision = await pushed(fixture);
    await requestExport(db, "org", "ada", { guideId: fixture.id, revisionId: revision.id, format: "pdf" });
    const job = (await claimExport(db))!;
    const { store, objects } = memoryStore();
    const calls: string[][] = [];
    let bundleSeen: string[] = [];
    const cli = fakeCli(calls);
    const deps: ExportDeps = {
      db,
      store,
      cli: async (args) => {
        const bundle = args[1]!;
        bundleSeen = [JSON.parse(await readFile(path.join(bundle, "guide.json"), "utf8")).id, ...imagePaths(fixture).filter((p) => existsSync(path.join(bundle, p)))];
        await cli(args);
      },
      zip: zipFolder,
    };

    const key = await runExport(deps, job, work);
    expect(key).toBe(exportKey("org", revision.id, "pdf"));
    expect(calls[0]).toEqual(["export", path.join(work, "guide.guiderecorder"), "--format", "pdf", "--out", path.join(work, "out.pdf")]);
    // Every image the store has is in the bundle at its path.
    const stored = imagePaths(fixture).filter((p) => objects.has(assetKey("org", hash(p))));
    expect(bundleSeen).toEqual([fixture.id, ...stored]);
    expect(objects.get(key)).toMatchObject({ contentType: "application/pdf" });
    expect(new TextDecoder().decode(objects.get(key)!.body)).toBe("pdf export");
  });

  it("zips Markdown and stores HTML as its one page", async () => {
    const revision = await pushed(fixture);
    const { store, objects } = memoryStore();
    const zips: string[] = [];
    const deps: ExportDeps = {
      db,
      store,
      cli: fakeCli([]),
      zip: async (dir, file) => {
        zips.push(dir);
        await writeFile(file, "zip");
      },
    };
    for (const format of ["md", "html"] as const) {
      await requestExport(db, "org", "ada", { guideId: fixture.id, revisionId: revision.id, format });
      const dir = await mkdtemp(path.join(work, format));
      await runExport(deps, (await claimExport(db))!, dir);
    }
    expect(zips).toHaveLength(1);
    expect(new TextDecoder().decode(objects.get(exportKey("org", revision.id, "md"))!.body)).toBe("zip");
    expect(objects.get(exportKey("org", revision.id, "html"))).toMatchObject({ contentType: "text/html; charset=utf-8" });
  });

  it("never writes outside the bundle", async () => {
    const sneaky: Guide = { ...structuredClone(fixture), items: [{ type: "step", id: "01M4WZCE2000000000000002R2", title: "x", kind: "manual", image: { full: "../../escape.webp", show: "full" } }] as unknown as Guide["items"] };
    await db.insert(asset).values({ organizationId: "org", sha256: hash("../../escape.webp"), size: 1, contentType: "image/webp" });
    const revision = await pushed(sneaky);
    await requestExport(db, "org", "ada", { guideId: fixture.id, revisionId: revision.id, format: "pdf" });
    const { store } = memoryStore();
    await expect(runExport({ db, store, cli: fakeCli([]), zip: zipFolder }, (await claimExport(db))!, work)).rejects.toThrow(/outside the bundle/);
  });

  // Set GUIDERECORDER_CLI to a built `guiderecorder` to run the real thing.
  it.skipIf(!process.env.GUIDERECORDER_CLI)("exports with the real CLI", async () => {
    const revision = await pushed(fixture);
    const { store, objects } = memoryStore();
    const deps: ExportDeps = { db, store, cli: cliRunner(process.env.GUIDERECORDER_CLI!), zip: zipFolder };
    for (const format of ["pdf", "docx", "md", "html"] as const) {
      await requestExport(db, "org", "ada", { guideId: fixture.id, revisionId: revision.id, format });
      const dir = await mkdtemp(path.join(work, format));
      const key = await runExport(deps, (await claimExport(db))!, dir);
      expect(objects.get(key)!.body.byteLength).toBeGreaterThan(100);
    }
    expect(new TextDecoder().decode(objects.get(exportKey("org", revision.id, "pdf"))!.body.slice(0, 5))).toBe("%PDF-");
  }, 120_000);
});

it("names downloads after the guide", () => {
  expect(exportFilename('Rotate: an "API" key / v2', "md")).toBe("Rotate an API key v2.zip");
  expect(exportFilename("  ", "pdf")).toBe("Guide.pdf");
});
