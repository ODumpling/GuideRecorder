import { expect, it } from "vitest";
import { contentDisposition, hexToBase64, s3Store } from "./storage";

const store = s3Store({
  endpoint: "https://s3.example.com/",
  region: "us-east-1",
  bucket: "guides",
  accessKeyId: "AKID",
  secretAccessKey: "secret",
});
const sha = "ca978112ca1bbdcafac231b39a23dc4da786eff8147c4e72b9807785afee48bb";

it("presigns PUTs bound to the body's hash, length and type", async () => {
  const put = await store.presignPut("orgs/o/assets/x", { size: 42, contentType: "image/webp", sha256: sha }, 900);
  const url = new URL(put.url);
  expect(url.origin + url.pathname).toBe("https://s3.example.com/guides/orgs/o/assets/x");
  expect(url.searchParams.get("X-Amz-Expires")).toBe("900");
  expect(url.searchParams.get("X-Amz-SignedHeaders")?.split(";")).toEqual(
    expect.arrayContaining(["content-length", "content-type", "x-amz-checksum-sha256"]),
  );
  expect(put.headers["x-amz-checksum-sha256"]).toBe(hexToBase64(sha));
  expect(put.headers["content-length"]).toBe("42");
});

it("presigns short-lived GETs", async () => {
  const url = new URL(await store.presignGet("orgs/o/assets/x", 300));
  expect(url.searchParams.get("X-Amz-Expires")).toBe("300");
  expect(url.searchParams.get("X-Amz-Signature")).toMatch(/^[0-9a-f]{64}$/);
});

it("signs server-side encryption into uploads when configured", async () => {
  const kms = s3Store({
    endpoint: "https://s3.example.com",
    region: "eu-west-1",
    bucket: "guides",
    accessKeyId: "AKID",
    secretAccessKey: "secret",
    sse: { algorithm: "aws:kms", kmsKeyId: "alias/guiderecorder" },
  });
  const put = await kms.presignPut("orgs/o/assets/x", { size: 1, contentType: "image/webp", sha256: sha }, 900);
  expect(put.headers).toMatchObject({
    "x-amz-server-side-encryption": "aws:kms",
    "x-amz-server-side-encryption-aws-kms-key-id": "alias/guiderecorder",
  });
  expect(new URL(put.url).searchParams.get("X-Amz-SignedHeaders")?.split(";")).toEqual(
    expect.arrayContaining(["x-amz-server-side-encryption", "x-amz-server-side-encryption-aws-kms-key-id"]),
  );
  // Unset (R2, local storage): no SSE headers at all.
  const plain = await store.presignPut("k", { size: 1, contentType: "image/webp", sha256: sha }, 900);
  expect(Object.keys(plain.headers).some((h) => h.startsWith("x-amz-server-side"))).toBe(false);
});

it("names downloads with a signed Content-Disposition", async () => {
  const url = new URL(await store.presignGet("orgs/o/exports/r/pdf.pdf", 300, { filename: "Drucker – Fach.pdf" }));
  expect(url.searchParams.get("response-content-disposition")).toBe(contentDisposition("Drucker – Fach.pdf"));
  expect(contentDisposition('a"b – c.pdf')).toBe(`attachment; filename="a_b _ c.pdf"; filename*=UTF-8''a%22b%20%E2%80%93%20c.pdf`);
});
