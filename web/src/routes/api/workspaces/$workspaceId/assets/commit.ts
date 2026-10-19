import { createFileRoute } from "@tanstack/react-router";
import { db } from "@/db";
import { authorize, objectStore, readJson } from "@/lib/api";
import { commitRequest, commitUploads } from "@/lib/assets";

/** Records uploaded images; reports any not found in storage. */
export const Route = createFileRoute("/api/workspaces/$workspaceId/assets/commit")({
  server: {
    handlers: {
      POST: async ({ request, params: { workspaceId } }) => {
        const access = await authorize(request, workspaceId, { guide: ["create"] }, "sync");
        if ("error" in access) return access.error;
        const body = await readJson(request, commitRequest);
        if ("error" in body) return body.error;
        return Response.json(await commitUploads(db(), objectStore(), workspaceId, access.userId, body.data.sha256));
      },
    },
  },
});
