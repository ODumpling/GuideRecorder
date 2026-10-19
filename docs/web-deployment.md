# Web app: production deployment

What runs in production, how to deploy it, and how to keep it backed up and
watched. Security details (encryption, key rotation, rate limits, headers)
are in [web-security.md](web-security.md).

## Pieces

| Piece | What | Notes |
|---|---|---|
| App | `web/Dockerfile` image, Node 24, port 3000 | Stateless; run two or more behind a load balancer. |
| Postgres | 17 | Managed (RDS, Cloud SQL, Neon…) or the `postgres` service in `web/compose.prod.yaml`. |
| Object storage | S3 or Cloudflare R2 | Images. Presigned URLs, so clients upload and download directly. |
| Jobs | `POST /api/jobs/retention` hourly | The `jobs` service in compose, or any scheduler (cron, EventBridge, Cloud Scheduler). |
| Backups | Nightly `pg_dump` plus the provider's snapshots | The `backup` service in compose. |
| Proxy | TLS termination | Caddy, nginx or a cloud load balancer. |

Exports (video, GIF, slides, PDF) are made by the desktop app today; a
server-side export worker would run the same image as another service with
its own entrypoint.

## Deploy with Docker Compose (single host)

```sh
cp web/.env.example web/.env.production    # fill in, see below
docker compose -f web/compose.prod.yaml --env-file web/.env.production up -d --build
```

`migrate` applies migrations (`pnpm -C web db:migrate`) and exits; `app`
starts once it succeeds. To upgrade: pull, then run the same command.

Elsewhere (Kubernetes, ECS, Fly, Render), build the image from the
repository root (`docker build -f web/Dockerfile .`), run migrations with
the `build` target before rolling out (`pnpm -C web db:migrate`), and run the
`runtime` target. Migrations are additive within a release, so the old
version keeps working while they run.

## Configuration

Everything in `web/.env.example`, plus:

| Variable | |
|---|---|
| `BETTER_AUTH_URL` | The public HTTPS URL. HSTS and secure cookies follow from it. |
| `BETTER_AUTH_SECRET` | 32+ random bytes (`openssl rand -base64 32`). See key rotation. |
| `SCIM_TOKEN_SECRET` | 32+ characters. Set it in production so rotating `BETTER_AUTH_SECRET` doesn't invalidate SCIM tokens. |
| `JOBS_TOKEN` | 32+ characters; the bearer token for `/api/jobs/*`. Jobs are off (404) without it. |
| `TRUSTED_PROXIES` | Comma-separated addresses or CIDRs of your proxies, so rate limits see the real client address. |
| `S3_SSE`, `S3_SSE_KMS_KEY_ID` | Encryption at rest on S3; leave unset on R2. |
| `POSTGRES_PASSWORD` | Compose only. |

### S3

- Bucket private, Block Public Access on, versioning on (recovers deletes
  for 30 days with a lifecycle rule expiring noncurrent versions).
- Default encryption SSE-KMS with a customer-managed key, and set
  `S3_SSE=aws:kms` and `S3_SSE_KMS_KEY_ID` so every presigned upload requires
  it too. A bucket policy denying `PutObject` without
  `s3:x-amz-server-side-encryption` makes it impossible to skip.
- CORS: allow `PUT` and `GET` from the app's origin, headers
  `content-type`, `x-amz-*` (browsers load images and the web editor uploads;
  the desktop app uploads outside a browser). Uploads send back every
  header the server signed, the SSE ones included.
- The app's IAM user or role needs `s3:PutObject`, `s3:GetObject`,
  `s3:DeleteObject` and `kms:GenerateDataKey`/`kms:Decrypt` on the key.

### Cloudflare R2

`S3_ENDPOINT=https://<account>.r2.cloudflarestorage.com`, `S3_REGION=auto`.
R2 encrypts every object at rest (AES-256-GCM, keys managed and rotated by
Cloudflare) and doesn't accept the SSE headers, so leave `S3_SSE` unset.
Scope the API token to the one bucket with Object Read & Write.

## Backups and restore

- **Postgres**: point-in-time recovery from the managed provider (7–35 days)
  is the primary backup. The compose `backup` service adds nightly dumps
  (7 daily, 4 weekly, 6 monthly) in `web/backups`; copy them off the host,
  encrypted, e.g. to a separate bucket in another account.
- **Images**: bucket versioning, plus replication to a second region or
  account if you need it. Images are content-addressed, so a restored
  database only needs the objects it names.
- **Restore drill** (quarterly, record the result in the SOC 2 evidence):
  restore the latest dump to a scratch database
  (`pg_restore -d scratch latest.dump`), point a staging app at it, sign in,
  open a synced guide and load its images. Note the time it took (RTO) and
  the age of the newest data (RPO).

## Monitoring

- **Health**: `GET /api/health` checks the database (the image's
  `HEALTHCHECK` uses it). Point an uptime check at it every minute and the
  load balancer's health check too.
- **Logs**: the app logs to stdout; ship them to your log store (CloudWatch,
  Datadog, Loki) and keep them a year. Security events are single JSON lines
  with `"msg":"security_event"` (`web/src/lib/security-events.ts`): admin and
  policy changes, SSO and SCIM changes, blocked sign-ins, rate limits hit and
  retention runs.
- **Alerts**:
  - health check failing for 2 minutes;
  - 5xx rate over 1% for 5 minutes;
  - `rate_limit.exceeded` or `sso.sign_in_blocked` spikes (abuse, or a broken
    IdP);
  - `retention.run` missing for 3 hours (the job stopped);
  - Postgres CPU, connections and storage over 80%; backup job failures;
  - Stripe webhook failures (in Stripe's dashboard).
