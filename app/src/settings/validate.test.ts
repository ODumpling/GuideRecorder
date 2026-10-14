import { describe, expect, it } from "vitest";
import { formatHotkey, hotkeyFromEvent, inRange, isBundleId, isHotkey, isLanguage, isModelId, TYPING_IDLE_MS } from "@/settings/validate";

const key = (code: string, mods: Partial<Record<"metaKey" | "ctrlKey" | "altKey" | "shiftKey", boolean>> = {}) => ({
  code,
  metaKey: false,
  ctrlKey: false,
  altKey: false,
  shiftKey: false,
  ...mods,
});

describe("settings checks (mirroring settings.rs)", () => {
  it("hotkeys", () => {
    for (const ok of ["", "CmdOrCtrl+Shift+R", "Alt+F5", "Ctrl+Option+Space", "Cmd+1"]) expect(isHotkey(ok)).toBe(true);
    for (const bad of ["R", "Shift+R", "Cmd+", "Cmd+Cmd+R", "Hyper+R", "Cmd+F21", "Cmd+RR"]) expect(isHotkey(bad)).toBe(false);
  });

  it("other values", () => {
    expect(isModelId("claude-sonnet-5-5")).toBe(true);
    expect(isModelId("a b")).toBe(false);
    expect(isLanguage("") && isLanguage("de-CH") && isLanguage("zh-Hant-TW")).toBe(true);
    expect(isLanguage("en_US") || isLanguage("-en")).toBe(false);
    expect(isBundleId("com.1password.1password")).toBe(true);
    expect(isBundleId("Safari") || isBundleId("com.x/y")).toBe(false);
    expect(inRange(1500, TYPING_IDLE_MS)).toBe(true);
    expect(inRange(50, TYPING_IDLE_MS) || inRange(1500.5, TYPING_IDLE_MS)).toBe(false);
  });

  it("records shortcuts by physical key", () => {
    expect(hotkeyFromEvent(key("KeyR", { metaKey: true, shiftKey: true }))).toBe("CmdOrCtrl+Shift+R");
    expect(hotkeyFromEvent(key("Digit4", { ctrlKey: true, altKey: true }))).toBe("Ctrl+Alt+4");
    expect(hotkeyFromEvent(key("F5", { altKey: true }))).toBe("Alt+F5");
    expect(hotkeyFromEvent(key("Space", { metaKey: true }))).toBe("CmdOrCtrl+Space");
    // Modifiers only, Shift only, or no modifier: not a shortcut yet.
    expect(hotkeyFromEvent(key("ShiftLeft", { shiftKey: true }))).toBeNull();
    expect(hotkeyFromEvent(key("KeyR", { shiftKey: true }))).toBeNull();
    expect(hotkeyFromEvent(key("KeyR"))).toBeNull();
    expect(hotkeyFromEvent(key("BracketLeft", { metaKey: true }))).toBeNull();
    // Whatever it records passes the check.
    expect(isHotkey(hotkeyFromEvent(key("KeyQ", { metaKey: true, ctrlKey: true, altKey: true, shiftKey: true }))!)).toBe(true);
  });

  it("shows shortcuts like macOS menus", () => {
    expect(formatHotkey("CmdOrCtrl+Shift+R")).toBe("⇧⌘R");
    expect(formatHotkey("Ctrl+Alt+Space")).toBe("⌃⌥Space");
    expect(formatHotkey("")).toBe("None");
  });
});
