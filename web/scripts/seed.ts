// Development accounts: one per plan, one per role in the Enterprise
// workspace, and a staff admin. `pnpm db:seed`, after `pnpm db:migrate`.
// Idempotent: fixed ids; existing rows are left alone, except that emails
// follow SEED_EMAIL_DOMAIN. Refuses to run with NODE_ENV=production.
//
// Every account is on SEED_EMAIL_DOMAIN (default dufflebear.dev) and signs
// in with password `password123`:
//   hello@      Staff console (/staff), Free personal workspace
//   staff@      Staff console (/staff), Free personal workspace
//   free@       Free, personal workspace
//   pro@        Pro, personal workspace
//   owner@      Enterprise ("Acme"), owner
//   admin@      Enterprise, admin
//   approver@   Enterprise, approver
//   editor@     Enterprise, editor
//   viewer@     Enterprise, viewer

import { hashPassword } from "better-auth/crypto";
import { drizzle } from "drizzle-orm/postgres-js";
import postgres from "postgres";
import type { Database } from "@/db";
import * as schema from "@/db/schema";
import { account, member, organization, staff, subscription, user } from "@/db/schema";
import type { PlanName } from "@/lib/plans";
import type { RoleName } from "@/lib/permissions";

const PASSWORD = "password123";
const DOMAIN = (process.env.SEED_EMAIL_DOMAIN || "dufflebear.dev").toLowerCase().replace(/^@/, "");
const email = (local: string) => `${local}@${DOMAIN}`;

if (process.env.NODE_ENV === "production") {
  console.error("seed: refusing to run with NODE_ENV=production");
  process.exit(1);
}

const url = process.env.DATABASE_URL ?? "postgres://guiderecorder:guiderecorder@localhost:5432/guiderecorder";
const client = postgres(url, { max: 1 });
const db = drizzle(client, { schema }) as unknown as Database;

interface Workspace {
  id: string;
  name: string;
  slug: string;
  plan: PlanName;
  members: { id: string; name: string; email: string; role: RoleName }[];
}

/** Seeded accounts that may use the staff console. */
const STAFF = ["seed-user-hello", "seed-user-staff"];

const WORKSPACES: Workspace[] = [
  {
    id: "seed-org-hello",
    name: "Hello's workspace",
    slug: "personal-seed-user-hello",
    plan: "free",
    members: [{ id: "seed-user-hello", name: "Hello", email: email("hello"), role: "owner" }],
  },
  {
    id: "seed-org-staff",
    name: "Staff's workspace",
    slug: "personal-seed-user-staff",
    plan: "free",
    members: [{ id: "seed-user-staff", name: "Staff", email: email("staff"), role: "owner" }],
  },
  {
    id: "seed-org-free",
    name: "Free's workspace",
    // The personal workspace slug `ensureWorkspace` would make.
    slug: "personal-seed-user-free",
    plan: "free",
    members: [{ id: "seed-user-free", name: "Free", email: email("free"), role: "owner" }],
  },
  {
    id: "seed-org-pro",
    name: "Pro's workspace",
    slug: "personal-seed-user-pro",
    plan: "pro",
    members: [{ id: "seed-user-pro", name: "Pro", email: email("pro"), role: "owner" }],
  },

  {
    id: "seed-org-acme",
    name: "Acme",
    slug: "acme",
    plan: "enterprise",
    members: (["owner", "admin", "approver", "editor", "viewer"] as const).map((role) => ({
      id: `seed-user-${role}`,
      name: role[0]!.toUpperCase() + role.slice(1),
      email: email(role),
      role,
    })),
  },
];

const password = await hashPassword(PASSWORD);
const now = new Date();

await db.transaction(async (tx) => {
  for (const ws of WORKSPACES) {
    await tx.insert(organization).values({ id: ws.id, name: ws.name, slug: ws.slug, createdAt: now }).onConflictDoNothing();
    if (ws.plan !== "free") {
      await tx
        .insert(subscription)
        .values({
          id: `${ws.id}-sub`,
          plan: ws.plan,
          referenceId: ws.id,
          status: "active",
          periodStart: now,
          seats: ws.plan === "enterprise" ? 25 : null,
        })
        .onConflictDoNothing();
    }
    for (const m of ws.members) {
      await tx
        .insert(user)
        .values({ id: m.id, name: m.name, email: m.email, emailVerified: true })
        .onConflictDoUpdate({ target: user.id, set: { email: m.email } });
      await tx
        .insert(account)
        .values({ id: `${m.id}-credential`, accountId: m.id, providerId: "credential", userId: m.id, password })
        .onConflictDoNothing();
      await tx
        .insert(member)
        .values({ id: `${m.id}-${ws.id}`, organizationId: ws.id, userId: m.id, role: m.role, createdAt: now })
        .onConflictDoNothing();
    }
  }
  for (const userId of STAFF) await tx.insert(staff).values({ userId, addedBy: "seed" }).onConflictDoNothing();
});

await client.end();

console.log(`seed: done. Sign in with password "${PASSWORD}":`);
for (const ws of WORKSPACES) {
  for (const m of ws.members) console.log(`  ${m.email.padEnd(30)} ${ws.plan.padEnd(10)} ${ws.name}, ${m.role}`);
}
