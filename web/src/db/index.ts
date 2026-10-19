import "@tanstack/react-start/server-only";
import { drizzle } from "drizzle-orm/postgres-js";
import type { PgDatabase, PgQueryResultHKT } from "drizzle-orm/pg-core";
import postgres from "postgres";
import { env } from "@/lib/env";
import * as schema from "./schema";

/** Any Drizzle Postgres database with our schema (postgres-js, or PGlite in tests). */
export type Database = PgDatabase<PgQueryResultHKT, typeof schema>;

let cached: Database | undefined;

export function db(): Database {
  cached ??= drizzle(postgres(env().DATABASE_URL), { schema });
  return cached;
}

export { schema };
