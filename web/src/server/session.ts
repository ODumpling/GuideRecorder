import { createServerFn } from "@tanstack/react-start";
import { authFeatures, getSession } from "@/lib/auth";

/** The signed-in user, or null. */
export const getViewer = createServerFn({ method: "GET" }).handler(async () => {
  const session = await getSession();
  return session ? { id: session.user.id, email: session.user.email, name: session.user.name } : null;
});

/** Which sign-in providers are configured. */
export const getProviders = createServerFn({ method: "GET" }).handler(async () => {
  const { google, microsoft } = authFeatures();
  return { google, microsoft };
});
