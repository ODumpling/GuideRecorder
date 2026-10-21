import { isWindows } from "@/lib/platform";

/**
 * The suggested excluded apps on macOS, with readable names. Mirrors
 * `DEFAULT_EXCLUDED_BUNDLE_IDS` in crates/guiderecorder-capture/src/macos/apps.rs
 * (the settings default; apps.test.ts checks they match).
 */
export const MAC_EXCLUDED_APPS: readonly { id: string; name: string }[] = [
  { id: "com.1password.1password", name: "1Password" },
  { id: "com.agilebits.onepassword7", name: "1Password 7" },
  { id: "com.agilebits.onepassword-osx", name: "1Password" },
  { id: "com.agilebits.onepassword4", name: "1Password 6" },
  { id: "2BUA8C4S2C.com.agilebits.onepassword7-helper", name: "1Password 7 mini" },
  { id: "2BUA8C4S2C.com.agilebits.onepassword-osx-helper", name: "1Password mini" },
  { id: "2BUA8C4S2C.com.agilebits.onepassword4-helper", name: "1Password 6 mini" },
  { id: "com.bitwarden.desktop", name: "Bitwarden" },
  { id: "com.apple.keychainaccess", name: "Keychain Access" },
  { id: "com.apple.Passwords", name: "Passwords" },
  { id: "com.apple.MobileSMS", name: "Messages" },
];

/**
 * The same on Windows, by executable. Mirrors `DEFAULT_EXCLUDED_BUNDLE_IDS`
 * in crates/guiderecorder-capture/src/windows/apps.rs.
 */
export const WINDOWS_EXCLUDED_APPS: readonly { id: string; name: string }[] = [
  { id: "1password.exe", name: "1Password" },
  { id: "bitwarden.exe", name: "Bitwarden" },
  { id: "keepass.exe", name: "KeePass" },
  { id: "keepassxc.exe", name: "KeePassXC" },
  { id: "enpass.exe", name: "Enpass" },
  { id: "credentialuibroker.exe", name: "Windows credential prompt" },
];

/** The suggested excluded apps on this platform. */
export const DEFAULT_EXCLUDED_APPS = isWindows() ? WINDOWS_EXCLUDED_APPS : MAC_EXCLUDED_APPS;

export const DEFAULT_EXCLUDED_IDS: readonly string[] = DEFAULT_EXCLUDED_APPS.map((a) => a.id);

/** A readable name for a bundle id (an executable on Windows) when it is a known one, else the id. */
export function appName(bundleId: string): string {
  return DEFAULT_EXCLUDED_APPS.find((a) => a.id === bundleId)?.name ?? bundleId;
}
