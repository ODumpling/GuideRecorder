import { redirect } from "@tanstack/react-router";
import { createServerFn } from "@tanstack/react-start";
import { getRequestHeaders } from "@tanstack/react-start/server";
import { eq } from "drizzle-orm";
import { z } from "zod";
import { db } from "@/db";
import { member, user } from "@/db/schema";
import { checkAccess, type Permission } from "@/lib/access";
import { deletePolicy, getReview, listPolicies, policyInput, reviewChange, reviewRequest, savePolicy, submitForReview, withdrawChange } from "@/lib/approvals";
import { requestMeta } from "@/lib/audit";
import { getSession } from "@/lib/auth";
import { roles, type RoleName } from "@/lib/permissions";
import { DEFAULT_POLICY } from "@/lib/review";

/** The signed-in user in their active workspace, with `permission` on a plan with approvals. */
async function approvalsSession(next: string, permission: Permission) {
  const session = await getSession();
  if (!session) throw redirect({ to: "/sign-in", search: { next } });
  const workspaceId = session.session.activeOrganizationId;
  if (!workspaceId) throw redirect({ to: "/dashboard" });
  const access = await checkAccess(db(), session.user.id, workspaceId, permission, "approvals");
  const actor = access.ok ? { userId: session.user.id, role: access.role, ...requestMeta(getRequestHeaders() as unknown as Headers) } : null;
  return { workspaceId, access, actor };
}

const fail = (error: string) => ({ ok: false as const, error });

/** The review page: a guide's change request, what it changes, and its reviews. */
export const getGuideReview = createServerFn({ method: "GET" })
  .validator(z.object({ guideId: z.string() }))
  .handler(async ({ data }) => {
    const { workspaceId, access, actor } = await approvalsSession(`/guides/${data.guideId}/review`, { guide: ["update"] });
    if (!actor) return { allowed: false as const, error: access.ok ? "" : access.error };
    const review = await getReview(db(), workspaceId, actor, data.guideId);
    if (!review) throw redirect({ to: "/dashboard" });
    return { allowed: true as const, workspaceId, userId: actor.userId, ...review };
  });

export const submitGuideForReview = createServerFn({ method: "POST" })
  .validator(z.object({ guideId: z.string(), revisionId: z.string(), note: z.string().max(2000).default("") }))
  .handler(async ({ data }) => {
    const { workspaceId, access, actor } = await approvalsSession(`/guides/${data.guideId}/review`, { guide: ["update"] });
    if (!actor) return fail(access.ok ? "" : access.error);
    const result = await submitForReview(db(), workspaceId, actor, data.guideId, data);
    return result.status === 200 ? { ok: true as const } : fail(result.error);
  });

export const reviewGuideChange = createServerFn({ method: "POST" })
  .validator(reviewRequest.extend({ guideId: z.string(), changeRequestId: z.string() }))
  .handler(async ({ data }) => {
    const { workspaceId, access, actor } = await approvalsSession(`/guides/${data.guideId}/review`, { guide: ["update"] });
    if (!actor) return fail(access.ok ? "" : access.error);
    const result = await reviewChange(db(), workspaceId, actor, data.changeRequestId, data);
    return result.status === 200 ? { ok: true as const, published: result.published } : fail(result.error);
  });

export const withdrawGuideChange = createServerFn({ method: "POST" })
  .validator(z.object({ guideId: z.string(), changeRequestId: z.string() }))
  .handler(async ({ data }) => {
    const { workspaceId, access, actor } = await approvalsSession(`/guides/${data.guideId}/review`, { guide: ["update"] });
    if (!actor) return fail(access.ok ? "" : access.error);
    const result = await withdrawChange(db(), workspaceId, actor, data.changeRequestId);
    return result.status === 200 ? { ok: true as const } : fail(result.error);
  });

/** The workspace's approval policies, for admins (`organization:update`). */
export const getApprovalSettings = createServerFn({ method: "GET" }).handler(async () => {
  const { workspaceId, actor } = await approvalsSession("/dashboard", { organization: ["update"] });
  if (!actor) return { available: false as const };
  const members = await db()
    .select({ id: user.id, name: user.name, email: user.email, role: member.role })
    .from(member)
    .innerJoin(user, eq(user.id, member.userId))
    .where(eq(member.organizationId, workspaceId));
  return {
    available: true as const,
    policies: await listPolicies(db(), workspaceId),
    defaultPolicy: DEFAULT_POLICY,
    approvers: members.filter((m) => m.role in roles && roles[m.role as RoleName].authorize({ guide: ["approve"] }).success),
  };
});

export const saveApprovalPolicy = createServerFn({ method: "POST" })
  .validator(policyInput)
  .handler(async ({ data }) => {
    const { workspaceId, access, actor } = await approvalsSession("/dashboard", { organization: ["update"] });
    if (!actor) return fail(access.ok ? "" : access.error);
    const result = await savePolicy(db(), workspaceId, actor, data);
    return result.status === 200 ? { ok: true as const } : fail(result.error);
  });

export const deleteApprovalPolicy = createServerFn({ method: "POST" })
  .validator(z.object({ folder: z.string() }))
  .handler(async ({ data }) => {
    const { workspaceId, access, actor } = await approvalsSession("/dashboard", { organization: ["update"] });
    if (!actor) return fail(access.ok ? "" : access.error);
    const result = await deletePolicy(db(), workspaceId, actor, data.folder);
    return result.status === 200 ? { ok: true as const } : fail(result.error);
  });
