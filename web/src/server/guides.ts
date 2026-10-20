import { redirect } from "@tanstack/react-router";
import { createServerFn } from "@tanstack/react-start";
import { getRequestHeaders } from "@tanstack/react-start/server";
import { z } from "zod";
import { pushRequest } from "@guiderecorder/sync";
import { db } from "@/db";
import { checkAccess } from "@/lib/access";
import { requestMeta } from "@/lib/audit";
import { objectStore } from "@/lib/api";
import { DOWNLOAD_TTL } from "@/lib/assets";
import { getSession } from "@/lib/auth";
import { EXPORT_FORMATS, exportFilename, exportFormat, requestExport, type ExportFormat } from "@/lib/exports";
import { library, setFolder } from "@/lib/library";
import { librarySearch } from "@/lib/library-search";
import { roles } from "@/lib/permissions";
import { getGuide, listGuides, push, readsDrafts, restoreGuide, trashGuide } from "@/lib/sync";

/** The signed-in user and their active workspace, with sync access checked. */
async function workspaceAccess(next: string) {
  const session = await getSession();
  if (!session) throw redirect({ to: "/sign-in", search: { next } });
  const workspaceId = session.session.activeOrganizationId;
  if (!workspaceId) throw redirect({ to: "/dashboard" });
  const access = await checkAccess(db(), session.user.id, workspaceId, { guide: ["read"] }, "sync");
  return { userId: session.user.id, workspaceId, access, meta: requestMeta(getRequestHeaders() as unknown as Headers) };
}

const can = (role: keyof typeof roles, action: "update" | "delete") => roles[role].authorize({ guide: [action] }).success;

/** The active workspace's synced guides, for the dashboard. */
export const getSyncedGuides = createServerFn({ method: "GET" }).handler(async () => {
  const { userId, workspaceId, access } = await workspaceAccess("/dashboard");
  if (!access.ok) return { available: false as const, reason: access.error };
  const reader = { userId, role: access.role };
  const [live, trashed] = await Promise.all([
    listGuides(db(), workspaceId, reader),
    listGuides(db(), workspaceId, reader, { trashed: true }),
  ]);
  const summary = ({ guide, head, review }: Awaited<ReturnType<typeof listGuides>>[number]) => ({
    id: guide.id,
    title: head.guide.title,
    steps: (head.guide.items ?? []).filter((i) => i.type === "step").length,
    updatedAt: head.createdAt,
    source: head.source,
    deletedAt: guide.deletedAt?.toISOString() ?? null,
    review,
  });
  return {
    available: true as const,
    workspaceId,
    canDelete: can(access.role, "delete"),
    guides: live.map(summary),
    trash: trashed.map(summary),
  };
});

/** One synced guide's revision the user sees, and what they may do with it. */
export const getSyncedGuide = createServerFn({ method: "GET" })
  .validator(z.object({ guideId: z.string() }))
  .handler(async ({ data }) => {
    const { userId, workspaceId, access } = await workspaceAccess(`/guides/${data.guideId}`);
    if (!access.ok) throw redirect({ to: "/dashboard" });
    const found = await getGuide(db(), workspaceId, { userId, role: access.role }, data.guideId);
    if (!found || found.guide.deletedAt) throw redirect({ to: "/dashboard" });
    return {
      workspaceId,
      revision: found.head,
      folder: found.guide.folder,
      canEdit: can(access.role, "update") && access.plan.entitlements.webEditor,
      // Approvals: authors and approvers see drafts and the review page; viewers only what's published.
      canReview: readsDrafts(access.role) && access.plan.entitlements.approvals,
      unpublished: readsDrafts(access.role) && found.guide.headRevisionId !== found.guide.publishedRevisionId,
      exportFormats: Object.entries(EXPORT_FORMATS).map(([id, f]) => ({ id: id as ExportFormat, label: f.label })),
    };
  });

