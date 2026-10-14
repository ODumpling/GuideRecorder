import { invoke } from "@tauri-apps/api/core";
import type { AppError, Guide, GuideListItem, ImageVariant, OpenedGuide } from "@/lib/types";

/** The Tauri commands the editor uses (see app/src-tauri/src/commands.rs). */
export interface EditorApi {
  listGuides(query?: string): Promise<GuideListItem[]>;
  openGuide(id: string): Promise<OpenedGuide>;
  saveGuide(guide: Guide): Promise<void>;
  stepImage(guideId: string, stepId: string, variant: ImageVariant): Promise<ArrayBuffer>;
  /** Exits the app (after the editor has saved). */
  quit(): Promise<void>;
}

export const tauriApi: EditorApi = {
  listGuides: (query) => invoke("list_guides", { query: query ?? null }),
  openGuide: (id) => invoke("open_guide", { id }),
  saveGuide: (guide) => invoke("save_guide", { guide }),
  stepImage: (guideId, stepId, variant) => invoke("step_image", { guideId, stepId, variant }),
  quit: () => invoke("quit_app"),
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
