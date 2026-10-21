import { afterEach, describe, expect, it, vi } from "vitest";
import { fileManager, isWindows, keyStore, shortcutLabel, thisComputer } from "@/lib/platform";

/** WebView2's user agent on Windows 11. */
const WINDOWS_USER_AGENT =
  "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36 Edg/141.0.0.0";
const MAC_USER_AGENT = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)";

afterEach(() => vi.unstubAllGlobals());

describe("platform", () => {
  it("is macOS unless the user agent says Windows", () => {
    vi.stubGlobal("navigator", { userAgent: MAC_USER_AGENT });
    expect(isWindows()).toBe(false);
    vi.stubGlobal("navigator", { userAgent: WINDOWS_USER_AGENT });
    expect(isWindows()).toBe(true);
  });

  it("keeps macOS shortcuts and words as they are", () => {
    vi.stubGlobal("navigator", { userAgent: MAC_USER_AGENT });
    expect(shortcutLabel("⇧⌘Z")).toBe("⇧⌘Z");
    expect([thisComputer(), fileManager(), keyStore()]).toEqual(["this Mac", "Finder", "your Keychain"]);
  });

  it("spells shortcuts out on Windows, with ⌘ as Ctrl", () => {
    vi.stubGlobal("navigator", { userAgent: WINDOWS_USER_AGENT });
    expect(shortcutLabel("⌘Z")).toBe("Ctrl+Z");
    expect(shortcutLabel("⇧⌘Z")).toBe("Ctrl+Shift+Z");
    expect(shortcutLabel("⌘⌥Z")).toBe("Ctrl+Alt+Z");
    expect(shortcutLabel("⌘,")).toBe("Ctrl+,");
    expect(shortcutLabel("⌥↑")).toBe("Alt+↑");
    expect(shortcutLabel("⇧↓")).toBe("Shift+↓");
    expect([thisComputer(), fileManager(), keyStore()]).toEqual(["this PC", "File Explorer", "Windows Credential Manager"]);
  });
});
