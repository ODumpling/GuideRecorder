import { expect, it } from "vitest";
import { hexToBase64, s3Store } from "./storage";

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
