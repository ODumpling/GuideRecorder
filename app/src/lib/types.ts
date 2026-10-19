// Mirrors guiderecorder_core::model (guide.json) and the app's IPC types.

export type Ulid = string;

export type StepKind =
  | "click"
  | "doubleClick"
  | "rightClick"
  | "type"
  | "shortcut"
  | "drag"
  | "manual";

export interface Point {
  x: number;
  y: number;
}

export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface ElementInfo {
  role?: string;
  label?: string;
  app?: string;
  bundleId?: string;
  window?: string;
}

export type ImageShow = "crop" | "full";

export interface StepImage {
  full: string;
  crop?: string;
  cropRect?: Rect;
  after?: string;
  show: ImageShow;
}

export type BlurMode = "blur" | "pixelate";

/** Visual style; unset fields use the renderer's defaults (core `export/annotate.rs`). */
export interface AnnotationStyle {
  /** CSS colour: hex or a CSS colour name. */
  color?: string;
  fill?: string;
  strokeWidth?: number;
  opacity?: number;
  fontSize?: number;
  /** Blur annotations only. */
  blurMode?: BlurMode;
  /** Blur radius or pixel block size, in image pixels. */
  blurAmount?: number;
}

export interface ArrowGeometry {
  from: Point;
  to: Point;
}

export interface CalloutGeometry extends Rect {
  /** Point the callout's pointer targets, if any. */
  anchor?: Point;
  /** Plain text. */
  text: string;
}

/**
 * A region the reader hovers or clicks to see its title and description.
 * Numbered 1, 2, 3, ... in annotation order among the step's hotspots.
 */
export interface HotspotGeometry extends Rect {
  /** Plain text. */
  title: string;
  /** Plain text; may hold line breaks. */
  description: string;
}

/**
 * Mirrors core's `Annotation` (`{ id, type, geometry, style }`). Geometry is
 * in full-screenshot pixels.
 */
export type Annotation = { id: Ulid; style?: AnnotationStyle } & (
  | { type: "arrow"; geometry: ArrowGeometry }
  | { type: "box" | "highlight" | "blur" | "crop"; geometry: Rect }
  | { type: "callout"; geometry: CalloutGeometry }
  | { type: "hotspot"; geometry: HotspotGeometry }
);

export type AnnotationType = Annotation["type"];

export interface AiState {
  status: "template" | "generating" | "ai" | "edited";
  model?: string;
  promptVersion?: string;
  /** Short message from the last failed AI pass; the text was left as it was. */
  error?: string | null;
  /**
   * Blur regions suggested for the step's screenshot, by the AI or found on
   * this Mac; never applied by themselves.
   */
  redactionSuggestions?: RedactionSuggestion[];
}

/** A suggested blur region; `rect` in full-screenshot pixels, like annotations. */
export interface RedactionSuggestion {
  rect: Rect;
  reason: string;
  /** Who suggested it; absent: the AI. "onDevice": text found on this Mac. */
  source?: "ai" | "onDevice";
}

export interface Step {
  type: "step";
  id: Ulid;
  title: string;
  description: string;
  kind: StepKind;
  point?: Point;
  endPoint?: Point;
  element?: ElementInfo;
  keystrokes?: string;
  image: StepImage;
  annotations: Annotation[];
  aiState: AiState;
}

export interface Section {
  type: "section";
  id: Ulid;
  title: string;
}

export type NoteKind = "tip" | "warning" | "note";

export interface Note {
  type: "note";
  id: Ulid;
  kind: NoteKind;
  body: string;
}

export type Item = Step | Section | Note;

/**
 * `walkthrough`: recorded steps. `annotated`: uploaded pages (images, PDF
 * pages), each a `manual` step showing its full image, explained by hotspots.
 */
export type GuideKind = "walkthrough" | "annotated";

/** Where a guide stands in its review cycle (core `GuideStatus`). */
export type GuideStatus = "draft" | "inReview" | "published" | "needsUpdate";

/**
 * Owner, status, version and review schedule (core `ReviewInfo`), at the
 * top level of `guide.json`. All optional; absent when unset.
 */
