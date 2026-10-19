// Response headers for every page and API response. The content security
// policy only applies in production: Vite's dev server injects inline
// modules and a websocket. Images and exports load from the object store's
// presigned URLs, so its origin is allowed for images, media and fetches.

export interface SecurityHeaderOptions {
  /** The app's public URL; HTTPS turns on HSTS. */
  baseURL: string;
  /** The S3 endpoint presigned URLs point at. */
  storageEndpoint?: string;
  production: boolean;
}

export function securityHeaders({ baseURL, storageEndpoint, production }: SecurityHeaderOptions): Record<string, string> {
  const https = baseURL.startsWith("https:");
  const storage = storageEndpoint ? new URL(storageEndpoint).origin : "";
  const headers: Record<string, string> = {
    "x-content-type-options": "nosniff",
    "x-frame-options": "DENY",
    "referrer-policy": "strict-origin-when-cross-origin",
    "permissions-policy": "camera=(), microphone=(), geolocation=(), payment=()",
    "cross-origin-opener-policy": "same-origin",
  };
  if (https) headers["strict-transport-security"] = "max-age=63072000; includeSubDomains";
  if (production) {
    headers["content-security-policy"] = [
      "default-src 'self'",
      // Start's hydration data is an inline script.
      "script-src 'self' 'unsafe-inline'",
      "style-src 'self' 'unsafe-inline'",
      `img-src 'self' data: blob: ${storage}`.trim(),
      `media-src 'self' blob: ${storage}`.trim(),
      "font-src 'self' data:",
      `connect-src 'self' ${storage}`.trim(),
      "object-src 'none'",
      "base-uri 'self'",
      // SAML's POST binding submits a form to the identity provider.
      "form-action 'self' https:",
      "frame-ancestors 'none'",
      ...(https ? ["upgrade-insecure-requests"] : []),
    ].join("; ");
  }
  return headers;
}

/** The response with `headers` added, copying it if its headers are immutable. */
export function withHeaders(response: Response, headers: Record<string, string>): Response {
  try {
    for (const [name, value] of Object.entries(headers)) if (!response.headers.has(name)) response.headers.set(name, value);
    return response;
  } catch {
    const copy = new Response(response.body, response);
    for (const [name, value] of Object.entries(headers)) if (!copy.headers.has(name)) copy.headers.set(name, value);
    return copy;
  }
}
