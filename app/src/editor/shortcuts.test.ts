import { describe, expect, it } from "vitest";
import { navigationFor, shortcutFor } from "@/editor/shortcuts";

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

describe("navigationFor", () => {
  it("maps J and K, and Cmd or Ctrl with [ and ], to the next and previous item", () => {
    expect(navigationFor(key("j"))).toBe("next");
    expect(navigationFor(key("k"))).toBe("previous");
    // Caps Lock on.
    expect(navigationFor(key("J"))).toBe("next");
    expect(navigationFor(key("]", { metaKey: true }))).toBe("next");
    expect(navigationFor(key("[", { metaKey: true }))).toBe("previous");
    expect(navigationFor(key("]", { ctrlKey: true }))).toBe("next");
  });

  it("maps Cmd+D or Ctrl+D to the guide details", () => {
    expect(navigationFor(key("d", { metaKey: true }))).toBe("details");
    expect(navigationFor(key("d", { ctrlKey: true }))).toBe("details");
    expect(navigationFor(key("d"))).toBeNull();
  });

  it("ignores other keys and modifiers", () => {
    expect(navigationFor(key("j", { metaKey: true }))).toBeNull();
    expect(navigationFor(key("["))).toBeNull();
    expect(navigationFor(key("j", { shiftKey: true }))).toBeNull();
    expect(navigationFor(key("k", { altKey: true }))).toBeNull();
    expect(navigationFor(key("d", { metaKey: true, shiftKey: true }))).toBeNull();
    expect(navigationFor(key("ArrowDown"))).toBeNull();
  });
});