export interface ReviewFields {
  owner?: string | null;
  status?: GuideStatus | null;
  /** Free text, e.g. `1.2`. */
  version?: string | null;
  /** `YYYY-MM-DD`. */
  lastReviewed?: string | null;
  /** Review every N months; unset or 0: no schedule. */
  reviewIntervalMonths?: number | null;
}

export interface Guide extends ReviewFields {
  schemaVersion: number;
  id: Ulid;
  title: string;
  summary: string;
  tags: string[];
  intro: string;
  prerequisites: string;
  audience: "beginner" | "expert";
  /** Absent: `walkthrough`. */
  kind?: GuideKind;
  language: string;
  createdAt: string;
  updatedAt: string;
  recording?: unknown;
  items: Item[];
  /**
   * Provenance of the guide-level fields (title, intro, prerequisites,
   * summary, tags). `edited` once the user changes one, and while any is in
   * `editedFields`. Absent in older guides (as `template`).
   */
  aiState?: AiState;
  /**
   * The guide-level fields the user wrote by hand; AI leaves them and may
   * still write the others. Absent in older guides: there an `edited`
   * `aiState` means every field is the user's.
   */
  editedFields?: GuideTextField[];
}

/** The guide-level fields the AI guide pass writes. */
export type GuideTextField = "title" | "intro" | "prerequisites" | "summary" | "tags";

export interface GuideListItem extends ReviewFields {
  id: Ulid;
  path: string;
  title: string;
  summary: string;
  tags: string[];
  stepCount: number;
  createdAt: string;
  updatedAt: string;
  /** The library folder holding it, `/` separated; `""` at the top. */
  folder: string;
  readOnly: boolean;
  duplicateOf: string | null;
}

/** `mark_guide_reviewed`'s result. */
export interface ReviewedGuide {
  lastReviewed: string;
  updatedAt: string;
}

/** `rename_guide`'s result. */
export interface RenamedGuide {
  path: string;
  title: string;
  updatedAt: string;
}

export interface OpenedGuide {
  guide: Guide;
  path: string;
  readOnly: boolean;
  migratedFrom: number | null;
  migrationError: string | null;
}

/** `exists`: an export target is already there and overwrite is off. */
export type ErrorKind = "notFound" | "readOnly" | "notOpen" | "conflict" | "invalid" | "exists" | "io";

export interface AppError {
  kind: ErrorKind;
  message: string;
}

export type ExportFormat = "md" | "html" | "walkthrough" | "docx" | "pdf";

/** Paper size of a DOCX or PDF export (core `export::docx::Paper`). */
export type PaperSize = "a4" | "letter";

/** Export branding: a logo the user chose in the logo dialog, and an accent colour. */
export interface ExportBranding {
  logo: string | null;
  accent: string | null;
}

/** What an export wrote and what degraded along the way. */
export interface ExportResult {
  /** Files written, the main document first. */
  files: string[];
  warnings: { stepId: Ulid; message: string }[];
  notes: string[];
}

/** `display`: every display; `oneDisplay`: one, picked at each recording. */
export type CaptureArea = "display" | "oneDisplay" | "window" | "region";

/** A display recording can capture (`list_capture_displays`); bounds in global points. */
export interface CaptureDisplay {
  displayId: number;
  main: boolean;
  bounds: Rect;
}

/** A window recording can capture (`list_capture_windows`); size in points. */
export interface CaptureWindow {
  windowId: number;
  appName: string;
  bundleId: string | null;
  title: string | null;
  width: number;
  height: number;
}

export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/**
 * What `start_recording` captures, when the user picked it. A display
 * without `displayId` is every display. Region `rect` is in global display
 * points.
 */
export type CaptureTargetArg =
  | { kind: "display"; displayId?: number }
  | { kind: "window"; windowId: number }
  | { kind: "region"; displayId: number | null; rect: Rect };

/** A region drawn on the overlay (`pick_region`); `rect` in global points. */
export interface RegionPick {
  displayId: number | null;
  rect: Rect;
}

/** Who writes the guide text (see `Provider` in settings.rs). */
export type AiProviderKind = "anthropic" | "openAiCompatible" | "ollama" | "lmStudio";

