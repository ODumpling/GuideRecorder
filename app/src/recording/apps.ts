/**
 * The suggested excluded apps, with readable names. Mirrors
 * `DEFAULT_EXCLUDED_BUNDLE_IDS` in crates/guidesya-capture/src/macos/session.rs
 * (the settings default; apps.test.ts checks they match).
 */
export const DEFAULT_EXCLUDED_APPS: readonly { id: string; name: string }[] = [
  { id: "com.1password.1password", name: "1Password" },
  { id: "com.agilebits.onepassword7", name: "1Password 7" },
  { id: "com.agilebits.onepassword-osx", name: "1Password" },
  { id: "2BUA8C4S2C.com.agilebits.onepassword7-helper", name: "1Password 7 mini" },
  { id: "2BUA8C4S2C.com.agilebits.onepassword-osx-helper", name: "1Password mini" },
  { id: "com.bitwarden.desktop", name: "Bitwarden" },
  { id: "com.apple.keychainaccess", name: "Keychain Access" },
  { id: "com.apple.Passwords", name: "Passwords" },
  { id: "com.apple.MobileSMS", name: "Messages" },
];

export const DEFAULT_EXCLUDED_IDS: readonly string[] = DEFAULT_EXCLUDED_APPS.map((a) => a.id);

/** A readable name for a bundle id when it is a known one, else the id. */
export function appName(bundleId: string): string {
  return DEFAULT_EXCLUDED_APPS.find((a) => a.id === bundleId)?.name ?? bundleId;
}
