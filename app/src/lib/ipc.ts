import { invoke } from "@tauri-apps/api/core";
import type { Guide, Rect, Step, StepImage } from "@guiderecorder/guide-schema";
import type { AiProviderKind, AiRewriteResult, AppError, CaptureDisplay, CaptureTargetArg, CaptureWindow, DetailsEdit, ExportBranding, ExportFormat, ExportPreviewData, ExportResult, GuideListItem, GuideTarget, LibraryInfo, McpSetup, MotionOptions, OpenedGuide, PaperSize, Permission, RenamedGuide, ReviewedGuide, PermissionRow, PermissionStatus, RecordingReadiness, RecordingSaved, RegionPick, RewriteStyleArg, HotkeyErrors, Settings, SettingsView, StampedStatus, UpdateStatus, VersionInfo } from "@/lib/types";
import type { SyncBackend } from "@/sync/engine";

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
  /**
   * Asks (natively) to delete the guide at `path`, then deletes it and its
   * images. False if the user cancelled.
   */
  deleteGuide(id: string, path: string, title: string): Promise<boolean>;
  /**
   * Asks (natively) once to delete several guides, then deletes them in
   * turn, stopping at the first that fails. False if the user cancelled.
   */
  deleteGuides(guides: GuideTarget[]): Promise<boolean>;
  /** The library's folders: relative paths with `/` separators. */
  listFolders(): Promise<string[]>;
  /** Makes folder `name` in `parent` (`""`: the top). Returns its path. */
  createFolder(parent: string, name: string): Promise<string>;
  /** Copies the guide at `path` into a new guide (new id) titled `title`, next to it. Returns its id. */
  duplicateGuide(id: string, path: string, title: string): Promise<string>;
  /** Retitles the guide at `path`; its folder on disk is renamed to match. */
  renameGuide(id: string, path: string, title: string): Promise<RenamedGuide>;
  /** Sets the last review day of the guide at `path` to `today` (`YYYY-MM-DD`). */
  markGuideReviewed(id: string, path: string, today: string): Promise<ReviewedGuide>;
  /** Moves the guide at `path` into `folder` (`""`: the top). Returns its new path. */
  /** Sets the status, owner and tags of the guide at `path`. Returns the guide as saved. */
  editGuideDetails(id: string, path: string, edit: DetailsEdit): Promise<Guide>;
  moveGuide(id: string, path: string, folder: string): Promise<string>;
  /** Shows a guide's bundle, or a library folder, in Finder. */
  reveal(target: { path: string } | { folder: string }): Promise<void>;
  /**
   * Asks (natively) for images or a PDF and saves them as a new annotation
   * guide (`kind: "annotated"`), one page per image or PDF page. Returns
   * its id; null if the user cancelled.
   */
  newAnnotatedGuide(): Promise<string | null>;
  /** WebP bytes of an image in the guide's bundle, by bundle-relative path. */
  stepImage(guideId: string, path: string): Promise<ArrayBuffer>;
  /** Saves the open guide as last saved as a version (even if unchanged). */
  saveVersion(guideId: string, label: string | null): Promise<VersionInfo>;
  /**
   * Makes a version the open guide's current state: keeps the state as
   * last saved as a version, puts the version's images back, saves its
   * guide and returns it.
   */
  restoreVersion(guideId: string, versionId: string): Promise<Guide>;
  /** Exits the app (after the editor has saved). */
  quit(): Promise<void>;
  /** The quit stays open: a held Dock Quit or logout is cancelled now. */
  cancelQuit(): Promise<void>;
}

/** A guide's version history (see core's `history` and app/src-tauri/src/editor.rs). */
export interface HistoryApi {
  /** The guide's versions, newest first. */
  list(guideId: string): Promise<VersionInfo[]>;
  /** The guide as a version saved it. */
  read(guideId: string, versionId: string): Promise<Guide>;
  /** WebP bytes of an image as a version saved it, by its path in that version's guide. */
  image(guideId: string, versionId: string, path: string): Promise<ArrayBuffer>;
}

export const tauriHistoryApi: HistoryApi = {
  list: (guideId) => invoke("list_versions", { guideId }),
  read: (guideId, versionId) => invoke("read_version", { guideId, versionId }),
  image: async (guideId, versionId, path) => toArrayBuffer(await invoke("version_image", { guideId, versionId, path })),
};

