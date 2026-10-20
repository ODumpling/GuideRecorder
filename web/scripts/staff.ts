// Who may use the staff console (/staff). Runs against DATABASE_URL, in
// production too. `add` needs the user to have signed up first.
//
//   pnpm staff list
//   pnpm staff add <email>
//   pnpm staff remove <email>
//   pnpm staff ensure      creates the users in STAFF_EMAILS (comma-separated)
//                          if missing and makes them staff; run on deploy

import { eq } from "drizzle-orm";
import { drizzle } from "drizzle-orm/postgres-js";
import postgres from "postgres";
import type { Database } from "@/db";
import * as schema from "@/db/schema";
import { staff, user } from "@/db/schema";
import { ensureStaff } from "@/lib/staff";

const url = process.env.DATABASE_URL ?? "postgres://guiderecorder:guiderecorder@localhost:5432/guiderecorder";
const client = postgres(url, { max: 1 });
const db = drizzle(client, { schema }) as unknown as Database;

const [command, email] = process.argv.slice(2);

async function userId(email: string | undefined) {
  if (!email) throw new Error("Give an email.");
  const [found] = await db.select({ id: user.id }).from(user).where(eq(user.email, email.toLowerCase()));
  if (!found) throw new Error(`No user with email ${email}; they must sign up first.`);
  return found.id;
}

try {
  if (command === "add") {
    await db.insert(staff).values({ userId: await userId(email), addedBy: "cli" }).onConflictDoNothing();
    console.log(`staff: added ${email}`);
  } else if (command === "ensure") {
    const emails = (process.env.STAFF_EMAILS ?? "").split(",").filter((e) => e.trim());
    for (const { email, created } of await ensureStaff(db, emails)) console.log(`staff: ${email}${created ? " (new user)" : ""}`);
    if (!emails.length) console.log("staff: STAFF_EMAILS is empty; nothing to do");
  } else if (command === "remove") {
    await db.delete(staff).where(eq(staff.userId, await userId(email)));
    console.log(`staff: removed ${email}`);
  } else if (command === "list" || !command) {
    const rows = await db.select({ email: user.email, since: staff.createdAt }).from(staff).innerJoin(user, eq(user.id, staff.userId));
    for (const r of rows) console.log(`${r.email.padEnd(32)} since ${r.since.toISOString().slice(0, 10)}`);
    if (!rows.length) console.log("staff: nobody yet; `pnpm staff add <email>`");
  } else {
    throw new Error("Usage: pnpm staff list | add <email> | remove <email> | ensure");
  }
} catch (error) {
  console.error(`staff: ${error instanceof Error ? error.message : error}`);
  process.exitCode = 1;
} finally {
  await client.end();
}
