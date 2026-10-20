import "@tanstack/react-start/server-only";
import { getRequestHeaders } from "@tanstack/react-start/server";
import Stripe from "stripe";
import { db } from "@/db";
import { createAuth, type Auth } from "./create-auth";
import { emailSender } from "./email";
import { env } from "./env";

let cached: Auth | undefined;

export function auth(): Auth {
  if (cached) return cached;
  const e = env();
  cached = createAuth({
    db: db(),
    baseURL: e.BETTER_AUTH_URL,
    secret: e.BETTER_AUTH_SECRET,
    sendEmail: emailSender({ resendApiKey: e.RESEND_API_KEY, from: e.EMAIL_FROM }),
    social: {
      google: e.GOOGLE_CLIENT_ID && e.GOOGLE_CLIENT_SECRET ? { clientId: e.GOOGLE_CLIENT_ID, clientSecret: e.GOOGLE_CLIENT_SECRET } : undefined,
      microsoft:
        e.MICROSOFT_CLIENT_ID && e.MICROSOFT_CLIENT_SECRET
          ? { clientId: e.MICROSOFT_CLIENT_ID, clientSecret: e.MICROSOFT_CLIENT_SECRET }
          : undefined,
    },
    stripe:
      e.STRIPE_SECRET_KEY && e.STRIPE_WEBHOOK_SECRET
        ? {
            client: new Stripe(e.STRIPE_SECRET_KEY),
            webhookSecret: e.STRIPE_WEBHOOK_SECRET,
            prices: { pro: e.STRIPE_PRICE_PRO, enterprise: e.STRIPE_PRICE_ENTERPRISE },
          }
        : undefined,
    scimTokenSecret: e.SCIM_TOKEN_SECRET,
    trustedProxies: e.TRUSTED_PROXIES?.split(",").map((p) => p.trim()).filter(Boolean),
  });
  return cached;
}

/** The signed-in session for this request, or null. */
export async function getSession() {
  return auth().api.getSession({ headers: getRequestHeaders() });
}

/** Which sign-in methods and billing are configured, for the UI. */
export function authFeatures() {
  const e = env();
  return {
    google: Boolean(e.GOOGLE_CLIENT_ID),
    microsoft: Boolean(e.MICROSOFT_CLIENT_ID),
    billing: Boolean(e.STRIPE_SECRET_KEY && e.STRIPE_WEBHOOK_SECRET),
  };
}
