#!/usr/bin/env bash
# Local development domain for the web app: https://app.guiderecorder.dev:3000.
#
# The web dev server answers on app.guiderecorder.dev with a certificate from
# a local CA (mkcert), so the web app and the desktop app's sign-in run over
# HTTPS like production. .dev is HTTPS-only in browsers, hence the CA.
# The certificate is shared by every checkout and worktree.
#
# Usage:
#   scripts/dev-domain.sh setup    trust the local CA, make the certificate, add the
#                                  hosts entry (idempotent; asks for your password)
#   scripts/dev-domain.sh status   report what's set up (exit 1 if anything is missing)

set -euo pipefail

DOMAIN="app.guiderecorder.dev"
CERT_DIR="${GUIDERECORDER_DEV_CERTS:-${HOME}/.guiderecorder-dev}"
CERT="${CERT_DIR}/${DOMAIN}.pem"
KEY="${CERT_DIR}/${DOMAIN}-key.pem"

log() { printf 'dev-domain: %s\n' "$*" >&2; }
die() { log "error: $*"; exit 1; }

usage() {
  sed -n '2,/^$/p' "$0" | sed -n '/^# Usage:/,$p' | sed 's/^# \{0,1\}//' >&2
  exit 2
}

has_hosts() { grep -Eq "^[[:space:]]*127\.0\.0\.1[[:space:]]+([^#]*[[:space:]])?${DOMAIN//./\\.}([[:space:]]|$)" /etc/hosts; }
ca_trusted() { security find-certificate -c "mkcert" /Library/Keychains/System.keychain >/dev/null 2>&1; }

setup() {
  command -v mkcert >/dev/null || die "mkcert is missing: brew install mkcert"
  if ca_trusted; then log "local CA already trusted"; else
    log "trusting mkcert's local CA (asks for your password)"
    mkcert -install
  fi
  if [[ -f "$CERT" && -f "$KEY" ]]; then log "certificate exists: $CERT"; else
    mkdir -p "$CERT_DIR"
    mkcert -cert-file "$CERT" -key-file "$KEY" "$DOMAIN"
  fi
  if has_hosts; then log "hosts entry exists"; else
    log "adding ${DOMAIN} to /etc/hosts (asks for your password)"
    printf '\n# GuideRecorder local web app (scripts/dev-domain.sh)\n127.0.0.1 %s\n::1 %s\n' "$DOMAIN" "$DOMAIN" | sudo tee -a /etc/hosts >/dev/null
    sudo dscacheutil -flushcache || true
  fi
  log "done. In web/.env.local: BETTER_AUTH_URL=https://${DOMAIN}:3000"
}

status() {
  local ok=0
  if ca_trusted; then log "local CA: trusted"; else log "local CA: not trusted"; ok=1; fi
  if [[ -f "$CERT" && -f "$KEY" ]]; then log "certificate: $CERT"; else log "certificate: missing"; ok=1; fi
  if has_hosts; then log "hosts: ${DOMAIN} -> 127.0.0.1"; else log "hosts: no entry for ${DOMAIN}"; ok=1; fi
  return $ok
}

case "${1:-}" in
  setup) setup ;;
  status) status ;;
  *) usage ;;
esac
