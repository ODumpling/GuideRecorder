import { getCurrentAdapter, type BetterAuthPlugin } from "better-auth";
import { APIError, createAuthMiddleware } from "better-auth/api";
import { eq } from "drizzle-orm";
import { z } from "zod";
import type { Database } from "@/db";
import { platformSettings } from "@/db/schema";
import type { PlanName } from "./plans";
import { securityEvent } from "./security-events";

// Settings for the whole service, set in the staff console (/staff/settings):
// whether anyone can create an account, and which paid plans can be bought.

export const PAID_PLANS = ["pro", "enterprise"] as const satisfies readonly PlanName[];
export type PaidPlan = (typeof PAID_PLANS)[number];

export const platformSchema = z.object({
  /** Anyone can create an account. When off, only invited people and SSO or SCIM users can. */
  registration: z.boolean(),
  /** Paid plans no one can subscribe to. Existing subscriptions and licence grants keep working. */
  disabledPlans: z.array(z.enum(PAID_PLANS)),
});

export type PlatformSettings = z.infer<typeof platformSchema>;

export const DEFAULT_PLATFORM: PlatformSettings = { registration: true, disabledPlans: [] };

const ID = "default";

export async function getPlatformSettings(db: Database): Promise<PlatformSettings> {
  const [row] = await db.select().from(platformSettings).where(eq(platformSettings.id, ID));
  if (!row) return DEFAULT_PLATFORM;
  return { registration: row.registration, disabledPlans: row.disabledPlans.filter((p): p is PaidPlan => (PAID_PLANS as readonly string[]).includes(p)) };
}

export async function setPlatformSettings(db: Database, actorId: string, update: Partial<PlatformSettings>): Promise<PlatformSettings> {
  const before = await getPlatformSettings(db);
  const next = platformSchema.parse({ ...before, ...update });
  next.disabledPlans = PAID_PLANS.filter((p) => next.disabledPlans.includes(p));
  await db
    .insert(platformSettings)
    .values({ id: ID, ...next, updatedBy: actorId })
    .onConflictDoUpdate({ target: platformSettings.id, set: { ...next, updatedBy: actorId } });
  const changed = Object.fromEntries(
    (Object.keys(next) as (keyof PlatformSettings)[])
      .filter((k) => JSON.stringify(before[k]) !== JSON.stringify(next[k]))
      .map((k) => [k, { from: before[k], to: next[k] }]),
  );
  await securityEvent({ type: "staff.platform_updated", actorId, details: changed });
  return next;
}

/** The paid plans workspaces can subscribe to: those with a Stripe price, minus the disabled ones. */
export function offeredPlans(settings: PlatformSettings, priced: Partial<Record<PaidPlan, unknown>>): PaidPlan[] {
  return PAID_PLANS.filter((p) => priced[p] && !settings.disabledPlans.includes(p));
}

/** Paths that create users for a workspace's own SSO or SCIM, which work while registration is closed. */
const PROVISIONING = /^\/(sso|scim)\//;

/**
 * Enforces the settings in Better Auth:
 * - while registration is closed, new users need a pending invitation, or
 *   to come from a workspace's SSO or SCIM;
 * - checkout refuses disabled plans.
 */
export function platformPlugin(db: Database) {
  return {
    id: "guiderecorder-platform",
    // Read through Better Auth's adapter while creating a user, which can be
    // inside its transaction.
    schema: {
      platformSettings: { fields: { registration: { type: "boolean", required: true } } },
    },

    init() {
      return {
        options: {
          databaseHooks: {
            user: {
              create: {
                before: async (created, ctx) => {
                  // Without a request (scripts, tests) or for SSO and SCIM.
                  if (!ctx || PROVISIONING.test(ctx.path ?? "")) return;
                  const adapter = await getCurrentAdapter(ctx.context.adapter);
                  const settings = await adapter.findOne<{ registration: boolean }>({ model: "platformSettings", where: [{ field: "id", value: ID }] });
                  if (settings?.registration !== false) return;
                  const invitations = await adapter.findMany<{ expiresAt: Date }>({
                    model: "invitation",
                    where: [
                      { field: "email", value: created.email.toLowerCase() },
                      { field: "status", value: "pending" },
                    ],
                  });
                  if (invitations.some((i) => new Date(i.expiresAt) > new Date())) return;
                  // Google and Microsoft land back on the sign-in page with the reason.
                  if (ctx.path?.startsWith("/callback/")) {
                    throw ctx.redirect(`${ctx.context.baseURL.replace(/\/api\/auth$/, "")}/sign-in?error=REGISTRATION_CLOSED`);
                  }
                  throw new APIError("FORBIDDEN", { message: "New sign-ups are closed.", code: "REGISTRATION_CLOSED" });
                },
              },
            },
          },
        },
      };
    },

    hooks: {
      before: [
        {
          matcher: (ctx) => ctx.path === "/subscription/upgrade",
          handler: createAuthMiddleware(async (ctx) => {
            const plan = (ctx.body as { plan?: string } | undefined)?.plan;
            const { disabledPlans } = await getPlatformSettings(db);
            if (plan && (disabledPlans as string[]).includes(plan)) {
              throw new APIError("FORBIDDEN", { message: "This plan isn't available.", code: "PLAN_UNAVAILABLE" });
            }
          }),
        },
      ],
    },
  } satisfies BetterAuthPlugin;
}
