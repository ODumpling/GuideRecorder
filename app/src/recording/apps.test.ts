import { afterEach, describe, expect, it, vi } from "vitest";
import { appName, DEFAULT_EXCLUDED_IDS, WINDOWS_EXCLUDED_APPS } from "@/recording/apps";
import { isBundleId, MAX_EXCLUDED_APPS } from "@/settings/validate";
import source from "../../../crates/guiderecorder-capture/src/macos/apps.rs?raw";
import windowsSource from "../../../crates/guiderecorder-capture/src/windows/apps.rs?raw";

const WINDOWS = { userAgent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36 Edg/141.0.0.0" };

/** The ids in a crate's `DEFAULT_EXCLUDED_BUNDLE_IDS`. */
function crateDefaults(source: string): string[] {
  const list = /pub const DEFAULT_EXCLUDED_BUNDLE_IDS: &\[&str\] = &\[([^\]]*)\]/.exec(source)?.[1];
  expect(list).toBeDefined();
  return [...list!.matchAll(/"([^"]+)"/g)].map((m) => m[1]);
}

describe("excluded apps", () => {
  it("lists the capture crate's defaults, in order", () => {
    expect(DEFAULT_EXCLUDED_IDS).toEqual(crateDefaults(source));
  });

  it("are valid settings values", () => {
    expect(DEFAULT_EXCLUDED_IDS.length).toBeLessThanOrEqual(MAX_EXCLUDED_APPS);
    for (const id of DEFAULT_EXCLUDED_IDS) expect(isBundleId(id), id).toBe(true);
  });

  it("names known apps and falls back to the id", () => {
    expect(appName("com.1password.1password")).toBe("1Password");
    expect(appName("com.apple.MobileSMS")).toBe("Messages");
    expect(appName("2BUA8C4S2C.com.agilebits.onepassword7-helper")).toBe("1Password 7 mini");
    expect(appName("2BUA8C4S2C.com.agilebits.onepassword-osx-helper")).toBe("1Password mini");
    expect(appName("com.agilebits.onepassword4")).toBe("1Password 6");
    expect(appName("2BUA8C4S2C.com.agilebits.onepassword4-helper")).toBe("1Password 6 mini");
    expect(appName("com.example.vault")).toBe("com.example.vault");
  });
});

describe("excluded apps on Windows", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.resetModules();
  });

  it("list the capture crate's Windows defaults, in order, as valid settings values", () => {
    const ids = WINDOWS_EXCLUDED_APPS.map((a) => a.id);
    expect(ids).toEqual(crateDefaults(windowsSource));
    vi.stubGlobal("navigator", WINDOWS);
    for (const id of ids) expect(isBundleId(id), id).toBe(true);
  });

  it("are the defaults on Windows", async () => {
    vi.stubGlobal("navigator", WINDOWS);
    vi.resetModules();
    const apps = await import("@/recording/apps");
    expect(apps.DEFAULT_EXCLUDED_IDS).toEqual(crateDefaults(windowsSource));
    expect(apps.appName("keepassxc.exe")).toBe("KeePassXC");
  });
});
