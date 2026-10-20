// Fixed-window rate limits for the public share pages, in memory. Each
// server process counts on its own, so behind N instances a client gets up
// to N times the limit; that's enough to stop scraping and password
// guessing without a shared store.

export interface RateLimit {
  /** Counts a hit for `key`; false once the window's limit is used up. */
  hit(key: string, now?: number): boolean;
}

export function rateLimit({ limit, windowMs, maxKeys = 10_000 }: { limit: number; windowMs: number; maxKeys?: number }): RateLimit {
  const windows = new Map<string, { start: number; count: number }>();
  return {
    hit(key, now = Date.now()) {
      let w = windows.get(key);
      if (!w || now - w.start >= windowMs) {
        // Forget finished windows before the map grows without bound.
        if (windows.size >= maxKeys) {
          for (const [k, v] of windows) if (now - v.start >= windowMs) windows.delete(k);
          if (windows.size >= maxKeys) windows.delete(windows.keys().next().value!);
        }
        w = { start: now, count: 0 };
        windows.set(key, w);
      }
      w.count += 1;
      return w.count <= limit;
    },
  };
}

/**
 * The client's address: the first `X-Forwarded-For` hop (the app runs behind
 * a proxy that sets it), else a shared bucket.
 */
export function clientIp(headers: Headers): string {
  return headers.get("x-forwarded-for")?.split(",")[0]?.trim() || headers.get("x-real-ip") || "unknown";
}

/** Opening share pages: per address. */
export const shareViews = rateLimit({ limit: 60, windowMs: 60_000 });
/** Share images: a page loads one per step. */
export const shareAssets = rateLimit({ limit: 1200, windowMs: 60_000 });
/** Password attempts: per address and link. */
export const sharePasswords = rateLimit({ limit: 10, windowMs: 15 * 60_000 });
