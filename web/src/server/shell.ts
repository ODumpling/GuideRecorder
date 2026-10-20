import { redirect } from "@tanstack/react-router";
import { createServerFn } from "@tanstack/react-start";
import { z } from "zod";
import { db } from "@/db";
import { checkAccess } from "@/lib/access";
import { getSession } from "@/lib/auth";
import { loadDashboard } from "@/lib/dashboard";
import { library } from "@/lib/library";
import { roles, type RoleName } from "@/lib/permissions";

type Permission = Parameters<(typeof roles)[RoleName]["authorize"]>[0];

/**
 * What the signed-in pages' sidebar shows: the user, their workspaces, and
 * the active workspace's library counts (null without sync).
 */
export const getShell = createServerFn({ method: "GET" })
  .validator(z.object({ next: z.string() }))
  .handler(async ({ data }) => {
    const session = await getSession();
    if (!session) throw redirect({ to: "/sign-in", search: { next: data.next } });
    const { workspaces, active } = await loadDashboard(session.user.id, session.session.activeOrganizationId);
    const can = (permission: Permission) =>
      Boolean(active && active.role in roles && roles[active.role as RoleName].authorize(permission).success);
    const access = active ? await checkAccess(db(), session.user.id, active.id, { guide: ["read"] }, "sync") : null;
    const found = access?.ok ? await library(db(), active!.id, { userId: session.user.id, role: access.role }) : null;
    return {
      user: { id: session.user.id, email: session.user.email, name: session.user.name },
      workspaces: workspaces.map((w) => ({ id: w.id, name: w.name })),
      active: active ? { id: active.id, name: active.name, plan: active.plan.plan } : null,
      can: {
        readAudit: can({ audit: ["read"] }) && Boolean(active?.plan.entitlements.audit),
        manageWorkspace: can({ workspace: ["manage"] }),
      },
      library: found ? { counts: found.counts, folders: found.folders } : null,
    };
  });
