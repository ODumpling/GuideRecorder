// Mirrors guidesya_core::model (guide.json) and the app's IPC types.

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
 * Mirrors core's `Annotation` (`{ id, type, geometry, style }`). Geometry is
 * in full-screenshot pixels.
 */
export type Annotation = { id: Ulid; style?: AnnotationStyle } & (
  | { type: "arrow"; geometry: ArrowGeometry }
  | { type: "box" | "highlight" | "blur" | "crop"; geometry: Rect }
  | { type: "callout"; geometry: CalloutGeometry }
);

export type AnnotationType = Annotation["type"];

export interface AiState {
  status: "template" | "generating" | "ai" | "edited";
  model?: string;
  promptVersion?: string;
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

export interface Guide {
  schemaVersion: number;
  id: Ulid;
  title: string;
  summary: string;
  tags: string[];
  intro: string;
  prerequisites: string;
  audience: "beginner" | "expert";
  language: string;
  createdAt: string;
  updatedAt: string;
  recording?: unknown;
  items: Item[];
}

export interface GuideListItem {
  id: Ulid;
  path: string;
  title: string;
  summary: string;
  tags: string[];
  stepCount: number;
  updatedAt: string;
  readOnly: boolean;
  duplicateOf: string | null;
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

export type ExportFormat = "md" | "html" | "docx" | "pdf";

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

export type CaptureArea = "display" | "window" | "region";

/** App settings, as stored in Application Support (see settings.rs). */
export interface Settings {
  version: number;
  general: {
    /** null: ~/Documents/GuidesYa. */
    library: string | null;
    launchAtLogin: boolean;
    menuBarIcon: boolean;
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
    stepModel: string;
    guideModel: string;
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
}

export interface SettingsView {
  settings: Settings;
  /** Saved by a newer GuidesYa: shown, not saved over. */
  readOnly: boolean;
  /** Why the stored settings could not be used, if so. */
  loadError: string | null;
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

export interface McpSetup {
  command: string;
  mcpPath: string;
  found: boolean;
  library: string | null;
}
