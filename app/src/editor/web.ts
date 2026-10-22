// Web pages of recorded steps (`Step.url`), as core's `web` module
// shows them in titles and exports.

/** Longest address shown, in characters, before it is cut with "…". */
export const MAX_SHOWN_URL_CHARS = 60;

/**
 * `url` as a reader would say it: without `https://`, `http://` and `www.`,
 * without a lone trailing `/`, cut at {@link MAX_SHOWN_URL_CHARS}.
 */
export function displayUrl(url: string): string {
  let shown = url.replace(/^https?:\/\//i, "").replace(/^www\./i, "");
  // Only the root path: `example.com/` reads as `example.com`.
  if (shown.endsWith("/") && shown.indexOf("/") === shown.length - 1) shown = shown.slice(0, -1);
  const chars = Array.from(shown);
  return chars.length > MAX_SHOWN_URL_CHARS ? `${chars.slice(0, MAX_SHOWN_URL_CHARS - 1).join("")}…` : shown;
}

/** Whether `url` is an http or https address, the only kind a step's page link opens. */
export function isWebAddress(url: string): boolean {
  try {
    const parsed = new URL(url);
    return (parsed.protocol === "https:" || parsed.protocol === "http:") && parsed.host !== "";
  } catch {
    return false;
  }
}
