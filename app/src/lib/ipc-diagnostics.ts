// Development only: details for Tauri's "IPC custom protocol failed"
// warning, after which the session uses postMessage (byte responses then
// arrive as number arrays, see `toArrayBuffer`). The warning prints only
// the error's text; this adds its name, cause and stack, and when it
// happened relative to page loads and Vite reloads.

const IPC_FALLBACK = "IPC custom protocol failed";

/** One line describing an error-like value. */
export function describeError(error: unknown): string {
  if (!(error instanceof Error)) return String(error);
  const cause = error.cause === undefined ? "" : ` cause=${describeError(error.cause)}`;
  return `${error.name}: ${error.message}${cause}\n${error.stack ?? "(no stack)"}`;
}

export function installIpcDiagnostics() {
  const navigation = performance.getEntriesByType("navigation")[0] as PerformanceNavigationTiming | undefined;
  const warn = console.warn.bind(console);
  console.warn = (...args: unknown[]) => {
    warn(...args);
    if (typeof args[0] === "string" && args[0].startsWith(IPC_FALLBACK)) {
      warn(
        `[guidesya] IPC fallback at ${Math.round(performance.now())} ms after ${navigation?.type ?? "unknown"} load` +
          ` (document ${document.readyState}): ${describeError(args[1])}`,
      );
    }
  };
  import.meta.hot?.on("vite:beforeFullReload", (payload) => {
    warn("[guidesya] Vite full reload; in-flight IPC requests are cancelled", payload);
  });
}
