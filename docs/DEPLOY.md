# Deploy Conflux on a Google Cloud VM

About 15 minutes, most of it waiting for the first build. Everything runs on
your VM; nothing here needs a paid service or a new account.

You'll end up with Conflux at `https://<your-vm-ip>.sslip.io` (or your own
domain), which iPhone Safari can add to the home screen.

## 1. Requirements

- A Compute Engine VM running Debian 12 or Ubuntu 22.04+ with an external IP.
  An e2-micro is enough to run it; the script adds swap so it can also build.
- A US region if you want The Roku Channel and US catalogs (providers
  serve by the VM's location, not your phone's).

## 2. Open ports 80 and 443

Console: **Compute Engine > VM instances >** your VM **> Edit > Networking >**
tick **Allow HTTP traffic** and **Allow HTTPS traffic**, then **Save**.

Or from Cloud Shell:

```
gcloud compute instances add-tags YOUR_VM --zone YOUR_ZONE --tags http-server,https-server
```

Port 80 is needed so the HTTPS certificate can be issued. The Rust server
itself listens only on 127.0.0.1:8080 behind Caddy.

Tip: an ephemeral external IP changes if the VM is stopped and started, which
changes the sslip.io address. Reserving it as a static IP (VPC network > IP
addresses) keeps the address fixed.

## 3. Get the code and run the installer

SSH in (the **SSH** button in the console works), then:

```
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

## 4. Check the providers

```
cd /opt/conflux && ./conflux check "star trek"
```

Each provider prints OK or FAIL with the reason. These are the real
provider sites, reached from your VM, so this is the true test of which ones
work. Live status is also at `https://<address>/api/providers`.

## 5. Add it to your iPhone

1. Open the address in **Safari** (not Chrome; iOS only installs web apps
   from Safari's share sheet).
2. Tap **Share**, then **Add to Home Screen**, then **Add**.
3. Open Conflux from the home screen. It runs full screen like an app.

Tapping a result opens that provider's page, or its app if you have it
installed.

## Updating

```
cd ~/conflux && git pull && bash deploy/install.sh
```

## Settings

Put any of these in `/etc/conflux.env` and run `sudo systemctl restart conflux`:

| Setting | Default | What it does |
| --- | --- | --- |
| `CONFLUX_PROVIDERS` | `pluto,plex,tubi,roku` | Which providers to use. Add `samsung` to turn Samsung TV Plus on. |
| `CONFLUX_REFRESH_MINUTES` | `60` | How often to re-download the channel and catalog lists. |
| `CONFLUX_SAMSUNG_REGION` | `us` | Samsung TV Plus region code. |
| `RUST_LOG` | `conflux=info,warn` | Log detail (`conflux=debug` for more). |

## Troubleshooting

- **Page won't load:** step 2 (ports) is the usual cause. `sudo systemctl status caddy conflux` shows both services.
- **Certificate errors:** Caddy's log says why: `sudo journalctl -u caddy -n 50`. Both ports 80 and 443 must be open.
- **A provider shows FAIL:** the provider changed its web app. The rest keep working; send the `conflux check` output to get the parser fixed.
