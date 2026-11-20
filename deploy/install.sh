#!/usr/bin/env bash
# Builds Conflux on this VM and runs it behind Caddy with HTTPS.
# Safe to re-run: it pulls the latest code, rebuilds and restarts. If the
# new build doesn't come up healthy, the previous one is put back.
#
#   bash deploy/install.sh                  # same domain as last time, else <external-ip>.sslip.io
#   bash deploy/install.sh tv.example.com   # use (and remember) your own domain
#   bash deploy/install.sh --rollback       # put the previous build back
#
# Tested target: Ubuntu 22.04+ on Oracle Cloud (Ampere A1).
set -euo pipefail

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
APP=/opt/conflux
DOMAIN_FILE="$APP/DOMAIN"

say() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }
die() { printf '\n\033[1;31m!! %s\033[0m\n' "$*" >&2; exit 1; }

# Waits up to 30 s for the server to answer /healthz.
healthy() {
  for _ in $(seq 1 30); do
    curl -fsS http://127.0.0.1:8080/healthz >/dev/null 2>&1 && return 0
    sleep 1
  done
  return 1
}

# Puts the previous build back and restarts it.
restore_previous() {
  [ -x "$APP/conflux.prev" ] || die "no previous build to go back to"
  sudo install -m 755 "$APP/conflux.prev" "$APP/conflux"
  if [ -d "$APP/web.prev" ]; then
    sudo rm -rf "$APP/web"
    sudo cp -r "$APP/web.prev" "$APP/web"
  fi
  sudo systemctl restart conflux
}

if [ "${1:-}" = "--rollback" ]; then
  say "Rolling back to the previous build"
  restore_previous
  healthy || die "the previous build isn't answering either; see: sudo journalctl -u conflux -n 50"
  echo "Previous build is running."
  exit 0
fi

DOMAIN="${1:-}"
if [ -z "$DOMAIN" ] && [ -s "$DOMAIN_FILE" ]; then
  DOMAIN="$(cat "$DOMAIN_FILE")"
fi
if [ -z "$DOMAIN" ]; then
  IP="$(curl -fsS --max-time 10 https://api.ipify.org || curl -fsS --max-time 10 https://ifconfig.me || true)"
  [ -n "$IP" ] || die "couldn't find this VM's public IP; pass a domain: bash deploy/install.sh tv.example.com"
  DOMAIN="${IP//./-}.sslip.io"
fi
say "Using https://$DOMAIN"

say "System packages"
sudo apt-get update -y
sudo apt-get install -y build-essential pkg-config git curl debian-keyring debian-archive-keyring apt-transport-https gnupg

# Small VMs (1 GB) run out of memory compiling Rust. Add swap once.
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

say "Updating the code"
cd "$REPO_DIR"
# A failed pull must stop the deploy, not quietly rebuild the old code.
if [ "${SKIP_PULL:-}" = 1 ]; then
  echo "SKIP_PULL=1: deploying the code as it is here"
else
  git pull --ff-only || die "git pull failed (local changes or a diverged branch?). Fix it, or deploy what's here with: SKIP_PULL=1 bash deploy/install.sh"
fi
REV="$(git rev-parse --short HEAD)"
git diff --quiet HEAD -- || REV="$REV-dirty"

say "Building $REV (first build takes a few minutes)"
cargo build --release

say "Installing to $APP"
sudo mkdir -p "$APP"
# Keep what's running now, so a bad build can be undone.
if [ -x "$APP/conflux" ]; then
  sudo cp -p "$APP/conflux" "$APP/conflux.prev"
  sudo rm -rf "$APP/web.prev"
  [ -d "$APP/web" ] && sudo cp -r "$APP/web" "$APP/web.prev"
fi
sudo install -m 755 target/release/conflux "$APP/conflux"
sudo rm -rf "$APP/web"
sudo cp -r web "$APP/web"
# A new cache name per build makes installed iPhone apps pick up the new
# files instead of serving the old ones from the service worker's cache.
sudo sed -i "s/^const VERSION = \"conflux-[^\"]*\";/const VERSION = \"conflux-$REV\";/" "$APP/web/sw.js"
grep -q "conflux-$REV" "$APP/web/sw.js" || die "couldn't stamp the service worker version in web/sw.js"
echo "$DOMAIN" | sudo tee "$DOMAIN_FILE" >/dev/null
sudo install -m 644 deploy/conflux.service /etc/systemd/system/conflux.service
sudo systemctl daemon-reload
sudo systemctl enable conflux
sudo systemctl restart conflux

say "Checking the new build"
if ! healthy; then
  sudo journalctl -u conflux -n 30 --no-pager || true
  if [ -x "$APP/conflux.prev" ]; then
    say "New build didn't come up; putting the previous one back"
    restore_previous
    if healthy; then
      die "deploy of $REV failed; the previous build is running again"
    fi
    die "deploy of $REV failed and the previous build isn't answering either"
  fi
  die "Conflux didn't come up; see: sudo journalctl -u conflux -n 50"
fi
echo "$REV is up."

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


cat <<DONE

Done. Open this on your iPhone in Safari:

    https://$DOMAIN

If it doesn't load, check that the VM allows HTTP and HTTPS traffic
(see docs/DEPLOY.md, step 2). Provider status: https://$DOMAIN/api/providers
Logs: sudo journalctl -u conflux -f
Undo this deploy: bash deploy/install.sh --rollback
DONE
