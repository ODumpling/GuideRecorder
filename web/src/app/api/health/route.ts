import { sql } from "drizzle-orm";
import { db } from "@/db";

export async function GET() {
  await db().execute(sql`select 1`);
  return Response.json({ ok: true });
}
