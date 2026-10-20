// Showing a workspace's images: short-lived download URLs from the web app,
// or the file on the phone when it's there.

import { call } from "./api";

/** Images on this phone, by hash: shown from the device, not downloaded again. */
const localFiles = new Map<string, string>();

export function rememberLocalImage(sha256: string, uri: string): void {
  localFiles.set(sha256, uri);
}

const urls = new Map<string, { url: string; expires: number }>();
const inFlight = new Map<string, Promise<string>>();

/** Where to load an image from: the phone if it's there, else a signed download URL. */
export function imageUrl(workspaceId: string, sha256: string): Promise<string> {
  const local = localFiles.get(sha256);
  if (local) return Promise.resolve(local);
  const key = `${workspaceId}/${sha256}`;
  const cached = urls.get(key);
  if (cached && cached.expires > Date.now()) return Promise.resolve(cached.url);
  let pending = inFlight.get(key);
  if (!pending) {
    pending = call<{ url: string; expiresIn: number }>("GET", `/api/workspaces/${workspaceId}/assets/${sha256}`)
      .then(({ url, expiresIn }) => {
        // Refreshed a minute early, so a URL never runs out while loading.
        urls.set(key, { url, expires: Date.now() + (expiresIn - 60) * 1000 });
        return url;
      })
      .finally(() => inFlight.delete(key));
    inFlight.set(key, pending);
  }
  return pending;
}
