// The workspace's guides on the web, followed through the sync change feed
// (`changes?since=<cursor>`) and cached on the device so the list opens
// offline. Read-only: guides are edited on the phone once downloaded
// (device.ts) and synced by sync.ts.

import AsyncStorage from "@react-native-async-storage/async-storage";
import type { ChangesResponse } from "@guiderecorder/sync";
import { create } from "zustand";
import { ApiError, refusal, request } from "./api";
import { applyChanges, type Entry } from "./revisions";

export { sortedGuides, type Entry } from "./revisions";

interface Cache {
  version: 1;
  cursor: string;
  guides: Record<string, Entry>;
}

interface WebStore {
  workspaceId: string | null;
  cursor: string;
  guides: Record<string, Entry>;
  refreshing: boolean;
  error: string | null;
  open(workspaceId: string | null): Promise<void>;
  refresh(): Promise<void>;
}

const cacheKey = (workspaceId: string) => `guiderecorder.library.${workspaceId}`;

let refreshing: Promise<void> | null = null;

export const webStore = create<WebStore>((set, get) => ({
  workspaceId: null,
  cursor: "0",
  guides: {},
  refreshing: false,
  error: null,

  async open(workspaceId) {
    if (get().workspaceId === workspaceId) return;
    set({ workspaceId, cursor: "0", guides: {}, error: null });
    if (!workspaceId) return;
    const saved = await AsyncStorage.getItem(cacheKey(workspaceId)).catch(() => null);
    if (saved && get().workspaceId === workspaceId) {
      try {
        const cache = JSON.parse(saved) as Cache;
        if (cache.version === 1) set({ cursor: cache.cursor, guides: cache.guides });
      } catch {
        // A broken cache: start over from the feed.
      }
    }
    await get().refresh();
  },

  refresh() {
    refreshing ??= (async () => {
      const workspaceId = get().workspaceId;
      if (!workspaceId) return;
      set({ refreshing: true });
      try {
        for (;;) {
          const response = await request<ChangesResponse>("GET", `/api/workspaces/${workspaceId}/sync/changes?since=${get().cursor}`);
          if (response.status !== 200) throw new ApiError(refusal(response), response.status);
          if (get().workspaceId !== workspaceId) return;
          set({ guides: applyChanges(get().guides, response.body.changes), cursor: response.body.cursor });
          if (!response.body.hasMore) break;
        }
        const cache: Cache = { version: 1, cursor: get().cursor, guides: get().guides };
        await AsyncStorage.setItem(cacheKey(workspaceId), JSON.stringify(cache)).catch(() => {});
        set({ error: null });
      } catch (error) {
        set({ error: error instanceof Error ? error.message : String(error) });
      } finally {
        set({ refreshing: false });
      }
    })().finally(() => (refreshing = null));
    return refreshing;
  }
}));
