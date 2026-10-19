# GuideRecorder Web

The web app for Pro and Enterprise workspaces: accounts, workspaces and
roles, billing, image storage for synced guides, and share links. Later
phases add the cloud library, the web editor, approvals and audit.

TanStack Start (Vite, file routes in `src/routes`, server functions in
`src/server`), Better Auth (email/password, magic link, Google, Microsoft,
organizations, Stripe), Drizzle on Postgres, and any S3-compatible store. The `guide.json` types come from
`packages/guide-schema`, generated from the Rust model.

## Run locally

```sh
docker compose up -d          # Postgres and S3-compatible storage (RustFS)
cp .env.example .env.local    # then set BETTER_AUTH_SECRET
pnpm install
pnpm db:migrate
pnpm dev                      # http://localhost:3000
```

Production: `pnpm build`, then `node .output/server/index.mjs` with the
environment set (`.env.local` is only read in development).

Without Stripe keys billing is hidden; set a workspace's plan by hand with
`insert into subscription (id, plan, reference_id, status) values ('dev', 'pro', '<workspace id>', 'active')`.
Without Resend, emails (magic links, invitations) are printed to the console.

## Model

- Every user has a personal workspace (an organization), made on first sign-in.
- Roles: owner, admin, approver, editor, viewer (`src/lib/permissions.ts`).
- Plans: free, pro, enterprise (`src/lib/plans.ts`). Subscriptions belong to
  workspaces; only owners manage billing. Seats and storage follow the plan.
- Images are content-addressed per workspace (`src/lib/assets.ts`): clients
  ask which SHA-256s are missing, PUT them to presigned URLs that only accept
  the matching bytes, then commit them.
- Synced guides (`src/lib/sync.ts`) are chains of immutable revisions, each the
  whole `guide.json` plus a map from its image paths to committed assets. A
  push names the revision it was made on; if that isn't the head, the server
  answers 409 with the head, and the client merges (`packages/sync`) and pushes
  again. Every change gives the guide a new `seq`; clients follow
  `changes?since=<cursor>`. Deleted guides sit in the trash for 30 days, then
  lose their revisions but stay in the feed as tombstones.
- Share links (`src/lib/sharing.ts`) show a guide to anyone with the link at
  `/s/<token>`: a pinned revision or the latest published one (the head,
  until approvals publish), optionally with an expiry, a password (scrypt
  hash; a correct one sets a signed cookie for 12 hours) and download
  allowed. The page is `noindex`, rate limited per address (in memory, per
  process), and serves only the guide and its images, through the token.
  Views are counted per link. On Enterprise, admins can require passwords,
  cap how long links last, or turn links off (`workspace_settings`); this
  applies to existing links too.
- The desktop app signs in with device authorization: it shows a code, the
  user approves it at `/device`, and the app calls the API with the session
  token as a bearer token.

## API

| Method | Path | |
|---|---|---|
| `*` | `/api/auth/*` | Better Auth (sign-in, workspaces, invitations, billing, Stripe webhook at `/api/auth/stripe/webhook`) |
| `POST` | `/api/workspaces/:id/assets/uploads` | `{ assets: [{ sha256, size, contentType }] }` → `{ present, uploads: [{ sha256, url, method, headers }] }` |
| `POST` | `/api/workspaces/:id/assets/commit` | `{ sha256: [...] }` → `{ committed, missing }` |
| `GET` | `/api/workspaces/:id/assets/:sha256` | Redirects to a short-lived download URL |
| `POST` | `/api/workspaces/:id/sync/push` | `{ guideId, baseRevision, guide, assets: { path: sha256 } }` → `{ revision }`; 409 `{ head }` when `baseRevision` isn't the head; 410 if trashed |
| `GET` | `/api/workspaces/:id/sync/changes?since=<cursor>` | `{ changes: [{ type: "revision", revision } \| { type: "deleted", deletedAt, purgeAt }], cursor, hasMore }` |
| `GET` | `/api/workspaces/:id/sync/events` | Server-sent `change` events when a guide changes |
| `GET` / `DELETE` | `/api/workspaces/:id/sync/guides/:guideId` | The head revision / move to the trash |
| `POST` | `/api/workspaces/:id/sync/guides/:guideId/restore` | Take out of the trash |
| `GET` | `/api/workspaces/:id/shares?guideId=<id>` | `{ links: [{ id, url, revisionId, expiresAt, hasPassword, allowDownload, views, lastViewedAt, state }], settings }` |
| `POST` | `/api/workspaces/:id/shares` | `{ guideId, revisionId?, expiresAt?, password?, allowDownload? }` → `{ link }`; `revisionId` null shares the latest published revision |
| `DELETE` | `/api/workspaces/:id/shares/:shareId` | Revoke a link |
| `GET` | `/api/share/:token/assets/:sha256` | Public: an image of the shared revision (password cookie if the link has one) |
| `GET` | `/api/health` | Database check |

API calls take the session cookie or `Authorization: Bearer <token>`; the
share API needs the `sharing` entitlement and the `guide: share` permission.

## Tests

`pnpm test` runs against in-memory Postgres (PGlite) with the real
migrations. After changing `src/db/schema.ts`, run `pnpm db:generate`.
