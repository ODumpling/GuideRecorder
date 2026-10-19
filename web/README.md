# GuideRecorder Web

The web app for Pro and Enterprise workspaces: accounts, workspaces and
roles, billing, image storage for synced guides, and the cloud library
(search, reading, downloads). Later phases add sharing, the web editor,
approvals and audit.

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

Downloads (PDF, DOCX, Markdown, HTML) need the export worker, which runs
the Rust CLI: `docker compose --profile worker up -d --build` (builds
`worker/Dockerfile` from the repository root), or locally
`cargo build -p guiderecorder-cli` and
`GUIDERECORDER_CLI=../target/debug/guiderecorder pnpm worker` with the
`DATABASE_URL` and `S3_*` variables set. Run as many workers as you like.

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
- The library (`/library`, `src/lib/library.ts`) lists the guides a member
  can see: every `workspace` guide plus their own `private` ones, never
  trashed ones. Tags, status, owner and the review schedule come from each
  head revision's guide.json (review due = `lastReviewed`, else creation,
  plus `reviewIntervalMonths`; flagged when overdue or due within 30 days).
  Folders are web-only (`guide.folder`); editors and up file guides.
- Search is Postgres full-text search (`src/lib/search.ts`): each push
  writes `guide.search`, a `simple`-configuration tsvector over title and
  tags (weight A), summary and step and section titles (B), and intro,
  prerequisites, step text, notes, hotspots and callouts (C), with a GIN
  index. Every query word matches as a prefix.
- The viewer (`src/components/guides/GuideViewer.tsx`) is the React
  counterpart of the HTML export and its walkthrough: steps, sections,
  notes, screenshots with annotations drawn as the exporter draws them,
  interactive hotspots, and a one-step-at-a-time mode. It takes a guide and
  a function from image paths to URLs, nothing else; in the library that's
  the asset endpoint. Blur shows as a blur over the original image, which
  is still downloadable by anyone who may read the guide: only the exports
  burn redactions into the pixels.
- Images are served by the asset endpoint, which checks that the member
  uploaded the image or can see a guide that shows it, then redirects to a
  five-minute signed URL.
- Exports (`src/lib/exports.ts`) are queued per revision and format in
  `export_job`. The worker (`worker/export.ts`) claims jobs with
  `for update skip locked`, rebuilds the bundle from the revision and its
  assets, runs `guiderecorder export`, zips Markdown, and stores the file at
  `orgs/<workspace>/exports/<revision>/<format>.<ext>`. Revisions never
  change, so a finished export is served again. Jobs whose worker died are
  taken back after 10 minutes, up to 3 tries.
- The desktop app signs in with device authorization: it shows a code, the
  user approves it at `/device`, and the app calls the API with the session
  token as a bearer token.

## API

| Method | Path | |
|---|---|---|
| `*` | `/api/auth/*` | Better Auth (sign-in, workspaces, invitations, billing, Stripe webhook at `/api/auth/stripe/webhook`) |
| `POST` | `/api/workspaces/:id/assets/uploads` | `{ assets: [{ sha256, size, contentType }] }` → `{ present, uploads: [{ sha256, url, method, headers }] }` |
| `POST` | `/api/workspaces/:id/assets/commit` | `{ sha256: [...] }` → `{ committed, missing }` |
| `GET` | `/api/workspaces/:id/assets/:sha256` | Redirects to a short-lived download URL, if the caller uploaded the image or can see a guide that shows it |
| `POST` | `/api/workspaces/:id/sync/push` | `{ guideId, baseRevision, guide, assets: { path: sha256 } }` → `{ revision }`; 409 `{ head }` when `baseRevision` isn't the head; 410 if trashed |
| `GET` | `/api/workspaces/:id/sync/changes?since=<cursor>` | `{ changes: [{ type: "revision", revision } \| { type: "deleted", deletedAt, purgeAt }], cursor, hasMore }` |
| `GET` | `/api/workspaces/:id/sync/events` | Server-sent `change` events when a guide changes |
| `GET` / `DELETE` | `/api/workspaces/:id/sync/guides/:guideId` | The head revision / move to the trash |
| `POST` | `/api/workspaces/:id/sync/guides/:guideId/restore` | Take out of the trash |

| `GET` | `/api/health` | Database check |

API calls take the session cookie or `Authorization: Bearer <token>`. The
library, viewer and downloads use server functions (`src/server/guides.ts`).

## Tests

`pnpm test` runs against in-memory Postgres (PGlite) with the real
migrations. After changing `src/db/schema.ts`, run `pnpm db:generate`.
Set `GUIDERECORDER_CLI` to a built CLI to also run the exports through it.
