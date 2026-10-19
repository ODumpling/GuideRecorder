// Plans and what they unlock. Workspaces without an active subscription are
// on `free`: they can sign in and manage members, nothing else.

export type PlanName = "free" | "pro" | "enterprise";

export interface Entitlements {
  /** Desktop app ⇄ web sync and the cloud library. */
  sync: boolean;
  /** Public and unlisted share links. */
  sharing: boolean;
  /** Admins can require passwords on share links, cap how long they last, or turn them off. */
  sharingControls: boolean;
  webEditor: boolean;
  approvals: boolean;
  audit: boolean;
  sso: boolean;
  /** Members per workspace, owner included. */
  seats: number;
  /** Image storage per workspace. */
  storageBytes: number;
}

const GB = 1024 ** 3;

export const ENTITLEMENTS: Record<PlanName, Entitlements> = {
  free: {
    sync: false,
    sharing: false,
    sharingControls: false,
    webEditor: false,
    approvals: false,
    audit: false,
    sso: false,
    seats: 1,
    storageBytes: 0,
  },
  pro: {
    sync: true,
    sharing: true,
    sharingControls: false,
    webEditor: true,
    approvals: false,
    audit: false,
    sso: false,
    seats: 1,
    storageBytes: 20 * GB,
  },
  enterprise: {
    sync: true,
    sharing: true,
    sharingControls: true,
    webEditor: true,
    approvals: true,
    audit: true,
    sso: true,
    seats: 1000,
    storageBytes: 1024 * GB,
  },
};

/** Subscription statuses that keep a plan's features on. */
const ACTIVE = new Set(["active", "trialing", "past_due"]);

export function planOf(subscriptions: { plan: string; status: string | null }[]): PlanName {
  const active = subscriptions.filter((s) => s.status && ACTIVE.has(s.status)).map((s) => s.plan);
  if (active.includes("enterprise")) return "enterprise";
  if (active.includes("pro")) return "pro";
  return "free";
}
