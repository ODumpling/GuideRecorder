import { redirect } from "@tanstack/react-router";
import { createServerFn } from "@tanstack/react-start";
import { getRequestHeaders } from "@tanstack/react-start/server";
import { asc, eq } from "drizzle-orm";
import { z } from "zod";
import { db } from "@/db";
import { member, user } from "@/db/schema";
import { checkAccess } from "@/lib/access";
import { AUDIT_ACTIONS, auditFilters, auditRetention, listAudit, parseFilters, pruneAudit, requestMeta, setAuditRetention, verifyWorkspaceChain } from "@/lib/audit";
import { getSession } from "@/lib/auth";
import type { Json } from "@/lib/diff";
import { roles } from "@/lib/permissions";

/** The signed-in user in their active workspace, with `audit:read` on a plan with `audit`. */
async function auditSession() {
  const session = await getSession();
  if (!session) throw redirect({ to: "/sign-in", search: { next: "/audit" } });
  const workspaceId = session.session.activeOrganizationId;
  if (!workspaceId) throw redirect({ to: "/dashboard" });
  const access = await checkAccess(db(), session.user.id, workspaceId, { audit: ["read"] }, "audit");
  return { userId: session.user.id, workspaceId, access };
}

/** A page of the audit log, newest first, with the chain's check and the retention setting. */
export const getAuditLog = createServerFn({ method: "GET" })
  .validator(auditFilters)
  .handler(async ({ data }) => {
    const { workspaceId, access } = await auditSession();
    if (!access.ok) return { allowed: false as const, error: access.error };
    await pruneAudit(db(), new Date(), workspaceId);
    const filters = { ...parseFilters(data), limit: 100 };
    const [events, chain, retentionDays, members] = await Promise.all([
      listAudit(db(), workspaceId, filters),
      verifyWorkspaceChain(db(), workspaceId),
      auditRetention(db(), workspaceId),
      db()
        .select({ id: user.id, name: user.name, email: user.email })
        .from(member)
        .innerJoin(user, eq(user.id, member.userId))
        .where(eq(member.organizationId, workspaceId))
        .orderBy(asc(user.name)),
    ]);
    return {
      allowed: true as const,
      workspaceId,
      events: events.map((e) => ({ ...e, metadata: e.metadata as Record<string, Json>, createdAt: e.createdAt.toISOString() })),
      hasMore: events.length === filters.limit,
      chain,
      retentionDays,
      canSetRetention: roles[access.role].authorize({ organization: ["update"] }).success,
      members,
      actions: AUDIT_ACTIONS,
    };
  });

/** Sets how long the workspace keeps audit events; admins only. */
export const saveAuditRetention = createServerFn({ method: "POST" })
  .validator(z.object({ days: z.number().int().nullable() }))
  .handler(async ({ data }) => {
    const { userId, workspaceId, access } = await auditSession();
    if (!access.ok) return { ok: false as const, error: access.error };
    if (!roles[access.role].authorize({ organization: ["update"] }).success) return { ok: false as const, error: "Your role doesn't allow this." };
    return setAuditRetention(db(), workspaceId, { userId, ...requestMeta(getRequestHeaders() as unknown as Headers) }, data.days);
  });
