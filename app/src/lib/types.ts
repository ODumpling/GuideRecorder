// The app's IPC types. The guide.json model comes from
// `@guiderecorder/guide-schema`, generated from guiderecorder_core::model.

import type { AiState, Guide, GuideField, GuideKind, GuideStatus, Point, Rect, RedactionSuggestion, Step } from "@guiderecorder/guide-schema";

export type Ulid = string;

/**
 * Owner, status, version and review schedule (core `ReviewInfo`), at the
 * top level of `guide.json`. All optional; absent when unset.
 */
export type ReviewFields = Pick<Guide, "owner" | "status" | "version" | "lastReviewed" | "reviewIntervalMonths">;

export interface GuideListItem extends ReviewFields {
  id: Ulid;
  path: string;
  title: string;
  summary: string;
  tags: string[];
  /** Steps, or an annotation guide's pages. */
  stepCount: number;
  kind: GuideKind;
  /** Hotspots across all steps (an annotation guide's pages). */
  hotspotCount: number;
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

/** `edit_guide_details`'s changes (the library's bulk actions); unset fields are left as they are. */
export interface DetailsEdit {
  status?: GuideStatus;
  /** Trimmed; empty clears the owner. */
  owner?: string;
  addTags?: string[];
  removeTags?: string[];
}

/** A listed guide a library action works on. */
export interface GuideTarget {
  id: Ulid;
  path: string;
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

export type ExportFormat = "md" | "html" | "walkthrough" | "docx" | "pdf" | "pptx" | "mp4" | "gif" | "skill" | "scorm";

/** When an LMS counts a SCORM course as done (core `export::scorm::Completion`). */
export type ScormCompletion = "lastStep" | "quizPassed";

/** SCORM package options (core `export::scorm::ScormOptions`). */
export interface ScormOptions {
  /** Course title; blank: the guide title. */
  title: string;
  /** Course identifier; blank: `guiderecorder-<guide id>`. */
  identifier: string;
  completion: ScormCompletion;
  /** 0 to 100: the quiz score that passes. */
  masteryScore: number;
  /** The quiz as text (a line per question, `- [x]` / `- [ ]` choices); blank for none. */
  quiz: string;
}

/** Agent skill options (core `export::skill::SkillOptions`). */
export interface SkillOptions {
  /** Also write `replay.js`, a macOS script that repeats the steps. */
  replay: boolean;
}

/** Frame shape of an MP4 or GIF export (core `export::motion::Aspect`). */
export type MotionAspect = "wide" | "square" | "vertical";

/** MP4 and GIF options (core `export::motion::MotionOptions`). */
export interface MotionOptions {
  /** 1 to 30. */
  secondsPerStep: number;
  aspect: MotionAspect;
  /** Zoom into each step's click. */
  zoom: boolean;
  /** Title cards and a caption with each step's number and title. */
  captions: boolean;
}

/** Paper size of a DOCX or PDF export (core `export::docx::Paper`). */
export type PaperSize = "a4" | "letter";

/** Export branding: a logo the user chose in the logo dialog, and an accent colour. */
export interface ExportBranding {
  logo: string | null;
  accent: string | null;
}

/**
 * The Export dialog's preview, made by the format's own exporter (Rust
 * `export::ExportPreview`). Images are PNG data URIs with the blurs burned
 * in, as in the file.
 */
export type ExportPreviewData =
  /** The exported HTML or walkthrough page, self-contained. */
  | { kind: "html"; html: string }
  /** The exported `guide.md` and the images it links, by link. */
  | { kind: "markdown"; markdown: string; images: { link: string; src: string }[] }
  /** The PDF's first pages; Word is approximated with them (`approximate`). */
  | { kind: "pages"; pages: string[]; pageCount: number; approximate: boolean }
  /** Each step's slide image, in step order. */
  | { kind: "slides"; images: string[] }
  /** A still of each of the first scenes of a video or GIF. */
  | { kind: "frames"; frames: string[]; sceneCount: number };

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
    /** Follow the system's appearance with `lightTheme` and `darkTheme` instead of `theme`. */
    matchSystem: boolean;
    lightTheme: Theme;
    darkTheme: Theme;
    /** Overrides the theme's accent; `#rgb` or `#rrggbb`. */
    accent: string | null;
    density: Density;
    /** No glow, backdrop blur or card hover animation. */
    reduceEffects: boolean;
  };
  recording: {
    hotkeys: { record: string; pause: string; manualStep: string };
    captureArea: CaptureArea;
    afterFrames: boolean;
    afterFrameDelayMs: number;
    excludedApps: string[];
    typingIdleMs: number;
    /** Briefly show a ring on screen where each click was captured. */
    clickRing: boolean;
    /** Narrating while recording: the toolbar's microphone, transcribed on this computer. */
    narration: {
      /** The microphone is on when a recording starts (the toolbar turns it on and off). */
      enabled: boolean;
      /** Keep the audio in the guide's folder (`narration.m4a`); it is never synced. */
      keepAudio: boolean;
    };
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
  /** How many saved versions each guide keeps; 0 is no limit. Labelled versions and the newest are always kept. */
  history: {
    keepVersions: number;
    keepDays: number;
  };
}

/** Why a guide version was saved. `other`: by a newer app, for a reason this one does not know. */
export type VersionReason = "closed" | "beforeAi" | "manual" | "beforeRestore" | "beforeMcpEdit" | "other";

/** A saved version of a guide (its `.history/`), without the guide itself. */
export interface VersionInfo {
  id: string;
  /** When the version was saved. */
  createdAt: string;
  reason: VersionReason;
  /** The user's name for it; such versions are never pruned. */
  label?: string;
  /** The guide's title then. */
  title: string;
  stepCount: number;
  /** The guide's `updatedAt` then. */
  updatedAt: string;
}

/** The app's colour theme (Settings → Appearance). */
export type Theme = "arctic" | "paper" | "paperDark" | "sage" | "steel" | "nebula";

/** How tightly the outline and the library's cards are laid out. */
export type Density = "comfortable" | "compact";

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

/** Recording needs the first three; narration also needs `microphone` and `speech_recognition`. */
export type Permission = "screen_recording" | "accessibility" | "input_monitoring" | "microphone" | "speech_recognition";
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
export type StampedStatus = RecordingStatus & {
  seq: number;
  /** The guide the recording goes into (a re-capture, steps inserted); absent for a new guide. */
  into?: RecordInto;
};

/**
 * Where a recording goes instead of a new guide (#12): `replace`
 * re-captures the step `stepId` (the first step recorded replaces its
 * images, point, element and keystrokes); `insert` adds every step
 * recorded after the item `afterId` (null: at the end). The guide must
 * be open in the editor.
 */
export type RecordInto =
  | { mode: "replace"; guideId: Ulid; stepId: Ulid }
  | { mode: "insert"; guideId: Ulid; afterId: Ulid | null };

/** The steps a recording into a guide made, for the editor to add. Bare steps, without the item `type` tag. */
export interface RecordedSteps {
  /** New for each stop: the command result and the saved event both carry it. */
  id: string;
  into: RecordInto;
  steps: Omit<Step, "type">[];
}

/** The fields `recording` and `paused` share. */
export interface RecordingRun {
  startedAt: string;
  title: string | null;
  captureAreaFallback: boolean;
  pausedMs: number;
  excludedApp: string | null;
  captureLost: string | null;
}

/**
 * A recording saved as a new guide, or recorded into one (`recorded`).
 * `skipped`: steps left out for lack of a screenshot.
 */
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
  /** Steps that got some of what was said while narrating. */
  narratedSteps: number;
  /** Recorded into a guide: the steps, for the editor to add. Absent for a new guide. */
  recorded?: RecordedSteps;
}

/** The toolbar's microphone (`narration_state`, `recording://narration`). */
export interface NarrationState {
  /** Narration works on this computer (macOS). */
  supported: boolean;
  /** On: what is said is transcribed while recording. Outside a recording, the same as `enabled`. */
  on: boolean;
  /** Whether the next recording starts with the microphone on (`recording.narration.enabled`). */
  enabled: boolean;
  /** Why the microphone could not be turned on. */
  error: string | null;
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
  /** A provisional step title (guide Markdown), e.g. "Click **Submit** in Safari". */
  title: string;
  /** Recorded time (pauses left out) when it was taken. */
  elapsedMs: number;
}

/** A capture's new title (`recording://snapshot-title`). */
export interface SnapshotTitle {
  id: string;
  title: string;
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
  written?: GuideField[];
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
