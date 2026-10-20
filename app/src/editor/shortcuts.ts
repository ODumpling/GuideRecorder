export type ShortcutAction = "undo" | "redo";

type Keys = Pick<KeyboardEvent, "key" | "metaKey" | "ctrlKey" | "shiftKey" | "altKey">;

/** Cmd+Z undoes, Cmd+Shift+Z (or Ctrl on other platforms) redoes. */
export function shortcutFor(e: Keys): ShortcutAction | null {
  if (!(e.metaKey || e.ctrlKey) || e.altKey || e.key.toLowerCase() !== "z") return null;
  return e.shiftKey ? "redo" : "undo";
}

export type NavigationAction = "previous" | "next" | "details";

/**
 * Moving around the open guide, for keys pressed outside text fields: J and
 * K (or Cmd+] and Cmd+[) go to the next and previous item, Cmd+D to the
 * guide details.
 */
export function navigationFor(e: Keys): NavigationAction | null {
  if (e.altKey || e.shiftKey) return null;
  const key = e.key.toLowerCase();
  if (e.metaKey || e.ctrlKey) {
    if (key === "[") return "previous";
    if (key === "]") return "next";
    return key === "d" ? "details" : null;
  }
  if (key === "k") return "previous";
  return key === "j" ? "next" : null;
}
