import { createCsrfMiddleware, createMiddleware, createStart } from "@tanstack/react-start";
import { apiRules, clientIp, limitApiRequest } from "@/lib/rate-limit";
import { securityEvent } from "@/lib/security-events";
import { securityHeaders, withHeaders } from "@/lib/security-headers";

// Rate limits for the workspace API and security headers on every response.
// Reads process.env directly: this file is shared with the client bundle.
const rules = apiRules();
const hardening = createMiddleware().server(async ({ request, next }) => {
  const limited = await limitApiRequest(request, rules);
  if (limited) {
    void securityEvent({ type: "rate_limit.exceeded", ip: clientIp(request.headers), details: { path: new URL(request.url).pathname } });
    return limited;
  }
  const result = await next();
  const headers = securityHeaders({
    baseURL: process.env.BETTER_AUTH_URL ?? "",
    storageEndpoint: process.env.S3_ENDPOINT,
    production: process.env.NODE_ENV === "production",
  });
  return { ...result, response: withHeaders(result.response, headers) };
});

// Server functions only accept same-origin requests. Better Auth checks the
// origin of its own endpoints; the asset API needs a session cookie or token.
export const startInstance = createStart(() => ({
  requestMiddleware: [hardening, createCsrfMiddleware({ filter: (ctx) => ctx.handlerType === "serverFn" })],
}));
