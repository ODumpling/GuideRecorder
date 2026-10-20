import { redirect } from "@tanstack/react-router";
import { createServerFn } from "@tanstack/react-start";
import { getRequestHeaders } from "@tanstack/react-start/server";
import { z } from "zod";
import { db } from "@/db";
import {
  createScimConnection,
  decommissionScim,
  deleteSso,
  domainVerificationToken,
  loadAdmin,
  policyUpdate,
  registerSso,
  revokeScimToken,
  rotateScimToken,
  ssoRegistration,
  updatePolicy,
  verifyDomain,
  type AdminActor,
} from "@/lib/admin";
import { auth, getSession } from "@/lib/auth";
import { env } from "@/lib/env";

/** The signed-in user acting on their active workspace. */
async function actor(): Promise<AdminActor> {
  const session = await getSession();
  if (!session) throw redirect({ to: "/sign-in", search: { next: "/admin" } });
  const organizationId = session.session.activeOrganizationId;
  if (!organizationId) throw redirect({ to: "/dashboard" });
  return { userId: session.user.id, organizationId, headers: getRequestHeaders() };
}

/** Results without a status, for the client. */
const result = <T extends { ok: boolean }>(r: T) => {
  if (!r.ok) return { ok: false as const, error: (r as unknown as { error: string }).error };
  return r as Extract<T, { ok: true }>;
};

export const getAdmin = createServerFn({ method: "GET" }).handler(async () => {
  const me = await actor();
  const loaded = await loadAdmin(db(), auth(), me);
  // Members are managed from the dashboard; the console itself is Enterprise.
  if (!loaded.ok || !loaded.enterprise) throw redirect({ to: "/dashboard" });
  return {
    ...loaded,
    workspaceId: me.organizationId,
    userId: me.userId,
    baseURL: env().BETTER_AUTH_URL,
  };
});

export const saveAdminPolicy = createServerFn({ method: "POST" })
  .validator(policyUpdate)
  .handler(async ({ data }) => result(await updatePolicy(db(), await actor(), data)));

export const addSsoProvider = createServerFn({ method: "POST" })
  .validator(ssoRegistration)
  .handler(async ({ data }) => result(await registerSso(db(), auth(), env().BETTER_AUTH_URL, await actor(), data)));

const provider = z.object({ providerId: z.string() });

export const getDomainToken = createServerFn({ method: "POST" })
  .validator(provider)
  .handler(async ({ data }) => result(await domainVerificationToken(db(), auth(), await actor(), data.providerId)));

export const checkDomain = createServerFn({ method: "POST" })
  .validator(provider)
  .handler(async ({ data }) => result(await verifyDomain(db(), auth(), await actor(), data.providerId)));

export const removeSsoProvider = createServerFn({ method: "POST" })
  .validator(provider)
  .handler(async ({ data }) => result(await deleteSso(db(), auth(), await actor(), data.providerId)));

export const addScimConnection = createServerFn({ method: "POST" }).handler(async () => result(await createScimConnection(db(), auth(), await actor())));

const connection = z.object({ connectionId: z.string() });

export const rotateScim = createServerFn({ method: "POST" })
  .validator(connection)
  .handler(async ({ data }) => result(await rotateScimToken(db(), auth(), await actor(), data.connectionId)));

export const revokeScim = createServerFn({ method: "POST" })
  .validator(connection.extend({ credentialId: z.string() }))
  .handler(async ({ data }) => result(await revokeScimToken(db(), auth(), await actor(), data.connectionId, data.credentialId)));

export const endScimConnection = createServerFn({ method: "POST" })
  .validator(connection)
  .handler(async ({ data }) => result(await decommissionScim(db(), auth(), await actor(), data.connectionId)));
