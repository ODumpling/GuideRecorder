# Releasing and updates

GuideRecorder ships as a signed, notarized universal macOS app on
[GitHub Releases](https://github.com/ODumpling/GuideRecorder/releases).
Installed apps update themselves from there with Tauri's updater.

## CI

`.github/workflows/ci.yml` runs on every push to `main` and every pull
request, but only the jobs whose files changed (the `changes` job maps paths
to jobs; editing `ci.yml` runs everything). The full suite runs nightly at
04:00 UTC, on manual dispatch, and before every release build:

| Job | Runner | What |
| --- | --- | --- |
| Rust | macOS | versions agree (`scripts/version.sh`), `cargo fmt --check`, `clippy -D warnings`, `cargo test` |
| Dependency licences | Ubuntu | `cargo deny check licenses` (policy in `deny.toml`) |
| Rust on Windows | Windows | `clippy -D warnings`, `cargo test` |
| Frontend | Ubuntu | `pnpm test`, `pnpm build`, Playwright e2e |
| Web | Ubuntu | guide types current, typecheck and test the packages and web app, build |

Tests that need TCC permissions or the login Keychain are `#[ignore]`d, so
CI never prompts.

## Cutting a release

```sh
scripts/version.sh set 0.2.0      # Cargo.toml, app/package.json, tauri.conf.json, Cargo.lock
git commit -am "Release 0.2.0"
git tag v0.2.0
git push origin main v0.2.0
```

The tag starts `.github/workflows/release.yml`, which:

1. fails unless the tag is `v` + the version in all three files;
2. builds `guiderecorder-mcp` for both architectures and joins them with
   `lipo`; it is bundled next to the app's executable, where Settings → MCP
   looks for it;
3. builds the app for `universal-apple-darwin` with
   `src-tauri/tauri.release.conf.json` merged in (`.app` + `.dmg`, updater
   artifacts, the MCP sidecar), signs it with the Developer ID certificate
   and notarizes it;
4. creates a **draft** release with GitHub's generated notes, the `.dmg`,
   `GuideRecorder.app.tar.gz` (+ `.sig`) and `latest.json`.

Check the draft, edit the notes if you like, and **publish** it. Only a
published release is "latest", so apps see the update only then. A version
with a pre-release suffix (`0.2.0-beta.1`) is published as a pre-release,
which `releases/latest` (and so the updater) skips.

## How apps update

`tauri.conf.json` → `plugins.updater` points at
`https://github.com/ODumpling/GuideRecorder/releases/latest/download/latest.json`
and holds the updater's public key. `src-tauri/src/updater.rs`:

- release builds check 20 s after startup and then every 6 hours, while
  Settings → General → **Check for updates automatically** is on (default);
- **Check for Updates…** in the app menu and **Check now** in Settings
  check at any time (dev builds too, though they can't install);
- an update is shown in a banner; nothing is downloaded until the user
  clicks **Install update**;
- **Restart now** quits through the normal quit flow (the open guide is
  saved, a recording is stopped and saved) and starts the new version.

The download is verified against the public key before it is installed.
macOS keeps Screen Recording and Accessibility grants across updates
because every release is signed with the same Developer ID.

## Secrets

Set these in GitHub → Settings → Secrets and variables → Actions:

| Secret | What |
| --- | --- |
| `APPLE_CERTIFICATE` | Developer ID Application certificate + key exported as `.p12`, base64 (`base64 -i cert.p12 \| pbcopy`) |
| `APPLE_CERTIFICATE_PASSWORD` | the `.p12` export password |
| `APPLE_SIGNING_IDENTITY` | e.g. `Developer ID Application: Jane Doe (TEAMID1234)` (`security find-identity -v -p codesigning`) |
| `APPLE_ID` | Apple ID email used for notarization |
| `APPLE_PASSWORD` | an [app-specific password](https://account.apple.com) for that Apple ID |
| `APPLE_TEAM_ID` | the 10-character team id |
| `TAURI_SIGNING_PRIVATE_KEY` | contents of the updater private key |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | its password |

### The updater key

The key pair was made with `pnpm tauri signer generate`. The private key
and its password live outside the repo (the maintainer keeps them in
`~/.tauri/guiderecorder_updater.key` and `…password.txt`); the public key is
in `tauri.conf.json`.

**Back the private key up.** Without it no future release can update
existing installs: they would have to download a new version by hand. If
it leaks, generate a new pair, put the new public key in `tauri.conf.json`,
and ship that release signed with the *old* key so installed apps accept it.

## A release build on your Mac

```sh
cd app
TAURI_SIGNING_PRIVATE_KEY="$(cat ~/.tauri/guiderecorder_updater.key)" \
TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$(cat ~/.tauri/guiderecorder_updater.password.txt)" \
APPLE_SIGNING_IDENTITY=- \
pnpm tauri build --config src-tauri/tauri.release.conf.json
```

This needs `app/src-tauri/binaries/guiderecorder-mcp-<target triple>` first
(`cargo build --release -p guiderecorder-mcp` and copy it there, e.g. as
`guiderecorder-mcp-aarch64-apple-darwin`). Plain `pnpm tauri build` and
`pnpm tauri dev` don't use the release config, so they need neither the
key nor the sidecar.

This build doesn't set `GUIDERECORDER_RELEASE`, so it keeps its secrets in
the dev Keychain service, apart from the installed release's (see
[signing.md](signing.md#keychain-dev-and-release-builds-keep-separate-entries)).
Don't set it here: an ad-hoc signature changes with every build, so the
release's entries would ask for your password each time.
