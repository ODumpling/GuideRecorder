# Local code signing (macOS)

GuidesYa needs Screen Recording and Accessibility. macOS stores these TCC
grants against the app's *designated requirement*, which comes from its code
signature. An ad-hoc signature (the default for `cargo build` and for unsigned
Tauri bundles) changes with every build. After each rebuild macOS treats the
app as new, and the grants you gave it no longer apply.

The fix is a stable, self-signed code-signing identity called
**`GuidesYa Local Dev`** in your login keychain. An app build signed with it has the
designated requirement `identifier "dev.guidesya.app" and certificate leaf = H"<cert hash>"`.
That stays the same across rebuilds for as long as you keep the certificate
(it is valid for 10 years).

Public releases will use Developer ID signing and notarization instead (see
design scratchpad #439). That is not covered here.

## One-time setup

```sh
scripts/macos-signing.sh create
scripts/macos-signing.sh status   # should print the identity and exit 0
```

`create` is idempotent. It:

1. Generates an RSA-2048 key and a self-signed certificate with
   `CN=GuidesYa Local Dev`, `extendedKeyUsage = codeSigning`, using Apple's
   `/usr/bin/openssl` (LibreSSL). Homebrew's OpenSSL 3 writes PKCS#12 files
   that `security import` rejects.
2. Imports the key and certificate into `~/Library/Keychains/login.keychain-db`
   with `-T /usr/bin/codesign`, so `codesign` can use the key without asking.
3. Marks the certificate trusted for code signing only
   (`security add-trusted-cert -r trustRoot -p codeSign`), as a user-level
   trust setting. macOS asks for your password here.

It never modifies or deletes other keychain items. The temporary key files go
in a `mktemp -d` directory with mode 700, which is removed on exit. If you
cancel the password prompt, run `create` again. It finds the existing
certificate and only retries the trust step.

### Commands

| Command | What it does |
| --- | --- |
| `create` | Create and trust the identity. Does nothing if it is already valid. |
| `status` | Report whether the identity is valid for code signing. Exits 1 if not. |
| `sign <path>...` | Re-sign a binary or `.app` with the identity. Exits non-zero if any signing fails. |
| `run <bin> [args...]` | Sign `<bin>` if the identity exists, then `exec` it. This is meant to be a cargo target runner. If the identity is missing or `codesign` fails, it prints a warning (with codesign's error) and still runs the binary with whatever signature it already had. |

Signing identifiers:

- A `.app` keeps the identifier from its `Info.plist` (`dev.guidesya.app`).
- The bare `guidesya-app` binary gets `dev.guidesya.app`, the same as the bundle.
- Every other bare binary gets `dev.guidesya.<basename>`: for example
  `dev.guidesya.guidesya` (CLI), `dev.guidesya.guidesya-mcp`, and
  `dev.guidesya.guidesya_core-<hash>` for test binaries. Each one therefore has
  its own designated requirement, so TCC and Keychain never mistake one for
  another.

Set `GUIDESYA_SIGNING_IDENTITY` to make the script use a different identity name.

## Bundled builds (`pnpm tauri build`)

`app/src-tauri/tauri.conf.json` sets
`bundle.macOS.signingIdentity = "GuidesYa Local Dev"`, so Tauri signs the
`.app` with that identity. Tauri's default hardened runtime stays on.

- **Without the identity** (a fresh machine, or CI) the bundle step fails. To
  override it for one build:
  - `APPLE_SIGNING_IDENTITY=- pnpm tauri build` for an ad-hoc signature (TCC
    grants won't persist), or
  - `APPLE_SIGNING_IDENTITY="Developer ID Application: …" pnpm tauri build` to
    use a real Developer ID.
- To check a build:
  `codesign -dv --verbose=2 target/release/bundle/macos/GuidesYa.app` and
  `codesign -d -r- target/release/bundle/macos/GuidesYa.app`.

## Dev loop (`pnpm tauri dev`)

`tauri dev` runs `cargo run`, which starts the bare binary
`target/debug/guidesya-app` with cargo's ad-hoc signature. Cargo has no
signing hook, but it does support a *target runner*. Point it at the script and
every `cargo run` (including the one inside `tauri dev`) signs the binary just
before it starts:

```sh
# from the repo root; add to your shell profile or a personal .envrc to make it stick
export CARGO_TARGET_AARCH64_APPLE_DARWIN_RUNNER="$PWD/scripts/macos-signing.sh run"
# Intel Macs: CARGO_TARGET_X86_64_APPLE_DARWIN_RUNNER
cd app && pnpm tauri dev
```

**Paths with spaces.** Cargo splits a runner given as an environment variable
on whitespace, and quoting inside the value doesn't help. If the repo path
contains a space (for example `~/My Code/GuidesYa`), cargo tries to run
`~/My`, and the export above breaks. The workaround is to symlink the script
to a path without spaces and point the runner at the symlink:

```sh
mkdir -p ~/.local/bin
ln -sf "$PWD/scripts/macos-signing.sh" ~/.local/bin/guidesya-signing
export CARGO_TARGET_AARCH64_APPLE_DARWIN_RUNNER="$HOME/.local/bin/guidesya-signing run"
```

(`$HOME` itself must not contain a space. If it does, use another space-free
directory.)

This is opt-in on purpose. It is not in a checked-in `.cargo/config.toml`, so
contributors without the identity and CI are unaffected (and the runner falls
back to running the binary with its existing ad-hoc signature anyway). The runner also applies to
`cargo run` for the CLI and MCP binaries and to `cargo test` binaries. Each
gets its own identifier (see above), and signing a debug binary takes
milliseconds.

### Caveat: who macOS charges permissions to

When a bare binary is started from a terminal, TCC charges Screen Recording
and Accessibility to the **responsible process**, which is the terminal app
(Terminal, iTerm, VS Code, Solo, …), not `guidesya-app`. So in `tauri dev`:

- Capture works if your *terminal* has the permission, and the grant persists
  because it belongs to the terminal.
- The onboarding permission checklist shows the terminal's status, not the
  app's.
- Signing the debug binary still helps. Keychain ACLs (for example the stored
  AI API key) are tied to the code signature, so with a stable signature you
  don't get an "allow access" prompt after every rebuild. A binary started on
  its own (`open`, Finder, launchd) also gets a stable identity.

To test the real first-run and permission flow as the app itself, build a
debug bundle and start it with `open`:

```sh
cd app && pnpm tauri build --debug --bundles app
open ../target/debug/bundle/macos/GuidesYa.app
```

That bundle is signed with `GuidesYa Local Dev` (from `tauri.conf.json`), so
grants persist across rebuilds.

## Troubleshooting

- **Grants look stale or duplicated in System Settings.** Reset them and grant
  again:
  `tccutil reset ScreenCapture dev.guidesya.app` and
  `tccutil reset Accessibility dev.guidesya.app`.
- **`status` says the certificate exists but is not valid.** The trust step
  never finished, or the private key is missing. Run `create` again.
- **You recreated the certificate.** The leaf hash changed, so every earlier
  grant is invalid. Reset with `tccutil` and grant again.
- **Removing the identity** (done by hand, the script never deletes anything):
  `security delete-identity -c "GuidesYa Local Dev" ~/Library/Keychains/login.keychain-db`.
