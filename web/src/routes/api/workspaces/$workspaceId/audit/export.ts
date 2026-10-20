import { createFileRoute } from "@tanstack/react-router";
import { db } from "@/db";
import { authorize, jsonError } from "@/lib/api";
import { auditFilters, listAudit, parseFilters, recordAudit, toCsv, toJson } from "@/lib/audit";

/** The most events one export holds; narrow the filters for more. */
const MAX_EVENTS = 50_000;

/** The audit log as CSV or JSON (`?format=csv|json`), with the audit page's filters. */
export const Route = createFileRoute("/api/workspaces/$workspaceId/audit/export")({
  server: {
    handlers: {
      GET: async ({ request, params: { workspaceId } }) => {
        const access = await authorize(request, workspaceId, { audit: ["read"] }, "audit");
        if ("error" in access) return access.error;
        const params = Object.fromEntries(new URL(request.url).searchParams);
        const format = params.format === "json" ? "json" : "csv";
        const parsed = auditFilters.safeParse(params);
        if (!parsed.success) return jsonError(400, "Invalid filters.", { issues: parsed.error.issues });

        const events = await listAudit(db(), workspaceId, { ...parseFilters(parsed.data), limit: MAX_EVENTS });
        // Exporting the log is itself logged (after reading, so it isn't in this export).
        await recordAudit(db(), workspaceId, {
          actorId: access.userId,
          action: "audit.export",
          targetType: "workspace",
          targetId: workspaceId,
          metadata: { format, count: events.length, filters: parsed.data },
          ip: access.ip,
          userAgent: access.userAgent,
        });
        const name = `audit-${workspaceId}-${new Date().toISOString().slice(0, 10)}.${format}`;
        const headers = { "content-disposition": `attachment; filename="${name}"`, "cache-control": "no-store" };
        if (format === "json") return Response.json(toJson(events), { headers });
        return new Response(toCsv(events), { headers: { ...headers, "content-type": "text/csv; charset=utf-8" } });
      },
    },
  },
});