/**
 * WebP bytes of a library guide's first screenshot, for its card; empty if
 * it has none. The guide needn't be open.
 */
export async function guideCover(guideId: string): Promise<ArrayBuffer> {
  return toArrayBuffer(await invoke("guide_cover", { guideId }));
}

export const tauriApi: EditorApi = {
  listGuides: (query) => invoke("list_guides", { query: query ?? null }),
  openGuide: (id) => invoke("open_guide", { id }),
  saveGuide: (guide) => invoke("save_guide", { guide }),
  closeGuide: (id) => invoke("close_guide", { id }),
  deleteGuide: (id, path, title) => invoke("delete_guide", { id, path, title }),
  deleteGuides: (guides) => invoke("delete_guides", { guides }),
  listFolders: () => invoke("list_folders"),
  createFolder: (parent, name) => invoke("create_folder", { parent, name }),
  duplicateGuide: (id, path, title) => invoke("duplicate_guide", { id, path, title }),
  renameGuide: (id, path, title) => invoke("rename_guide", { id, path, title }),
  markGuideReviewed: (id, path, today) => invoke("mark_guide_reviewed", { id, path, today }),
  editGuideDetails: (id, path, edit) => invoke("edit_guide_details", { id, path, edit }),
  moveGuide: (id, path, folder) => invoke("move_guide", { id, path, folder }),
  reveal: (target) =>
    invoke("reveal_in_library", {
      path: "path" in target ? target.path : null,
      folder: "folder" in target ? target.folder : null,
    }),
  newAnnotatedGuide: () => invoke("new_annotated_guide"),
  stepImage: async (guideId, path) => toArrayBuffer(await invoke("step_image", { guideId, path })),
  saveVersion: (guideId, label) => invoke("save_version", { guideId, label }),
  restoreVersion: (guideId, versionId) => invoke("restore_version", { guideId, versionId }),
  quit: () => invoke("quit_app"),
  cancelQuit: () => invoke("quit_cancel"),
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
  return invoke("add_image_step", png, { headers: { "guiderecorder-guide-id": guideId } });
}

/**
 * Asks (natively) for images or a PDF and stores them as pages (manual
 * steps showing their full image) of the open guide. The pages are
 * returned in order, not inserted: the editor inserts them as one
 * undoable command. `[]` if the user cancelled.
 */
export function addAnnotatedPages(guideId: string): Promise<Step[]> {
  return invoke("add_annotated_pages", { guideId });
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
    /** Used by DOCX and PDF; Markdown and HTML ignore it. */
    paper: PaperSize,
    /** Show the version, owner and last review day (those set) under the summary, or on the PDF cover. */
    reviewDetails: boolean,
    /** Used by MP4 and GIF. */
    motion: MotionOptions,
  ): Promise<ExportResult>;
  /** A preview of exporting from the saved bundle with these options; writes nothing. */
  previewExport(
    guideId: string,
    format: ExportFormat,
    branding: ExportBranding,
    paper: PaperSize,
    reviewDetails: boolean,
    motion: MotionOptions,
  ): Promise<ExportPreviewData>;
  /** A logo `chooseLogo` returned (PNG or JPEG, up to 2 MB) as a PNG data URI. */
  logoPreview(path: string): Promise<string>;
  /** Shows a file an export wrote in Finder. */
  reveal(path: string): Promise<void>;
}

export const tauriExportApi: ExportApi = {
  chooseFolder: (directory) => invoke("choose_export_folder", { directory: directory ?? null }),
  chooseFile: (format, fileName, directory) =>
    invoke("choose_export_file", { format, fileName, directory: directory ?? null }),
  chooseLogo: () => invoke("choose_logo"),
  exportGuide: (guideId, format, out, overwrite, branding, paper, reviewDetails, motion) =>
    invoke("export_guide", { guideId, format, out, overwrite, branding, paper, reviewDetails, motion }),
  previewExport: (guideId, format, branding, paper, reviewDetails, motion) =>
    invoke("preview_export", { guideId, format, branding, paper, reviewDetails, motion }),
  logoPreview: (path) => invoke("logo_preview", { path }),
  reveal: (path) => invoke("reveal_export", { path }),
};

