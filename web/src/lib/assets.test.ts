import { createHash } from "node:crypto";
import { beforeEach, describe, expect, it } from "vitest";
import { testDb } from "../../test/db";
import type { Database } from "@/db";
import { organization, user } from "@/db/schema";
import { assetKey, commitUploads, downloadUrl, planUploads, QuotaExceededError, storageUsed, uploadRequest } from "./assets";
import type { ObjectStore } from "./storage";

/** An in-memory store; `put` stands in for a client's presigned upload. */
function memoryStore() {
  const objects = new Map<string, { size: number; contentType: string }>();
  const store: ObjectStore = {
    async presignPut(key, object) {
      return { url: `https://s3.test/${key}?signed`, method: "PUT", headers: { "content-type": object.contentType } };
    },
    async presignGet(key) {
      return `https://s3.test/${key}?get`;
    },
    async head(key) {
      return objects.get(key) ?? null;
    },
  };
  return { store, put: (key: string, size: number, contentType = "image/webp") => objects.set(key, { size, contentType }) };
}

const hash = (text: string) => createHash("sha256").update(text).digest("hex");
const a = hash("a");
const b = hash("b");

let db: Database;
let s3: ReturnType<typeof memoryStore>;

beforeEach(async () => {
  db = await testDb();
  s3 = memoryStore();
  await db.insert(organization).values([
    { id: "org", name: "Acme", slug: "acme", createdAt: new Date() },
    { id: "other", name: "Other", slug: "other", createdAt: new Date() },
  ]);
  await db.insert(user).values({ id: "u", name: "U", email: "u@example.com" });
});

const request = (...assets: [string, number][]) =>
  uploadRequest.parse({ assets: assets.map(([sha256, size]) => ({ sha256, size, contentType: "image/webp" })) });

describe("assets", () => {
  it("rejects malformed hashes and types", () => {
    expect(uploadRequest.safeParse({ assets: [{ sha256: "ABC", size: 1, contentType: "image/webp" }] }).success).toBe(false);
    expect(uploadRequest.safeParse({ assets: [{ sha256: a, size: 1, contentType: "text/html" }] }).success).toBe(false);
  });

  it("uploads only what the workspace lacks, then commits it", async () => {
    const first = await planUploads(db, s3.store, "org", request([a, 10], [b, 20], [a, 10]), 1000);
    expect(first.present).toEqual([]);
    expect(first.uploads.map((u) => u.sha256).sort()).toEqual([a, b].sort());

    s3.put(assetKey("org", a), 10);
    expect(await commitUploads(db, s3.store, "org", "u", [a, b])).toEqual({ committed: [a], missing: [b] });
    expect(await storageUsed(db, "org")).toBe(10);

    const second = await planUploads(db, s3.store, "org", request([a, 10], [b, 20]), 1000);
    expect(second.present).toEqual([a]);
    expect(second.uploads.map((u) => u.sha256)).toEqual([b]);
  });

  it("keeps workspaces apart", async () => {
    s3.put(assetKey("org", a), 10);
    await commitUploads(db, s3.store, "org", "u", [a]);
    expect(await downloadUrl(db, s3.store, "org", a)).toContain("orgs/org/assets/");
    expect(await downloadUrl(db, s3.store, "other", a)).toBeNull();
    expect((await planUploads(db, s3.store, "other", request([a, 10]), 1000)).uploads).toHaveLength(1);
  });

  it("enforces the storage limit", async () => {
    s3.put(assetKey("org", a), 600);
    await commitUploads(db, s3.store, "org", "u", [a]);
    await expect(planUploads(db, s3.store, "org", request([b, 500]), 1000)).rejects.toBeInstanceOf(QuotaExceededError);
    // Already-stored assets cost nothing.
    await expect(planUploads(db, s3.store, "org", request([a, 600]), 1000)).resolves.toMatchObject({ present: [a] });
  });

  it("doesn't presign downloads for bad hashes", async () => {
    expect(await downloadUrl(db, s3.store, "org", "../../etc/passwd")).toBeNull();
  });
});
