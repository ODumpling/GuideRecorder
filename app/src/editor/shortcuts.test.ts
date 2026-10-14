import { describe, expect, it } from "vitest";
import { shortcutFor } from "@/editor/shortcuts";

const key = (key: string, mods: Partial<Record<"metaKey" | "ctrlKey" | "shiftKey" | "altKey", boolean>> = {}) => ({
  key,
  metaKey: false,
  ctrlKey: false,
  shiftKey: false,
  altKey: false,
  ...mods,
});

describe("shortcutFor", () => {
  it("maps Cmd+Z and Cmd+Shift+Z", () => {
    expect(shortcutFor(key("z", { metaKey: true }))).toBe("undo");
    expect(shortcutFor(key("Z", { metaKey: true, shiftKey: true }))).toBe("redo");
    expect(shortcutFor(key("z", { ctrlKey: true }))).toBe("undo");
  });

  it("ignores other keys", () => {
    expect(shortcutFor(key("z"))).toBeNull();
    expect(shortcutFor(key("y", { metaKey: true }))).toBeNull();
    expect(shortcutFor(key("z", { metaKey: true, altKey: true }))).toBeNull();
  });
});
