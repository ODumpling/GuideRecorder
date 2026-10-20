# GuideRecorder Web

The web app for Pro and Enterprise workspaces: accounts, workspaces and
roles, billing, image storage for synced guides, the cloud library (search,
reading, downloads), the web editor, share links, and for Enterprise
approvals, an audit log, SSO, SCIM, an admin console and data retention.

TanStack Start (Vite, file routes in `src/routes`, server functions in
`src/server`), Better Auth (email/password, magic link, Google, Microsoft,
organizations, Stripe), Drizzle on Postgres, and any S3-compatible store. The `guide.json` types come from
`packages/guide-schema`, generated from the Rust model; the editors, step
view, annotation shapes and theme tokens from `packages/ui`, shared with the
desktop app.

## Run locally

```sh
docker compose up -d          # Postgres and S3-compatible storage (RustFS)
cp .env.example .env.local    # then set BETTER_AUTH_SECRET
pnpm install
pnpm db:migrate
pnpm db:seed                  # dev accounts, password "password123" (scripts/seed.ts)
pnpm dev                      # http://localhost:3000
```

Production: `pnpm build`, then `node .output/server/index.mjs` with the
environment set (`.env.local` is only read in development), or the Docker
image and `compose.prod.yaml`. See [deployment](../docs/web-deployment.md),
[security](../docs/web-security.md), [Enterprise](../docs/web-enterprise.md)
and [SOC 2](../docs/soc2.md).

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
- The web editor (`/guides/:id/edit`, `src/lib/edit.ts`) edits a guide's text:
  its title, introduction and prerequisites, step titles and descriptions,
  section titles and notes; steps, sections and notes can be reordered and
  deleted, not added. Images and annotations are shown read-only. A save goes
  through the same `push` as a sync from the desktop app (source `web`), on
  the revision the edit began on. If the guide changed meanwhile, the server
  merges the edit into the head with `packages/sync`, as the app would, and
  pushes again; clashing edits to the same field come back for the user to
  choose which to keep. Text the user writes is marked edited, so the
  app's AI passes leave it. Needs the `guide:update` permission and a plan
  with `webEditor`.
- Approvals (Enterprise, `src/lib/review.ts`, `src/lib/approvals.ts`): a guide
  has a head (the latest revision) and a published revision. Viewers (no
  `guide:update`) and unpinned share links (`publishedRevisionFor`) only
  see the published one. Ungated guides publish on every push. A guide is gated
  when the workspace has `approvals` and its folder's nearest policy
  (`approval_policy`, keyed by folder path, "" for the whole workspace;
  default 1 approval, no self-approval, reset on new revisions) needs at
  least one approval. Pushes to a gated guide only move the head, and the
  server sets its `status`, `lastReviewed` and `version`, so neither the app
  nor the editor can publish it. Submitting opens a `change_request` that
  follows the head while open; reviewers (`guide:approve`) approve, reject,
  request changes or comment (`review`, optionally on one item by ULID) on
  `/guides/:id/review`, which diffs the head against the published revision
  item by item with old and new screenshots side by side. Once the policy's
  N of M approvals count and nobody's request for changes is outstanding,
  the server publishes: a new revision with `status: published`, today's
  `lastReviewed` and the next `version`. Status maps onto `GuideStatus`:
  draft, inReview (open request), needsUpdate (changes requested),
  published (head is published). Review steps move the guide in the sync
  feed, which carries each guide's `review` state. A guide's folder is its
  library folder (`guide.folder`).
- Audit log (Enterprise, `src/lib/audit.ts`): `audit_event` is append-only (a
  trigger refuses updates and deletes, except retention pruning) and
  hash-chained per workspace: each event hashes its fields (actor, action,
  target, before/after state hashes, IP, user agent, time, metadata) and the
  previous event's hash. Sign-ins, membership, role and invitation changes,
  billing webhooks, guide create/edit/delete/restore/publish, reviews,
  policies, audit exports and retention changes are logged; `share.*` and
  `guide.export` are reserved for share links and exports. `/audit` filters
  the log, checks the chain, exports CSV or JSON and sets retention (30 to
  3650 days, or forever); pruning records where the chain now starts.
  Reading needs `audit:read` and the `audit` entitlement.
- Share links (`src/lib/sharing.ts`) show a guide to anyone with the link at
  `/s/<token>`: a pinned revision or the latest published one (see
  Approvals), optionally with an expiry, a password (scrypt
  hash; a correct one sets a signed cookie for 12 hours) and download
  allowed. The page is `noindex`, rate limited per address (in memory, per
  process), and serves only the guide and its images, through the token.
  Views are counted per link. On Enterprise, admins can require passwords,
  cap how long links last, or turn links off (`workspace_settings`); this
  applies to existing links too.
- The desktop app signs in with device authorization: it shows a code, the
  user approves it at `/device`, and the app calls the API with the session
  token as a bearer token.
- Enterprise (`sso` entitlement): OIDC and SAML single sign-on per workspace
  with DNS domain verification and domain capture (`src/lib/sso.ts`), SCIM
  provisioning (`src/lib/scim.ts`), and a policy for sharing, required SSO
  and retention (`src/lib/policy.ts`), managed at `/admin` by owners and
  admins (`workspace: manage`). `src/lib/enterprise.ts` ties them to plans
  and sessions.
- An hourly job deletes what's past its retention (`src/lib/retention.ts`).
- Security events (admin changes, SSO, SCIM, rate limits) are logged as JSON
  (`src/lib/security-events.ts`); the audit log plugs in there.

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
| `GET` / `POST` | `/api/workspaces/:id/sync/guides/:guideId/review` | `{ review }`: where the guide stands in review / submit `{ revisionId, note }` for review (Enterprise) |
| `GET` | `/api/workspaces/:id/audit/export?format=csv\|json&action=&actorId=&targetId=&from=&to=` | The audit log (`audit:read`, Enterprise) |
| `GET` | `/api/workspaces/:id/shares?guideId=<id>` | `{ links: [{ id, url, revisionId, expiresAt, hasPassword, allowDownload, views, lastViewedAt, state }], settings }` |
| `POST` | `/api/workspaces/:id/shares` | `{ guideId, revisionId?, expiresAt?, password?, allowDownload? }` → `{ link }`; `revisionId` null shares the latest published revision |
| `DELETE` | `/api/workspaces/:id/shares/:shareId` | Revoke a link |
| `GET` | `/api/share/:token/assets/:sha256` | Public: an image of the shared revision (password cookie if the link has one) |
| `GET` | `/api/health` | Database check |
| `POST` | `/api/jobs/retention` | Runs the retention job; `Authorization: Bearer $JOBS_TOKEN` |
| `*` | `/api/auth/scim/v2/*` | SCIM 2.0 `Users` and `Groups`, with a workspace's SCIM token |
| `*` | `/api/auth/sso/*` | SSO callbacks (`callback/:providerId`, `saml2/sp/acs/:providerId`) and SP metadata |

API calls take the session cookie or `Authorization: Bearer <token>`. The
workspace API is rate limited per user (429 with `Retry-After`). The share
API needs the `sharing` entitlement and the `guide: share` permission, and
the admin console's sharing policy must allow links for anyone.
The library, viewer and downloads use server functions (`src/server/guides.ts`).

## Tests

`pnpm test` runs against in-memory Postgres (PGlite) with the real
migrations. After changing `src/db/schema.ts`, run `pnpm db:generate`.
Set `GUIDERECORDER_CLI` to a built CLI to also run the exports through it.
