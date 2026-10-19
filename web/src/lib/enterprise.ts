import type { BetterAuthPlugin } from "better-auth";
import { APIError, createAuthMiddleware } from "better-auth/api";
import { deleteSessionCookie } from "better-auth/cookies";
import { eq } from "drizzle-orm";
import type { Database } from "@/db";
import { session as sessionTable, ssoProvider } from "@/db/schema";
import { scimTokenWorkspace } from "./scim";
import { securityEvent } from "./security-events";
import { joinWorkspace, signInDecision } from "./sso";
import { ensureWorkspace, workspacePlan } from "./workspace";

// Ties SSO and SCIM to the `sso` entitlement and our workspaces:
// - SSO provider endpoints only work for workspaces whose plan includes SSO,
//   and providers always belong to a workspace;
// - SCIM requests are refused once a workspace's plan stops including SSO;
// - every new session goes through `signInDecision` (domain capture and
//   "require SSO"), and the user gets their personal workspace.
// Session work happens in an after hook, once Better Auth's transaction has
// committed, so it can use the database directly.

/** SSO endpoints that change a workspace's providers. */
const MANAGE_PATHS = new Set(["/sso/register", "/sso/update-provider", "/sso/request-domain-verification", "/sso/verify-domain"]);

const notEntitled = () => new APIError("PAYMENT_REQUIRED", { message: "Your plan doesn't include sso. Upgrade to use it.", code: "SSO_NOT_AVAILABLE" });

export function enterprisePlugin(db: Database) {
  return {
    id: "guiderecorder-enterprise",
    // Tables the SCIM projection reads through Better Auth's adapter (inside
    // its transaction). `subscription` merges with the Stripe plugin's.
    schema: {
      workspacePolicy: {
        fields: {
          organizationId: { type: "string", required: true },
          sharing: { type: "string", required: true },
          requireSso: { type: "boolean", required: true },
          provisioningRole: { type: "string", required: true },
          revisionRetentionDays: { type: "number", required: false },
          auditRetentionDays: { type: "number", required: false },
        },
      },
      subscription: {
        fields: {
          plan: { type: "string", required: true },
          referenceId: { type: "string", required: true },
          status: { type: "string", required: false },
          seats: { type: "number", required: false },
        },
      },
    },

    async onRequest(request, ctx) {
      if (!new URL(request.url).pathname.includes("/scim/v2/")) return;
      const organizationId = await scimTokenWorkspace(ctx.adapter, request.headers.get("authorization"));
      // Unknown tokens get the plugin's own 401.
      if (!organizationId || (await workspacePlan(db, organizationId)).entitlements.sso) return;
      await securityEvent({ type: "scim.request_blocked", organizationId, actorId: "scim", details: { reason: "plan" } });
      return {
        response: Response.json(
          { schemas: ["urn:ietf:params:scim:api:messages:2.0:Error"], status: "403", detail: "The workspace's plan doesn't include SCIM provisioning." },
          { status: 403, headers: { "content-type": "application/scim+json" } },
        ),
      };
    },

    hooks: {
      before: [
        {
          matcher: (ctx) => MANAGE_PATHS.has(ctx.path ?? ""),
          handler: createAuthMiddleware(async (ctx) => {
            const body = (ctx.body ?? {}) as { organizationId?: string; providerId?: string };
            let organizationId = body.organizationId;
            if (ctx.path === "/sso/register") {
              if (!organizationId) throw new APIError("BAD_REQUEST", { message: "SSO providers belong to a workspace: pass organizationId." });
            } else if (body.providerId) {
              const [provider] = await db.select({ organizationId: ssoProvider.organizationId }).from(ssoProvider).where(eq(ssoProvider.providerId, body.providerId));
              organizationId = provider?.organizationId ?? undefined;
            }
            if (organizationId && !(await workspacePlan(db, organizationId)).entitlements.sso) throw notEntitled();
          }),
        },
      ],
      after: [
        {
          matcher: () => true,
          handler: createAuthMiddleware(async (ctx) => {
            const created = ctx.context.newSession;
            if (!created) return;
            const path = ctx.path ?? "";
            const providerId = (ctx.params as { providerId?: string } | undefined)?.providerId;
            const decision = await signInDecision(db, created.user, { path, providerId });

            if (!decision.allow) {
              await db.delete(sessionTable).where(eq(sessionTable.id, created.session.id));
              deleteSessionCookie(ctx);
              await securityEvent({
                type: "sso.sign_in_blocked",
                organizationId: decision.organizationId,
                targetId: created.user.id,
                ip: created.session.ipAddress,
                details: { code: decision.code, path },
              });
              // Browser flows land back on the sign-in page with the reason.
              if (path.startsWith("/sso/") || path.startsWith("/callback/") || path.startsWith("/magic-link/")) {
                throw ctx.redirect(`${ctx.context.baseURL.replace(/\/api\/auth$/, "")}/sign-in?error=${decision.code}`);
              }
              throw new APIError("FORBIDDEN", { message: decision.message, code: decision.code });
            }

            const personal = await ensureWorkspace(db, created.user);
            const joined = decision.join && (await joinWorkspace(db, decision.join.organizationId, created.user.id, decision.join.policy));
            const active = joined ? decision.join!.organizationId : (created.session.activeOrganizationId ?? personal);
            if (active !== created.session.activeOrganizationId) {
              await db.update(sessionTable).set({ activeOrganizationId: active }).where(eq(sessionTable.id, created.session.id));
            }
          }),
        },
        {
          matcher: (ctx) => ctx.path === "/sso/register" || ctx.path === "/sso/verify-domain" || ctx.path === "/sso/delete-provider" || ctx.path === "/sso/request-domain-verification",
          handler: createAuthMiddleware(async (ctx) => {
            if (ctx.context.returned instanceof Error) return;
            const body = (ctx.body ?? {}) as { organizationId?: string; providerId?: string };
            const type = (
              {
                "/sso/register": "sso.provider_registered",
                "/sso/verify-domain": "sso.domain_verified",
                "/sso/delete-provider": "sso.provider_deleted",
                "/sso/request-domain-verification": "sso.domain_verification_requested",
              } as const
            )[ctx.path as "/sso/register"];
            await securityEvent({ type, organizationId: body.organizationId, actorId: ctx.context.session?.user.id, targetId: body.providerId });
          }),
        },
      ],
    },
  } satisfies BetterAuthPlugin;
}
