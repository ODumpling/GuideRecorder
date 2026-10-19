import { redirect } from "@tanstack/react-router";
import { createServerFn } from "@tanstack/react-start";
import { z } from "zod";
import { pushRequest } from "@guiderecorder/sync";
import { db } from "@/db";
import { checkAccess } from "@/lib/access";
import { getSession } from "@/lib/auth";
import { roles } from "@/lib/permissions";
import { getGuide, listGuides, push, restoreGuide, trashGuide } from "@/lib/sync";

/** The signed-in user and their active workspace, with sync access checked. */
async function workspaceAccess(next: string) {
  const session = await getSession();
  if (!session) throw redirect({ to: "/sign-in", search: { next } });
  const workspaceId = session.session.activeOrganizationId;
  if (!workspaceId) throw redirect({ to: "/dashboard" });
  const access = await checkAccess(db(), session.user.id, workspaceId, { guide: ["read"] }, "sync");
  return { userId: session.user.id, workspaceId, access };
}

const can = (role: keyof typeof roles, action: "update" | "delete") => roles[role].authorize({ guide: [action] }).success;

/** The active workspace's synced guides, for the dashboard. */
export const getSyncedGuides = createServerFn({ method: "GET" }).handler(async () => {
  const { userId, workspaceId, access } = await workspaceAccess("/dashboard");
  if (!access.ok) return { available: false as const, reason: access.error };
  const [live, trashed] = await Promise.all([
    listGuides(db(), workspaceId, userId),
    listGuides(db(), workspaceId, userId, { trashed: true }),
  ]);
  const summary = ({ guide, head }: Awaited<ReturnType<typeof listGuides>>[number]) => ({
    id: guide.id,
    title: head.guide.title,
    steps: (head.guide.items ?? []).filter((i) => i.type === "step").length,
    updatedAt: head.createdAt,
    source: head.source,
    deletedAt: guide.deletedAt?.toISOString() ?? null,
  });
  return {
    available: true as const,
    workspaceId,
    canDelete: can(access.role, "delete"),
    guides: live.map(summary),
    trash: trashed.map(summary),
  };
});

/** One synced guide's head revision, and what the user may do with it. */
export const getSyncedGuide = createServerFn({ method: "GET" })
  .validator(z.object({ guideId: z.string() }))
  .handler(async ({ data }) => {
    const { userId, workspaceId, access } = await workspaceAccess(`/guides/${data.guideId}`);
    if (!access.ok) throw redirect({ to: "/dashboard" });
    const found = await getGuide(db(), workspaceId, userId, data.guideId);
    if (!found || found.guide.deletedAt) throw redirect({ to: "/dashboard" });
    return {
      workspaceId,
      revision: found.head,
      canEdit: can(access.role, "update") && access.plan.entitlements.webEditor,
    };
  });

/** Saves an edit made on the web as a new revision. */
export const saveSyncedGuide = createServerFn({ method: "POST" })
  .validator(pushRequest)
  .handler(async ({ data }) => {
    const { userId, workspaceId, access } = await workspaceAccess(`/guides/${data.guideId}`);
    if (!access.ok) return { ok: false as const, error: access.error };
    if (!access.plan.entitlements.webEditor) return { ok: false as const, error: "Your plan doesn't include the web editor." };
    const result = await push(db(), workspaceId, { userId, role: access.role }, data, "web");
    if (result.status === 200) return { ok: true as const, revisionId: result.revision.id };
    if (result.status === 409) return { ok: false as const, error: "Someone changed this guide since you opened it. Reload to see their changes." };
    return { ok: false as const, error: result.error };
  });

/** Moves a guide to the trash, or takes it out. */
export const setGuideTrashed = createServerFn({ method: "POST" })
  .validator(z.object({ guideId: z.string(), trashed: z.boolean() }))
  .handler(async ({ data }) => {
    const { userId, workspaceId, access } = await workspaceAccess("/dashboard");
    if (!access.ok) return { ok: false as const, error: access.error };
    const actor = { userId, role: access.role };
    const result = data.trashed
      ? await trashGuide(db(), workspaceId, actor, data.guideId)
      : await restoreGuide(db(), workspaceId, actor, data.guideId);
    return result.status === 200 ? { ok: true as const } : { ok: false as const, error: result.error };
  });
