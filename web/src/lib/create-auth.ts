import { createHmac } from "node:crypto";
import { stripe as stripePlugin } from "@better-auth/stripe";
import { betterAuth, getCurrentAdapter } from "better-auth";
import { APIError } from "better-auth/api";
import { drizzleAdapter } from "better-auth/adapters/drizzle";
import { bearer, deviceAuthorization, magicLink, organization } from "better-auth/plugins";
import { tanstackStartCookies } from "better-auth/tanstack-start";
import type Stripe from "stripe";
import type { Database } from "@/db";
import * as schema from "@/db/schema";
import { auditBilling, membershipAudit } from "./auth-audit";
import type { SendEmail } from "./email";
import { enterprisePlugin } from "./enterprise";
import { platformPlugin } from "./platform";
import { ac, roles } from "./permissions";
import { scimPlugin } from "./scim";
import { ssoPlugin } from "./sso";
import { memberRole, seatsTaken, workspacePlan } from "./workspace";

/** The desktop app's client id for device authorization. */
export const DESKTOP_CLIENT_ID = "guiderecorder-desktop";

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
  /** HMAC secret for SCIM bearer tokens (32+ characters); derived from `secret` when unset. */
  scimTokenSecret?: string;
  /** Better Auth's own rate limits; on in production when unset. */
  rateLimit?: boolean;
  /** Reverse proxies whose X-Forwarded-For to trust for client addresses. */
  trustedProxies?: string[];
}

/**
 * Better Auth for the web app: email and password, magic links, Google and
 * Microsoft, workspaces (organizations) with our roles, and Stripe
 * subscriptions owned by workspaces. Every user has a personal workspace,
 * made on first sign-in. The desktop app signs in with device authorization
 * and calls the API with a bearer token. Enterprise workspaces add SSO (OIDC
 * and SAML) and SCIM provisioning.
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
      ...membershipAudit(db),
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
    // Native transactions: the SCIM plugin needs them. Hooks that can run
    // inside one read through Better Auth's adapter, not `db`.
    database: drizzleAdapter(db, { provider: "pg", schema, transaction: true }),
    emailAndPassword: { enabled: true, minPasswordLength: 10 },
    emailVerification: {
      sendVerificationEmail: async ({ user, url }) => {
        await sendEmail({ to: user.email, subject: "Verify your email", text: `Verify your email: ${url}` });
      },
    },
    socialProviders,
    // Per client address, in the database so every app instance shares the
    // counts. The API outside /api/auth has its own limits (rate-limit.ts).
    rateLimit: {
      enabled: deps.rateLimit,
      storage: "database",
      window: 60,
      max: 100,
      customRules: {
        "/sign-in/*": { window: 60, max: 10 },
        "/sign-up/*": { window: 60, max: 5 },
        "/magic-link/*": { window: 60, max: 10 },
        "/request-password-reset": { window: 300, max: 5 },
        "/device/code": { window: 60, max: 10 },
        // The desktop app polls every few seconds while a code is pending.
        "/device/token": { window: 60, max: 30 },
        "/sso/verify-domain": { window: 60, max: 10 },
        // Directory syncs come in bursts.
        "/scim/v2/*": { window: 60, max: 600 },
      },
    },
    advanced: { ipAddress: { trustedProxies: deps.trustedProxies } },
    databaseHooks: {
      session: {
        create: {
          // Starts in the user's oldest workspace. This can run inside a
          // transaction, so it reads through the adapter; new users get
          // their personal workspace once it commits (enterprise.ts).
          before: async (session, ctx) => {
            if (!ctx) return;
            const adapter = await getCurrentAdapter(ctx.context.adapter);
            const [first] = await adapter.findMany<{ organizationId: string }>({
              model: "member",
              where: [{ field: "userId", value: session.userId }],
              sortBy: { field: "createdAt", direction: "asc" },
              limit: 1,
            });
            return { data: { ...session, activeOrganizationId: first?.organizationId ?? null } };
          },
        },
      },
    },
    plugins: [
      workspaces,
      // Before SCIM: refuses SCIM requests from workspaces without `sso`.
      enterprisePlugin(db),
      platformPlugin(db),
      ssoPlugin(),
      scimPlugin({ credentialHashSecret: deps.scimTokenSecret ?? deriveSecret(deps.secret, "scim-token") }),
      magicLink({
        sendMagicLink: async ({ email, url }) => {
          await sendEmail({ to: email, subject: "Sign in to GuideRecorder", text: `Sign in: ${url}` });
        },
      }),
      // The desktop app signs in with a code shown on /device and then sends
      // the session token as a bearer token.
      deviceAuthorization({
        verificationUri: "/device",
        expiresIn: "15m",
        interval: "5s",
        validateClient: (clientId) => clientId === DESKTOP_CLIENT_ID,
      }),
      bearer(),
      ...(billing ? [billing] : []),
      // Must stay last: sets cookies when server functions call auth.api.
      tanstackStartCookies(),
    ],
  });
}

export type Auth = ReturnType<typeof createAuth>;

/** A secret for one purpose, derived from the main one. */
function deriveSecret(secret: string, purpose: string): string {
  return createHmac("sha256", secret).update(purpose).digest("hex");
}

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
      onSubscriptionCreated: ({ subscription, event }) => auditBilling(db, "created", subscription, event),
      onSubscriptionComplete: ({ subscription, event }) => auditBilling(db, "complete", subscription, event),
      onSubscriptionUpdate: ({ subscription, event }) => auditBilling(db, "update", subscription, event),
      onSubscriptionCancel: ({ subscription, event }) => auditBilling(db, "cancel", subscription, event),
      onSubscriptionDeleted: ({ subscription, event }) => auditBilling(db, "deleted", subscription, event),
    },
  });
}
