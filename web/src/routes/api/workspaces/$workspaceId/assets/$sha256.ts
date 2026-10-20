import { createFileRoute } from "@tanstack/react-router";
import { db } from "@/db";
import { authorize, jsonError, objectStore } from "@/lib/api";
import { canDownload, DOWNLOAD_TTL, downloadUrl } from "@/lib/assets";

/**
 * Redirects to a short-lived download URL for the image, if the user may
 * see it. The browser may reuse the redirect until shortly before the URL
 * expires, so a page showing an image twice downloads it once.
 */
export const Route = createFileRoute("/api/workspaces/$workspaceId/assets/$sha256")({
  server: {
    handlers: {
      GET: async ({ request, params: { workspaceId, sha256 } }) => {
        const access = await authorize(request, workspaceId, { guide: ["read"] });
        if ("error" in access) return access.error;
        const allowed = await canDownload(db(), workspaceId, access.userId, sha256);
        const url = allowed ? await downloadUrl(db(), objectStore(), workspaceId, sha256) : null;
        if (!url) return jsonError(404, "Asset not found.");
        return new Response(null, {
          status: 302,
          headers: { location: url, "cache-control": `private, max-age=${DOWNLOAD_TTL - 60}` },
        });
      },
    },
  },
});
