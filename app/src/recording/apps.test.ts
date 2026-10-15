import { describe, expect, it } from "vitest";
import { appName, DEFAULT_EXCLUDED_IDS } from "@/recording/apps";
import { isBundleId, MAX_EXCLUDED_APPS } from "@/settings/validate";
import source from "../../../crates/guidesya-capture/src/macos/session.rs?raw";

describe("excluded apps", () => {
  it("lists the capture crate's defaults, in order", () => {
    const list = /pub const DEFAULT_EXCLUDED_BUNDLE_IDS: &\[&str\] = &\[([^\]]*)\]/.exec(source)?.[1];
    expect(list).toBeDefined();
    const ids = [...list!.matchAll(/"([^"]+)"/g)].map((m) => m[1]);
    expect(DEFAULT_EXCLUDED_IDS).toEqual(ids);
  });

  it("are valid settings values", () => {
    expect(DEFAULT_EXCLUDED_IDS.length).toBeLessThanOrEqual(MAX_EXCLUDED_APPS);
    for (const id of DEFAULT_EXCLUDED_IDS) expect(isBundleId(id), id).toBe(true);
  });

  it("names known apps and falls back to the id", () => {
    expect(appName("com.1password.1password")).toBe("1Password");
    expect(appName("com.apple.MobileSMS")).toBe("Messages");
    expect(appName("com.example.vault")).toBe("com.example.vault");
  });
});
