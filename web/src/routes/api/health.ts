import { createFileRoute } from "@tanstack/react-router";
import { sql } from "drizzle-orm";
import { db } from "@/db";

export const Route = createFileRoute("/api/health")({
  server: {
    handlers: {
      GET: async () => {
        await db().execute(sql`select 1`);
        return Response.json({ ok: true });
      },
    },
  },
});
