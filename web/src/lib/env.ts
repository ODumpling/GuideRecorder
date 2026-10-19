import "@tanstack/react-start/server-only";
import { z } from "zod";

// Server configuration, read once on first use so `next build` runs without
// secrets. Optional groups (OAuth, Stripe, Resend) switch features on when set.
const schema = z.object({
  DATABASE_URL: z.url(),
  BETTER_AUTH_SECRET: z.string().min(32),
  BETTER_AUTH_URL: z.url(),

  S3_ENDPOINT: z.url(),
  S3_REGION: z.string().default("auto"),
  S3_BUCKET: z.string().min(1),
  S3_ACCESS_KEY_ID: z.string().min(1),
  S3_SECRET_ACCESS_KEY: z.string().min(1),
  /** Server-side encryption for uploads: AES256 or aws:kms. Leave unset for R2 (always encrypted at rest). */
  S3_SSE: z.enum(["AES256", "aws:kms"]).optional(),
  S3_SSE_KMS_KEY_ID: z.string().optional(),

  /** HMAC secret for SCIM bearer tokens; derived from BETTER_AUTH_SECRET when unset. */
  SCIM_TOKEN_SECRET: z.string().min(32).optional(),
  /** Bearer token for /api/jobs/* (the retention job). Jobs are off without it. */
  JOBS_TOKEN: z.string().min(32).optional(),
  /** Comma-separated proxy addresses or CIDRs whose X-Forwarded-For to trust. */
  TRUSTED_PROXIES: z.string().optional(),

  GOOGLE_CLIENT_ID: z.string().optional(),
  GOOGLE_CLIENT_SECRET: z.string().optional(),
  MICROSOFT_CLIENT_ID: z.string().optional(),
  MICROSOFT_CLIENT_SECRET: z.string().optional(),

  STRIPE_SECRET_KEY: z.string().optional(),
  STRIPE_WEBHOOK_SECRET: z.string().optional(),
  STRIPE_PRICE_PRO: z.string().optional(),
  STRIPE_PRICE_ENTERPRISE: z.string().optional(),

  RESEND_API_KEY: z.string().optional(),
  EMAIL_FROM: z.string().default("GuideRecorder <no-reply@guiderecorder.app>"),
});

export type Env = z.infer<typeof schema>;

let cached: Env | undefined;

export function env(): Env {
  cached ??= schema.parse(process.env);
  return cached;
}
