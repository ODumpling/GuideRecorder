import { redirect } from "@tanstack/react-router";
import { createServerFn } from "@tanstack/react-start";
import { z } from "zod";
import type { Guide } from "@guiderecorder/guide-schema";
import { ulid } from "@guiderecorder/sync";
import { db } from "@/db";
import { getSession } from "@/lib/auth";
import { editorAccess, saveEdit } from "@/lib/edit";
import { getGuide } from "@/lib/sync";

/** The signed-in user and their active workspace, with web editor access checked. */
async function editorSession(guideId: string) {
  const session = await getSession();
  if (!session) throw redirect({ to: "/sign-in", search: { next: `/guides/${guideId}/edit` } });
  const workspaceId = session.session.activeOrganizationId;
  if (!workspaceId) throw redirect({ to: "/dashboard" });
  const access = await editorAccess(db(), session.user.id, workspaceId);
  return { userId: session.user.id, workspaceId, access };
}

/** A synced guide's head revision, to edit on the web. */
export const getGuideToEdit = createServerFn({ method: "GET" })
  .validator(z.object({ guideId: z.string() }))
  .handler(async ({ data }) => {
    const { userId, workspaceId, access } = await editorSession(data.guideId);
    if (!access.ok) return { allowed: false as const, error: access.error };
    const found = await getGuide(db(), workspaceId, userId, data.guideId);
    if (!found || found.guide.deletedAt) throw redirect({ to: "/dashboard" });
    return { allowed: true as const, workspaceId, revision: found.head };
  });

/**
 * Saves the editor's guide as a new revision. Clashes with changes made
 * meanwhile come back for the user to settle with `prefer`.
 */
export const saveGuideEdit = createServerFn({ method: "POST" })
  .validator(
    z.object({
      guideId: ulid,
      baseRevision: z.string(),
      guide: z.record(z.string(), z.unknown()),
      prefer: z.enum(["ours", "theirs"]).optional(),
    }),
  )
  .handler(async ({ data }) => {
    const { userId, workspaceId, access } = await editorSession(data.guideId);
    if (!access.ok) return { ok: false as const, error: access.error, conflicts: [] };
    const result = await saveEdit(db(), workspaceId, { userId, role: access.role }, { ...data, guide: data.guide as unknown as Guide });
    if (result.status === 200) return { ok: true as const, revisionId: result.revision.id, merged: result.merged };
    return { ok: false as const, error: result.error, conflicts: result.status === 409 ? result.conflicts : [] };
  });
