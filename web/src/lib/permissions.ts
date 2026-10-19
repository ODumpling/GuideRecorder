import { createAccessControl } from "better-auth/plugins/access";
import { defaultStatements, adminAc, ownerAc, memberAc } from "better-auth/plugins/organization/access";

// Workspace roles. `approver` and `audit` only take effect on plans with
// approvals and audit (see plans.ts); the server checks both. `workspace:
// manage` opens the admin console: SSO, SCIM, sharing policy and retention.
export const statements = {
  ...defaultStatements,
  guide: ["read", "create", "update", "delete", "share", "approve"],
  audit: ["read"],
  billing: ["manage"],
  workspace: ["manage"],
} as const;

export const ac = createAccessControl(statements);

export const roles = {
  owner: ac.newRole({
    ...ownerAc.statements,
    guide: ["read", "create", "update", "delete", "share", "approve"],
    audit: ["read"],
    billing: ["manage"],
    workspace: ["manage"],
  }),
  admin: ac.newRole({
    ...adminAc.statements,
    guide: ["read", "create", "update", "delete", "share", "approve"],
    audit: ["read"],
    workspace: ["manage"],
  }),
  approver: ac.newRole({
    ...memberAc.statements,
    guide: ["read", "create", "update", "share", "approve"],
  }),
  editor: ac.newRole({
    ...memberAc.statements,
    guide: ["read", "create", "update", "share"],
  }),
  viewer: ac.newRole({
    ...memberAc.statements,
    guide: ["read"],
  }),
};

export type RoleName = keyof typeof roles;
export const ROLE_NAMES = Object.keys(roles) as RoleName[];

/** Roles SSO and SCIM may give; never owner. */
export const PROVISIONING_ROLES = ["admin", "approver", "editor", "viewer"] as const satisfies readonly RoleName[];

/** Most to least privileged, for picking one role from several grants. */
export const ROLE_RANK: RoleName[] = ["owner", "admin", "approver", "editor", "viewer"];
