// The app's Tauri commands, served in memory from copies of the fixture
// guides (fixtures/guides). Nothing is ever written to disk.

import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative, resolve } from "node:path";
import type {
  AiFinishedEvent,
  AiGuideEvent,
  AiRewriteResult,
  AiStepEvent,
  CaptureArea,
  CaptureTargetArg,
  CaptureWindow,
  ExportBranding,
  ExportResult,
  Guide,
  HotkeyErrors,
  GuideListItem,
  OpenedGuide,
  Rect,
  RecordingSaved,
  RecordingSnapshot,
  RecordingStatus,
  RedactionSuggestion,
  RewriteStyleArg,
  Settings,
  StampedStatus,
  Step,
  StepImage,
  UpdateStatus,
} from "../src/lib/types";
import { DEFAULT_EXCLUDED_IDS } from "../src/recording/apps";

/** `ai_generate`'s options. */
type AiGenerateArgs = { stepIds?: string[]; guidePass?: boolean; force?: boolean };

const FIXTURES = resolve(import.meta.dirname, "../../fixtures/guides");
/** Core's `SCHEMA_VERSION`: a guide saved with a newer one opens read-only. */
export const SCHEMA_VERSION = 2;

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

/** Rust's `capture_lost_error`: a resume or manual step refused once capture is lost. */
const captureLostError = (reason: string) => `Capture stopped: ${reason}. Stop to save what was recorded.`;

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
   * app's opens read-only, as a guide from a newer GuideRecorder would.
   */
  schemaVersions?: Record<string, number>;
  /** First-run setup not finished yet (default: finished, so setup stays out of the way). */
  firstRun?: boolean;
  /** The library failed to open at startup with this error. */
  libraryError?: string;
  /** The stored capture area (default: display). */
  captureArea?: CaptureArea;
  /** A newer version `check_for_update` finds (default: none, up to date). */
  updateVersion?: string;
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
  /**
   * `verify_crop` also fails a crop whose bytes aren't a WebP image, as
   * the Rust side does (it decodes the crop). Off by default: the check
   * then accepts damaged bytes.
   */
  cropCheckRejectsDamaged = false;
  /** Milliseconds to hold a command's reply, by command (to order replies). */
  delayMs: Partial<Record<string, number>> = {};
  /** `step_image` reads held until released, by image path (see `holdImage`). */
  private heldImages = new Map<string, { released: Promise<void>; release(): void }>();

  /** The updater's state; `check_for_update` finds `options.updateVersion`. */
  updateStatus: UpdateStatus = { state: "idle" };
  /** Whether `restart_to_update` was called. */
  restartedToUpdate = false;
  private updateVersion: string | undefined;

  /** Why the record or pause shortcut could not be registered. */
  hotkeyErrors: HotkeyErrors = { record: null, pause: null };

  /** The stored settings (the app's defaults to start with). */
  settings: Settings = {
    version: 2,
    general: { library: null, launchAtLogin: false, menuBarIcon: false, onboarded: true, checkForUpdates: true, theme: "arctic" },
    recording: {
      hotkeys: { record: "Ctrl+Alt+R", pause: "Ctrl+Alt+P", manualStep: "Ctrl+Alt+S" },
      captureArea: "display",
      afterFrames: true,
      afterFrameDelayMs: 400,
      excludedApps: [...DEFAULT_EXCLUDED_IDS],
      typingIdleMs: 1500,
    },
    ai: {
      enabled: true,
      autoRun: true,
      provider: "anthropic",
      stepModel: "claude-haiku-5-5",
      guideModel: "claude-sonnet-5-5",
      openAiCompatible: { baseUrl: "", stepModel: "", guideModel: "", images: true },
      ollama: { baseUrl: "", stepModel: "", guideModel: "", images: true },
      lmStudio: { baseUrl: "", stepModel: "", guideModel: "", images: true },
      language: "",
      audience: "beginner",
    },
    export: { format: "pdf", folder: null, logo: null, accent: null },
    redaction: { mode: "suggest", patterns: [], words: [] },
    mcp: { writable: false },
  };
  /** Whether an API key is "stored"; the key itself is never kept. */
  apiKeyStored = false;
  /** Permissions allowed since launch that need a restart (`restartNeeded`). */
  restartNeeded: string[] = [];
  /** Permissions whose System Settings pane was opened, in order. */
  readonly openedPermissionSettings: string[] = [];
  /** Whether `restart_app` was called. */
  restarted = false;
  /** Whether `request_permission` grants at once (false: macOS showed its prompt, nothing changed yet). */
  requestGrants = true;
  /** Permission states; `request_permission` grants (see `requestGrants`). */
  permissions: Record<string, string> = {
    screen_recording: "granted",
    accessibility: "denied",
    input_monitoring: "not_determined",
  };
  /** The library as `library_info` reports it. */
  library: { path: string; error: string | null } = { path: "/Users/me/Documents/GuideRecorder", error: null };
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
  /**
   * How many pages the next images-or-PDF dialog (`new_annotated_guide`,
   * `add_annotated_pages`) picks; null: the user cancels it.
   */
  pagesDialog: number | null = 2;
  /** What the next images-or-PDF command fails with, if anything. */
  pagesError: AppError | null = null;
  /** Annotation guides `new_annotated_guide` made, by id, in order. */
  readonly annotatedGuides: string[] = [];
  private pages = 0;
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
  /** Events the next command reply carries; mock-entry.ts emits them after resolving it. */
  private after: EmittedEvent[] = [];

  /**
   * AI runs (`ai_generate`) send `ai://started` at once. Unless `aiHold`
   * is set, the rest follows straight away too: each step not edited in the
   * saved guide is "written" as `AI step <n>` (n: its 1-based step number),
   * then the guide pass, then `ai://finished`. With `aiHold`, the rest waits
   * for `aiRelease()` (the test emits it) or `ai_cancel`.
   */
  aiHold = false;
  /** The AI run still holding its events, if any. */
  private aiRun: { runId: number; guideId: string; pending: EmittedEvent[] } | null = null;
  private aiRuns = 0;
  /** Every `ai_generate` that started a run, in order, with its options. */
  readonly aiGenerations: { guideId: string; runId: number; options?: AiGenerateArgs }[] = [];
  /** Blur suggestions the mock AI sends with a step it writes, by step id. */
  aiSuggestions: Record<string, RedactionSuggestion[]> = {};
  /** Every `ai_rewrite_step`, in order. */
  readonly aiRewrites: { guideId: string; stepId: string; style: RewriteStyleArg }[] = [];
  /** Guides opened and not closed since, as the Rust session tracks them. */
  private openGuides = new Set<string>();
  /** Run ids `ai_cancel` was called with. */
  readonly aiCancels: number[] = [];

  constructor(options: BackendOptions = {}) {
    for (const name of readdirSync(FIXTURES).filter((n) => n.endsWith(".guiderecorder")).sort()) {
      const path = join(FIXTURES, name);
      const files = readFiles(path);
      const guide = JSON.parse(files.get("guide.json")!.toString("utf8")) as Guide;
      files.delete("guide.json");
      guide.schemaVersion = options.schemaVersions?.[guide.id] ?? guide.schemaVersion;
      this.bundles.set(guide.id, { path, guide, files });
    }
    if (options.firstRun) this.settings.general.onboarded = false;
    this.updateVersion = options.updateVersion;
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

  /** Changes a step of a guide as loaded, before the app opens it. */
  editStep(guideId: string, stepId: string, edit: (step: Step) => void) {
    const item = this.bundle(guideId).guide.items.find((i) => i.id === stepId);
    if (item?.type !== "step") throw new Error(`no step ${stepId}`);
    edit(item);
  }

  /** Removes a file from a guide's bundle (e.g. to make an image unreadable). */
  removeFile(guideId: string, path: string) {
    this.bundle(guideId).files.delete(path);
  }

  /** Replaces a file of a guide's bundle (e.g. with bytes that aren't an image). */
  setFile(guideId: string, path: string, bytes: Buffer) {
    this.bundle(guideId).files.set(path, bytes);
  }

  /** Holds every `step_image` read of `path` until `releaseImage`. */
  holdImage(path: string) {
    let release = () => {};
    const released = new Promise<void>((resolve) => (release = resolve));
    this.heldImages.set(path, { released, release });
  }

  releaseImage(path: string) {
    this.heldImages.get(path)?.release();
    this.heldImages.delete(path);
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
  ): Promise<({ ok: unknown } | { error: AppError }) & { events: EmittedEvent[]; after?: EmittedEvent[] }> {
    this.calls.push({ cmd, args });
    const delay = this.delayMs[cmd];
    if (delay) await new Promise((resolve) => setTimeout(resolve, delay));
    const held = cmd === "step_image" ? this.heldImages.get(String((args as { path?: unknown })?.path)) : undefined;
    if (held) await held.released;
    try {
      const ok = this.run(cmd, (args ?? {}) as Record<string, unknown>);
      return { ok, events: this.takeEvents(), after: this.after.splice(0) };
    } catch (error) {
      const events = this.takeEvents();
      this.after = [];
      if (error instanceof CommandError) return { error: error.error, events };
      return { error: { kind: "io", message: String(error) }, events };
    }
  }

  private takeEvents(): EmittedEvent[] {
    return this.events.splice(0);
  }

  /** The held AI run's remaining events (`aiHold`), for the test to emit; the run is over. */
  aiRelease(): EmittedEvent[] {
    const run = this.aiRun ?? fail("invalid", "no AI run is held");
    this.aiRun = null;
    return run.pending;
  }

  /** The text the mock AI writes for step `n`. */
  static aiStepText(n: number) {
    return { title: `AI step ${n}`, description: `Written by AI for step ${n}.` };
  }

  /**
   * Starts an AI run on the guide as last saved, like Rust's `ai_generate`:
   * the same checks in the same order, and step events only for the steps
   * it writes (`stepIds`, default all, `[]` none, less the edited ones
   * unless `force`).
   */
  private aiGenerate(guideId: string, options?: AiGenerateArgs): { runId: number } {
    if (!this.settings.ai.enabled) fail("invalid", "AI is turned off in Settings.");
    if (!this.apiKeyStored) fail("invalid", "Add an API key in Settings to write with AI.");
    if (!this.openGuides.has(guideId)) fail("notOpen", `guide ${guideId} is not open`);
    const bundle = this.bundle(guideId);
    if (this.readOnly(bundle)) fail("readOnly", `guide ${guideId} is read-only`);
    const guide = bundle.guide;
    const steps = guide.items.filter((item): item is Step => item.type === "step");
    for (const id of options?.stepIds ?? []) {
      if (!steps.some((step) => step.id === id)) fail("invalid", `step ${id} is not in the saved guide`);
    }
    if (this.aiRun?.guideId === guideId) fail("invalid", "Already writing this guide.");
    const runId = ++this.aiRuns;
    this.aiGenerations.push(options ? { guideId, runId, options: structuredClone(options) } : { guideId, runId });
    const wanted = (step: Step) => !options?.stepIds || options.stepIds.includes(step.id);
    const force = options?.force === true;
    const targets = steps.filter((step) => wanted(step) && (force || step.aiState.status !== "edited"));
    const base = { runId, guideId };
    const stepEvents = targets.map(
      (step): AiStepEvent => ({
        ...base,
        stepId: step.id,
        outcome: "applied",
        step: {
          ...structuredClone(step),
          ...Backend.aiStepText(steps.indexOf(step) + 1),
          aiState: {
            status: "ai",
            model: this.settings.ai.stepModel,
            promptVersion: "step.v1",
            ...(this.aiSuggestions[step.id] && { redactionSuggestions: structuredClone(this.aiSuggestions[step.id]) }),
          },
        },
        redactionSuggestions: structuredClone(this.aiSuggestions[step.id] ?? []),
        error: null,
      }),
    );
    // Edited, or not asked for: no fields.
    const guideSkipped = (guide.aiState?.status === "edited" && !force) || options?.guidePass === false;
    const guidePass: AiGuideEvent = {
      ...base,
      fields: guideSkipped
        ? null
        : { title: `AI: ${guide.title}`, intro: "Written by AI.", prerequisites: "", summary: "An AI summary.", tags: ["ai"] },
      aiState: guideSkipped
        ? structuredClone(guide.aiState ?? { status: "template" })
        : { status: "ai", model: this.settings.ai.guideModel, promptVersion: "guide.v1" },
      error: null,
    };
    const finished: AiFinishedEvent = {
      ...base,
      cancelled: false,
      applied: targets.length,
      failed: 0,
      // Steps left out by `stepIds` are not counted, as in Rust.
      skippedEdited: force ? 0 : steps.filter((step) => wanted(step) && step.aiState.status === "edited").length,
      usage: { inputTokens: 1000, outputTokens: 200 },
      error: null,
    };
    const rest: EmittedEvent[] = [
      ...stepEvents.map((payload) => ({ event: "ai://step", payload })),
      { event: "ai://guide", payload: guidePass },
      { event: "ai://finished", payload: finished },
    ];
    this.after.push({ event: "ai://started", payload: { ...base, stepIds: targets.map((step) => step.id) } });
    if (this.aiHold) this.aiRun = { runId, guideId, pending: rest };
    else this.after.push(...rest);
    return { runId };
  }

  /** The text the mock AI rewrites a step to. */
  static aiRewriteText(style: RewriteStyleArg, n: number) {
    const how = style.kind === "custom" ? `custom (${style.instruction})` : style.kind;
    return { title: `Rewritten ${how} ${n}`, description: `Rewritten ${how} for step ${n}.` };
  }

  /**
   * Rewrites a step of the guide as last saved, like Rust's
   * `ai_rewrite_step`: edited or not, no events, suggestions kept.
   */
  private aiRewriteStep(guideId: string, stepId: string, style: RewriteStyleArg): AiRewriteResult {
    if (!this.settings.ai.enabled) fail("invalid", "AI is turned off in Settings.");
    if (!this.apiKeyStored) fail("invalid", "Add an API key in Settings to write with AI.");
    if (!this.openGuides.has(guideId)) fail("notOpen", `guide ${guideId} is not open`);
    const bundle = this.bundle(guideId);
    if (this.readOnly(bundle)) fail("readOnly", `guide ${guideId} is read-only`);
    const steps = bundle.guide.items.filter((item): item is Step => item.type === "step");
    const step = steps.find((s) => s.id === stepId) ?? fail("invalid", `step ${stepId} is not in the saved guide`);
    if (style.kind === "custom") {
      const length = style.instruction.trim().length;
      if (length < 1 || length > 500) fail("invalid", "The instruction must be 1 to 500 characters.");
    }
    this.aiRewrites.push({ guideId, stepId, style: structuredClone(style) });
    return {
      ...Backend.aiRewriteText(style, steps.indexOf(step) + 1),
      aiState: {
        status: "ai",
        model: this.settings.ai.stepModel,
        promptVersion: "rewrite.v1",
        error: null,
        ...(step.aiState.redactionSuggestions && { redactionSuggestions: structuredClone(step.aiState.redactionSuggestions) }),
      },
    };
  }

  /** Cancels the held AI run: what was left comes back cancelled. */
  private aiCancel(runId: number) {
    this.aiCancels.push(runId);
    const run = this.aiRun;
    if (run?.runId !== runId) return;
    this.aiRun = null;
    for (const { event, payload } of run.pending) {
      if (event === "ai://step") {
        const step = payload as AiStepEvent;
        this.after.push({
          event,
          payload: step.outcome === "applied" ? { ...step, outcome: "cancelled", step: null } : step,
        });
      } else if (event === "ai://guide") {
        this.after.push({ event, payload: { ...(payload as AiGuideEvent), fields: null } });
      } else {
        this.after.push({ event, payload: { ...(payload as AiFinishedEvent), cancelled: true, applied: 0 } });
      }
    }
  }

  /** Stores `status` under the next `seq`, as the Rust `Recorder` does, and queues its event. */
  private setRecording(status: RecordingStatus) {
    this.recording = status;
    this.recordingSeq += 1;
    if (status.state === "idle") this.snapshots.clear();
    this.events.push({ event: "recording://status", payload: this.stamped() });
  }

  /** Stores an updater status and emits it with the reply (`events`) or after it (`after`). */
  private setUpdate(status: UpdateStatus, when: "events" | "after" = "events") {
    this.updateStatus = status;
    this[when].push({ event: "updater://status", payload: structuredClone(status) });
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

  /**
   * Capture stops by itself (the window closed, the display was unplugged),
   * as the Rust side's capture-lost sink reports it (`null`: a resume
   * captured again). Returns the new status; the test emits it.
   */
  loseCapture(captureLost: string | null): StampedStatus {
    this.recording = { ...this.active(), captureLost };
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
    const path = `/Users/me/Documents/GuideRecorder/${guide.title}.guiderecorder`;
    this.bundles.set(id, { path, guide, files: new Map(source.files) });
    const saved: RecordingSaved = {
      guideId: id,
      steps: guide.items.filter((item) => item.type === "step").length,
      skipped: this.recordingSkipped,
    };
    this.recordings.push(saved);
    return saved;
  }

  /**
   * Pages as Rust stores them from the dialog's images or PDF: a manual
   * step each, showing its full image (the first fixture step's), in
   * `bundle`'s files. Bare steps, without the item `type` tag.
   */
  private pickPages(bundle: Bundle): Step[] | null {
    if (this.pagesError) fail(this.pagesError.kind, this.pagesError.message);
    if (this.pagesDialog === null) return null;
    const image = [...this.bundles.values()]
      .flatMap((b) => [...b.files.entries()])
      .find(([path]) => /^steps\/[^.]+\.webp$/.test(path));
    if (!image) return fail("notFound", "the fixtures have no step image");
    return Array.from({ length: this.pagesDialog }, () => {
      const n = ++this.pages;
      const id = `01M4X${"0".repeat(16)}PG${String(n).padStart(3, "0")}`;
      const full = `steps/${id}.webp`;
      bundle.files.set(full, image[1]);
      const page = {
        id,
        title: `Page ${n}`,
        description: "",
        kind: "manual",
        image: { full, show: "full" },
        annotations: [],
        aiState: { status: "template" },
      };
      return page as unknown as Step;
    });
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
              createdAt: bundle.guide.createdAt,
              updatedAt: bundle.guide.updatedAt,
              folder: "",
              readOnly: this.readOnly(bundle),
              duplicateOf: null,
            }),
          );
      }
      case "list_folders":
        return [];
      case "open_guide": {
        const bundle = this.bundle(String(args.id));
        this.openGuides.add(bundle.guide.id);
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
      case "new_annotated_guide": {
        const n = this.annotatedGuides.length + 1;
        const id = `01M4X${"0".repeat(17)}ANN${n}`;
        const bundle: Bundle = {
          path: `/Users/me/Documents/GuideRecorder/Floor plan ${n}.guiderecorder`,
          files: new Map(),
          guide: {
            schemaVersion: SCHEMA_VERSION,
            id,
            title: `Floor plan ${n}`,
            summary: "",
            tags: [],
            intro: "",
            prerequisites: "",
            audience: "beginner",
            kind: "annotated",
            language: "en",
            createdAt: "2026-10-18T10:00:00Z",
            updatedAt: "2026-10-18T10:00:00Z",
            items: [],
          },
        };
        const pages = this.pickPages(bundle);
        if (!pages) return null;
        bundle.guide.items = pages.map((page): Step => ({ ...page, type: "step" }));
        this.bundles.set(id, bundle);
        this.annotatedGuides.push(id);
        return id;
      }
      case "add_annotated_pages": {
        const guideId = String(args.guideId);
        if (!this.openGuides.has(guideId)) fail("notOpen", `guide ${guideId} is not open`);
        const bundle = this.bundle(guideId);
        if (this.readOnly(bundle)) fail("readOnly", `guide ${guideId} is read-only`);
        return this.pickPages(bundle) ?? [];
      }
      case "close_guide":
        this.bundle(String(args.id));
        this.openGuides.delete(String(args.id));
        return [];
      case "step_image": {
        const path = String(args.path);
        const bytes = this.bundle(String(args.guideId)).files.get(path) ?? fail("notFound", `no image ${path}`);
        // A JSON number array, as over Tauri's postMessage fallback.
        return [...bytes];
      }
      case "guide_cover": {
        const { guide, files } = this.bundle(String(args.guideId));
        const step = guide.items.find((item) => item.type === "step" && item.image.full);
        const bytes = step?.type === "step" ? files.get(step.image.full) : undefined;
        return bytes ? [...bytes] : [];
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
        const bytes = image.crop ? bundle.files.get(image.crop) : undefined;
        const webp = !!bytes && bytes.subarray(0, 4).toString("latin1") === "RIFF" && bytes.subarray(8, 12).toString("latin1") === "WEBP";
        return !!bytes && !!image.cropRect && this.cropVerifies && (webp || !this.cropCheckRejectsDamaged);
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
      case "hotkey_errors":
        return structuredClone(this.hotkeyErrors);
      case "library_info":
        return { ...this.library, starting: false, fromEnv: false, defaultPath: "/Users/me/Documents/GuideRecorder" };
      case "choose_library_folder":
        return this.nextLibrary;
      case "switch_library": {
        const path = (args.path as string | null) ?? "/Users/me/Documents/GuideRecorder";
        this.library = { path, error: null };
        this.settings.general.library = (args.path as string | null) ?? null;
        return path;
      }
      case "request_permission":
        if (this.requestGrants) this.permissions[String(args.permission)] = "granted";
        return this.permissions[String(args.permission)];
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
        return Object.entries(this.permissions).map(([permission, status]) => ({
          permission,
          status,
          restartNeeded: this.restartNeeded.includes(permission),
        }));
      case "open_permission_settings":
        this.openedPermissionSettings.push(String(args.permission));
        return null;
      case "restart_app":
        this.restarted = true;
        return null;
      case "mcp_setup":
        return {
          command: `claude mcp add guiderecorder -- /Applications/GuideRecorder.app/Contents/MacOS/guiderecorder-mcp --library /Users/me/Documents/GuideRecorder${this.settings.mcp.writable ? " --writable" : ""}`,
          mcpPath: "/Applications/GuideRecorder.app/Contents/MacOS/guiderecorder-mcp",
          found: true,
          library: "/Users/me/Documents/GuideRecorder",
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
          captureLost: null,
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
          // Like Rust: the window or display is still gone; the session stays paused.
          if (status.captureLost !== null) fail("invalid", captureLostError(status.captureLost));
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
        const status = this.recording;
        if (status.state !== "recording") return fail("invalid", "nothing is recording");
        if (status.captureLost !== null) fail("invalid", captureLostError(status.captureLost));
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
      case "ai_generate":
        return this.aiGenerate(
          String(args.guideId),
          (args.options ?? undefined) as AiGenerateArgs | undefined,
        );
      case "ai_rewrite_step":
        return this.aiRewriteStep(String(args.guideId), String(args.stepId), args.style as RewriteStyleArg);
      case "ai_cancel":
        this.aiCancel(Number(args.runId));
        return null;
      case "quit_ack":
      case "quit_app":
      case "quit_cancel":
        return null;
      case "app_info":
        return { version: "0.1.0", schemaVersion: SCHEMA_VERSION, captureBackend: "mock" };
      case "update_status":
        return structuredClone(this.updateStatus);
      case "check_for_update":
        this.setUpdate({ state: "checking", manual: true });
        this.setUpdate(
          this.updateVersion
            ? { state: "available", version: this.updateVersion, notes: null }
            : { state: "upToDate", manual: true },
          "after",
        );
        return null;
      case "install_update": {
        const status = this.updateStatus;
        if (status.state !== "available") return fail("invalid", "there is no update to install; check for updates first");
        this.setUpdate({ state: "downloading", version: status.version, downloaded: 0, total: 1000 }, "after");
        this.setUpdate({ state: "downloading", version: status.version, downloaded: 500, total: 1000 }, "after");
        this.setUpdate({ state: "ready", version: status.version }, "after");
        return null;
      }
      // The main window's title bar follows the theme.
      case "plugin:window|set_theme":
        return null;
      case "restart_to_update":
        if (this.updateStatus.state !== "ready") return fail("invalid", "no update is installed yet");
        this.restartedToUpdate = true;
        return null;
      default:
        return fail("invalid", `the e2e backend has no command ${cmd}`);
    }
  }
}
