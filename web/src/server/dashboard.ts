import { redirect } from "@tanstack/react-router";
import { createServerFn } from "@tanstack/react-start";
import { authFeatures, getSession } from "@/lib/auth";
import { loadDashboard } from "@/lib/dashboard";
import { roles, type RoleName } from "@/lib/permissions";

type Permission = Parameters<(typeof roles)[RoleName]["authorize"]>[0];

/** Everything the dashboard shows, with what the user may do in their active workspace. */
export const getDashboard = createServerFn({ method: "GET" }).handler(async () => {
  const session = await getSession();
  if (!session) throw redirect({ to: "/sign-in", search: { next: "/dashboard" } });
  const { workspaces, active } = await loadDashboard(session.user.id, session.session.activeOrganizationId);
  const can = (permission: Permission) =>
    Boolean(active && active.role in roles && roles[active.role as RoleName].authorize(permission).success);
  return {
    user: { id: session.user.id, email: session.user.email },
    workspaces,
    active,
    billingEnabled: authFeatures().billing,
    can: {
      invite: can({ invitation: ["create"] }),
      manageMembers: can({ member: ["update"] }),
      manageBilling: can({ billing: ["manage"] }),
      readAudit: can({ audit: ["read"] }) && Boolean(active?.plan.entitlements.audit),
      manageWorkspace: can({ workspace: ["manage"] }),
    },
  };
});
