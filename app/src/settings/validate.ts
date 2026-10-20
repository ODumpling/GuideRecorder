// Mirrors of the Rust checks in app/src-tauri/src/settings.rs, for
// feedback while typing. Rust checks again when saving.

export const TYPING_IDLE_MS = { min: 300, max: 10_000 };
/** Version history retention; 0 is no limit. */
export const KEEP_VERSIONS = { min: 0, max: 1000 };
export const KEEP_DAYS = { min: 0, max: 3650 };
export const AFTER_FRAME_DELAY_MS = { min: 0, max: 3_000 };
export const MAX_EXCLUDED_APPS = 100;
/** Limits of the redaction rules (core's `redact::detect`). */
export const MAX_REDACTION_PATTERNS = 50;
export const MAX_REDACTION_WORDS = 500;

const MODIFIERS = ["CmdOrCtrl", "CommandOrControl", "Cmd", "Command", "Super", "Ctrl", "Control", "Alt", "Option", "Shift"];
const NAMED_KEYS = ["Space", "Enter", "Tab", "Escape", "Backspace", "Delete"];

/** A global shortcut such as `CmdOrCtrl+Shift+R`; empty means none. */
export function isHotkey(value: string): boolean {
  if (value === "") return true;
  const parts = value.split("+");
  const key = parts.pop()!;
  const keyOk =
    /^[A-Za-z0-9]$/.test(key) || /^F([1-9]|1[0-9]|20)$/.test(key) || NAMED_KEYS.includes(key);
  const modsOk =
    parts.every((m) => MODIFIERS.includes(m)) &&
    parts.some((m) => m !== "Shift") &&
    new Set(parts).size === parts.length;
  return keyOk && modsOk;
}

/** A model id such as `claude-haiku-5-5`, `qwen3-vl:8b` or `lmstudio-community/gemma-3-12b`. */
export function isModelId(value: string): boolean {
  return value.length >= 1 && value.length <= 200 && /^[A-Za-z0-9\-._:@/]+$/.test(value);
}

/** Empty, or an `http` or `https` URL without spaces or a query. */
export function isBaseUrl(value: string): boolean {
  if (value === "") return true;
  const rest = /^https?:\/\/(.*)$/.exec(value)?.[1];
  return value.length <= 2048 && !!rest && !rest.startsWith("/") && /^[\x21-\x7e]+$/.test(rest) && !/[?#]/.test(rest);
}

/** Empty, or a BCP 47-looking tag. */
export function isLanguage(value: string): boolean {
  return value === "" || (value.length <= 35 && value.split("-").every((p) => /^[A-Za-z0-9]{1,8}$/.test(p)));
}

function hasBundleIdChars(value: string): boolean {
  return value.length >= 1 && value.length <= 255 && /^[A-Za-z0-9\-._]+$/.test(value);
}

/**
 * A full bundle id such as `com.1password.1password`: at least three
 * non-empty dot-separated parts. Exclusion also matches `<id>.` as a prefix,
 * so `com.apple` would leave out every Apple app.
 */
export function isBundleId(value: string): boolean {
  const parts = value.split(".");
  return hasBundleIdChars(value) && parts.length >= 3 && parts.every((p) => p !== "");
}

/** Why `value` can't be added as an excluded app, or null if it can (or is empty). */
export function bundleIdError(value: string): string | null {
  if (value === "" || isBundleId(value)) return null;
  if (hasBundleIdChars(value) && value.includes(".")) {
    return `${value} is too broad: it would exclude every app whose id starts with it. Use a full bundle id, such as com.1password.1password.`;
  }
  return "Use a bundle id, such as com.1password.1password.";
}

export function inRange(value: number, range: { min: number; max: number }): boolean {
  return Number.isInteger(value) && value >= range.min && value <= range.max;
}

const CODE_KEYS: Record<string, string> = {
  Space: "Space",
  Enter: "Enter",
  Tab: "Tab",
  Escape: "Escape",
  Backspace: "Backspace",
  Delete: "Delete",
};

/**
 * The shortcut a key press spells, by physical key (`KeyR` is `R` on any
 * layout), or null while only modifiers are held or no modifier other
 * than Shift is.
 */
export function hotkeyFromEvent(e: {
  code: string;
  metaKey: boolean;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
}): string | null {
  const letter = /^Key([A-Z])$/.exec(e.code)?.[1];
  const digit = /^Digit([0-9])$/.exec(e.code)?.[1];
  const fn = /^(F([1-9]|1[0-9]|20))$/.exec(e.code)?.[1];
  const key = letter ?? digit ?? fn ?? CODE_KEYS[e.code];
  if (!key) return null;
  const mods = [
    e.metaKey && "CmdOrCtrl",
    e.ctrlKey && "Ctrl",
    e.altKey && "Alt",
    e.shiftKey && "Shift",
  ].filter((m): m is string => !!m);
  if (!mods.some((m) => m !== "Shift")) return null;
  return [...mods, key].join("+");
}

const SYMBOLS: Record<string, string> = {
  CmdOrCtrl: "⌘",
  CommandOrControl: "⌘",
  Cmd: "⌘",
  Command: "⌘",
  Super: "⌘",
  Ctrl: "⌃",
  Control: "⌃",
  Alt: "⌥",
  Option: "⌥",
  Shift: "⇧",
  Space: "Space",
  Enter: "↩",
  Tab: "⇥",
  Escape: "⎋",
  Backspace: "⌫",
  Delete: "⌦",
};

/** A shortcut as macOS menus show it, e.g. `⇧⌘R`. */
export function formatHotkey(value: string): string {
  if (!value) return "None";
  const parts = value.split("+");
  const key = parts.pop()!;
  const order = ["⌃", "⌥", "⇧", "⌘"];
  const mods = parts.map((m) => SYMBOLS[m] ?? m).sort((a, b) => order.indexOf(a) - order.indexOf(b));
  return mods.join("") + (SYMBOLS[key] ?? key);
}
