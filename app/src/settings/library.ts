import { editorStore } from "@/editor/editor";
import type { SettingsApi } from "@/lib/ipc";
import { settingsStore } from "@/settings/settings";

/**
 * Opens another library (null: the default folder). The open guide is
 * saved and closed first; if that save fails the leave dialog asks and
 * nothing switches (resolves to false). Then settings and the guide list
 * are reloaded. Rejects with the switch error.
 */
export async function switchLibrary(api: SettingsApi, path: string | null): Promise<boolean> {
  if (editorStore.getState().doc && !(await editorStore.getState().closeGuide())) return false;
  await api.switchLibrary(path);
  await settingsStore.getState().load();
  await editorStore.getState().loadGuides();
  return true;
}
