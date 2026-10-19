import { db } from "@/db";
import { authorize, jsonError, objectStore, readJson } from "@/lib/api";
import { planUploads, QuotaExceededError, uploadRequest } from "@/lib/assets";

/** Which images the workspace still needs, with presigned PUTs for them. */
export async function POST(request: Request, ctx: RouteContext<"/api/workspaces/[workspaceId]/assets/uploads">) {
  const { workspaceId } = await ctx.params;
  const access = await authorize(request, workspaceId, { guide: ["create"] }, "sync");
  if ("error" in access) return access.error;
  const body = await readJson(request, uploadRequest);
  if ("error" in body) return body.error;
  try {
    return Response.json(
      await planUploads(db(), objectStore(), workspaceId, body.data, access.plan.entitlements.storageBytes),
    );
  } catch (error) {
    if (error instanceof QuotaExceededError) return jsonError(413, error.message);
    throw error;
  }
}
