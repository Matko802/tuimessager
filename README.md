# tuimessager

Self-hosted Discord-free messenger with a Concord-style TUI, written in Rust.

TUI design informed by [concord](https://github.com/chojs23/concord), reimplemented against a self-hosted protocol. No Discord code included.

## Features

- 4-pane TUI (Servers / Channels / Messages / Members) with vim keys and `Space` leader
- Markdown + code highlighting, reactions + emoji picker, threads, search, inbox, fuzzy channel switcher
- Axum + SQLite server with REST + live WebSocket events, Argon2 passwords, opaque tokens
- Free public HTTPS via Caddy + Let's Encrypt, Podman-ready (PC x86_64 + Pi 5 aarch64)

## Building

```sh
git clone https://github.com/Matko802/tuimessager.git
cd tuimessager
cargo build --release
```

Binaries land in `target/release/` (`tuimessager`, `tuimessager-server`).

## Server on Raspberry Pi 5

```sh
# on the Pi:
sudo apt install -y podman podman-compose
echo 'net.ipv4.ip_unprivileged_port_start=80' | sudo tee /etc/sysctl.d/90-rootless-ports.conf
sudo sysctl --system
git clone https://github.com/Matko802/tuimessager.git ~/tuimessager
cd ~/tuimessager && ./podman-build.sh
```

One container holds server + Caddy. LAN only:

```sh
podman-compose up -d
curl http://127.0.0.1:3000/health
```

Public with free TLS (forward router ports 443, and 80 unless HTTPS-only, to the Pi; point your DuckDNS domain at your IP):

```sh
TUIMESSAGER_DOMAIN=tuimessager.duckdns.org podman-compose up -d
curl https://tuimessager.duckdns.org/health
```

Set `TUIMESSAGER_ALLOW_REGISTRATION: "false"` in `compose.yaml` after creating your account.

## Usage

```sh
TUIMESSAGER_URL=https://tuimessager.duckdns.org tuimessager
```

First launch registers your account. Config lives in `~/.config/tuimessager/config.toml` (`--init-config`, `--check-config`).

| Key | Action |
|---|---|
| `1-4`, `Tab` | focus pane, cycle |
| `j` / `k`, `g` / `G` | move, top / bottom |
| `i`, `Enter`, `Esc` | compose, send, close |
| `/`, `:`, `Space Space` | search, emoji, channel switcher |
| `y`, `r`, `R`, `e`, `d`, `o` | copy, react, reply, edit, delete, open URL |
| `Space a` / `n` / `p` / `l` | actions, inbox, profile, log out |
| `q` | quit |

Env: `TUIMESSAGER_URL`, `TUIMESSAGER_TOKEN`, server-side `TUIMESSAGER_BIND`, `TUIMESSAGER_DATA_DIR`, `TUIMESSAGER_ALLOW_REGISTRATION`.

## Any distro with Nix

```sh
nix run github:Matko802/tuimessager
```

### As flake input

```nix
{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    tuimessager = {
      url = "github:Matko802/tuimessager";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { nixpkgs, tuimessager, ... }: {
    packages.x86_64-linux.default = tuimessager.packages.x86_64-linux.default;
  };
}
```

### As overlay

```nix
{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    tuimessager = {
      url = "github:Matko802/tuimessager";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { nixpkgs, tuimessager, ... }:
    let
      system = "x86_64-linux";
    in
    {
      nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
        inherit system;
        modules = [
          tuimessager.nixosModules.default
          {
            nixpkgs.overlays = [ tuimessager.overlays.default ];
            services.tuimessager = {
              enable = true;
              domain = "tuimessager.duckdns.org"; # free Caddy + Let's Encrypt TLS
              acmeEmail = "you@example.com";
              allowRegistration = false;
            };
          }
        ];
      };
    };
}
```

## License

GPL-3.0-only. See [LICENSE](./LICENSE).
