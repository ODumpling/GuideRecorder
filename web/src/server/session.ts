import { createServerFn } from "@tanstack/react-start";
import { db } from "@/db";
import { authFeatures, getSession } from "@/lib/auth";
import { getPlatformSettings } from "@/lib/platform";

/** The signed-in user, or null. */
export const getViewer = createServerFn({ method: "GET" }).handler(async () => {
  const session = await getSession();
  return session ? { id: session.user.id, email: session.user.email, name: session.user.name } : null;
});

/** Which sign-in providers are configured, and whether new accounts can be created. */
export const getProviders = createServerFn({ method: "GET" }).handler(async () => {
  const { google, microsoft } = authFeatures();
  const { registration } = await getPlatformSettings(db());
  return { google, microsoft, registration };
});
