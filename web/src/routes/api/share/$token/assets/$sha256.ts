import { createFileRoute } from "@tanstack/react-router";
import { db } from "@/db";
import { jsonError, objectStore } from "@/lib/api";
import { downloadUrl } from "@/lib/assets";
import { env } from "@/lib/env";
import { clientIp, shareAssets } from "@/lib/rate-limit";
import { cookieReader, shareAsset } from "@/lib/sharing";

const NOINDEX = { "X-Robots-Tag": "noindex, nofollow", "Cache-Control": "private, no-store" };

/** An image of a shared guide: redirects to a short-lived download URL. No session; the token decides. */
export const Route = createFileRoute("/api/share/$token/assets/$sha256")({
  server: {
    handlers: {
      GET: async ({ request, params: { token, sha256 } }) => {
        if (!shareAssets.hit(clientIp(request.headers))) return jsonError(429, "Too many requests. Try again in a minute.");
        const found = await shareAsset(db(), env().BETTER_AUTH_SECRET, token, sha256, cookieReader(request.headers.get("cookie")));
        if (found.status !== 200) return Response.json({ error: found.error }, { status: found.status, headers: NOINDEX });
        const url = await downloadUrl(db(), objectStore(), found.organizationId, sha256);
        if (!url) return jsonError(404, "Image not found.");
        return new Response(null, { status: 302, headers: { Location: url, ...NOINDEX } });
      },
    },
  },
});