/** Emitted by the Rust side on Cmd+Q; answer with `quit_ack`, then save and quit. */
export const QUIT_REQUESTED_EVENT = "guiderecorder://quit-requested";

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
  /** As of the last change (record and pause are registered on every change). */
  hotkeyErrors(): Promise<HotkeyErrors>;
  libraryInfo(): Promise<LibraryInfo>;
  /** A library folder from the native dialog; null if cancelled. */
  chooseLibrary(): Promise<string | null>;
  /** Opens that library (null: the default one); returns its path. */
  switchLibrary(path: string | null): Promise<string>;
  launchAtLogin(): Promise<boolean>;
  /** Returns the login item's actual state. */
  setLaunchAtLogin(enabled: boolean): Promise<boolean>;
  /** Whether `provider`'s key is stored (default Anthropic; never for a local one). */
  apiKeyStored(provider?: AiProviderKind): Promise<boolean>;
  setApiKey(key: string, provider?: AiProviderKind): Promise<void>;
  deleteApiKey(provider?: AiProviderKind): Promise<void>;
  permissions(): Promise<PermissionRow[]>;
  openPermissionSettings(permission: Permission): Promise<void>;
  /** Asks macOS for the permission (may prompt); only from a click. */
  requestPermission(permission: Permission): Promise<PermissionStatus>;
  /** The live check recording starts with. */
  recordingReadiness(): Promise<RecordingReadiness>;
  mcpSetup(): Promise<McpSetup>;
  /** Quits through the quit flow (the editor saves first) and starts again. */
  restartApp(): Promise<void>;
}

export const tauriSettingsApi: SettingsApi = {
  get: () => invoke("get_settings"),
  update: (settings) => invoke("update_settings", { settings }),
  hotkeyErrors: () => invoke("hotkey_errors"),
  libraryInfo: () => invoke("library_info"),
  chooseLibrary: () => invoke("choose_library_folder"),
  switchLibrary: (path) => invoke("switch_library", { path }),
  launchAtLogin: () => invoke("launch_at_login"),
  setLaunchAtLogin: (enabled) => invoke("set_launch_at_login", { enabled }),
  apiKeyStored: (provider) => invoke("api_key_status", { provider }),
  setApiKey: (key, provider) => invoke("set_api_key", { key, provider }),
  deleteApiKey: (provider) => invoke("delete_api_key", { provider }),
  permissions: () => invoke("permission_status"),
  openPermissionSettings: (permission) => invoke("open_permission_settings", { permission }),
  requestPermission: (permission) => invoke("request_permission", { permission }),
  recordingReadiness: () => invoke("recording_readiness"),
  mcpSetup: () => invoke("mcp_setup"),
  restartApp: () => invoke("restart_app"),
};

/** Recording commands (see app/src-tauri/src/recording.rs). */
export interface RecordingApi {
  /**
   * Checks the permissions live, then starts. `title` null: named after the
   * date. Without `target`, the capture area comes from the settings.
   */
  start(title: string | null, target?: CaptureTargetArg): Promise<StampedStatus>;
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
  start: (title, target) => invoke("start_recording", target ? { title, target } : { title }),
  pause: () => invoke("pause_recording"),
  resume: () => invoke("resume_recording"),
  stop: () => invoke("stop_recording"),
  cancel: () => invoke("cancel_recording"),
  status: () => invoke("recording_status"),
  snapshotImage: async (id) => toArrayBuffer(await invoke("recording_snapshot", { id })),
};

/** The toolbar's live capture feed (see app/src-tauri/src/feed.rs and toolbar.rs). */
export interface FeedApi {
  /** Leaves a capture's steps out of the guide. */
  deleteCapture(id: string): Promise<void>;
  /** Gives a capture's first step `title` (plain text); resolves to it as stored (guide Markdown). */
  renameCapture(id: string, title: string): Promise<string>;
  /** Sizes the toolbar window to `height` CSS px (from the bar alone to a cap). */
  resizeToolbar(height: number): Promise<void>;
  /** Lets the toolbar take the keyboard (to rename a capture), or gives it back. */
  setToolbarTyping(typing: boolean): Promise<void>;
}

