export type ShortcutAction = "undo" | "redo";

/** Cmd+Z undoes, Cmd+Shift+Z (or Ctrl on other platforms) redoes. */
export function shortcutFor(e: Pick<KeyboardEvent, "key" | "metaKey" | "ctrlKey" | "shiftKey" | "altKey">): ShortcutAction | null {
  if (!(e.metaKey || e.ctrlKey) || e.altKey || e.key.toLowerCase() !== "z") return null;
  return e.shiftKey ? "redo" : "undo";
}
