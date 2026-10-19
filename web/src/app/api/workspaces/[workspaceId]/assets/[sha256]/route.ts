import { db } from "@/db";
import { authorize, jsonError, objectStore } from "@/lib/api";
import { downloadUrl } from "@/lib/assets";

/** Redirects to a short-lived download URL for the image. */
export async function GET(request: Request, ctx: RouteContext<"/api/workspaces/[workspaceId]/assets/[sha256]">) {
  const { workspaceId, sha256 } = await ctx.params;
  const access = await authorize(request, workspaceId, { guide: ["read"] });
  if ("error" in access) return access.error;
  const url = await downloadUrl(db(), objectStore(), workspaceId, sha256);
  if (!url) return jsonError(404, "Asset not found.");
  return Response.redirect(url, 302);
}
