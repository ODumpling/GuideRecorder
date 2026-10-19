import "@tanstack/react-start/server-only";
import { and, asc, eq } from "drizzle-orm";
import { db } from "@/db";
import { invitation, member, organization, user } from "@/db/schema";
import { storageUsed } from "./assets";
import { workspacePlan } from "./workspace";

/** Everything the dashboard shows for the user's active workspace. */
export async function loadDashboard(userId: string, activeOrganizationId: string | null | undefined) {
  const database = db();
  const workspaces = await database
    .select({ id: organization.id, name: organization.name, role: member.role })
    .from(member)
    .innerJoin(organization, eq(organization.id, member.organizationId))
    .where(eq(member.userId, userId))
    .orderBy(asc(member.createdAt));
  const active = workspaces.find((w) => w.id === activeOrganizationId) ?? workspaces[0];
  if (!active) return { workspaces, active: null } as const;

  const [members, invitations, plan, used] = await Promise.all([
    database
      .select({ id: member.id, userId: member.userId, role: member.role, name: user.name, email: user.email })
      .from(member)
      .innerJoin(user, eq(user.id, member.userId))
      .where(eq(member.organizationId, active.id))
      .orderBy(asc(member.createdAt)),
    database
      .select({ id: invitation.id, email: invitation.email, role: invitation.role, expiresAt: invitation.expiresAt })
      .from(invitation)
      .where(and(eq(invitation.organizationId, active.id), eq(invitation.status, "pending"))),
    workspacePlan(database, active.id),
    storageUsed(database, active.id),
  ]);
  return { workspaces, active: { ...active, members, invitations, plan, storageUsed: used } } as const;
}
