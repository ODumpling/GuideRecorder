import { createFileRoute } from "@tanstack/react-router";
import { db } from "@/db";
import { authorize, jsonError, readJson, withUrl } from "@/lib/api";
import { createShareLink, createShareRequest, listShareLinks } from "@/lib/sharing";

export const Route = createFileRoute("/api/workspaces/$workspaceId/shares/")({
  server: {
    handlers: {
      /** A guide's share links (`?guideId=`), with the workspace's sharing settings. */
      GET: async ({ request, params: { workspaceId } }) => {
        const access = await authorize(request, workspaceId, { guide: ["share"] }, "sharing");
        if ("error" in access) return access.error;
        const guideId = new URL(request.url).searchParams.get("guideId");
        if (!guideId) return jsonError(400, "guideId is required.");
        const result = await listShareLinks(db(), workspaceId, access, guideId);
        if (result.status !== 200) return jsonError(result.status, result.error);
        return Response.json({ links: result.links.map(withUrl), settings: result.settings });
      },
      /** Makes a share link. */
      POST: async ({ request, params: { workspaceId } }) => {
        const access = await authorize(request, workspaceId, { guide: ["share"] }, "sharing");
        if ("error" in access) return access.error;
        const body = await readJson(request, createShareRequest);
        if ("error" in body) return body.error;
        const result = await createShareLink(db(), workspaceId, access, body.data);
        if (result.status !== 200) return jsonError(result.status, result.error);
        return Response.json({ link: withUrl(result.link) });
      },
    },
  },
});
