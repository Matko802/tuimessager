# tuimessager

Self-hosted, Discord-free messenger with a **Concord-style TUI** — written in Rust.

- `tuimessager-server`: Axum REST + WebSocket server, SQLite storage. Runs in **Podman** on your PC (`x86_64`) and Raspberry Pi 5 (`aarch64`).
- `tuimessager` (TUI): ratatui client that ports Concord's layout and features onto the self-hosted protocol. **Zero Discord API** — no gateway, no REST-to-Discord, no QR/MFA/captcha, no Rich Presence socket, no Discord voice.
- Nix flake (crane + rust-overlay, like Concord's) with `x86_64-linux` + `aarch64-linux` packages, OCI image for `podman load`, and a NixOS module.

Origin: cloned `https://github.com/chojs23/concord` for TUI/UX reference, then reimplemented the client against a new self-hosted protocol. No Discord code is included.

## Layout

```
.
├── Cargo.toml                    # workspace
├── crates/
│   ├── tuimessager-protocol/     # shared REST/WS types (servers, channels, messages…)
│   ├── tuimessager-server/       # Axum + SQLite + broadcast WS
│   └── tuimessager-tui/          # ratatui client (Servers|Channels|Messages|Members)
├── Containerfile.server          # multi-arch server image (amd64 + arm64)
├── compose.yaml                  # podman compose (PC + RPi5)
├── quadlet/                      # systemd-quadlet for Podman
├── nix/package.nix
├── flake.nix
└── podman-*.sh
```

## Quickstart — PC (NixOS x86_64, Podman)

```sh
# 1) build image (native arch)
./podman-build.sh
# cross-build for the Pi from your PC:
./podman-build.sh arm64

# 2a) compose (needs the compose plugin: `podman-compose` or `docker-compose`)
podman compose up -d
curl http://127.0.0.1:3000/health   # -> ok

# 2b) plain podman (no plugin needed)
./podman-run.sh

# 2c) Podman systemd via Quadlet (race-free, no compose needed)
cp quadlet/* ~/.config/containers/systemd/ && systemctl --user daemon-reload
```

Create your first account (open registration is on by default; turn it off after):

```sh
curl -X POST http://127.0.0.1:3000/api/v1/register \
  -H 'Content-Type: application/json' \
  -d '{"name":"alice","password":"correct-horse"}'
# -> {"token":"tm_...","user":{...}}
export TUIMESSAGER_TOKEN='tm_...'
export TUIMESSAGER_URL='http://127.0.0.1:3000'
```

Run the TUI:

```sh
cargo run -p tuimessager-tui
# or via nix:
nix run .#tui
```

## Raspberry Pi 5 (aarch64)

Option A — Podman on Raspberry Pi OS:

```sh
scp tuimessager-server.tar pi@pi5:~   # if cross-built, or just build on the Pi
# on the Pi:
./podman-build.sh          # native arm64 build
podman compose up -d
```

Option B — NixOS on the Pi (aarch64-linux):

```sh
nix build .#tuimessager-server --system aarch64-linux
# or with remote builder from your PC:
nixos-rebuild switch --flake .#pi5 --target-host pi@pi5 --use-remote-sudo
```

services.tuimessager example (NixOS, PC or Pi):

```nix
{
  services.tuimessager = {
    enable = true;
    bind = "0.0.0.0:3000";
    dataDir = "/var/lib/tuimessager";
    allowRegistration = false;
  };
}
```

OCI image for `podman load` (builds natively per-arch):

```sh
nix build .#server-image
podman load < result
podman run -p 3000:3000 -v ./data:/data localhost/tuimessager-server:latest
```

## Public internet over public wifi — free HTTPS (Caddy + Let's Encrypt, $0)

Your phone/laptop on café wifi can reach a server at home if the server has
a domain name and ports 80+443 forwarded to it. Everything below is free.

1. **Get a free domain.** Any domain works; free options: a DuckDNS subdomain
   (`yourname.duckdns.org`, free, supports dynamic IP updates) or any domain
   you already own. Point its A record at your home public IP.
2. **Forward ports 80 and 443** on your home router to the machine running
   the server (needed for the TLS challenge + traffic).
3. **Bring up the `public` profile:**
   ```sh
   TUIMESSAGER_DOMAIN=yourname.duckdns.org podman compose --profile public up -d
   ```
   Caddy fetches a Let's Encrypt certificate automatically on the first
   request and renews it forever. `curl https://yourname.duckdns.org/health`.
4. **Lock it down:** set `TUIMESSAGER_ALLOW_REGISTRATION=false` in
   `compose.yaml` after creating your account, then
   `podman compose --profile public up -d` again.
5. **Connect from anywhere:** `TUIMESSAGER_URL=https://yourname.duckdns.org tuimessager`.

No port forwarding possible (CGNAT, dorm wifi)? Use Tailscale instead —
also free, no ports or DNS needed (see previous section).

### Raspberry Pi 5 walkthrough (Raspberry Pi OS + Podman)

Assumes the Pi is at home on your LAN and your domain is
`tuidns.duckdns.org`.

1. **Copy the project to the Pi** (from your PC):
   ```sh
   scp -r --exclude=target /mnt/ssd/My-Files/Projects/tuimessager pi@<pi-lan-ip>:~/tuimessager
   # or: push to GitHub once, then on the Pi: git clone <your-repo>
   ```
2. **On the Pi — install Podman + compose:**
   ```sh
   sudo apt update && sudo apt install -y podman podman-compose ca-certificates curl cron
   ```
3. **Give the Pi a fixed LAN IP** (router DHCP reservation for the Pi's MAC,
   e.g. `192.168.1.50`), then **forward ports 80/443 (TCP, plus 443/UDP for
   HTTP/3)** on your router to that IP.
4. **Keep DuckDNS updated** (home IPs change). Get your token from
   https://www.duckdns.org, then on the Pi:
   ```sh
   cd ~/tuimessager
   TOKEN=paste-your-token-here ./duckdns-update.sh tuidns   # expect "OK"
   (crontab -l 2>/dev/null; echo '*/5 * * * * TOKEN=paste-your-token-here /home/pi/tuimessager/duckdns-update.sh tuidns >> /home/pi/duckdns.log 2>&1') | crontab -
   ```
5. **Build (native arm64) and launch with free TLS:**
   ```sh
   cd ~/tuimessager
   ./podman-build.sh
   TUIMESSAGER_DOMAIN=tuidns.duckdns.org podman-compose --profile public up -d
   ```
   First request triggers the Let's Encrypt certificate (takes ~30s).
6. **Verify from outside your LAN** (phone on mobile data, wifi off):
   ```sh
   curl https://tuidns.duckdns.org/health   # -> ok
   ```
7. **Create your account, then lock registration:**
   ```sh
   curl -X POST https://tuidns.duckdns.org/api/v1/register \
     -H 'Content-Type: application/json' \
     -d '{"name":"alice","password":"a-strong-password"}'
   ```
   Edit `compose.yaml` → `TUIMESSAGER_ALLOW_REGISTRATION: "false"`, then
   `TUIMESSAGER_DOMAIN=tuidns.duckdns.org podman-compose --profile public up -d`.
8. **Connect the TUI from anywhere:**
   ```sh
   TUIMESSAGER_URL=https://tuidns.duckdns.org tuimessager
   ```

Pi running NixOS instead? Add the flake input + overlay (same as fish-flake
on PC), import `inputs.tuimessager.nixosModules.default`, and set
`services.tuimessager = { enable = true; domain = "tuidns.duckdns.org";
acmeEmail = "you@example.com"; allowRegistration = false; };`, then
`nixos-rebuild switch`.

**NixOS module with free TLS** (PC or Pi on NixOS, in fish-flake):

```nix
{
  imports = [ inputs.tuimessager.nixosModules.default ];
  services.tuimessager = {
    enable = true;
    bind = "127.0.0.1:3000";
    allowRegistration = false;
    domain = "yourname.duckdns.org";  # enables Caddy + Let's Encrypt, opens 80/443
    acmeEmail = "you@example.com";
  };
}
```

## Nix flake

> First run `nix flake update` to generate `flake.lock` (inputs: nixpkgs,
> flake-utils, rust-overlay, crane — same stack as Concord's flake).

```sh
nix develop            # rust 1.90 + sqlite + podman
nix build .#tuimessager-server
nix build .#tuimessager-tui
nix run .              # TUI
nix run .#server       # server
nix flake check
```

## TUI — Concord parity (Discord parts removed)

| Concord feature | tuimessager status |
|---|---|
| 4-pane dashboard (Servers/Channels/Messages/Members), header, composer | ✅ ported (`tui/ui.rs`) |
| vim keys `j/k/gg/G/Ctrl-d/u/Tab/1-4`, leader `Space` | ✅ (`Space Space` switcher, `Space a/n/o/p/r`, `Space 1/2/4` toggles) |
| `config.toml`/`keymap.toml`/`theme.toml` XDG paths | ✅ subset in `~/.config/tuimessager/config.toml` (`--init-config`, `--check-config`) |
| Markdown subset + syntect code blocks, underline URLs (`o` opens) | ✅ |
| Reactions + emoji picker (`:` / `r`), favorites roadmap | ✅ unicode; custom-emoji-as-links dropped (no Discord CDN) |
| Threads/forum as channel kinds, reply (`R`), pin, edit (`e`), delete (`d`) | ✅ backend-native |
| Message search (`/`) with author/channel filter | ✅ `GET /api/v1/search` |
| Notification inbox (`Space n`) + desktop toasts, mention `@name` detection | ✅ server-side mention scan |
| Fuzzy channel switcher (`Space Space`, `*` prefix) | ✅ subsequence scorer |
| Composer: `Enter` send, `Esc` cancel, nonce idempotency, reply/edit, `$EDITOR` roadmap | ✅ |
| Image preview protocols (kitty/sixel/iterm2/halfblocks) | ⚠️ config kept, v1 renders link rows; ratatui-image wiring is roadmap |
| Voice calls / screen-share / noise suppression / portal capture | ⚠️ UI placeholder only; self-hosted audio is roadmap (no Discord voice/RTP/opus) |
| Auth token/email/QR/captcha/keychain | ❌ replaced: username+password → opaque `tm_` token (`TUIMESSAGER_TOKEN` env or `~/.local/state/tuimessager/token` `0600`) |
| Discord gateway/REST/RPC/Rich Presence | ❌ removed by design |

Keys (defaults): `1-4` focus · `Tab` cycle · `j/k` move · `g`/`G` top/bottom · `i` compose · `Enter` send/select · `Esc` close · `/` search · `:` emoji · `y` copy · `r` react · `R` reply · `e` edit · `d` delete-confirm · `o` open URL · `Space Space` switcher · `Space a` actions · `Space n` inbox · `Space o` help · `Space p` profile · `Space l` log out · `q` quit.

Env:

```
TUIMESSAGER_URL=http://127.0.0.1:3000
TUIMESSAGER_TOKEN=tm_...
TUIMESSAGER_BIND=0.0.0.0:3000            # server
TUIMESSAGER_DATA_DIR=/data               # server (sqlite file tuimessager.db)
TUIMESSAGER_ALLOW_REGISTRATION=true      # server
```

## API (v1)

```
POST /api/v1/register {name,password} -> {token,user}
POST /api/v1/login    {name,password} -> {token,user}
GET  /api/v1/me
POST /api/v1/logout   (invalidates current token)
GET  /api/v1/servers | POST /api/v1/servers
GET  /api/v1/servers/:id/channels | POST ...
GET  /api/v1/servers/:id/members
GET  /api/v1/channels/:id/messages?limit&before | POST ...
PATCH/DELETE /api/v1/messages/:id
POST /api/v1/messages/:id/reactions/:emoji   (toggle)
GET  /api/v1/search?query&channel_id&author
GET  /api/v1/notifications
GET  /api/v1/ws?token=...   (WS JSON: hello/message_created/edited/deleted/…)
GET  /health
```

## Security notes

- Passwords are Argon2-hashed; tokens are opaque random `tm_` UUIDs.
- Token file is `0600`, state dir `0700`.
- No Discord token handling, no keychain, no browser-cookie store.
- Put the server behind TLS (Caddy/Nginx) for internet exposure; compose binds plain HTTP for LAN.

## License

GPL-3.0-only (same as Concord, whose TUI design informed this client).
