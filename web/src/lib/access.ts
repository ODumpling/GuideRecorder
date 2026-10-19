import type { Database } from "@/db";
import type { Entitlements } from "./plans";
import { roles, statements, type RoleName } from "./permissions";
import { memberRole, workspacePlan, type WorkspacePlan } from "./workspace";

export type Permission = { [K in keyof typeof statements]?: (typeof statements)[K][number][] };

export type BooleanEntitlement = { [K in keyof Entitlements]: Entitlements[K] extends boolean ? K : never }[keyof Entitlements];

export type AccessResult =
  | { ok: true; role: RoleName; plan: WorkspacePlan }
  | { ok: false; status: 401 | 402 | 403 | 404; error: string };

/**
 * Whether the user may do `permission` in the workspace on its current plan.
 * Non-members get 404 so workspace ids can't be probed.
 */
export async function checkAccess(
  db: Database,
  userId: string | undefined,
  organizationId: string,
  permission: Permission,
  feature?: BooleanEntitlement,
): Promise<AccessResult> {
  if (!userId) return { ok: false, status: 401, error: "Sign in first." };
  const role = await memberRole(db, organizationId, userId);
  if (!role || !(role in roles)) return { ok: false, status: 404, error: "Workspace not found." };
  if (!roles[role as RoleName].authorize(permission).success) {
    return { ok: false, status: 403, error: "Your role doesn't allow this." };
  }
  const plan = await workspacePlan(db, organizationId);
  if (feature && !plan.entitlements[feature]) {
    return { ok: false, status: 402, error: `Your plan doesn't include ${feature}. Upgrade to use it.` };
  }
  return { ok: true, role: role as RoleName, plan };
}