export const tauriFeedApi: FeedApi = {
  deleteCapture: (id) => invoke("delete_capture", { id }),
  renameCapture: (id, title) => invoke("rename_capture", { id, title }),
  resizeToolbar: (height) => invoke("resize_toolbar", { height }),
  setToolbarTyping: (typing) => invoke("set_toolbar_typing", { typing }),
};

/** Picking what to record (see app/src-tauri/src/picker.rs). */
export interface PickerApi {
  /** Windows that can be recorded, frontmost first. */
  listCaptureWindows(): Promise<CaptureWindow[]>;
  /** Active displays, the main one first, then left to right. */
  listCaptureDisplays(): Promise<CaptureDisplay[]>;
  /** Shows the region overlay and waits for it; null if cancelled. */
  pickRegion(): Promise<RegionPick | null>;
}

export const tauriPickerApi: PickerApi = {
  listCaptureWindows: () => invoke("list_capture_windows"),
  listCaptureDisplays: () => invoke("list_capture_displays"),
  pickRegion: () => invoke("pick_region"),
};

/**
 * The region overlay's answer (app/region.html): `rect` in the overlay's
 * CSS px, null to cancel. `pick`: the pick the overlay was opened for
 * (`region.html?pick=<id>`), if given. The Rust side closes the overlay.
 */
export function finishRegionPick(result: { rect: Rect } | null, pick?: number): Promise<void> {
  return invoke("finish_region_pick", pick === undefined ? { result } : { result, pick });
}

/** The pick id in the region overlay's URL (`?pick=<id>`), if any. */
export function regionPickId(search: string): number | undefined {
  const raw = new URLSearchParams(search).get("pick");
  if (raw === null || !/^\d+$/.test(raw)) return undefined;
  return Number(raw);
}

/** The region overlay's window (app/region.html), made by `pick_region`. */
export const REGION_WINDOW_LABEL = "region";

/** Emitted with a `StampedStatus` on every change of the recorder's state. */
export const RECORDING_STATUS_EVENT = "recording://status";
/** Emitted with a `RecordingSaved` after a recording was saved. */
export const RECORDING_SAVED_EVENT = "recording://saved";
/** Emitted with a `RecordingSnapshot` for each screenshot the running recording takes. */
export const RECORDING_SNAPSHOT_EVENT = "recording://snapshot";
/** Emitted with a `SnapshotTitle` when a capture's title is refined or renamed. */
export const RECORDING_SNAPSHOT_TITLE_EVENT = "recording://snapshot-title";
/** Emitted with `{ id }` when a capture is deleted (in the feed or with ⌘⌥Z). */
export const RECORDING_SNAPSHOT_REMOVED_EVENT = "recording://snapshot-removed";
/**
 * Emitted (no payload) when the tray or the record hotkey starts a
 * recording while the capture area is One display, Window or Region: the main window
 * asks what to record, then starts.
 */
export const RECORDING_PICK_AREA_EVENT = "recording://pick-area";

/**
 * Adds a step from the screen as it is now (the toolbar's Manual step).
 * Rejects with `invalid` unless a recording is running (paused counts as not).
 */
export function addManualStep(): Promise<void> {
  return invoke("add_manual_step");
}

/** The floating recording toolbar's window (app/toolbar.html), made by the Rust side while recording. */
export const TOOLBAR_WINDOW_LABEL = "toolbar";

/** AI writing (see app/src-tauri/src/ai.rs). Results arrive as `ai://` events. */
export interface AiApi {
  /**
   * Starts writing the guide as last saved, in the background. Defaults:
   * every step that is not `edited`, and the guide pass. Rejects with
   * `invalid` when AI is off, no key is set or a run is already going.
   */
  generate(guideId: string, options?: AiGenerateOptions): Promise<{ runId: number }>;
  /** Stops a run; nothing if it is over. */
  cancel(runId: number): Promise<void>;
  /**
   * Rewrites one step of the guide as last saved, edited or not, and
   * returns the result (no events). Rejects like `generate`; an unknown
   * step or a bad custom instruction is `invalid`.
   */
  rewriteStep(guideId: string, stepId: string, style: RewriteStyleArg): Promise<AiRewriteResult>;
}

