// Syncing the phone's guides with the web, with the desktop app's sync loop
// (`@guiderecorder/sync/engine`). Only guides the user turns sync on for go
// up; any guide in the workspace can be downloaded, and a downloaded guide
// stays synced. Edits, merges and conflicts work as on the desktop.

import AsyncStorage from "@react-native-async-storage/async-storage";
import type { Guide } from "@guiderecorder/guide-schema";
import { imagePaths, type AssetMap, type Revision, type ReviewState } from "@guiderecorder/sync";
import {
  emptyState,
  loadedState,
  resolveConflict,
  setGuideEnabled,
  setWorkspace,
  syncOnce,
  SyncStopped,
  type PreparedGuide,
  type Resolution,
  type SyncBackend,
  type SyncState,
} from "@guiderecorder/sync/engine";
import { CryptoDigestAlgorithm, digest } from "expo-crypto";
import { File } from "expo-file-system";
import { fetch as expoFetch } from "expo/fetch";
import { create } from "zustand";
import { request } from "./api";
import { imageUrl, rememberLocalImage } from "./assets";
import { deviceStore, duplicateGuide, guideIds, imageFile, readGuide, writeGuide, type LocalImage } from "./device";

const STATE_KEY = "guiderecorder.sync";

function contentType(path: string): string {
  if (/\.png$/i.test(path)) return "image/png";
  if (/\.webp$/i.test(path)) return "image/webp";
  return "image/jpeg";
}

function hex(buffer: ArrayBuffer): string {
  return Array.from(new Uint8Array(buffer), (b) => b.toString(16).padStart(2, "0")).join("");
}

/** Hashes by file, size and time, so unchanged images aren't read again every run. */
const hashes = new Map<string, string>();

async function hashFile(file: File): Promise<{ sha256: string; size: number }> {
  const info = file.info();
  const key = `${file.uri}:${info.size}:${info.modificationTime}`;
  let sha256 = hashes.get(key);
  if (!sha256) {
    sha256 = hex(await digest(CryptoDigestAlgorithm.SHA256, await file.bytes()));
    hashes.set(key, sha256);
  }
  rememberLocalImage(sha256, file.uri);
  return { sha256, size: info.size ?? 0 };
}

/** Prepared images by hash, for `upload`. */
const prepared = new Map<string, string>();

/** The sync loop's view of the phone. */
export const phoneBackend: SyncBackend = {
  request: (method, path, body) => request(method, path, body),

  async readGuide(id) {
    const guide = readGuide(id);
    return guide ? { guide, path: id, readOnly: false } : null;
  },

  async prepareGuide(id): Promise<PreparedGuide> {
    const guide = readGuide(id);
    if (!guide) throw new Error("The guide isn't on this phone.");
    const images: PreparedGuide["images"] = [];
    for (const path of imagePaths(guide)) {
      const file = imageFile(id, path);
      if (!file.exists) throw new Error(`An image of this guide is missing (${path}).`);
      const { sha256, size } = await hashFile(file);
      prepared.set(sha256, file.uri);
      images.push({ path, sha256, size, contentType: contentType(path) });
    }
    return { guide, images };
  },

  async upload(uploads) {
    for (const upload of uploads) {
      const uri = prepared.get(upload.sha256);
      if (!uri) throw new Error(`Image ${upload.sha256} wasn't prepared.`);
      const response = await expoFetch(upload.url, { method: upload.method, headers: upload.headers, body: new File(uri) });
      if (!response.ok) throw new Error(`An image didn't upload (${response.status}).`);
    }
  },

  async applyGuide(workspaceId, guide, assets) {
    await downloadImages(workspaceId, guide.id, assets);
    writeGuide(guide);
    return guide.id;
  },

  duplicateGuide: (id, _path, title) => duplicateGuide(id, title),

  async listGuideIds() {
    return guideIds();
  },
};

