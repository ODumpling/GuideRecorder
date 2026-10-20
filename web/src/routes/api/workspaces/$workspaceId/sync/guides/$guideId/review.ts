import { createFileRoute } from "@tanstack/react-router";
import { submitReviewRequest } from "@guiderecorder/sync";
import { db } from "@/db";
import { authorize, jsonError, readJson } from "@/lib/api";
import { reviewStateOf, submitForReview } from "@/lib/approvals";

export const Route = createFileRoute("/api/workspaces/$workspaceId/sync/guides/$guideId/review")({
  server: {
    handlers: {
      /** Where the guide stands in review. */
      GET: async ({ request, params: { workspaceId, guideId } }) => {
        const access = await authorize(request, workspaceId, { guide: ["read"] }, "sync");
        if ("error" in access) return access.error;
        const review = await reviewStateOf(db(), workspaceId, access, guideId);
        if (!review) return jsonError(404, "Guide not found.");
        return Response.json({ review });
      },
      /** Submits the head for review (approval-gated guides; Enterprise). */
      POST: async ({ request, params: { workspaceId, guideId } }) => {
        const access = await authorize(request, workspaceId, { guide: ["update"] }, "approvals");
        if ("error" in access) return access.error;
        const body = await readJson(request, submitReviewRequest);
        if ("error" in body) return body.error;
        const { status, ...result } = await submitForReview(db(), workspaceId, access, guideId, body.data);
        return Response.json(result, { status });
      },
    },
  },
});
