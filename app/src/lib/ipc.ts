import { invoke } from "@tauri-apps/api/core";
import type {
  AppError,
  ExportBranding,
  ExportFormat,
  ExportResult,
  Guide,
  GuideListItem,
  LibraryInfo,
  McpSetup,
  OpenedGuide,
  Permission,
  PermissionStatus,
  RecordingReadiness,
  RecordingSaved,
  Settings,
  SettingsView,
  StampedStatus,
  Step,
  StepImage,
} from "@/lib/types";

/** The Tauri commands the editor uses (see app/src-tauri/src/commands.rs). */
export interface EditorApi {
  listGuides(query?: string): Promise<GuideListItem[]>;
  openGuide(id: string): Promise<OpenedGuide>;
  saveGuide(guide: Guide): Promise<void>;
  /**
   * After saving: forgets the guide and deletes step images its saved copy
   * no longer uses. Returns their paths.
   */
  closeGuide(id: string): Promise<string[]>;
  /** WebP bytes of an image in the guide's bundle, by bundle-relative path. */
  stepImage(guideId: string, path: string): Promise<ArrayBuffer>;
  /** Exits the app (after the editor has saved). */
  quit(): Promise<void>;
}

export const tauriApi: EditorApi = {
  listGuides: (query) => invoke("list_guides", { query: query ?? null }),
  openGuide: (id) => invoke("open_guide", { id }),
  saveGuide: (guide) => invoke("save_guide", { guide }),
  closeGuide: (id) => invoke("close_guide", { id }),
  stepImage: async (guideId, path) => toArrayBuffer(await invoke("step_image", { guideId, path })),
  quit: () => invoke("quit_app"),
};

/**
 * Bytes from a command that returns raw bytes. Over Tauri's custom IPC
 * protocol they arrive as an ArrayBuffer; when Tauri has fallen back to
 * postMessage they arrive as a JSON number array.
 */
export function toArrayBuffer(response: ArrayBuffer | ArrayBufferView | number[]): ArrayBuffer {
  if (response instanceof ArrayBuffer) return response;
  if (Array.isArray(response)) return new Uint8Array(response).buffer;
  const view = new Uint8Array(response.buffer, response.byteOffset, response.byteLength);
  return view.slice().buffer;
}

/**
 * Stores a PNG as a new manual step's screenshot. The step is returned,
 * not inserted: the editor inserts it as an undoable command.
 */
export function addImageStep(guideId: string, png: Uint8Array): Promise<Step> {
  return invoke("add_image_step", png, { headers: { "guidesya-guide-id": guideId } });
}

/** Copies a step's image files for the new step `newStepId`; returns its image. */
export function copyStepImages(guideId: string, image: StepImage, newStepId: string): Promise<StepImage> {
  return invoke("copy_step_images", { guideId, image, newStepId });
}

/** The bytes of an image file dropped on the window (only those). */
export async function readImageFile(path: string): Promise<ArrayBuffer> {
  return toArrayBuffer(await invoke("read_image_file", { path }));
}

/**
 * Whether a step's `cropRect` places its crop image exactly, by the
 * exporter's check (size and pixels against the full screenshot).
 */
export function verifyCrop(guideId: string, image: StepImage): Promise<boolean> {
  return invoke("verify_crop", { guideId, image });
}

/**
 * Export commands. The native dialogs run on the Rust side, which accepts
 * only destinations and logos they returned.
 */
export interface ExportApi {
  /** A folder a Markdown or HTML export goes into; null if cancelled. `directory`: where the dialog starts. */
  chooseFolder(directory?: string | null): Promise<string | null>;
  /** Where a DOCX or PDF file is saved; null if cancelled. */
  chooseFile(format: ExportFormat, fileName: string, directory?: string | null): Promise<string | null>;
  /** A PNG or JPEG logo; null if cancelled. */
  chooseLogo(): Promise<string | null>;
  exportGuide(
    guideId: string,
    format: ExportFormat,
    out: string,
    overwrite: boolean,
    branding: ExportBranding,
  ): Promise<ExportResult>;
  /** Shows a file an export wrote in Finder. */
  reveal(path: string): Promise<void>;
}

export const tauriExportApi: ExportApi = {
  chooseFolder: (directory) => invoke("choose_export_folder", { directory: directory ?? null }),
  chooseFile: (format, fileName, directory) =>
    invoke("choose_export_file", { format, fileName, directory: directory ?? null }),
  chooseLogo: () => invoke("choose_logo"),
  exportGuide: (guideId, format, out, overwrite, branding) =>
    invoke("export_guide", { guideId, format, out, overwrite, branding }),
  reveal: (path) => invoke("reveal_export", { path }),
};