export interface AiGenerateOptions {
  /** The steps to write; `[]`: none (only the guide pass). Default: every step. */
  stepIds?: string[];
  guidePass?: boolean;
  /** The listed steps (and, with `guidePass`, the guide fields) count as not edited. */
  force?: boolean;
}

export const tauriAiApi: AiApi = {
  generate: (guideId, options) => invoke("ai_generate", options ? { guideId, options } : { guideId }),
  cancel: (runId) => invoke("ai_cancel", { runId }),
  rewriteStep: (guideId, stepId, style) => invoke("ai_rewrite_step", { guideId, stepId, style }),
};

/** `AiStartedEvent`. */
export const AI_STARTED_EVENT = "ai://started";
/** `AiStepEvent`. */
export const AI_STEP_EVENT = "ai://step";
/** `AiGuideEvent`. */
export const AI_GUIDE_EVENT = "ai://guide";
/** `AiFinishedEvent`. */
export const AI_FINISHED_EVENT = "ai://finished";

/** App updates (see app/src-tauri/src/updater.rs). Progress arrives as `UPDATE_STATUS_EVENT`s. */
export interface UpdaterApi {
  /** The running app's version. */
  version(): Promise<string>;
  status(): Promise<UpdateStatus>;
  /** Starts a check; returns at once. */
  check(): Promise<void>;
  /** Downloads and installs the update the last check found. */
  install(): Promise<void>;
  /** Quits through the quit flow (the editor saves first) and starts the installed update. */
  restart(): Promise<void>;
}

export const tauriUpdaterApi: UpdaterApi = {
  version: async () => (await invoke<{ version: string }>("app_info")).version,
  status: () => invoke("update_status"),
  check: () => invoke("check_for_update"),
  install: () => invoke("install_update"),
  restart: () => invoke("restart_to_update"),
};

/** Emitted with an `UpdateStatus` on every change of the updater's state. */
export const UPDATE_STATUS_EVENT = "updater://status";

/** Where the app is signed in to the web app (see app/src-tauri/src/sync.rs). */
export interface SyncAccount {
  server: string | null;
  signedIn: boolean;
}

/** A device code to show while the user approves it in the browser. */
export interface DeviceCode {
  deviceCode: string;
  userCode: string;
  verificationUri: string;
  verificationUriComplete: string;
  expiresIn: number;
  interval: number;
}

export type SignInStatus = "pending" | "slowDown" | "approved" | "denied" | "expired";

/** Sync commands: signing in, and the stored sync state. */
export interface SyncApi {
  account(): Promise<SyncAccount>;
  /** Asks `server` for a device code and opens its approval page in the browser. */
  signInStart(server: string): Promise<DeviceCode>;
  signInPoll(deviceCode: string): Promise<SignInStatus>;
  signOut(): Promise<void>;
  loadState(): Promise<unknown>;
  saveState(state: unknown): Promise<void>;
}

export const tauriSyncApi: SyncApi = {
  account: () => invoke("sync_account"),
  signInStart: (server) => invoke("sync_sign_in_start", { server }),
  signInPoll: (deviceCode) => invoke("sync_sign_in_poll", { deviceCode }),
  signOut: () => invoke("sync_sign_out"),
  loadState: () => invoke("sync_load_state"),
  saveState: (state) => invoke("sync_save_state", { state }),
};

/** What the sync loop needs from the app (`src/sync/engine.ts`). */
export const tauriSyncBackend: SyncBackend = {
  request: (method, path, body) => invoke("sync_request", { method, path, body: body ?? null }),
  readGuide: (id) => invoke("sync_read_guide", { id }),
  prepareGuide: (id) => invoke("sync_prepare_guide", { id }),
  upload: (uploads) => invoke("sync_upload", { uploads }),
  applyGuide: (workspaceId, guide, assets) => invoke("sync_apply_guide", { workspaceId, guide, assets }),
  duplicateGuide: (id, path, title) => invoke("duplicate_guide", { id, path, title }),
  listGuideIds: async () => ((await invoke("list_guides", { query: null })) as GuideListItem[]).filter((g) => !g.duplicateOf).map((g) => g.id),
};
