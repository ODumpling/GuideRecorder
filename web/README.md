# GuideRecorder Web

The web app for Pro and Enterprise workspaces: accounts, workspaces and
roles, billing, and image storage for synced guides. Later phases add the
cloud library, sharing, the web editor, approvals and audit.

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

## API

| Method | Path | |
|---|---|---|
| `*` | `/api/auth/*` | Better Auth (sign-in, workspaces, invitations, billing, Stripe webhook at `/api/auth/stripe/webhook`) |
| `POST` | `/api/workspaces/:id/assets/uploads` | `{ assets: [{ sha256, size, contentType }] }` → `{ present, uploads: [{ sha256, url, method, headers }] }` |
| `POST` | `/api/workspaces/:id/assets/commit` | `{ sha256: [...] }` → `{ committed, missing }` |
| `GET` | `/api/workspaces/:id/assets/:sha256` | Redirects to a short-lived download URL |
| `GET` | `/api/health` | Database check |

## Tests

`pnpm test` runs against in-memory Postgres (PGlite) with the real
migrations. After changing `src/db/schema.ts`, run `pnpm db:generate`.
