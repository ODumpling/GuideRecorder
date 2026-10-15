import { useStore } from "zustand";
import { editorStore } from "@/editor/editor";
import { tauriPickerApi, tauriRecordingApi } from "@/lib/ipc";
import type { RecordingSaved } from "@/lib/types";
import { createStartFlow } from "@/recording/startFlow";
import { createRecordingStore, type RecordingState } from "@/recording/store";
import { settingsStore } from "@/settings/settings";

/** Opens a saved recording in the editor (after the leave flow) and refreshes the library. */
async function openSaved({ guideId, skipped }: RecordingSaved) {
  const editor = editorStore.getState();
  void editor.loadGuides();
  // Not opened if the open guide's changes could not be saved (the leave
  // dialog shows) or the editor is busy; the new guide is in the library.
  if (!(await editor.openGuide(guideId))) return;
  // After opening, which clears the notice.
  if (skipped === 1) editor.notify("1 step had no screenshot and was left out.");
  else if (skipped > 1) editor.notify(`${skipped} steps had no screenshot and were left out.`);
}

/** The app's recording store. */
export const recordingStore = createRecordingStore({
  api: tauriRecordingApi,
  onSaved: (saved) => void openSaved(saved),
});

export function useRecording<T>(selector: (state: RecordingState) => T): T {
  return useStore(recordingStore, selector);
}

/** Starting a recording as the capture area says (the Record button, `recording://pick-area`). */
export const startFlow = createStartFlow({
  recording: recordingStore,
  picker: tauriPickerApi,
  captureArea: () => settingsStore.getState().settings?.recording.captureArea ?? null,
});
