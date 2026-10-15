// The app's Tauri commands, served in memory from copies of the fixture
// guides (fixtures/guides). Nothing is ever written to disk.

import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative, resolve } from "node:path";
import type {
  CaptureArea,
  CaptureTargetArg,
  CaptureWindow,
  ExportBranding,
  ExportResult,
  Guide,
  GuideListItem,
  OpenedGuide,
  Rect,
  RecordingSaved,
  RecordingSnapshot,
  RecordingStatus,
  Settings,
  StampedStatus,
  StepImage,
} from "../src/lib/types";
import { DEFAULT_EXCLUDED_IDS } from "../src/recording/apps";

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
  /** First-run setup not finished yet (default: finished, so setup stays out of the way). */
  firstRun?: boolean;
  /** The library failed to open at startup with this error. */
  libraryError?: string;
  /** The stored capture area (default: display). */
  captureArea?: CaptureArea;
}

/** An event the app receives, as the Rust side would emit it. */
export interface EmittedEvent {
  event: string;
  payload: unknown;
}

const PERMISSION_NAMES: Record<string, string> = {
  screen_recording: "Screen Recording",
  accessibility: "Accessibility",
  input_monitoring: "Input Monitoring",
};

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
    general: { library: null, launchAtLogin: false, menuBarIcon: false, onboarded: true },
    recording: {
      hotkeys: { record: "CmdOrCtrl+Shift+R", pause: "CmdOrCtrl+Shift+P", manualStep: "CmdOrCtrl+Shift+M" },
      captureArea: "display",
      afterFrames: true,
      afterFrameDelayMs: 400,
      excludedApps: [...DEFAULT_EXCLUDED_IDS],
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
  /** Permission states; `request_permission` grants. */
  permissions: Record<string, string> = {
    screen_recording: "granted",
    accessibility: "denied",
    input_monitoring: "not_determined",
  };
  /** The library as `library_info` reports it. */
  library: { path: string; error: string | null } = { path: "/Users/me/Documents/GuidesYa", error: null };
  /** What `choose_library_folder` returns next (null: cancelled). */
  nextLibrary: string | null = null;

  /** The recorder's state, as `recording_status` reports it. */
  recording: RecordingStatus = { state: "idle" };
  /** The `seq` `recording` was stored with. */
  recordingSeq = 0;
  /** The capture area falls back to all displays (settings say window or region). */
  captureAreaFallback = false;
  /** Steps the next saved recording leaves out for lack of a screenshot. */
  recordingSkipped = 0;
  /** Guides made by `stop_recording`, in order. */
  readonly recordings: RecordingSaved[] = [];
  /** Screenshots `recording_snapshot` serves (any fixture image), by id. */
  readonly snapshots = new Map<string, RecordingSnapshot>();
  /** How many manual steps `add_manual_step` added. */
  manualSteps = 0;
  /** What the next `add_manual_step` fails with, if anything. */
  manualStepError: AppError | null = null;
  /** What `list_capture_windows` returns, frontmost first. */
  captureWindows: CaptureWindow[] = [
    { windowId: 101, appName: "Safari", bundleId: "com.apple.Safari", title: "Apple Support", width: 1280, height: 800 },
    { windowId: 102, appName: "Notes", bundleId: "com.apple.Notes", title: "Shopping list", width: 720, height: 540 },
    { windowId: 103, appName: "Finder", bundleId: "com.apple.finder", title: null, width: 900, height: 600 },
  ];
  /** What the next `pick_region` returns (null: the user cancelled). */
  regionPick: { displayId: number | null; rect: Rect } | null = null;
  /** The region overlay's answers (`finish_region_pick`), in order. */
  readonly regionFinishes: ({ rect: Rect } | null)[] = [];
  /** The `pick` id each `finish_region_pick` carried (undefined: none), in order. */
  readonly regionFinishPicks: (number | undefined)[] = [];
  /** The `target` of each `start_recording` (undefined: none given), in order. */
  readonly recordingTargets: (CaptureTargetArg | undefined)[] = [];
  /** Events the next command reply carries; mock-entry.ts emits them before resolving it. */
  private events: EmittedEvent[] = [];

  constructor(options: BackendOptions = {}) {
    for (const name of readdirSync(FIXTURES).filter((n) => n.endsWith(".guidesya")).sort()) {
      const path = join(FIXTURES, name);
      const files = readFiles(path);
      const guide = JSON.parse(files.get("guide.json")!.toString("utf8")) as Guide;
      files.delete("guide.json");
      guide.schemaVersion = options.schemaVersions?.[guide.id] ?? guide.schemaVersion;
      this.bundles.set(guide.id, { path, guide, files });
    }
    if (options.firstRun) this.settings.general.onboarded = false;
    if (options.captureArea) this.settings.recording.captureArea = options.captureArea;
    if (options.libraryError) {
      this.settings.general.library = "/Volumes/Team/Guides";
      this.library = { path: "/Volumes/Team/Guides", error: options.libraryError };
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
  async invoke(
    cmd: string,
    args: unknown,
  ): Promise<({ ok: unknown } | { error: AppError }) & { events: EmittedEvent[] }> {
    this.calls.push({ cmd, args });
    try {
      return { ok: this.run(cmd, (args ?? {}) as Record<string, unknown>), events: this.takeEvents() };
    } catch (error) {
      const events = this.takeEvents();
      if (error instanceof CommandError) return { error: error.error, events };
      return { error: { kind: "io", message: String(error) }, events };
    }
  }

  private takeEvents(): EmittedEvent[] {
    return this.events.splice(0);
  }

  /** Stores `status` under the next `seq`, as the Rust `Recorder` does, and queues its event. */
  private setRecording(status: RecordingStatus) {
    this.recording = status;
    this.recordingSeq += 1;
    if (status.state === "idle") this.snapshots.clear();
    this.events.push({ event: "recording://status", payload: this.stamped() });
  }

  /** The recorder's status with its `seq`, as commands return and events carry it. */
  stamped(): StampedStatus {
    return { ...structuredClone(this.recording), seq: this.recordingSeq };
  }

  /**
   * An excluded app comes to the front (`null`: a non-excluded one does),
   * as the Rust side's exclusion sink reports it. Returns the new status;
   * the test emits it (the sink sends it at once, not with a reply).
   */
  frontmost(excludedApp: string | null): StampedStatus {
    this.recording = { ...this.active(), excludedApp };
    this.recordingSeq += 1;
    return this.stamped();
  }

  /** Replaces the running recording's status as is (e.g. to age it), without an event. */
  setActive(status: Extract<RecordingStatus, { state: "recording" | "paused" }>) {
    this.active();
    this.recording = structuredClone(status);
    this.recordingSeq += 1;
  }

  /**
   * A screenshot the running recording takes, as the Rust side announces
   * it: the event is queued (sent with the next command's reply) and
   * `recording_snapshot` serves the first fixture step image for it.
   */
  takeSnapshot(id: string, point: RecordingSnapshot["point"] = null): RecordingSnapshot {
    this.active();
    const snapshot: RecordingSnapshot = { id, ts: this.snapshots.size * 1_000_000, width: 1600, height: 1000, point };
    this.snapshots.set(id, snapshot);
    this.events.push({ event: "recording://snapshot", payload: structuredClone(snapshot) });
    return snapshot;
  }

  private active(): Extract<RecordingStatus, { state: "recording" | "paused" }> {
    const status = this.recording;
    if (status.state !== "recording" && status.state !== "paused") return fail("invalid", "nothing is recording");
    return status;
  }

  /** Saves the recording as a new guide: a copy of the first fixture guide. */
  private saveRecording(title: string | null): RecordingSaved {
    const n = this.recordings.length + 1;
    const id = `01M4X${"0".repeat(17)}REC${n}`;
    const source = [...this.bundles.values()][0];
    const guide = structuredClone(source.guide);
    guide.id = id;
    guide.title = title ?? `Recording 2026-10-15 10:0${n}`;
    guide.schemaVersion = SCHEMA_VERSION;
    const path = `/Users/me/Documents/GuidesYa/${guide.title}.guidesya`;
    this.bundles.set(id, { path, guide, files: new Map(source.files) });
    const saved: RecordingSaved = {
      guideId: id,
      steps: guide.items.filter((item) => item.type === "step").length,
      skipped: this.recordingSkipped,
    };
    this.recordings.push(saved);
    return saved;
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
        return { ...this.library, starting: false, fromEnv: false, defaultPath: "/Users/me/Documents/GuidesYa" };
      case "choose_library_folder":
        return this.nextLibrary;
      case "switch_library": {
        const path = (args.path as string | null) ?? "/Users/me/Documents/GuidesYa";
        this.library = { path, error: null };
        this.settings.general.library = (args.path as string | null) ?? null;
        return path;
      }
      case "request_permission":
        this.permissions[String(args.permission)] = "granted";
        return "granted";
      case "recording_readiness": {
        const missing = Object.keys(this.permissions).filter((p) => this.permissions[p] !== "granted");
        return { ready: missing.length === 0, missing };
      }
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
        return Object.entries(this.permissions).map(([permission, status]) => ({ permission, status }));
      case "open_permission_settings":
        return null;
      case "mcp_setup":
        return {
          command: "claude mcp add guidesya -- /Applications/GuidesYa.app/Contents/MacOS/guidesya-mcp --library /Users/me/Documents/GuidesYa",
          mcpPath: "/Applications/GuidesYa.app/Contents/MacOS/guidesya-mcp",
          found: true,
          library: "/Users/me/Documents/GuidesYa",
        };
      case "start_recording": {
        if (this.recording.state !== "idle") fail("invalid", "a recording is already running");
        const missing = Object.keys(this.permissions).filter((p) => this.permissions[p] !== "granted");
        if (missing.length) {
          fail("invalid", `recording needs ${missing.map((p) => PERMISSION_NAMES[p]).join(", ")} permission`);
        }
        const target = args.target as CaptureTargetArg | undefined;
        if (target?.kind === "window" && !this.captureWindows.some((w) => w.windowId === target.windowId)) {
          fail("invalid", "That window is no longer open.");
        }
        if (target?.kind === "region" && (target.rect.width < 40 || target.rect.height < 40)) {
          fail("invalid", "the region must be at least 40 × 40 points");
        }
        this.recordingTargets.push(target === undefined ? undefined : structuredClone(target));
        const title = typeof args.title === "string" && args.title.trim() ? args.title.trim() : null;
        this.setRecording({
          state: "recording",
          startedAt: new Date().toISOString(),
          title,
          // Like Rust: a given target is never a fallback.
          captureAreaFallback: target ? false : this.captureAreaFallback,
          pausedMs: 0,
          excludedApp: null,
        });
        return this.stamped();
      }
      case "pause_recording":
      case "resume_recording": {
        // Like Rust's `set_paused`: no event when nothing changes; a resume
        // adds the pause to `pausedMs`.
        const status = this.active();
        if (cmd === "pause_recording" && status.state === "recording") {
          this.setRecording({ ...status, state: "paused", pausedAt: new Date().toISOString() });
        } else if (cmd === "resume_recording" && status.state === "paused") {
          const { pausedAt, ...run } = status;
          const pausedMs = run.pausedMs + Math.max(0, Date.now() - Date.parse(pausedAt));
          this.setRecording({ ...run, state: "recording", pausedMs });
        }
        return this.stamped();
      }
      case "stop_recording": {
        const { startedAt, title } = this.active();
        this.setRecording({ state: "saving", startedAt, title });
        const saved = this.saveRecording(title);
        this.setRecording({ state: "idle" });
        this.events.push({ event: "recording://saved", payload: saved });
        return saved;
      }
      case "cancel_recording":
        this.active();
        this.setRecording({ state: "idle" });
        return null;
      case "recording_status":
        return this.stamped();
      case "add_manual_step": {
        // Like the Rust side: only while running (paused counts as not).
        if (this.recording.state !== "recording") fail("invalid", "nothing is recording");
        if (this.manualStepError) fail(this.manualStepError.kind, this.manualStepError.message);
        this.manualSteps += 1;
        return null;
      }
      case "recording_snapshot": {
        const id = String(args.id);
        if (!/^[0-9-]+$/.test(id)) fail("invalid", "not a snapshot id");
        this.active();
        if (!this.snapshots.has(id)) fail("notFound", `no snapshot ${id}`);
        const source = [...this.bundles.values()][0];
        const image = [...source.files.entries()].find(([path]) => path.startsWith("steps/") && path.endsWith(".webp"));
        if (!image) return fail("notFound", "the fixtures have no step image");
        return [...image[1]];
      }
      case "list_capture_windows":
        return structuredClone(this.captureWindows);
      case "pick_region":
        return structuredClone(this.regionPick);
      case "finish_region_pick":
        if (args.pick !== undefined && !Number.isInteger(args.pick)) fail("invalid", "pick must be a number");
        this.regionFinishes.push(structuredClone((args.result ?? null) as { rect: Rect } | null));
        this.regionFinishPicks.push(args.pick as number | undefined);
        return null;
      case "quit_ack":
      case "quit_app":
        return null;
      default:
        return fail("invalid", `the e2e backend has no command ${cmd}`);
    }
  }
}
