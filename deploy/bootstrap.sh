#!/usr/bin/env bash
# One-time setup of the deploy directory on the VPS. Safe to rerun: keeps an
# existing .env and certificate.
#
#   ssh ovh-vps 'bash -s' < deploy/bootstrap.sh
set -euo pipefail

dir=/srv/guiderecorder
sudo mkdir -p "$dir"
sudo chown "$(id -u):$(id -g)" "$dir"
cd "$dir"
mkdir -p certs backups
# The backup image writes as postgres (999).
sudo chown 999:999 backups

secret() { openssl rand -base64 48 | tr -d '/+=\n' | cut -c1-48; }

if [ ! -f .env ]; then
  umask 077
  cat > .env <<EOF
APP_HOST=gr.dufflebear.dev
POSTGRES_PASSWORD=$(secret)
BETTER_AUTH_SECRET=$(secret)
SCIM_TOKEN_SECRET=$(secret)
JOBS_TOKEN=$(secret)
STORAGE_ACCESS_KEY=guiderecorder
STORAGE_SECRET_KEY=$(secret)
# Optional: GOOGLE_*, MICROSOFT_*, STRIPE_*, RESEND_API_KEY, EMAIL_FROM
# Set by each deploy:
IMAGE_TAG=latest
EOF
  echo "wrote $dir/.env"
fi

# A placeholder until the Cloudflare origin certificate is copied in.
if [ ! -f certs/origin.pem ]; then
  openssl req -x509 -newkey rsa:2048 -nodes -days 30 -subj "/CN=gr.dufflebear.dev" \
    -keyout certs/origin.key -out certs/origin.pem 2>/dev/null
  echo "wrote a self-signed placeholder certificate in $dir/certs"
fi
chmod 600 certs/origin.key

# ufw-docker filters published ports with `ufw route`: let only Cloudflare
# reach Caddy (https://www.cloudflare.com/ips/).
for net in $(curl -fsS https://www.cloudflare.com/ips-v4) $(curl -fsS https://www.cloudflare.com/ips-v6); do
  sudo ufw route allow proto tcp from "$net" to any port 443 comment cloudflare >/dev/null
done

# Storage admin (S3 API 9000, console 9001) over Tailscale only. Log in with
# `sudo tailscale up` once; the rules apply to whatever arrives on tailscale0.
command -v tailscale >/dev/null || curl -fsSL https://tailscale.com/install.sh | sh
for port in 9000 9001; do
  sudo ufw route allow in on tailscale0 proto tcp to any port "$port" comment tailscale-storage >/dev/null
done
