#!/usr/bin/env bash
# Local code-signing identity for GuideRecorder on macOS.
#
# macOS keys TCC grants (Screen Recording, Accessibility) to a binary's
# designated requirement. Ad-hoc signatures change on every rebuild, so the
# grants are lost; a stable self-signed identity keeps them. See docs/signing.md.
#
# Usage:
#   scripts/macos-signing.sh create          create + trust the identity (idempotent; asks for your password)
#   scripts/macos-signing.sh status          report whether the identity exists and is valid (exit 1 if not)
#   scripts/macos-signing.sh sign <path>...  re-sign a binary or .app with the identity
#   scripts/macos-signing.sh run <bin> [args...]
#                                            sign <bin> if the identity exists, then exec it
#                                            (for use as a cargo target runner)

set -euo pipefail

IDENTITY="${GUIDERECORDER_SIGNING_IDENTITY:-GuideRecorder Local Dev}"
BUNDLE_ID="dev.dufflebear.guiderecorder"
KEYCHAIN="${HOME}/Library/Keychains/login.keychain-db"
WORK_DIR=""
# Apple's LibreSSL writes PKCS#12 files that `security import` accepts;
# OpenSSL 3's defaults do not, so prefer it over a Homebrew openssl on PATH.
OPENSSL="/usr/bin/openssl"

log() { printf 'macos-signing: %s\n' "$*" >&2; }
die() { log "error: $*"; exit 1; }

usage() {
  sed -n '2,/^$/p' "$0" | sed -n '/^# Usage:/,$p' | sed 's/^# \{0,1\}//' >&2
  exit "${1:-2}"
}

require_macos() {
  [[ "$(uname -s)" == "Darwin" ]] || die "macOS only"
}

# True if a certificate with this exact common name is in the login keychain.
cert_exists() {
  security find-certificate -c "$IDENTITY" "$KEYCHAIN" >/dev/null 2>&1
}

# True if the identity is usable for code signing (cert + key, trusted).
identity_valid() {
  security find-identity -v -p codesigning "$KEYCHAIN" 2>/dev/null \
    | grep -Fq "\"$IDENTITY\""
}

cmd_status() {
  require_macos
  if identity_valid; then
    local line
    line="$(security find-identity -v -p codesigning "$KEYCHAIN" | grep -F "\"$IDENTITY\"" | head -n1)"
    log "identity \"$IDENTITY\" is valid for code signing:"
    printf '%s\n' "$line"
    return 0
  fi
  if cert_exists; then
    log "certificate \"$IDENTITY\" exists but is not a valid code-signing identity"
    log "(missing private key or not trusted for code signing). Run: scripts/macos-signing.sh create"
    security find-identity -p codesigning "$KEYCHAIN" 2>/dev/null | grep -F "\"$IDENTITY\"" >&2 || true
  else
    log "identity \"$IDENTITY\" not found. Run: scripts/macos-signing.sh create"
  fi
  return 1
}

cmd_create() {
  require_macos
  if identity_valid; then
    log "identity \"$IDENTITY\" already exists and is valid; nothing to do"
    return 0
  fi

  [[ -x "$OPENSSL" ]] || die "$OPENSSL not found"

  # Private key material lives here briefly; always remove it.
  WORK_DIR="$(mktemp -d)"
  trap 'rm -rf "$WORK_DIR"' EXIT
  chmod 700 "$WORK_DIR"
  local work="$WORK_DIR"

  if cert_exists; then
    # Cert is present but not trusted (e.g. a previous run was cancelled at the
    # trust prompt). Export it and only add the trust setting.
    log "certificate \"$IDENTITY\" exists; adding code-signing trust only"
    security find-certificate -c "$IDENTITY" -p "$KEYCHAIN" >"$work/cert.pem"
  else
    log "creating self-signed code-signing certificate \"$IDENTITY\""
    cat >"$work/openssl.cnf" <<CNF
[ req ]
distinguished_name = dn
x509_extensions    = ext
prompt             = no

[ dn ]
CN = $IDENTITY

[ ext ]
basicConstraints       = critical, CA:false
keyUsage               = critical, digitalSignature
extendedKeyUsage       = critical, codeSigning
subjectKeyIdentifier   = hash
CNF
    (
      umask 077
      "$OPENSSL" req -x509 -newkey rsa:2048 -nodes -sha256 -days 3650 \
        -config "$work/openssl.cnf" \
        -keyout "$work/key.pem" -out "$work/cert.pem" >/dev/null 2>&1
      local pass
      pass="$("$OPENSSL" rand -hex 16)"
      "$OPENSSL" pkcs12 -export -inkey "$work/key.pem" -in "$work/cert.pem" \
        -name "$IDENTITY" -passout "pass:$pass" -out "$work/identity.p12"
      # -T lets codesign use the key without a keychain prompt on every build.
      security import "$work/identity.p12" -k "$KEYCHAIN" -f pkcs12 \
        -P "$pass" -T /usr/bin/codesign
    ) || die "failed to create or import the certificate"
  fi

  log "marking \"$IDENTITY\" trusted for code signing (macOS will ask for your password)"
  security add-trusted-cert -r trustRoot -p codeSign -k "$KEYCHAIN" "$work/cert.pem" \
    || die "failed to set trust; re-run 'scripts/macos-signing.sh create' to retry"

  if identity_valid; then
    log "done; \"$IDENTITY\" is ready"
  else
    die "identity still not valid; see 'scripts/macos-signing.sh status'"
  fi
}

# Sign one path; returns non-zero on failure. A .app keeps its bundle's
# identifier. Bare Mach-O binaries get an explicit one so the designated
# requirement (identifier + certificate) is stable: guiderecorder-app matches the
# bundled app, everything else (CLI, MCP, test binaries) gets its own.
sign_path() {
  local path="$1"
  if [[ ! -e "$path" ]]; then
    log "error: no such file: $path"
    return 1
  fi
  local args=(--force --sign "$IDENTITY" --timestamp=none)
  if [[ -d "$path" ]]; then
    args+=(--deep)
  else
    args+=(--identifier "$(binary_identifier "$path")")
  fi
  codesign "${args[@]}" "$path" || return 1
  log "signed $path"
}

binary_identifier() {
  local name
  name="$(basename "$1")"
  if [[ "$name" == "guiderecorder-app" ]]; then
    printf '%s\n' "$BUNDLE_ID"
  else
    printf 'dev.dufflebear.guiderecorder.%s\n' "$name"
  fi
}

cmd_sign() {
  require_macos
  [[ $# -ge 1 ]] || usage
  identity_valid || die "identity \"$IDENTITY\" not found or not valid; run: scripts/macos-signing.sh create"
  local p
  for p in "$@"; do
    sign_path "$p" || die "failed to sign $p"
  done
}

cmd_run() {
  [[ $# -ge 1 ]] || usage
  local bin="$1"
  shift
  if [[ "$(uname -s)" == "Darwin" ]]; then
    if identity_valid; then
      sign_path "$bin" || log "warning: failed to sign $bin; running it with its existing signature"
    else
      log "warning: \"$IDENTITY\" not set up; running $bin with its ad-hoc signature"
    fi
  fi
  exec "$bin" "$@"
}

main() {
  [[ $# -ge 1 ]] || usage
  local cmd="$1"
  shift
  case "$cmd" in
    create) cmd_create "$@" ;;
    status) cmd_status "$@" ;;
    sign) cmd_sign "$@" ;;
    run) cmd_run "$@" ;;
    -h | --help | help) usage 0 ;;
    *) log "unknown command: $cmd"; usage ;;
  esac
}

main "$@"