/** Downloads the images the guide names that the phone doesn't have yet. */
async function downloadImages(workspaceId: string, guideId: string, assets: AssetMap) {
  for (const [path, sha256] of Object.entries(assets)) {
    const target = imageFile(guideId, path);
    if (target.exists) continue;
    const parent = target.parentDirectory;
    if (!parent.exists) parent.create({ intermediates: true, idempotent: true });
    await File.downloadFileAsync(await imageUrl(workspaceId, sha256), target);
  }
}

interface SyncStore {
  state: SyncState;
  running: boolean;
  error: string | null;
  lastSynced: number | null;
  load(): Promise<void>;
  /** Follows the signed-in workspace; sync starts over in a new one. */
  useWorkspace(workspaceId: string | null): void;
  syncNow(): Promise<void>;
  /** Syncs a moment after an edit, once edits stop coming. */
  syncSoon(): void;
  setEnabled(id: string, enabled: boolean): void;
  /** Saves a guide from the web on the phone; it stays synced. */
  download(revision: Revision, review?: ReviewState): Promise<void>;
  resolve(id: string, resolution: Resolution): Promise<void>;
}

let soon: ReturnType<typeof setTimeout> | undefined;
let running: Promise<void> | null = null;

export const syncStore = create<SyncStore>((set, get) => {
  const persist = async (state: SyncState) => {
    set({ state });
    await AsyncStorage.setItem(STATE_KEY, JSON.stringify(state)).catch(() => {});
  };

  return {
    state: emptyState(),
    running: false,
    error: null,
    lastSynced: null,

    async load() {
      const saved = await AsyncStorage.getItem(STATE_KEY).catch(() => null);
      set({ state: loadedState(saved ? JSON.parse(saved) : null) });
    },

    useWorkspace(workspaceId) {
      const next = setWorkspace(get().state, workspaceId);
      if (next !== get().state) void persist(next);
    },

    syncNow() {
      running ??= (async () => {
        if (!get().state.workspaceId) return;
        set({ running: true });
        try {
          const state = await syncOnce(phoneBackend, get().state, persist);
          await persist(state);
          set({ error: null, lastSynced: Date.now() });
        } catch (error) {
          set({ error: error instanceof SyncStopped || error instanceof Error ? error.message : String(error) });
        } finally {
          deviceStore.getState().reload();
          set({ running: false });
        }
      })().finally(() => (running = null));
      return running;
    },

    syncSoon() {
      clearTimeout(soon);
      soon = setTimeout(() => void get().syncNow(), 1500);
    },

    setEnabled(id, enabled) {
      void persist(setGuideEnabled(get().state, id, enabled));
      if (enabled) get().syncSoon();
    },

    async download(revision, review) {
      const workspaceId = get().state.workspaceId;
      if (!workspaceId) throw new Error("Choose a workspace first.");
      await phoneBackend.applyGuide(workspaceId, revision.guide, revision.assets);
      const state = structuredClone(get().state);
      state.guides[revision.guideId] = {
        enabled: true,
        revisionId: revision.id,
        // The guide as saved here, so the next run sees no change.
        base: readGuide(revision.guideId) ?? revision.guide,
        status: "synced",
        review,
      };
      await persist(state);
      deviceStore.getState().reload();
    },

    async resolve(id, resolution) {
      await persist(await resolveConflict(phoneBackend, get().state, id, resolution));
      deviceStore.getState().reload();
    },
  };
});

/** Saves an edit made here, then syncs it if the guide syncs. */
export async function saveGuide(guide: Guide, images: LocalImage[] = []): Promise<void> {
  await deviceStore.getState().save({ ...guide, updatedAt: new Date().toISOString() }, images);
  syncStore.getState().syncSoon();
}

/** Takes the guide off the phone. Its copy on the web, if any, stays. */
export function removeFromPhone(id: string): void {
  syncStore.getState().setEnabled(id, false);
  deviceStore.getState().remove(id);
}