/** Emitted by the Rust side on Cmd+Q; answer with `quit_ack`, then save and quit. */
export const QUIT_REQUESTED_EVENT = "guidesya://quit-requested";

/** A readable message from a rejected command (an `AppError`) or anything else. */
export function errorMessage(error: unknown): string {
  if (error && typeof error === "object" && "message" in error) {
    return String((error as AppError).message);
  }
  return String(error);
}

/** Settings commands (see app/src-tauri/src/settings_commands.rs). */
export interface SettingsApi {
  get(): Promise<SettingsView>;
  /** Validates and stores; returns the settings as stored. */
  update(settings: Settings): Promise<Settings>;
  libraryInfo(): Promise<LibraryInfo>;
  /** A library folder from the native dialog; null if cancelled. */
  chooseLibrary(): Promise<string | null>;
  /** Opens that library (null: the default one); returns its path. */
  switchLibrary(path: string | null): Promise<string>;
  launchAtLogin(): Promise<boolean>;
  /** Returns the login item's actual state. */
  setLaunchAtLogin(enabled: boolean): Promise<boolean>;
  apiKeyStored(): Promise<boolean>;
  setApiKey(key: string): Promise<void>;
  deleteApiKey(): Promise<void>;
  permissions(): Promise<{ permission: Permission; status: PermissionStatus }[]>;
  openPermissionSettings(permission: Permission): Promise<void>;
  /** Asks macOS for the permission (may prompt); only from a click. */
  requestPermission(permission: Permission): Promise<PermissionStatus>;
  /** The live check recording starts with. */
  recordingReadiness(): Promise<RecordingReadiness>;
  mcpSetup(): Promise<McpSetup>;
}

export const tauriSettingsApi: SettingsApi = {
  get: () => invoke("get_settings"),
  update: (settings) => invoke("update_settings", { settings }),
  libraryInfo: () => invoke("library_info"),
  chooseLibrary: () => invoke("choose_library_folder"),
  switchLibrary: (path) => invoke("switch_library", { path }),
  launchAtLogin: () => invoke("launch_at_login"),
  setLaunchAtLogin: (enabled) => invoke("set_launch_at_login", { enabled }),
  apiKeyStored: () => invoke("api_key_status"),
  setApiKey: (key) => invoke("set_api_key", { key }),
  deleteApiKey: () => invoke("delete_api_key"),
  permissions: () => invoke("permission_status"),
  openPermissionSettings: (permission) => invoke("open_permission_settings", { permission }),
  requestPermission: (permission) => invoke("request_permission", { permission }),
  recordingReadiness: () => invoke("recording_readiness"),
  mcpSetup: () => invoke("mcp_setup"),
};

/** Recording commands (see app/src-tauri/src/recording.rs). */
export interface RecordingApi {
  /** Checks the permissions live, then starts. `title` null: named after the date. */
  start(title: string | null): Promise<StampedStatus>;
  pause(): Promise<StampedStatus>;
  resume(): Promise<StampedStatus>;
  /** Saves the recording as a new guide in the open library. */
  stop(): Promise<RecordingSaved>;
  /** Discards the recording. */
  cancel(): Promise<void>;
  status(): Promise<StampedStatus>;
  /** WebP bytes of a screenshot the running recording announced, by its id. */
  snapshotImage(id: string): Promise<ArrayBuffer>;
}

export const tauriRecordingApi: RecordingApi = {
  start: (title) => invoke("start_recording", { title }),
  pause: () => invoke("pause_recording"),
  resume: () => invoke("resume_recording"),
  stop: () => invoke("stop_recording"),
  cancel: () => invoke("cancel_recording"),
  status: () => invoke("recording_status"),
  snapshotImage: async (id) => toArrayBuffer(await invoke("recording_snapshot", { id })),
};

/** Emitted with a `StampedStatus` on every change of the recorder's state. */
export const RECORDING_STATUS_EVENT = "recording://status";
/** Emitted with a `RecordingSaved` after a recording was saved. */
export const RECORDING_SAVED_EVENT = "recording://saved";
/** Emitted with a `RecordingSnapshot` for each screenshot the running recording takes. */
export const RECORDING_SNAPSHOT_EVENT = "recording://snapshot";
