import { eq } from "drizzle-orm";
import { z } from "zod";
import type { Database } from "@/db";
import { workspacePolicy } from "@/db/schema";
import { PROVISIONING_ROLES } from "./permissions";

// Enterprise workspace policy, set in the admin console: who guides can be
// shared with, whether members on a captured domain must use SSO, the role
// SSO and SCIM give new members, and how long history is kept. Workspaces
// without a row have the defaults. Changing it needs the `sso` entitlement.

/**
 * The most a guide in the workspace may be shared with:
 * - `off`: no sharing beyond the workspace's own members;
 * - `workspace`: links that only signed-in members can open;
 * - `public`: links anyone can open (the default).
 */
export const SHARING_POLICIES = ["off", "workspace", "public"] as const;
export type SharingPolicy = (typeof SHARING_POLICIES)[number];

/** Who a share link is for. */
export type ShareAudience = "workspace" | "public";

export const policySchema = z.object({
  sharing: z.enum(SHARING_POLICIES),
  /** Members whose email is on one of the workspace's verified SSO domains must sign in with SSO. Owners are exempt, so a broken IdP can't lock everyone out. */
  requireSso: z.boolean(),
  /** The role SSO sign-in and SCIM give new members, unless a SCIM group maps to another. */
  provisioningRole: z.enum(PROVISIONING_ROLES),
  /** Revisions older than this are deleted, except each guide's head and published revision. Null keeps them forever. */
  revisionRetentionDays: z.number().int().min(7).max(3650).nullable(),
});

export type WorkspacePolicy = z.infer<typeof policySchema>;

export const DEFAULT_POLICY: WorkspacePolicy = {
  sharing: "public",
  requireSso: false,
  provisioningRole: "editor",
  revisionRetentionDays: null,
};

export async function getPolicy(db: Database, organizationId: string): Promise<WorkspacePolicy> {
  const [row] = await db.select().from(workspacePolicy).where(eq(workspacePolicy.organizationId, organizationId));
  if (!row) return DEFAULT_POLICY;
  return {
    sharing: row.sharing as SharingPolicy,
    requireSso: row.requireSso,
    provisioningRole: row.provisioningRole as WorkspacePolicy["provisioningRole"],
    revisionRetentionDays: row.revisionRetentionDays,
  };
}

export async function setPolicy(db: Database, organizationId: string, update: Partial<WorkspacePolicy>): Promise<WorkspacePolicy> {
  const next = policySchema.parse({ ...(await getPolicy(db, organizationId)), ...update });
  await db
    .insert(workspacePolicy)
    .values({ id: organizationId, organizationId, ...next })
    .onConflictDoUpdate({ target: workspacePolicy.organizationId, set: next });
  return next;
}

/** Whether the policy allows sharing with `audience`. */
export function sharingAllows(policy: Pick<WorkspacePolicy, "sharing">, audience: ShareAudience): boolean {
  if (audience === "public") return policy.sharing === "public";
  return policy.sharing !== "off";
}

/**
 * Whether a guide in the workspace may be shared with `audience` now. Share
 * links check it when a link is made and again every time one is opened, so
 * tightening the policy also closes existing links.
 */
export async function canShare(db: Database, organizationId: string, audience: ShareAudience): Promise<boolean> {
  return sharingAllows(await getPolicy(db, organizationId), audience);
}