/** An OpenAI-compatible provider's settings. */
export interface AiEndpoint {
  /** Empty: the local preset's default (required for a custom API). */
  baseUrl: string;
  /** Writes each step, and rewrites; empty until chosen. */
  stepModel: string;
  /** Writes the guide fields; empty until chosen. */
  guideModel: string;
  /** The step model takes images; off: steps are written from their text only. */
  images: boolean;
}

/** App settings, as stored in Application Support (see settings.rs). */
export interface Settings {
  version: number;
  general: {
    /** null: ~/Documents/GuideRecorder. */
    library: string | null;
    launchAtLogin: boolean;
    menuBarIcon: boolean;
    /** First-run setup was finished. */
    onboarded: boolean;
    /** Check for updates at startup and every few hours. */
    checkForUpdates: boolean;
    theme: Theme;
  };
  recording: {
    hotkeys: { record: string; pause: string; manualStep: string };
    captureArea: CaptureArea;
    afterFrames: boolean;
    afterFrameDelayMs: number;
    excludedApps: string[];
    typingIdleMs: number;
  };
  ai: {
    enabled: boolean;
    autoRun: boolean;
    provider: AiProviderKind;
    /** Anthropic's step model. */
    stepModel: string;
    /** Anthropic's guide model. */
    guideModel: string;
    openAiCompatible: AiEndpoint;
    ollama: AiEndpoint;
    lmStudio: AiEndpoint;
    /** BCP 47 tag; empty: the guide's own language. */
    language: string;
    audience: "beginner" | "expert";
  };
  export: {
    format: ExportFormat;
    folder: string | null;
    logo: string | null;
    accent: string | null;
  };
  /** On-device redaction of new recordings. */
  redaction: {
    mode: RedactionMode;
    /** Regular expressions (Rust `regex` syntax). */
    patterns: string[];
    /** Words and phrases, matched ignoring case. */
    words: string[];
  };
  mcp: {
    /** The setup command passes `--writable`, so MCP clients may edit and create guides. */
    writable: boolean;
  };
}

/** The app's colour theme (Settings → Appearance). */
export type Theme = "arctic" | "paper" | "paperDark" | "sage" | "steel" | "nebula";

/** What happens to sensitive text found in a new recording's screenshots. */
export type RedactionMode = "off" | "suggest" | "apply";

export interface SettingsView {
  settings: Settings;
  /** Saved by a newer GuideRecorder: shown, not saved over. */
  readOnly: boolean;
  /** Why the stored settings could not be used, if so. */
  loadError: string | null;
}

/** Why the record or pause shortcut could not be registered; null when it is (or is not set). */
export interface HotkeyErrors {
  record: string | null;
  pause: string | null;
}

export interface LibraryInfo {
  path: string | null;
  error: string | null;
  starting: boolean;
  fromEnv: boolean;
  defaultPath: string | null;
}

export type Permission = "screen_recording" | "accessibility" | "input_monitoring";
export type PermissionStatus = "granted" | "denied" | "not_determined" | "unknown";

/** A row of `permission_status`. */
export interface PermissionRow {
  permission: Permission;
  status: PermissionStatus;
  /** Allowed in System Settings since launch; macOS applies it after a restart (Screen Recording). */
  restartNeeded: boolean;
}

/** Whether recording may start (checked live before each recording). */
export interface RecordingReadiness {
  ready: boolean;
  missing: Permission[];
}

export interface McpSetup {
  command: string;
  mcpPath: string;
  found: boolean;
  library: string | null;
}

/**
 * The recorder's state (see app/src-tauri/src/recording.rs). Times are
 * RFC 3339 UTC. `captureAreaFallback`: the settings asked for a window or
 * region but the recording was started without one, so the display is
 * recorded.
 * `pausedMs`: time spent in finished pauses; `pausedAt`: when the current
 * pause began (so recorded time is `now − startedAt − pausedMs`, less
 * `now − pausedAt` while paused). `excludedApp`: bundle id of the excluded
 * app in front, while one is; nothing is captured meanwhile.
 * `captureLost`: why capture stopped by itself (e.g. "the window was
 * closed"); nothing more is captured, manual steps are refused and the user
 * should stop to save what was recorded. The recording is not stopped.
 */
export type RecordingStatus =
  | { state: "idle" }
  | ({ state: "recording" } & RecordingRun)
  | ({ state: "paused"; pausedAt: string } & RecordingRun)
  | { state: "saving"; startedAt: string; title: string | null };

