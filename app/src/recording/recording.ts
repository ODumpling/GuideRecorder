import { useStore } from "zustand";
import { aiStore } from "@/ai/ai";
import { autoRun } from "@/ai/controller";
import { editorStore } from "@/editor/editor";
import { tauriPickerApi, tauriRecordingApi, tauriSettingsApi } from "@/lib/ipc";
import type { RecordingSaved } from "@/lib/types";
import { createStartFlow } from "@/recording/startFlow";
import { createRecordingStore, type RecordingState } from "@/recording/store";
import { settingsStore } from "@/settings/settings";

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/** What to say about a saved recording: left-out steps, and text found sensitive on this Mac. */
export function savedNotice({
  skipped,
  suggestedBlurs,
  appliedBlurs,
  unscannedSteps,
}: Omit<RecordingSaved, "guideId" | "steps" | "narratedSteps">): string | null {
  const parts: string[] = [];
  if (skipped) {
    parts.push(`${plural(skipped, "step", "steps")} had no screenshot and ${skipped === 1 ? "was" : "were"} left out.`);
  }
  if (suggestedBlurs) {
    parts.push(`${plural(suggestedBlurs, "area looks", "areas look")} sensitive: review the suggested blurs.`);
  }
  if (appliedBlurs) parts.push(`${plural(appliedBlurs, "sensitive area was", "sensitive areas were")} blurred.`);
  if (unscannedSteps) {
    parts.push(
      `${plural(unscannedSteps, "step", "steps")} couldn't be scanned for sensitive text; check ${unscannedSteps === 1 ? "it" : "them"} before sharing.`,
    );
  }
  return parts.length ? parts.join(" ") : null;
}

/**
 * Opens a saved recording in the editor (after the leave flow) and refreshes
 * the library; then auto-runs AI writing if the settings ask for it.
 */
async function openSaved(saved: RecordingSaved) {
  const { guideId } = saved;
  const editor = editorStore.getState();
  void editor.loadGuides();
  // Not opened if the open guide's changes could not be saved (the leave
  // dialog shows) or the editor is busy; the new guide is in the library.
  if (!(await editor.openGuide(guideId))) return;
  // After opening, which clears the notice.
  const notice = savedNotice(saved);
  if (notice) editor.notify(notice);
  // Write it with AI when the settings say so and a key is set.
  await autoRun(aiStore, guideId, settingsStore.getState().settings, tauriSettingsApi.apiKeyStored);
}

/** The app's recording store. */
export const recordingStore = createRecordingStore({
  api: tauriRecordingApi,
  onSaved: (saved) => void openSaved(saved),
});

export function useRecording<T>(selector: (state: RecordingState) => T): T {
  return useStore(recordingStore, selector);
}

/** Starting a recording as the capture area says (the New Guide dialog, `recording://pick-area`). */
export const startFlow = createStartFlow({
  recording: recordingStore,
  picker: tauriPickerApi,
  captureArea: () => settingsStore.getState().settings?.recording.captureArea ?? null,
});
