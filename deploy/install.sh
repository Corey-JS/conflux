#!/usr/bin/env bash
# Builds Conflux on this VM and runs it behind Caddy with HTTPS.
# Safe to re-run: it pulls the latest code, rebuilds and restarts.
#
#   bash deploy/install.sh                  # uses <external-ip>.sslip.io
#   bash deploy/install.sh tv.example.com   # uses your own domain
#
# Tested target: Debian 12 / Ubuntu 22.04+ on Google Compute Engine.
set -euo pipefail

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
DOMAIN="${1:-}"

say() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }

if [ -z "$DOMAIN" ]; then
  IP="$(curl -fsS -H 'Metadata-Flavor: Google' \
    http://metadata.google.internal/computeMetadata/v1/instance/network-interfaces/0/access-configs/0/external-ip || true)"
  [ -n "$IP" ] || IP="$(curl -fsS https://api.ipify.org)"
  DOMAIN="${IP//./-}.sslip.io"
fi
say "Using https://$DOMAIN"

say "System packages"
sudo apt-get update -y
sudo apt-get install -y build-essential pkg-config git curl debian-keyring debian-archive-keyring apt-transport-https gnupg

# Small VMs (e2-micro, 1 GB) run out of memory compiling Rust. Add swap once.
if [ "$(swapon --show | wc -l)" -eq 0 ]; then
  say "Adding 2 GB swap for the build"
  sudo fallocate -l 2G /swapfile
  sudo chmod 600 /swapfile
  sudo mkswap /swapfile
  sudo swapon /swapfile
  grep -q '^/swapfile' /etc/fstab || echo '/swapfile none swap sw 0 0' | sudo tee -a /etc/fstab
fi

if ! command -v cargo >/dev/null 2>&1; then
  say "Installing Rust"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
fi
# shellcheck disable=SC1091
source "$HOME/.cargo/env"

say "Building (first build takes a few minutes)"
cd "$REPO_DIR"
git pull --ff-only || true
cargo build --release

say "Installing to /opt/conflux"
sudo mkdir -p /opt/conflux
sudo install -m 755 target/release/conflux /opt/conflux/conflux
sudo rm -rf /opt/conflux/web
sudo cp -r web /opt/conflux/web
sudo install -m 644 deploy/conflux.service /etc/systemd/system/conflux.service
sudo systemctl daemon-reload
sudo systemctl enable conflux
sudo systemctl restart conflux

if ! command -v caddy >/dev/null 2>&1; then
  say "Installing Caddy"
  curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/gpg.key' \
    | sudo gpg --dearmor --yes -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
  curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt' \
    | sudo tee /etc/apt/sources.list.d/caddy-stable.list >/dev/null
  sudo chmod o+r /usr/share/keyrings/caddy-stable-archive-keyring.gpg /etc/apt/sources.list.d/caddy-stable.list
  sudo apt-get update -y
  sudo apt-get install -y caddy
fi

say "Configuring Caddy"
sed "s/^DOMAIN {/$DOMAIN {/" deploy/Caddyfile.template | sudo tee /etc/caddy/Caddyfile >/dev/null
sudo systemctl reload caddy || sudo systemctl restart caddy

say "Checking the server"
for _ in $(seq 1 20); do
  curl -fsS http://127.0.0.1:8080/healthz >/dev/null 2>&1 && break
  sleep 1
done
curl -fsS http://127.0.0.1:8080/healthz && echo " (server is up)"

cat <<DONE

Done. Open this on your iPhone in Safari:

    https://$DOMAIN

If it doesn't load, check that the VM allows HTTP and HTTPS traffic
(see docs/DEPLOY.md, step 2). Provider status: https://$DOMAIN/api/providers
Logs: sudo journalctl -u conflux -f
DONE
