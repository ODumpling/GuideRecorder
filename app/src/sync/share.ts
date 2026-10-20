import { createStore, type StoreApi } from "zustand/vanilla";
import { errorMessage } from "@/lib/ipc";
import type { ApiResponse, SyncBackend } from "@/sync/engine";

// Share links from the desktop app: the web app's share API (`/api/
// workspaces/:id/shares`), called with the sync sign-in. Only guides the
// server has (synced ones) can be shared.

export interface ShareLink {
  id: string;
  url: string;
  /** Null: always the latest version. */
  revisionId: string | null;
  expiresAt: string | null;
  hasPassword: boolean;
  allowDownload: boolean;
  views: number;
  lastViewedAt: string | null;
  createdAt: string;
  createdBy: string | null;
  /** `blocked`: the workspace's settings stop it working. */
  state: "active" | "expired" | "blocked";
}

export interface SharingSettings {
  shareLinks: "anyone" | "password" | "off";
  shareLinkMaxDays: number | null;
}

export interface NewShareLink {
  revisionId: string | null;
  expiresAt: string | null;
  password: string | null;
  allowDownload: boolean;
}

type Request = Pick<SyncBackend, "request">;

function refused(response: ApiResponse): Error {
  if (response.status === 401) return new Error("You were signed out. Sign in again in Settings → Sync.");
  if (response.status === 402) return new Error("Share links come with Pro. Upgrade the workspace on the web.");
  const body = response.body as { error?: string } | null;
  return new Error(body?.error ?? `The server answered ${response.status}.`);
}

export async function listShareLinks(backend: Request, workspaceId: string, guideId: string) {
  const response = await backend.request<{ links: ShareLink[]; settings: SharingSettings }>(
    "GET",
    `/api/workspaces/${encodeURIComponent(workspaceId)}/shares?guideId=${encodeURIComponent(guideId)}`,
  );
  if (response.status !== 200) throw refused(response);
  return response.body;
}

export async function createShareLink(backend: Request, workspaceId: string, guideId: string, link: NewShareLink): Promise<ShareLink> {
  const response = await backend.request<{ link: ShareLink }>("POST", `/api/workspaces/${encodeURIComponent(workspaceId)}/shares`, {
    guideId,
    ...link,
  });
  if (response.status !== 200) throw refused(response);
  return response.body.link;
}

export async function revokeShareLink(backend: Request, workspaceId: string, linkId: string): Promise<void> {
  const response = await backend.request("DELETE", `/api/workspaces/${encodeURIComponent(workspaceId)}/shares/${encodeURIComponent(linkId)}`);
  if (response.status !== 204 && response.status !== 200) throw refused(response);
}

export interface ShareStoreState {
  /** The guide whose share dialog is open. */
  guideId: string | null;
  /** The synced revision to pin a link to. */
  revisionId: string | null;
  links: ShareLink[];
  settings: SharingSettings | null;
  loading: boolean;
  busy: boolean;
  error: string | null;

  open(guideId: string, revisionId: string | null): Promise<void>;
  close(): void;
  /** Makes a link; returns it so it can be copied. */
  create(link: NewShareLink): Promise<ShareLink | null>;
  revoke(linkId: string): Promise<void>;
}

export type ShareStore = StoreApi<ShareStoreState>;

export function createShareStore({ backend, workspaceId }: { backend: Request; workspaceId: () => string | null }): ShareStore {
  return createStore<ShareStoreState>()((set, get) => {
    /** Runs `action` for the open guide in the current workspace; false if refused. */
    async function run(action: (workspace: string, guide: string) => Promise<void>, flag: "loading" | "busy"): Promise<boolean> {
      const workspace = workspaceId();
      const guide = get().guideId;
      if (!workspace || !guide) return false;
      set({ [flag]: true, error: null });
      try {
        await action(workspace, guide);
        return true;
      } catch (error) {
        if (get().guideId === guide) set({ error: errorMessage(error) });
        return false;
      } finally {
        set({ [flag]: false });
      }
    }

    async function reload(workspace: string, guide: string) {
      const { links, settings } = await listShareLinks(backend, workspace, guide);
      if (get().guideId === guide) set({ links, settings });
    }

    return {
      guideId: null,
      revisionId: null,
      links: [],
      settings: null,
      loading: false,
      busy: false,
      error: null,

      async open(guideId, revisionId) {
        set({ guideId, revisionId, links: [], settings: null, error: null });
        await run(reload, "loading");
      },

      close() {
        set({ guideId: null, revisionId: null, links: [], settings: null, error: null });
      },

      async create(link) {
        let created: ShareLink | null = null;
        await run(async (workspace, guide) => {
          created = await createShareLink(backend, workspace, guide, link);
          await reload(workspace, guide);
        }, "busy");
        return created;
      },

      async revoke(linkId) {
        await run(async (workspace, guide) => {
          await revokeShareLink(backend, workspace, linkId);
          await reload(workspace, guide);
        }, "busy");
      },
    };
  });
}
