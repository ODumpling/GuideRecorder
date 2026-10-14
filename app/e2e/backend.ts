// The app's Tauri commands, served in memory from copies of the fixture
// guides (fixtures/guides). Nothing is ever written to disk.

import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative, resolve } from "node:path";
import type {
  ExportBranding,
  ExportResult,
  Guide,
  GuideListItem,
  OpenedGuide,
  Settings,
  StepImage,
} from "../src/lib/types";

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

  /**
   * What the next native dialog returns (null: cancelled). The app's Rust
   * side runs these dialogs; tests set the answer before clicking.
   */
  dialog: { folder: string | null; file: string | null; logo: string | null } = {
    folder: null,
    file: null,
    logo: null,
  };
  /** Every `export_guide` call that succeeded. */
  readonly exports: { guideId: string; format: string; out: string; overwrite: boolean; branding: ExportBranding }[] =
    [];
  /** Export targets that exist (written by an earlier export). */
  readonly exported = new Set<string>();
  /** Paths `reveal_export` was asked to show. */
  readonly revealed: string[] = [];
  /** `verify_crop`'s answer for a crop that has a `cropRect`. */
  cropVerifies = true;

  /** The stored settings (the app's defaults to start with). */
  settings: Settings = {
    version: 1,
    general: { library: null, launchAtLogin: false, menuBarIcon: false },
    recording: {
      hotkeys: { record: "CmdOrCtrl+Shift+R", pause: "CmdOrCtrl+Shift+P", manualStep: "CmdOrCtrl+Shift+M" },
      captureArea: "display",
      afterFrames: true,
      afterFrameDelayMs: 400,
      excludedApps: [],
      typingIdleMs: 1500,
    },
    ai: {
      enabled: true,
      autoRun: true,
      stepModel: "claude-haiku-5-5",
      guideModel: "claude-sonnet-5-5",
      language: "",
      audience: "beginner",
    },
    export: { format: "pdf", folder: null, logo: null, accent: null },
  };
  /** Whether an API key is "stored"; the key itself is never kept. */
  apiKeyStored = false;

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
      case "verify_crop": {
        const bundle = this.bundle(String(args.guideId));
        const image = args.image as StepImage;
        return !!image.crop && !!image.cropRect && bundle.files.has(image.crop) && this.cropVerifies;
      }
      case "choose_export_folder":
        return this.dialog.folder;
      case "choose_logo":
        return this.dialog.logo;
      case "choose_export_file": {
        const ext = String(args.format);
        if (ext !== "docx" && ext !== "pdf") fail("invalid", "Markdown and HTML export into a folder");
        const file = this.dialog.file;
        return file && !file.toLowerCase().endsWith(`.${ext}`) ? `${file}.${ext}` : file;
      }
      case "export_guide": {
        const guideId = String(args.guideId);
        this.bundle(guideId);
        const format = String(args.format);
        const out = String(args.out);
        const overwrite = args.overwrite === true;
        if (this.exported.has(out) && !overwrite) fail("exists", `${out} already exists`);
        this.exported.add(out);
        this.exports.push({ guideId, format, out, overwrite, branding: args.branding as ExportBranding });
        const main = format === "md" ? `${out}/guide.md` : format === "html" ? `${out}/guide.html` : out;
        return { files: [main], warnings: [], notes: [] } satisfies ExportResult;
      }
      case "reveal_export":
        this.revealed.push(String(args.path));
        return null;
      case "get_settings":
        return { settings: structuredClone(this.settings), readOnly: false, loadError: null };
      case "update_settings": {
        const next = args.settings as Settings;
        if (next.recording.typingIdleMs < 300 || next.recording.typingIdleMs > 10_000) {
          fail("invalid", "the typing pause must be 300–10000 ms");
        }
        this.settings = structuredClone(next);
        return structuredClone(next);
      }
      case "library_info":
        return { path: "/Users/me/Documents/GuidesYa", error: null, starting: false, fromEnv: false, defaultPath: "/Users/me/Documents/GuidesYa" };
      case "choose_library_folder":
        return null;
      case "launch_at_login":
        return this.settings.general.launchAtLogin;
      case "set_launch_at_login":
        this.settings.general.launchAtLogin = args.enabled === true;
        return this.settings.general.launchAtLogin;
      case "api_key_status":
        return this.apiKeyStored;
      case "set_api_key":
        if (!String(args.key).trim()) fail("invalid", "no API key");
        this.apiKeyStored = true;
        return null;
      case "delete_api_key":
        this.apiKeyStored = false;
        return null;
      case "permission_status":
        return [
          { permission: "screen_recording", status: "granted" },
          { permission: "accessibility", status: "denied" },
          { permission: "input_monitoring", status: "not_determined" },
        ];
      case "open_permission_settings":
        return null;
      case "mcp_setup":
        return {
          command: "claude mcp add guidesya -- /Applications/GuidesYa.app/Contents/MacOS/guidesya-mcp --library /Users/me/Documents/GuidesYa",
          mcpPath: "/Applications/GuidesYa.app/Contents/MacOS/guidesya-mcp",
          found: true,
          library: "/Users/me/Documents/GuidesYa",
        };
      case "quit_ack":
      case "quit_app":
        return null;
      default:
        return fail("invalid", `the e2e backend has no command ${cmd}`);
    }
  }
}
