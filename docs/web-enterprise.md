# Web app: Enterprise features

Single sign-on, SCIM provisioning and workspace policy for Enterprise
workspaces. All of it is gated on the plan's `sso` entitlement
(`web/src/lib/plans.ts`); owners and admins (`workspace: manage` in
`web/src/lib/permissions.ts`) manage it from the admin console at `/admin`.
Members, roles and seats are in the admin console on every plan.

## Single sign-on

Built on `@better-auth/sso`. Each workspace registers its own identity
provider (IdP) for one or more email domains.

1. **Add the provider** in Admin → Single sign-on.
   - **OIDC**: issuer URL, client id and secret (discovery comes from
     `<issuer>/.well-known/openid-configuration` unless you give a URL). In
     the IdP, set the redirect URI to
     `https://<app>/api/auth/sso/callback/<provider id>`.
   - **SAML 2.0**: the IdP's entity id, sign-in URL and signing certificate.
     In the IdP, set the ACS URL to
     `https://<app>/api/auth/sso/saml2/sp/acs/<provider id>` and the SP entity
     id / metadata to
     `https://<app>/api/auth/sso/saml2/sp/metadata?providerId=<provider id>`.
     Send `email`, `displayName`, `givenName` and `surname` attributes.
     Assertions must be signed and timestamped; SHA-1 and other deprecated
     algorithms are rejected.
2. **Verify the domains.** For each domain, publish a TXT record
   `_guiderecorder-verification-<provider id>.<domain>` whose value is the
   token the console shows (or `_guiderecorder-verification-<provider id>=<token>`),
   then press Verify. Every domain on the provider must verify. Tokens last
   seven days. Sign-in through the provider only works once verified.
3. **People sign in** with "Sign in with SSO" on the sign-in page: the app
   finds the provider from their email's domain.

### Domain capture

A verified domain is *captured* by its workspace (`capturingWorkspace` in
`web/src/lib/sso.ts`), as long as the workspace is on Enterprise and no other
workspace has verified the same domain:

- Anyone who signs in through the workspace's IdP joins the workspace, with
  the policy's provisioning role, if there's a seat. It becomes their active
  workspace.
- Anyone who signs in another way with a **verified** email on the domain
  joins too.
- With **Require SSO** on, people on the domain can only sign in through the
  IdP; other sign-ins are refused (`SSO_REQUIRED`) and their new session is
  deleted. Workspace owners are exempt so a broken IdP can't lock everyone
  out, and so is the desktop app's token exchange (its approval already came
  from a web session).

All of this runs once per new session, in an after hook in
`web/src/lib/enterprise.ts`. If a workspace leaves Enterprise, SSO sign-ins
are refused (`SSO_NOT_AVAILABLE`) and its domains stop being captured.

## SCIM provisioning

Built on `@better-auth/scim`; SCIM 2.0 (RFC 7643/7644) at
`https://<app>/api/auth/scim/v2` with `Users`, `Groups`, `ServiceProviderConfig`,
`Schemas` and `ResourceTypes`.

1. Admin → SCIM provisioning → **New connection** shows a bearer token once.
   Only an HMAC of it is stored (keyed by `SCIM_TOKEN_SECRET`). Tokens last a
   year.
2. In the IdP (Okta, Entra ID, OneLogin…), set the SCIM base URL and the
   token as "HTTP header" / bearer authentication. Entra's legacy group
   schema is accepted.
3. **Rotate** with "New token" (the old token keeps working until revoked, so
   switch the IdP over, then revoke). **Disconnect** decommissions the
   connection: its tokens stop working and the members it provisioned are
   removed.

How the directory maps to the workspace (`web/src/lib/scim.ts`):

| Directory | Workspace |
|---|---|
| Active user | Member. Needs a free seat (403 otherwise). |
| User in a group named `Admins`, `Approvers`, `Editors` or `Viewers` (optionally prefixed `GuideRecorder `) | That role; the most privileged wins. |
| User in no such group | The policy's provisioning role. |
| User deactivated or deleted | Removed from the workspace and signed out everywhere. |
| Owners | Never changed. |

A provisioned user whose email is already a GuideRecorder account is linked
to it only if the email is on one of the workspace's verified domains;
otherwise provisioning that user is refused. Once a workspace leaves
Enterprise, every SCIM request from its connections gets 403.

## Policy

Stored in `workspace_policy`; `web/src/lib/policy.ts`.

| Setting | Values | Default |
|---|---|---|
| `sharing` | `off`, `workspace`, `public` | `public` |
| `requireSso` | see above | off |
| `provisioningRole` | `admin`, `approver`, `editor`, `viewer` | `editor` |
| `revisionRetentionDays` | 7–3650, or none | none (keep) |
| `auditRetentionDays` | 30–3650, or none | none (keep) |

### Sharing policy

The most a guide in the workspace may be shared with:

- `public`: share links anyone can open;
- `workspace`: links that only signed-in members of the workspace can open;
- `off`: nothing beyond the workspace's members.

Share-link code (#55) enforces it with
`canShare(db, organizationId, audience)` (or `sharingAllows(policy, audience)`
when it already has the policy), where `audience` is `"public"` or
`"workspace"`. Check it both when a link is made and **every time a link is
opened**, so tightening the policy also closes existing links. #55's own
`shareLinks` setting (anyone / password / off) sits underneath: the policy is
the ceiling, `shareLinks` how links work within it.

## Retention

The retention job (`web/src/lib/retention.ts`, `POST /api/jobs/retention`,
hourly) deletes:

- guides in the trash for 30 days (their revisions; the tombstone stays for
  sync);
- revisions older than `revisionRetentionDays`, except each guide's head and
  published revision;
- images no revision names any more, from storage and the database, a day
  after upload at the earliest;
- expired sessions, verification tokens, device codes and rate-limit
  counters;
- audit events older than `auditRetentionDays`, through the
  `setRetentionHooks({ auditEvents })` hook that the audit log (#51)
  registers.
