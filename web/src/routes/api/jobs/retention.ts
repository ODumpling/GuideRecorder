import { timingSafeEqual } from "node:crypto";
import { createFileRoute } from "@tanstack/react-router";
import { db } from "@/db";
import { jsonError, objectStore } from "@/lib/api";
import { env } from "@/lib/env";
import { runRetention } from "@/lib/retention";

/** Runs the retention job; called hourly by cron with `Authorization: Bearer $JOBS_TOKEN`. */
export const Route = createFileRoute("/api/jobs/retention")({
  server: {
    handlers: {
      POST: async ({ request }) => {
        const expected = env().JOBS_TOKEN;
        if (!expected) return jsonError(404, "Not found.");
        const given = request.headers.get("authorization")?.replace(/^Bearer\s+/i, "") ?? "";
        const a = Buffer.from(given);
        const b = Buffer.from(expected);
        if (a.length !== b.length || !timingSafeEqual(a, b)) return jsonError(401, "Unauthorized.");
        return Response.json(await runRetention(db(), objectStore()));
      },
    },
  },
});
