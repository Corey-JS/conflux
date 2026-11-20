# Deploy Conflux on Oracle Cloud

About 15 minutes. Everything runs on one Oracle Cloud VM in the Always Free
tier; nothing here needs a paid service.

You'll end up with Conflux at `https://<your-vm-ip>.sslip.io` (or your own
domain), which iPhone Safari can add to the home screen.

## Current server

| | |
| --- | --- |
| Address | https://132-145-136-144.sslip.io |
| VM | `instance-20261119-2049`, Ampere A1 (ARM), 4 OCPU / 24 GB, Ubuntu 26.04 |
| SSH | `ssh -i ~/.ssh/conflux_oracle ubuntu@132.145.136.144` |

The old Google Cloud VM is retired; its Conflux service is stopped and
disabled.

## Updating

From your Mac, after pushing to GitHub:

```
ssh -i ~/.ssh/conflux_oracle ubuntu@132.145.136.144 \
  'cd ~/conflux && git pull --ff-only && bash deploy/install.sh'
```

The script rebuilds, reinstalls and restarts Conflux, then checks that it's
up. A rebuild takes a minute or two on the A1 VM. Installed iPhone apps pick
up a new version after being closed and reopened once or twice.

## Setting up a new server

### 1. Create the VM

In the Oracle Cloud console: **Compute > Instances > Create instance**.

- **Image:** Canonical Ubuntu (22.04 or newer).
- **Shape:** Ampere `VM.Standard.A1.Flex`. Up to 4 OCPU and 24 GB is in the
  Always Free tier. Conflux needs far less; the extra just makes builds fast.
- **Region:** a US one (the current server is in one) if you want The Roku
  Channel and US catalogs. Providers serve by the VM's location, not your
  phone's.
- **Networking:** a public subnet with a public IPv4 address.
- **SSH keys:** upload your public key (for example `~/.ssh/conflux_oracle.pub`).

Tip: an ephemeral public IP can change if the VM is recreated, which changes
the sslip.io address. **Networking > IP management > Reserved public IPs**
keeps it fixed.

### 2. Open ports 80 and 443 (two places)

Oracle blocks web traffic twice: in the cloud network and on the VM itself.

**Cloud network.** **Networking > Virtual cloud networks >** your VCN **>
Security Lists > Default Security List > Add Ingress Rules**, and add two
rules: source CIDR `0.0.0.0/0`, IP protocol TCP, destination port `80`; and
the same for `443`.

**On the VM.** Oracle's Ubuntu images reject everything but SSH with
iptables. SSH in and run:

```
sudo iptables -I INPUT 1 -p tcp --dport 80 -m state --state NEW -j ACCEPT
sudo iptables -I INPUT 1 -p tcp --dport 443 -m state --state NEW -j ACCEPT
sudo netfilter-persistent save
```

The last line keeps the rules after a reboot. Don't use `ufw` on Oracle
images; it fights with these rules.

Port 80 is needed so the HTTPS certificate can be issued. The Rust server
itself listens only on 127.0.0.1:8080 behind Caddy.

### 3. Get the code and run the installer

```
ssh -i ~/.ssh/conflux_oracle ubuntu@<vm-ip>
sudo apt-get update && sudo apt-get install -y git
git clone https://github.com/Corey-JS/conflux.git
cd conflux
bash deploy/install.sh
```

If the repo is private, `git clone` asks you to sign in to GitHub; a
personal access token works as the password, or install `gh` and run
`gh repo clone Corey-JS/conflux`.

To use your own domain instead of sslip.io, point an A record at the VM's IP
and run `bash deploy/install.sh tv.yourdomain.com`.

The script installs build tools and Rust, adds 2 GB of swap if the VM has
none, builds Conflux, installs it as a systemd service, installs Caddy for
HTTPS, and prints the address to open.

### 4. Check the providers

```
cd /opt/conflux && ./conflux check "star trek"
```

Each provider prints OK or FAIL with the reason. These are the real
provider sites, reached from your VM, so this is the true test of which ones
work. Live status is also at `https://<address>/api/providers`.

### 5. Add it to your iPhone

1. Open the address in **Safari** (not Chrome; iOS only installs web apps
   from Safari's share sheet).
2. Tap **Share**, then **Add to Home Screen**, then **Add**.
3. Open Conflux from the home screen. It runs full screen like an app.

Tapping a result opens that provider's page, or its app if you have it
installed.

## Settings

Put any of these in `/etc/conflux.env` and run `sudo systemctl restart conflux`:

| Setting | Default | What it does |
| --- | --- | --- |
| `CONFLUX_PROVIDERS` | `pluto,plex,roku` | Which providers to use. Add `samsung` to turn Samsung TV Plus on. `tubi` can be added too, but its search has answered 401 since Nov 2026. |
| `CONFLUX_REFRESH_MINUTES` | `60` | How often to re-download the channel and catalog lists. |
| `CONFLUX_PLEX_REGION` | `us` | Plex region code: `us`, `ca`, `gb`, `au`, `nz`, `mx`, `es` or `fr`. |
| `CONFLUX_SAMSUNG_REGION` | `us` | Samsung TV Plus region code. |
| `CONFLUX_ART_DIR` | `/var/cache/conflux/art` under systemd | Where shrunken show art is kept between restarts. Without it (and outside systemd) thumbnails are kept in memory only. Each is deleted 30 days after it's made, and made again if still needed. |
| `RUST_LOG` | `conflux=info,warn` | Log detail (`conflux=debug` for more). |

## Troubleshooting

- **Page won't load:** step 2 (ports) is the usual cause; check both the
  security list and the VM's iptables (`sudo iptables -S INPUT` should show
  ACCEPT rules for 80 and 443 above the final REJECT).
  `sudo systemctl status caddy conflux` shows both services.
- **Certificate errors:** Caddy's log says why: `sudo journalctl -u caddy -n 50`. Both ports 80 and 443 must be open.
- **A provider shows FAIL:** the provider changed its web app. The rest keep working; send the `conflux check` output to get the parser fixed.
- **Conflux logs:** `sudo journalctl -u conflux -f`.
