import { redirect } from "@tanstack/react-router";
import { createServerFn } from "@tanstack/react-start";
import { getCookie, getRequestHeaders, setCookie, setResponseHeader } from "@tanstack/react-start/server";
import { z } from "zod";
import { db } from "@/db";
import { checkAccess } from "@/lib/access";
import { withUrl } from "@/lib/api";
import { getSession } from "@/lib/auth";
import { env } from "@/lib/env";
import { clientIp, sharePasswords, shareViews } from "@/lib/rate-limit";
import {
  createShareLink,
  createShareRequest,
  getSharingSettings,
  listShareLinks,
  openShareLink,
  revokeShareLink,
  setSharingSettings,
  sharingSettings,
  UNLOCK_SECONDS,
  unlockShareLink,
} from "@/lib/sharing";

/** The signed-in user in their active workspace, with sharing access checked. */
async function shareAccess(next: string) {
  const session = await getSession();
  if (!session) throw redirect({ to: "/sign-in", search: { next } });
  const workspaceId = session.session.activeOrganizationId;
  if (!workspaceId) throw redirect({ to: "/dashboard" });
  const access = await checkAccess(db(), session.user.id, workspaceId, { guide: ["share"] }, "sharing");
  return { userId: session.user.id, workspaceId, access };
}

/** A guide's share links, for its page; unavailable without the plan or the role. */
export const getShareLinks = createServerFn({ method: "GET" })
  .validator(z.object({ guideId: z.string() }))
  .handler(async ({ data }) => {
    const { userId, workspaceId, access } = await shareAccess(`/guides/${data.guideId}`);
    if (!access.ok) return { available: false as const, reason: access.error, upgrade: access.status === 402 };
    const result = await listShareLinks(db(), workspaceId, { userId, role: access.role }, data.guideId);
    if (result.status !== 200) return { available: false as const, reason: result.error, upgrade: false };
    return { available: true as const, links: result.links.map(withUrl), settings: result.settings };
  });

export const createShare = createServerFn({ method: "POST" })
  .validator(createShareRequest)
  .handler(async ({ data }) => {
    const { userId, workspaceId, access } = await shareAccess(`/guides/${data.guideId}`);
    if (!access.ok) return { ok: false as const, error: access.error };
    const result = await createShareLink(db(), workspaceId, { userId, role: access.role }, data);
    return result.status === 200 ? { ok: true as const, link: withUrl(result.link) } : { ok: false as const, error: result.error };
  });

export const revokeShare = createServerFn({ method: "POST" })
  .validator(z.object({ linkId: z.string() }))
  .handler(async ({ data }) => {
    const session = await getSession();
    const workspaceId = session?.session.activeOrganizationId;
    if (!session || !workspaceId) return { ok: false as const, error: "Sign in first." };
    // No entitlement check: a workspace that lost sharing can still revoke.
    const access = await checkAccess(db(), session.user.id, workspaceId, { guide: ["share"] });
    if (!access.ok) return { ok: false as const, error: access.error };
    const result = await revokeShareLink(db(), workspaceId, { userId: session.user.id, role: access.role }, data.linkId);
    return result.status === 200 ? { ok: true as const } : { ok: false as const, error: result.error };
  });

/** The active workspace's sharing settings, and whether the user may change them (admins, on Enterprise). */
export const getWorkspaceSharing = createServerFn({ method: "GET" }).handler(async () => {
  const session = await getSession();
  const workspaceId = session?.session.activeOrganizationId;
  if (!session || !workspaceId) return null;
  const access = await checkAccess(db(), session.user.id, workspaceId, { organization: ["update"] });
  if (!access.ok || !access.plan.entitlements.sharing) return null;
  return { settings: await getSharingSettings(db(), workspaceId), canManage: access.plan.entitlements.sharingControls };
});

export const saveWorkspaceSharing = createServerFn({ method: "POST" })
  .validator(sharingSettings)
  .handler(async ({ data }) => {
    const session = await getSession();
    const workspaceId = session?.session.activeOrganizationId;
    if (!session || !workspaceId) return { ok: false as const, error: "Sign in first." };
    const access = await checkAccess(db(), session.user.id, workspaceId, { organization: ["update"] }, "sharingControls");
    if (!access.ok) return { ok: false as const, error: access.error };
    await setSharingSettings(db(), workspaceId, data);
    return { ok: true as const };
  });

// The public page. No session: the token, and a password cookie, decide.

function publicResponse() {
  setResponseHeader("X-Robots-Tag", "noindex, nofollow");
  setResponseHeader("Cache-Control", "private, no-store");
  return clientIp(new Headers(getRequestHeaders() as HeadersInit));
}

const token = z.object({ token: z.string().max(100) });

/** The shared guide, counting a view; or why it can't be shown. */
export const getSharedGuide = createServerFn({ method: "GET" })
  .validator(token)
  .handler(async ({ data }) => {
    const ip = publicResponse();
    if (!shareViews.hit(ip)) return { status: 429 as const, error: "Too many requests. Try again in a minute." };
    return openShareLink(db(), env().BETTER_AUTH_SECRET, data.token, (name) => getCookie(name));
  });

/** Checks a shared guide's password and keeps it open in this browser for a while. */
export const unlockSharedGuide = createServerFn({ method: "POST" })
  .validator(token.extend({ password: z.string().max(200) }))
  .handler(async ({ data }) => {
    const ip = publicResponse();
    if (!sharePasswords.hit(`${ip}:${data.token}`)) return { ok: false as const, error: "Too many attempts. Try again later." };
    const result = await unlockShareLink(db(), env().BETTER_AUTH_SECRET, data.token, data.password);
    if (result.status !== 200) return { ok: false as const, error: result.error };
    if (result.cookie) {
      setCookie(result.cookie.name, result.cookie.value, {
        httpOnly: true,
        sameSite: "lax",
        secure: env().BETTER_AUTH_URL.startsWith("https:"),
        path: "/",
        maxAge: UNLOCK_SECONDS,
      });
    }
    return { ok: true as const };
  });
