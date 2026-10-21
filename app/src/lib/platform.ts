// The platform the app runs on: Windows (WebView2) or macOS (WKWebView),
// from the WebView's user agent. Read on each call, so tests can stub
// `navigator`; anything not Windows is macOS.

/** Whether GuideRecorder runs on Windows. */
export function isWindows(): boolean {
  return typeof navigator !== "undefined" && /Windows/.test(navigator.userAgent);
}

const WINDOWS_KEYS: Record<string, string> = { "⌃": "Ctrl", "⌘": "Ctrl", "⌥": "Alt", "⇧": "Shift" };

/**
 * An app shortcut written as macOS shows it, such as `⇧⌘Z`, as this
 * platform shows it: unchanged on macOS, `Ctrl+Shift+Z` on Windows (where
 * ⌘ is Ctrl, as the key handlers take either).
 */
export function shortcutLabel(mac: string): string {
  if (!isWindows()) return mac;
  const mods = new Set<string>();
  let i = 0;
  while (i < mac.length - 1 && WINDOWS_KEYS[mac[i]]) mods.add(WINDOWS_KEYS[mac[i++]]);
  const order = ["Ctrl", "Alt", "Shift"];
  return [...[...mods].sort((a, b) => order.indexOf(a) - order.indexOf(b)), mac.slice(i)].join("+");
}

/** What copy calls the user's computer: "this Mac", or "this PC" on Windows. */
export function thisComputer(): string {
  return isWindows() ? "this PC" : "this Mac";
}

/** The file manager guides and exports are shown in: Finder, or File Explorer on Windows. */
export function fileManager(): string {
  return isWindows() ? "File Explorer" : "Finder";
}

/** Where API keys are kept, as in "The key is kept in …": your Keychain, or Windows Credential Manager. */
export function keyStore(): string {
  return isWindows() ? "Windows Credential Manager" : "your Keychain";
}
