import { invoke } from "@tauri-apps/api/core";
import type { AppError, Guide, GuideListItem, OpenedGuide, Step, StepImage } from "@/lib/types";

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

/** Emitted by the Rust side on Cmd+Q; answer with `quit_ack`, then save and quit. */
export const QUIT_REQUESTED_EVENT = "guidesya://quit-requested";

/** A readable message from a rejected command (an `AppError`) or anything else. */
export function errorMessage(error: unknown): string {
  if (error && typeof error === "object" && "message" in error) {
    return String((error as AppError).message);
  }
  return String(error);
}
