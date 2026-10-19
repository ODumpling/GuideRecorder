import { createFileRoute } from "@tanstack/react-router";
import { db } from "@/db";
import { authorize } from "@/lib/api";
import { restoreGuide } from "@/lib/sync";

/** Takes the guide out of the trash. */
export const Route = createFileRoute("/api/workspaces/$workspaceId/sync/guides/$guideId/restore")({
  server: {
    handlers: {
      POST: async ({ request, params: { workspaceId, guideId } }) => {
        const access = await authorize(request, workspaceId, { guide: ["read"] }, "sync");
        if ("error" in access) return access.error;
        const { status, ...result } = await restoreGuide(db(), workspaceId, access, guideId);
        return status === 200 ? new Response(null, { status: 204 }) : Response.json(result, { status });
      },
    },
  },
});
