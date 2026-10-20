import { AwsClient } from "aws4fetch";

/** The object storage the server needs: presigned uploads and downloads, and existence checks. */
export interface ObjectStore {
  /** A URL and headers for a PUT that only succeeds with exactly these bytes. */
  presignPut(key: string, object: { size: number; contentType: string; sha256: string }, ttlSeconds: number): Promise<PresignedRequest>;
  /** A download URL; with `filename`, browsers save it under that name. */
  presignGet(key: string, ttlSeconds: number, options?: { filename?: string }): Promise<string>;
  /** The stored object's size and type, or null if there is none. */
  head(key: string): Promise<{ size: number; contentType: string } | null>;
}

/** Reading and writing whole objects, for the export worker. */
export interface BlobStore {
  get(key: string): Promise<Uint8Array | null>;
  put(key: string, body: Uint8Array<ArrayBuffer>, contentType: string): Promise<void>;
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
export function s3Store(config: S3Config): ObjectStore & BlobStore {
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

    async presignGet(key, ttlSeconds, options) {
      const url = new URL(objectUrl(key, ttlSeconds));
      if (options?.filename) url.searchParams.set("response-content-disposition", contentDisposition(options.filename));
      const signed = await client.sign(url.toString(), { method: "GET", aws: { signQuery: true } });
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

    async get(key) {
      const response = await client.fetch(objectUrl(key));
      if (response.status === 404) return null;
      if (!response.ok) throw new Error(`GET ${key} failed with ${response.status}`);
      return new Uint8Array(await response.arrayBuffer());
    },

    async put(key, body, contentType) {
      const response = await client.fetch(objectUrl(key), {
        method: "PUT",
        body,
        headers: { "content-type": contentType, "content-length": String(body.byteLength) },
      });
      if (!response.ok) throw new Error(`PUT ${key} failed with ${response.status}`);
    },
  };
}

/** An `attachment` Content-Disposition for `filename`, with an ASCII fallback (RFC 6266). */
export function contentDisposition(filename: string): string {
  const ascii = filename.replace(/[^\x20-\x7e]|["\\]/g, "_");
  return `attachment; filename="${ascii}"; filename*=UTF-8''${encodeURIComponent(filename)}`;
}

export function hexToBase64(hex: string): string {
  return Buffer.from(hex, "hex").toString("base64");
}
