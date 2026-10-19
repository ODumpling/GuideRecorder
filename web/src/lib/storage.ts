import { AwsClient } from "aws4fetch";

/** The object storage the server needs: presigned uploads and downloads, and existence checks. */
export interface ObjectStore {
  /** A URL and headers for a PUT that only succeeds with exactly these bytes. */
  presignPut(key: string, object: { size: number; contentType: string; sha256: string }, ttlSeconds: number): Promise<PresignedRequest>;
  presignGet(key: string, ttlSeconds: number): Promise<string>;
  /** The stored object's size and type, or null if there is none. */
  head(key: string): Promise<{ size: number; contentType: string } | null>;
}

export interface PresignedRequest {
  url: string;
  method: "PUT";
  headers: Record<string, string>;
}

export interface S3Config {
  endpoint: string;
  region: string;
  bucket: string;
  accessKeyId: string;
  secretAccessKey: string;
}

/** Any S3-compatible store (S3, R2, MinIO), path-style addressing. */
export function s3Store(config: S3Config): ObjectStore {
  const client = new AwsClient({
    accessKeyId: config.accessKeyId,
    secretAccessKey: config.secretAccessKey,
    service: "s3",
    region: config.region,
  });
  const objectUrl = (key: string, ttlSeconds?: number) => {
    const url = new URL(`${config.endpoint.replace(/\/$/, "")}/${config.bucket}/${key}`);
    if (ttlSeconds) url.searchParams.set("X-Amz-Expires", String(ttlSeconds));
    return url.toString();
  };

  return {
    async presignPut(key, { size, contentType, sha256 }, ttlSeconds) {
      // Signed headers: S3 rejects the upload unless the body's SHA-256 and
      // length match, so a key always holds the bytes its hash names.
      const headers = {
        "content-type": contentType,
        "content-length": String(size),
        "x-amz-checksum-sha256": hexToBase64(sha256),
        "x-amz-sdk-checksum-algorithm": "SHA256",
      };
      const signed = await client.sign(objectUrl(key, ttlSeconds), {
        method: "PUT",
        headers,
        aws: { signQuery: true, allHeaders: true },
      });
      return { url: signed.url, method: "PUT", headers };
    },

    async presignGet(key, ttlSeconds) {
      const signed = await client.sign(objectUrl(key, ttlSeconds), { method: "GET", aws: { signQuery: true } });
      return signed.url;
    },

    async head(key) {
      const response = await client.fetch(objectUrl(key), { method: "HEAD" });
      if (response.status === 404) return null;
      if (!response.ok) throw new Error(`HEAD ${key} failed with ${response.status}`);
      return {
        size: Number(response.headers.get("content-length")),
        contentType: response.headers.get("content-type") ?? "application/octet-stream",
      };
    },
  };
}

export function hexToBase64(hex: string): string {
  return Buffer.from(hex, "hex").toString("base64");
}
