import { db } from "@/db";
import { authorize, objectStore, readJson } from "@/lib/api";
import { commitRequest, commitUploads } from "@/lib/assets";

/** Records uploaded images; reports any not found in storage. */
export async function POST(request: Request, ctx: RouteContext<"/api/workspaces/[workspaceId]/assets/commit">) {
  const { workspaceId } = await ctx.params;
  const access = await authorize(request, workspaceId, { guide: ["create"] }, "sync");
  if ("error" in access) return access.error;
  const body = await readJson(request, commitRequest);
  if ("error" in body) return body.error;
  return Response.json(await commitUploads(db(), objectStore(), workspaceId, access.userId, body.data.sha256));
}
