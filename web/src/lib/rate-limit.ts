// Fixed-window rate limits in memory. Each server process counts on its own,
// so behind N instances a client gets up to N times the limit; that's enough
// to stop runaway clients and scraping without a shared store. Better Auth's
// endpoints (sign-in and the like) have their own limits, kept in Postgres.

export interface RateLimit {
  /** Counts a hit for `key`; false once the window's limit is used up. */
  hit(key: string, now?: number): boolean;
  /** Seconds until `key`'s window resets. */
  retryAfter(key: string, now?: number): number;
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
    retryAfter(key, now = Date.now()) {
      const w = windows.get(key);
      return w ? Math.max(1, Math.ceil((w.start + windowMs - now) / 1000)) : 0;
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

/**
 * Who is calling: a hash of their bearer token or session cookie, so one
 * user's limit doesn't depend on their network; else their address.
 */
export async function clientKey(headers: Headers): Promise<string> {
  const bearer = headers.get("authorization")?.match(/^Bearer\s+(.+)$/i)?.[1];
  const cookie = headers.get("cookie")?.match(/(?:^|;\s*)(?:__Secure-)?better-auth\.session_token=([^;]+)/)?.[1];
  const credential = bearer ?? cookie;
  if (!credential) return `ip:${clientIp(headers)}`;
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(credential));
  return `u:${Array.from(new Uint8Array(digest).slice(0, 16), (b) => b.toString(16).padStart(2, "0")).join("")}`;
}

/** A path prefix's limit; the first matching rule applies. */
interface ApiRule {
  name: string;
  match: (pathname: string, method: string) => boolean;
  limit: RateLimit;
}

const MINUTE = 60_000;

/**
 * Limits for the workspace API (`/api/workspaces/...`). Generous for the
 * desktop app's sync; tighter where each call does real work.
 */
export function apiRules(): ApiRule[] {
  return [
    { name: "uploads", match: (p, m) => m === "POST" && p.endsWith("/assets/uploads"), limit: rateLimit({ limit: 120, windowMs: MINUTE }) },
    { name: "push", match: (p, m) => m === "POST" && p.endsWith("/sync/push"), limit: rateLimit({ limit: 240, windowMs: MINUTE }) },
    { name: "writes", match: (_p, m) => m !== "GET" && m !== "HEAD", limit: rateLimit({ limit: 300, windowMs: MINUTE }) },
    { name: "reads", match: () => true, limit: rateLimit({ limit: 1200, windowMs: MINUTE }) },
  ];
}

/** Paths with limits of their own (Better Auth) or none (health checks). */
const UNLIMITED = ["/api/auth/", "/api/health"];

/**
 * Checks an API request against `rules`; a 429 response once a limit is
 * used up, else null. Only `/api/` paths count.
 */
export async function limitApiRequest(request: Request, rules: ApiRule[], now = Date.now()): Promise<Response | null> {
  const { pathname } = new URL(request.url);
  if (!pathname.startsWith("/api/") || UNLIMITED.some((p) => pathname.startsWith(p))) return null;
  const rule = rules.find((r) => r.match(pathname, request.method));
  if (!rule) return null;
  const key = await clientKey(request.headers);
  if (rule.limit.hit(key, now)) return null;
  const retryAfter = rule.limit.retryAfter(key, now);
  return Response.json(
    { error: "Too many requests. Try again shortly.", retryAfter },
    { status: 429, headers: { "retry-after": String(retryAfter) } },
  );
}
