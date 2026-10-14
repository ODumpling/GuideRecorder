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
  // Annotations are edited later (#351); kept as opaque JSON here.
  annotations: unknown[];
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

export type ErrorKind = "notFound" | "readOnly" | "notOpen" | "conflict" | "invalid" | "io";

export interface AppError {
  kind: ErrorKind;
  message: string;
}
