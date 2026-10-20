import { notFound, redirect } from "@tanstack/react-router";
import { createServerFn } from "@tanstack/react-start";
import { getRequestHeaders } from "@tanstack/react-start/server";
import { z } from "zod";
import { db } from "@/db";
import { requestMeta } from "@/lib/audit";
import { getSession } from "@/lib/auth";
import { findUsers, isStaff, issueLicence, licenceInput, restoreDeletedGuide, revokeLicence, userDetail, type StaffActor } from "@/lib/staff";

/** The signed-in staff member; anyone else gets a 404, so the console's existence doesn't show. */
async function staffActor(next = "/staff"): Promise<StaffActor & { email: string; name: string }> {
  const session = await getSession();
  if (!session) throw redirect({ to: "/sign-in", search: { next } });
  if (!(await isStaff(db(), session.user.id))) throw notFound();
  return {
    userId: session.user.id,
    email: session.user.email,
    name: session.user.name,
    ...requestMeta(getRequestHeaders() as unknown as Headers),
  };
}

/** The staff member, for the console's frame. */
export const getStaffViewer = createServerFn({ method: "GET" })
  .validator(z.object({ next: z.string() }))
  .handler(async ({ data }) => {
    const { userId, email, name } = await staffActor(data.next);
    return { id: userId, email, name };
  });

export const searchUsers = createServerFn({ method: "GET" })
  .validator(z.object({ q: z.string().max(200).optional() }))
  .handler(async ({ data }) => {
    await staffActor();
    return findUsers(db(), data.q);
  });

export const getStaffUser = createServerFn({ method: "GET" })
  .validator(z.object({ userId: z.string() }))
  .handler(async ({ data }) => {
    await staffActor(`/staff/users/${data.userId}`);
    const detail = await userDetail(db(), data.userId);
    if (!detail) throw notFound();
    return detail;
  });

export const grantLicence = createServerFn({ method: "POST" })
  .validator(licenceInput)
  .handler(async ({ data }) => issueLicence(db(), await staffActor(), data));

export const endLicence = createServerFn({ method: "POST" })
  .validator(z.object({ grantId: z.string() }))
  .handler(async ({ data }) => revokeLicence(db(), await staffActor(), data.grantId));

export const restoreGuideForUser = createServerFn({ method: "POST" })
  .validator(z.object({ organizationId: z.string(), guideId: z.string() }))
  .handler(async ({ data }) => restoreDeletedGuide(db(), await staffActor(), data.organizationId, data.guideId));
