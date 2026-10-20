import { createFileRoute } from "@tanstack/react-router";
import { db } from "@/db";
import { authorize, jsonError } from "@/lib/api";
import { pruneAudit } from "@/lib/audit";
import { changes, purgeTrash } from "@/lib/sync";

const HOUR = 60 * 60 * 1000;
let lastPurge = 0;

/** Guides changed after `since` (a cursor from the last response; 0 for everything). */
export const Route = createFileRoute("/api/workspaces/$workspaceId/sync/changes")({
  server: {
    handlers: {
      GET: async ({ request, params: { workspaceId } }) => {
        const access = await authorize(request, workspaceId, { guide: ["read"] }, "sync");
        if ("error" in access) return access.error;
        const since = new URL(request.url).searchParams.get("since") ?? "0";
        if (!/^\d{1,19}$/.test(since)) return jsonError(400, "since must be a cursor from a previous response.");

        // Empty the trash now and then; tombstones stay, so no client misses a
        // delete. Audit events past their workspace's retention go too.
        if (Date.now() - lastPurge > HOUR) {
          lastPurge = Date.now();
          await purgeTrash(db());
          await pruneAudit(db());
        }
        return Response.json(await changes(db(), workspaceId, access, BigInt(since)));
      },
    },
  },
});
