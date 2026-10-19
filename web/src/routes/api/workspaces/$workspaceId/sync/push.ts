import { createFileRoute } from "@tanstack/react-router";
import { pushRequest } from "@guiderecorder/sync";
import { db } from "@/db";
import { authorize, readJson } from "@/lib/api";
import { push } from "@/lib/sync";

/** Adds a revision on top of `baseRevision`; 409 with the head if that's stale. */
export const Route = createFileRoute("/api/workspaces/$workspaceId/sync/push")({
  server: {
    handlers: {
      POST: async ({ request, params: { workspaceId } }) => {
        const access = await authorize(request, workspaceId, { guide: ["read"] }, "sync");
        if ("error" in access) return access.error;
        const body = await readJson(request, pushRequest);
        if ("error" in body) return body.error;
        const source = request.headers.get("x-guiderecorder-client") === "web" ? "web" : "app";
        const { status, ...result } = await push(db(), workspaceId, access, body.data, source);
        return Response.json(result, { status });
      },
    },
  },
});