/** The active workspace's library: the guides the user can see, searched and filtered. */
export const getLibrary = createServerFn({ method: "GET" })
  .validator(librarySearch)
  .handler(async ({ data }) => {
    const { userId, workspaceId, access } = await workspaceAccess("/library");
    if (!access.ok) return { available: false as const, reason: access.error };
    const found = await library(db(), workspaceId, { userId, role: access.role }, {
      query: data.q,
      folder: data.folder,
      tag: data.tag,
      status: data.status,
      reviewDue: data.review === "due",
    });
    return { available: true as const, workspaceId, canOrganize: can(access.role, "update"), canDelete: can(access.role, "delete"), ...found };
  });

/** Files a guide in a library folder, or takes it out of one (null). */
export const setGuideFolder = createServerFn({ method: "POST" })
  .validator(z.object({ guideId: z.string(), folder: z.string().max(200).nullable() }))
  .handler(async ({ data }) => {
    const { userId, workspaceId, access } = await workspaceAccess("/library");
    if (!access.ok) return { ok: false as const, error: access.error };
    const result = await setFolder(db(), workspaceId, { userId, role: access.role }, data.guideId, data.folder);
    return result.status === 200 ? { ok: true as const, folder: result.folder } : { ok: false as const, error: result.error };
  });

/**
 * A download of one revision as PDF, DOCX, Markdown or HTML: its URL once
 * the export worker has made it, else how far it got. Poll until done;
 * `retry` queues a failed export again.
 */
export const requestGuideExport = createServerFn({ method: "POST" })
  .validator(z.object({ guideId: z.string(), revisionId: z.string(), format: exportFormat, retry: z.boolean().optional() }))
  .handler(async ({ data }) => {
    const { userId, workspaceId, access } = await workspaceAccess(`/guides/${data.guideId}`);
    if (!access.ok) return { status: "failed" as const, error: access.error };
    const result = await requestExport(db(), workspaceId, userId, data);
    if (typeof result.status === "number") return { status: "failed" as const, error: result.error };
    if (result.status === "failed") return { status: "failed" as const, error: result.error };
    if (result.status !== "done") return { status: result.status };
    const found = await getGuide(db(), workspaceId, { userId, role: access.role }, data.guideId);
    const url = await objectStore().presignGet(result.key, DOWNLOAD_TTL, {
      filename: exportFilename(found?.head.guide.title ?? "Guide", data.format),
    });
    return { status: "done" as const, url };
  });

/** Saves an edit made on the web as a new revision. */
export const saveSyncedGuide = createServerFn({ method: "POST" })
  .validator(pushRequest)
  .handler(async ({ data }) => {
    const { userId, workspaceId, access, meta } = await workspaceAccess(`/guides/${data.guideId}`);
    if (!access.ok) return { ok: false as const, error: access.error };
    if (!access.plan.entitlements.webEditor) return { ok: false as const, error: "Your plan doesn't include the web editor." };
    const result = await push(db(), workspaceId, { userId, role: access.role, ...meta }, data, "web");
    if (result.status === 200) return { ok: true as const, revisionId: result.revision.id };
    if (result.status === 409) return { ok: false as const, error: "Someone changed this guide since you opened it. Reload to see their changes." };
    return { ok: false as const, error: result.error };
  });

/** Moves a guide to the trash, or takes it out. */
export const setGuideTrashed = createServerFn({ method: "POST" })
  .validator(z.object({ guideId: z.string(), trashed: z.boolean() }))
  .handler(async ({ data }) => {
    const { userId, workspaceId, access, meta } = await workspaceAccess("/dashboard");
    if (!access.ok) return { ok: false as const, error: access.error };
    const actor = { userId, role: access.role, ...meta };
    const result = data.trashed
      ? await trashGuide(db(), workspaceId, actor, data.guideId)
      : await restoreGuide(db(), workspaceId, actor, data.guideId);
    return result.status === 200 ? { ok: true as const } : { ok: false as const, error: result.error };
  });
