import { stripeClient } from "@better-auth/stripe/client";
import { magicLinkClient, organizationClient } from "better-auth/client/plugins";
import { createAuthClient } from "better-auth/react";
import { ac, roles } from "./permissions";

export const authClient = createAuthClient({
  plugins: [organizationClient({ ac, roles }), magicLinkClient(), stripeClient({ subscription: true })],
});
