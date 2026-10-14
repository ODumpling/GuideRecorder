// The app's Tauri commands, served in memory from copies of the fixture
// guides (fixtures/guides). Nothing is ever written to disk.

import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative, resolve } from "node:path";
import type { Guide, GuideListItem, OpenedGuide, StepImage } from "../src/lib/types";

const FIXTURES = resolve(import.meta.dirname, "../../fixtures/guides");
const SCHEMA_VERSION = 1;

export interface AppError {
  kind: string;
  message: string;
}

class CommandError extends Error {
  constructor(readonly error: AppError) {
    super(error.message);
  }
}

const fail = (kind: string, message: string): never => {
  throw new CommandError({ kind, message });
};

interface Bundle {
  path: string;
  guide: Guide;
  /** Bundle-relative path → bytes. */
  files: Map<string, Buffer>;
}

/** Every file under `dir`, by path relative to `root`. */
function readFiles(root: string, dir = root, files = new Map<string, Buffer>()): Map<string, Buffer> {
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) readFiles(root, path, files);
    else files.set(relative(root, path).split("\\").join("/"), readFileSync(path));
  }
  return files;
}

export interface BackendOptions {
  /**
   * Schema versions to serve guides with, by guide id. One newer than the
   * app's opens read-only, as a guide from a newer GuidesYa would.
   */
  schemaVersions?: Record<string, number>;
}

export class Backend {
  private bundles = new Map<string, Bundle>();
  /** Every guide `save_guide` received, in order. */
  readonly saves: Guide[] = [];
  /** Every command called, in order. */
  readonly calls: { cmd: string; args: unknown }[] = [];

  constructor(options: BackendOptions = {}) {
    for (const name of readdirSync(FIXTURES).filter((n) => n.endsWith(".guidesya")).sort()) {
      const path = join(FIXTURES, name);
      const files = readFiles(path);
      const guide = JSON.parse(files.get("guide.json")!.toString("utf8")) as Guide;
      files.delete("guide.json");
      guide.schemaVersion = options.schemaVersions?.[guide.id] ?? guide.schemaVersion;
      this.bundles.set(guide.id, { path, guide, files });
    }
  }

  /** The guide as last saved (or as loaded). */
  guide(id: string): Guide {
    return structuredClone(this.bundle(id).guide);
  }

  /** The last saved copy of a guide; fails if it was never saved. */
  lastSave(id: string): Guide {
    const saved = this.saves.findLast((g) => g.id === id);
    if (!saved) throw new Error(`guide ${id} was never saved`);
    return saved;
  }

  hasFile(guideId: string, path: string): boolean {
    return this.bundle(guideId).files.has(path);
  }

  private bundle(id: string): Bundle {
    return this.bundles.get(id) ?? fail("notFound", `no guide ${id}`);
  }

  private readOnly(bundle: Bundle): boolean {
    return bundle.guide.schemaVersion > SCHEMA_VERSION;
  }

  /** Runs a command; the reply shape is what mock-entry.ts expects. */
  async invoke(cmd: string, args: unknown): Promise<{ ok: unknown } | { error: AppError }> {
    this.calls.push({ cmd, args });
    try {
      return { ok: this.run(cmd, (args ?? {}) as Record<string, unknown>) };
    } catch (error) {
      if (error instanceof CommandError) return { error: error.error };
      return { error: { kind: "io", message: String(error) } };
    }
  }

  private run(cmd: string, args: Record<string, unknown>): unknown {
    switch (cmd) {
      case "list_guides": {
        const query = typeof args.query === "string" ? args.query.toLowerCase() : "";
        return [...this.bundles.values()]
          .filter(({ guide }) => !query || guide.title.toLowerCase().includes(query))
          .map(
            (bundle): GuideListItem => ({
              id: bundle.guide.id,
              path: bundle.path,
              title: bundle.guide.title,
              summary: bundle.guide.summary,
              tags: bundle.guide.tags,
              stepCount: bundle.guide.items.filter((item) => item.type === "step").length,
              updatedAt: bundle.guide.updatedAt,
              readOnly: this.readOnly(bundle),
              duplicateOf: null,
            }),
          );
      }
      case "open_guide": {
        const bundle = this.bundle(String(args.id));
        return {
          guide: structuredClone(bundle.guide),
          path: bundle.path,
          readOnly: this.readOnly(bundle),
          migratedFrom: null,
          migrationError: null,
        } satisfies OpenedGuide;
      }
      case "save_guide": {
        const guide = args.guide as Guide;
        const bundle = this.bundle(guide.id);
        if (this.readOnly(bundle)) fail("readOnly", "this guide is read-only");
        bundle.guide = structuredClone(guide);
        this.saves.push(structuredClone(guide));
        return null;
      }
      case "close_guide":
        this.bundle(String(args.id));
        return [];
      case "step_image": {
        const path = String(args.path);
        const bytes = this.bundle(String(args.guideId)).files.get(path) ?? fail("notFound", `no image ${path}`);
        // A JSON number array, as over Tauri's postMessage fallback.
        return [...bytes];
      }
      case "copy_step_images": {
        const bundle = this.bundle(String(args.guideId));
        const image = args.image as StepImage;
        const newId = String(args.newStepId);
        const copy = (source: string, suffix: string): string => {
          const bytes = bundle.files.get(source) ?? fail("notFound", `no image ${source}`);
          const target = `steps/${newId}${suffix}.webp`;
          if (bundle.files.has(target)) fail("conflict", `${target} already exists`);
          bundle.files.set(target, bytes);
          return target;
        };
        return {
          ...image,
          full: copy(image.full, ""),
          crop: image.crop && copy(image.crop, ".crop"),
          after: image.after && copy(image.after, ".after"),
        } satisfies StepImage;
      }
      case "quit_ack":
      case "quit_app":
        return null;
      default:
        return fail("invalid", `the e2e backend has no command ${cmd}`);
    }
  }
}
