# Web app: security controls

## Encryption

**In transit.** HTTPS only: terminate TLS 1.2+ at the proxy, and the app
sends HSTS when `BETTER_AUTH_URL` is HTTPS. Presigned storage URLs are HTTPS.
Connect to Postgres with `sslmode=require` when it isn't on the same host.

**At rest.**

| Data | Where | How |
|---|---|---|
| Images | S3 | SSE-KMS (`S3_SSE=aws:kms`, customer-managed key); the header is signed into every presigned upload. |
| Images | R2 | Always encrypted by Cloudflare (AES-256-GCM). |
| Database (guides, users, SSO config) | Postgres | The provider's storage encryption (RDS/Cloud SQL use KMS), or an encrypted volume on your own host. |
| Backups | Dump files | Encrypt before copying off the host (`age` or the bucket's SSE-KMS). |
| Passwords | `account.password` | scrypt (Better Auth). |
| SCIM tokens | `scim_managed_credential.token_digest` | HMAC-SHA-256 keyed by `SCIM_TOKEN_SECRET`; the token itself is never stored. |
| Sessions | `session.token` | Random; cookies are `HttpOnly`, `Secure`, `SameSite=Lax`. |
| OIDC client secrets, SAML keys | `sso_provider` | Rely on database encryption at rest; app-level encryption of these columns is a follow-up. |

## Key rotation runbook

Rotate on a schedule (yearly) and at once if a key may have leaked or
someone with access leaves. Record each rotation (date, who, why) for SOC 2.

**`BETTER_AUTH_SECRET`** (signs cookies and state, encrypts tokens).
1. Generate: `openssl rand -base64 32`.
2. Set `BETTER_AUTH_SECRETS=2:<new>,1:<old>` (newest first) and keep
   `BETTER_AUTH_SECRET=<old>` as the legacy fallback; deploy. New data uses the
   new key; old data still decrypts.
3. After the longest session lifetime (7 days), remove the old entry.
   If the secret leaked, skip the overlap: replace it outright and delete all
   sessions (`delete from session;`), which signs everyone out.

**`SCIM_TOKEN_SECRET`.** Changing it invalidates every SCIM token. Set the
new secret, deploy, then have each Enterprise admin make a new token
(Admin → SCIM → New token) and update their IdP. Coordinate with customers
first unless it leaked.

**SCIM tokens** (per workspace). Admins rotate them in the console: New
token, update the IdP, revoke the old one. Tokens expire after a year.

**`JOBS_TOKEN`.** Set the new value on the app and the scheduler together;
a missed run is harmless.

**S3 / R2 access keys.** Create a second key, deploy it, confirm uploads and
downloads work, then delete the old key. Presigned URLs signed with the old
key stop working once it's deleted (they last 15 minutes at most).

**KMS key (S3).** Turn on automatic yearly rotation for the
customer-managed key; AWS keeps old key material for decryption, so nothing
changes for the app. To move to a different key, set `S3_SSE_KMS_KEY_ID` and
the bucket default to the new key; existing objects keep the old key until
copied (`aws s3 cp s3://b s3://b --recursive --sse aws:kms --sse-kms-key-id <new>`).

**Postgres password.** Create a second role or set a new password, update
`DATABASE_URL`, deploy, then drop the old credential.

**Stripe and Resend keys.** Roll in their dashboards (Stripe can keep the old
key valid for a while); update the environment and deploy.

**OIDC client secrets / SAML certificates** (per workspace). Admins update
them in their IdP and re-add the provider; SAML providers accept several
signing certificates during a rollover.

## Rate limiting

- **Better Auth endpoints** (`/api/auth/*`): Better Auth's limiter, stored in
  the `rate_limit` table so all instances share it. 100 requests a minute per
  client address and path by default; sign-in 10, sign-up 5, password reset
  and magic links 5–10, device codes 10, device token polling 30, domain
  verification 10, SCIM 600. On in production. Set `TRUSTED_PROXIES` so the
  client address is the real one, not the proxy's.
- **Workspace API** (`/api/workspaces/*`, `/api/jobs/*`): per user (hash of
  their session token or bearer token), else per address, in memory per
  instance (`web/src/lib/rate-limit.ts`): uploads 120, pushes 240, other
  writes 300, reads 1200 a minute. Over the limit: 429 with `Retry-After`.
- Put your CDN's or load balancer's coarse per-address limit in front for
  floods.

## Security headers

On every response (`web/src/lib/security-headers.ts`): `X-Content-Type-Options:
nosniff`, `X-Frame-Options: DENY`, `Referrer-Policy:
strict-origin-when-cross-origin`, `Permissions-Policy` (no camera, microphone,
location or payment), `Cross-Origin-Opener-Policy: same-origin`, HSTS on
HTTPS, and in production a Content Security Policy: same-origin by default,
images, media and fetches also from the storage endpoint, no plugins, no
framing, forms only to the app or HTTPS (SAML's POST binding). Scripts allow
`'unsafe-inline'` for Start's hydration data; moving to nonces is a
follow-up.

## Other controls

- CSRF: server functions only accept same-origin requests; Better Auth
  checks origins on its endpoints.
- Workspace isolation: every API call checks membership (non-members get
  404), role and plan (`web/src/lib/access.ts`); images are scoped per
  workspace.
- SSO: domain verification before sign-in; SAML assertions must be signed
  and timestamped; deprecated algorithms rejected; IdP-initiated SAML off.
