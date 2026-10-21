import { createStore } from "zustand/vanilla";
import { useStore } from "zustand";
import type { LibraryInfo, Permission, PermissionStatus, Settings } from "@/lib/types";
import { isWindows } from "@/lib/platform";

export type SetupStep = "permissions" | "library" | "apiKey";
/** Windows asks for no permissions to record, so setup starts at the library there. */
export const STEPS: SetupStep[] = isWindows() ? ["library", "apiKey"] : ["permissions", "library", "apiKey"];

/**
 * Whether first-run setup shows at launch, and at which step: on first
 * launch (setup never finished), or when the library could not be opened.
 */
export function setupAtLaunch(settings: Settings | null, library: LibraryInfo | null): SetupStep | null {
  if (library?.error) return "library";
  if (settings && !settings.general.onboarded) return STEPS[0];
  return null;
}

/** Recording may start only with every permission granted. */
export function allGranted(rows: { permission: Permission; status: PermissionStatus }[] | null): boolean {
  return !!rows && rows.length > 0 && rows.every((r) => r.status === "granted");
}

/** Whether setup is open, and where. Settings can reopen it. */
export const setupStore = createStore<{ step: SetupStep | null; open(step?: SetupStep): void; close(): void }>()(
  (set) => ({
    step: null,
    open: (step = STEPS[0]) => set({ step }),
    close: () => set({ step: null }),
  }),
);

export function useSetupStep(): SetupStep | null {
  return useStore(setupStore, (s) => s.step);
}

/**
 * Decides at launch whether to show setup: waits for the library to finish
 * opening (a few seconds at most), then applies {@link setupAtLaunch}.
 */
export async function setupAtStartup(
  libraryInfo: () => Promise<LibraryInfo>,
  settings: () => Settings | null,
  { tries = 50, delayMs = 200 }: { tries?: number; delayMs?: number } = {},
): Promise<SetupStep | null> {
  let library: LibraryInfo | null = null;
  for (let i = 0; i < tries; i++) {
    library = await libraryInfo().catch(() => null);
    if (!library?.starting) break;
    await new Promise((resolve) => setTimeout(resolve, delayMs));
  }
  return setupAtLaunch(settings(), library);
}
