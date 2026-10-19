import { createFileRoute } from "@tanstack/react-router";
import { db } from "@/db";
import { authorize, jsonError } from "@/lib/api";
import { getGuide, trashGuide } from "@/lib/sync";

export const Route = createFileRoute("/api/workspaces/$workspaceId/sync/guides/$guideId/")({
  server: {
    handlers: {
      /** The guide's head revision. */
      GET: async ({ request, params: { workspaceId, guideId } }) => {
        const access = await authorize(request, workspaceId, { guide: ["read"] }, "sync");
        if ("error" in access) return access.error;
        const found = await getGuide(db(), workspaceId, access.userId, guideId);
        if (!found) return jsonError(404, "Guide not found.");
        return Response.json({ revision: found.head, deletedAt: found.guide.deletedAt });
      },
      /** Moves the guide to the trash for 30 days. */
      DELETE: async ({ request, params: { workspaceId, guideId } }) => {
        const access = await authorize(request, workspaceId, { guide: ["read"] }, "sync");
        if ("error" in access) return access.error;
        const { status, ...result } = await trashGuide(db(), workspaceId, access, guideId);
        return status === 200 ? new Response(null, { status: 204 }) : Response.json(result, { status });
      },
    },
  },
});
