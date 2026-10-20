import { redirect } from "@tanstack/react-router";
import { createServerFn } from "@tanstack/react-start";
import { getRequestHeaders } from "@tanstack/react-start/server";
import { z } from "zod";
import { db } from "@/db";
import { checkAccess } from "@/lib/access";
import { requestMeta } from "@/lib/audit";
import { getSession } from "@/lib/auth";
import { addOwner, guideAccess, removeOwner, setLocked } from "@/lib/ownership";

/** The signed-in user in their active workspace, if they can read its synced guides. */
async function ownershipSession(next: string) {
  const session = await getSession();
  if (!session) throw redirect({ to: "/sign-in", search: { next } });
  const workspaceId = session.session.activeOrganizationId;
  if (!workspaceId) throw redirect({ to: "/dashboard" });
  const access = await checkAccess(db(), session.user.id, workspaceId, { guide: ["read"] }, "sync");
  const actor = access.ok ? { userId: session.user.id, role: access.role, ...requestMeta(getRequestHeaders() as unknown as Headers) } : null;
  return { workspaceId, actor, error: access.ok ? null : access.error };
}

const guideId = z.object({ guideId: z.string() });
const done = (result: { status: number; error?: string }) => (result.status === 200 ? { ok: true as const } : { ok: false as const, error: result.error! });

/** A guide's owners and lock, and what the user may change. */
export const getGuideAccess = createServerFn({ method: "GET" })
  .validator(guideId)
  .handler(async ({ data }) => {
    const { workspaceId, actor } = await ownershipSession(`/guides/${data.guideId}`);
    return actor ? await guideAccess(db(), workspaceId, actor, data.guideId) : null;
  });

export const setGuideLocked = createServerFn({ method: "POST" })
  .validator(guideId.extend({ locked: z.boolean() }))
  .handler(async ({ data }) => {
    const { workspaceId, actor, error } = await ownershipSession(`/guides/${data.guideId}`);
    if (!actor) return { ok: false as const, error: error! };
    return done(await setLocked(db(), workspaceId, actor, data.guideId, data.locked));
  });

export const addGuideOwner = createServerFn({ method: "POST" })
  .validator(guideId.extend({ userId: z.string() }))
  .handler(async ({ data }) => {
    const { workspaceId, actor, error } = await ownershipSession(`/guides/${data.guideId}`);
    if (!actor) return { ok: false as const, error: error! };
    return done(await addOwner(db(), workspaceId, actor, data.guideId, data.userId));
  });

export const removeGuideOwner = createServerFn({ method: "POST" })
  .validator(guideId.extend({ userId: z.string() }))
  .handler(async ({ data }) => {
    const { workspaceId, actor, error } = await ownershipSession(`/guides/${data.guideId}`);
    if (!actor) return { ok: false as const, error: error! };
    return done(await removeOwner(db(), workspaceId, actor, data.guideId, data.userId));
  });
