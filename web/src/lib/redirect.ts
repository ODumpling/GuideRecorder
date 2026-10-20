/** A same-site path to continue to after signing in; anything else falls back to the library. */
export function safeNext(next: string | string[] | undefined): string {
  const value = Array.isArray(next) ? next[0] : next;
  return value && value.startsWith("/") && !value.startsWith("//") && !value.startsWith("/\\") ? value : "/library";
}
