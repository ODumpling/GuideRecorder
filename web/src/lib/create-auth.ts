import { stripe as stripePlugin } from "@better-auth/stripe";
import { betterAuth } from "better-auth";
import { APIError } from "better-auth/api";
import { drizzleAdapter } from "better-auth/adapters/drizzle";
import { nextCookies } from "better-auth/next-js";
import { magicLink, organization } from "better-auth/plugins";
import type Stripe from "stripe";
import type { Database } from "@/db";
import * as schema from "@/db/schema";
import type { SendEmail } from "./email";
import { ac, roles } from "./permissions";
import { ensureWorkspace, memberRole, seatsTaken, workspacePlan } from "./workspace";

export interface AuthDeps {
  db: Database;
  baseURL: string;
  secret: string;
  sendEmail: SendEmail;
  social?: {
    google?: { clientId: string; clientSecret: string };
    microsoft?: { clientId: string; clientSecret: string };
  };
  /** Billing; off when unset (local development without Stripe). */
  stripe?: {
    client: Stripe;
    webhookSecret: string;
    prices: { pro?: string; enterprise?: string };
  };
}

/**
 * Better Auth for the web app: email and password, magic links, Google and
 * Microsoft, workspaces (organizations) with our roles, and Stripe
 * subscriptions owned by workspaces. Every user has a personal workspace,
 * made on first sign-in.
 */
export function createAuth(deps: AuthDeps) {
  const { db, sendEmail } = deps;

  const workspaces = organization({
    ac,
    roles,
    // Seats come from the workspace's plan. Better Auth checks them when an
    // invitation is accepted; pending invitations hold a seat too.
    membershipLimit: async (_user, org) => (await workspacePlan(db, org.id)).entitlements.seats,
    organizationHooks: {
      beforeCreateInvitation: async ({ invitation }) => {
        const { seats } = (await workspacePlan(db, invitation.organizationId)).entitlements;
        if ((await seatsTaken(db, invitation.organizationId)) >= seats) {
          throw new APIError("FORBIDDEN", { message: "No seats left. Upgrade to invite more people." });
        }
      },
    },
    sendInvitationEmail: async ({ id, email, organization, inviter }) => {
      await sendEmail({
        to: email,
        subject: `Join ${organization.name} on GuideRecorder`,
        text: `${inviter.user.name} invited you to ${organization.name}.\n\n${deps.baseURL}/invitations/${id}`,
      });
    },
  });

  const billing = deps.stripe && billingPlugin(db, deps.stripe);

  const socialProviders: Record<string, { clientId: string; clientSecret: string }> = {};
  if (deps.social?.google) socialProviders.google = deps.social.google;
  if (deps.social?.microsoft) socialProviders.microsoft = deps.social.microsoft;

  return betterAuth({
    baseURL: deps.baseURL,
    secret: deps.secret,
    database: drizzleAdapter(db, { provider: "pg", schema }),
    emailAndPassword: { enabled: true, minPasswordLength: 10 },
    emailVerification: {
      sendVerificationEmail: async ({ user, url }) => {
        await sendEmail({ to: user.email, subject: "Verify your email", text: `Verify your email: ${url}` });
      },
    },
    socialProviders,
    databaseHooks: {
      session: {
        create: {
          before: async (session, ctx) => {
            const user = await ctx?.context.internalAdapter.findUserById(session.userId);
            const name = user?.name ?? "";
            return { data: { ...session, activeOrganizationId: await ensureWorkspace(db, { id: session.userId, name }) } };
          },
        },
      },
    },
    plugins: [
      workspaces,
      magicLink({
        sendMagicLink: async ({ email, url }) => {
          await sendEmail({ to: email, subject: "Sign in to GuideRecorder", text: `Sign in: ${url}` });
        },
      }),
      ...(billing ? [billing] : []),
      // Must stay last: sets cookies from server actions.
      nextCookies(),
    ],
  });
}

export type Auth = ReturnType<typeof createAuth>;

/** Stripe subscriptions owned by workspaces; only owners manage them. */
function billingPlugin(db: Database, config: NonNullable<AuthDeps["stripe"]>) {
  const { prices } = config;
  return stripePlugin({
    stripeClient: config.client,
    stripeWebhookSecret: config.webhookSecret,
    organization: { enabled: true },
    subscription: {
      enabled: true,
      plans: [
        ...(prices.pro ? [{ name: "pro", priceId: prices.pro }] : []),
        ...(prices.enterprise ? [{ name: "enterprise", priceId: prices.enterprise }] : []),
      ],
      authorizeReference: async ({ user, referenceId }) => (await memberRole(db, referenceId, user.id)) === "owner",
    },
  });
}