/**
 * A status as the recorder stored it: every status event and command result
 * carries the `seq` it was stored with, which grows with each change, so one
 * that arrives after a newer one can be dropped.
 */
export type StampedStatus = RecordingStatus & { seq: number };

/** The fields `recording` and `paused` share. */
export interface RecordingRun {
  startedAt: string;
  title: string | null;
  captureAreaFallback: boolean;
  pausedMs: number;
  excludedApp: string | null;
  captureLost: string | null;
}

/** A recording saved as a new guide. `skipped`: steps left out for lack of a screenshot. */
export interface RecordingSaved {
  guideId: Ulid;
  steps: number;
  skipped: number;
  /** Blur suggestions added for text found sensitive on this Mac. */
  suggestedBlurs: number;
  /** Blurs added for it, when Settings apply them. */
  appliedBlurs: number;
  /** Steps that could not be scanned for sensitive text. */
  unscannedSteps: number;
}

/**
 * A screenshot the running recording took (`recording://snapshot`).
 * Provisional: which steps the guide gets, and which screenshots they
 * keep, is decided when the recording stops.
 */
export interface RecordingSnapshot {
  /** Names the image for `recording_snapshot`; unique within the recording. */
  id: string;
  /** When the event that took it happened; monotonic nanoseconds, for ordering only. */
  ts: number;
  /** Image size in pixels. */
  width: number;
  height: number;
  /** Where the click landed, in image pixels; null for a key. */
  point: Point | null;
}

/** What the guide pass writes (`ai://guide`). */
export interface AiGuideFields {
  title: string;
  intro: string;
  prerequisites: string;
  summary: string;
  tags: string[];
}

/** How `ai_rewrite_step` rewrites a step; a custom `instruction` is 1–500 characters after trimming. */
export type RewriteStyleArg =
  | { kind: "shorter" }
  | { kind: "beginner" }
  | { kind: "expert" }
  | { kind: "custom"; instruction: string };

/** `ai_rewrite_step`'s result: the step's new text and `aiState` (status `ai`). */
export interface AiRewriteResult {
  title: string;
  description: string | null;
  aiState: AiState;
}

/** `ai://started`: the run began; `stepIds` are the steps it will write. */
export interface AiStartedEvent {
  runId: number;
  guideId: Ulid;
  stepIds: Ulid[];
}

export type AiStepOutcome = "applied" | "failed" | "skippedEdited" | "cancelled";

/** `ai://step`: one step's result. `step` is the pipeline's updated step. */
export interface AiStepEvent {
  runId: number;
  guideId: Ulid;
  stepId: Ulid;
  outcome: AiStepOutcome;
  step: Step | null;
  redactionSuggestions: RedactionSuggestion[];
  error: string | null;
}

/** `ai://guide`: the guide pass; `fields` null when it was skipped, cancelled or failed. */
export interface AiGuideEvent {
  runId: number;
  guideId: Ulid;
  fields: AiGuideFields | null;
  /** The fields the pass wrote (not ones the user edited); the rest of `fields` is as it was. Absent: all. */
  written?: GuideTextField[];
  aiState: AiState;
  error: string | null;
}

/** `ai://finished`: the run is over. */
export interface AiFinishedEvent {
  runId: number;
  guideId: Ulid;
  cancelled: boolean;
  applied: number;
  failed: number;
  skippedEdited: number;
  usage: { inputTokens: number; outputTokens: number };
  /**
   * Why the run broke off, if it did; the counts are zero then, and step
   * results already sent still stand.
   */
  error: string | null;
}

/**
 * The updater's state (see updater.rs). `manual`: the user asked for this
 * check, so its result is shown even when there is nothing to install.
 */
export type UpdateStatus =
  | { state: "idle" }
  | { state: "checking"; manual: boolean }
  | { state: "upToDate"; manual: boolean }
  | { state: "available"; version: string; notes: string | null }
  /** `total` null: the size is unknown. */
  | { state: "downloading"; version: string; downloaded: number; total: number | null }
  /** Installed; takes effect after a restart. */
  | { state: "ready"; version: string }
  /** A check or install failed; `message` says which. */
  | { state: "failed"; message: string; manual: boolean };
