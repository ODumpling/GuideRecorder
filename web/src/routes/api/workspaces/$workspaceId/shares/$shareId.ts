import { createFileRoute } from "@tanstack/react-router";
import { db } from "@/db";
import { authorize } from "@/lib/api";
import { revokeShareLink } from "@/lib/sharing";

export const Route = createFileRoute("/api/workspaces/$workspaceId/shares/$shareId")({
  server: {
    handlers: {
      /** Revokes the link; it stops working at once. */
      DELETE: async ({ request, params: { workspaceId, shareId } }) => {
        // No entitlement check: a workspace that lost sharing can still revoke.
        const access = await authorize(request, workspaceId, { guide: ["share"] });
        if ("error" in access) return access.error;
        const { status, ...result } = await revokeShareLink(db(), workspaceId, access, shareId);
        return status === 200 ? new Response(null, { status: 204 }) : Response.json(result, { status });
      },
    },
  },
});
