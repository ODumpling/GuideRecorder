import "@tanstack/react-start/server-only";
import { z } from "zod";
import { db } from "@/db";
import { checkAccess, type BooleanEntitlement, type Permission } from "./access";
import { auth } from "./auth";
import { env } from "./env";
import { shareUrl, type ShareLinkInfo } from "./sharing";
import { s3Store, type ObjectStore } from "./storage";

export function jsonError(status: number, error: string, extra?: object) {
  return Response.json({ error, ...extra }, { status });
}

/** Signs the request in and checks workspace access; returns an error Response on failure. */
export async function authorize(
  request: Request,
  organizationId: string,
  permission: Permission,
  feature?: BooleanEntitlement,
) {
  const session = await auth().api.getSession({ headers: request.headers });
  const access = await checkAccess(db(), session?.user.id, organizationId, permission, feature);
  if (!access.ok) return { error: jsonError(access.status, access.error) } as const;
  return { userId: session!.user.id, ...access } as const;
}

/** Parses a JSON body with `schema`; returns an error Response on failure. */
export async function readJson<T extends z.ZodType>(request: Request, schema: T) {
  const body = await request.json().catch(() => undefined);
  const parsed = schema.safeParse(body);
  if (!parsed.success) return { error: jsonError(400, "Invalid request.", { issues: parsed.error.issues }) } as const;
  return { data: parsed.data as z.infer<T> } as const;
}

let store: ObjectStore | undefined;

export function objectStore(): ObjectStore {
  const e = env();
  store ??= s3Store({
    endpoint: e.S3_ENDPOINT,
    region: e.S3_REGION,
    bucket: e.S3_BUCKET,
    accessKeyId: e.S3_ACCESS_KEY_ID,
    secretAccessKey: e.S3_SECRET_ACCESS_KEY,
  });
  return store;
}

/** A share link with its public URL. */
export function withUrl(link: ShareLinkInfo) {
  return { ...link, url: shareUrl(env().BETTER_AUTH_URL, link.token) };
}
