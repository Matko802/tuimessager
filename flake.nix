{
  description = "tuimessager — self-hosted Concord-style TUI messenger (no Discord API). Server runs on Podman (PC x86_64 + RPi5 aarch64).";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane.url = "github:ipetkov/crane";
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay, crane }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs { inherit system overlays; };

        rustToolchain = pkgs.rust-bin.stable."1.90.0".default;
        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;

        src = craneLib.cleanCargoSource ./.;

        mkPkg = pname: binary: desc: pkgs.callPackage ./nix/package.nix {
          inherit craneLib src pname;
          version = "0.1.0";
          description = desc;
        };

        tuimessager-server = mkPkg "tuimessager-server" "tuimessager-server"
          "Self-hosted tuimessager server (Podman-ready, x86_64 PC + aarch64 RPi5)";
        tuimessager-tui = mkPkg "tuimessager-tui" "tuimessager"
          "Concord-style TUI client for tuimessager (no Discord API)";

        # OCI image for `podman load` / `skopeo` — same derivation builds on
        # x86_64-linux (PC) and aarch64-linux (RPi5, e.g. remote builder).
        server-image = pkgs.dockerTools.buildLayeredImage {
          name = "tuimessager-server";
          tag = "latest";
          contents = [ tuimessager-server pkgs.sqlite pkgs.cacert ];
          config = {
            Entrypoint = [ "${tuimessager-server}/bin/tuimessager-server" ];
            ExposedPorts = { "3000/tcp" = { }; };
            Env = [
              "TUIMESSAGER_DATA_DIR=/data"
              "TUIMESSAGER_BIND=0.0.0.0:3000"
              "SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
            ];
            Volumes = { "/data" = { }; };
          };
        };
      in
      {
        packages = {
          default = tuimessager-tui;
          inherit tuimessager-server tuimessager-tui server-image;
        };

        apps = {
          default = flake-utils.lib.mkApp { drv = tuimessager-tui; name = "tuimessager"; };
          server = flake-utils.lib.mkApp { drv = tuimessager-server; name = "tuimessager-server"; };
          tui = flake-utils.lib.mkApp { drv = tuimessager-tui; name = "tuimessager"; };
        };

        devShells.default = craneLib.devShell {
          inputsFrom = [ tuimessager-server tuimessager-tui ];
          packages = [
            rustToolchain
            pkgs.rust-analyzer
            pkgs.cargo-edit
            pkgs.pkg-config
            pkgs.sqlite
            pkgs.podman
            pkgs.podman-compose
          ];
        };

        formatter = pkgs.nixpkgs-fmt;
      }) // {
        # Overlay for system flakes (fish-flake pattern, like artixy/sharkvis):
        #   tuimessager.inputs.nixpkgs.follows = "nixpkgs";
        #   nixpkgs.overlays = [ tuimessager.overlays.default ];
        # then `tuimessager` (TUI) and `tuimessager-server` are in pkgs.
        overlays.default = final: _prev: {
          tuimessager = self.packages.${final.stdenv.hostPlatform.system}.tuimessager-tui;
          tuimessager-server = self.packages.${final.stdenv.hostPlatform.system}.tuimessager-server;
        };

        # NixOS module for the server (PC or Pi running NixOS).
        nixosModules.default = { config, lib, pkgs, ... }:
          let cfg = config.services.tuimessager;
          in {
            options.services.tuimessager = {
              enable = lib.mkEnableOption "tuimessager self-hosted server";
              package = lib.mkOption {
                type = lib.types.package;
                default = self.packages.${pkgs.stdenv.hostPlatform.system}.tuimessager-server;
                description = "Server package to run.";
              };
              bind = lib.mkOption {
                type = lib.types.str;
                default = "127.0.0.1:3000";
                description = "Bind address (ip:port).";
              };
              dataDir = lib.mkOption {
                type = lib.types.str;
                default = "/var/lib/tuimessager";
                description = "SQLite data directory.";
              };
              allowRegistration = lib.mkOption {
                type = lib.types.bool;
                default = true;
                description = "Allow open registration (set false after first user).";
              };
              domain = lib.mkOption {
                type = lib.types.nullOr lib.types.str;
                default = null;
                example = "chat.example.com";
                description = ''
                  Public domain to expose via Caddy with a free automatic
                  Let's Encrypt certificate ($0, auto-renewed). Null = no
                  reverse proxy (LAN/Tailscale only).
                '';
              };
              acmeEmail = lib.mkOption {
                type = lib.types.str;
                default = "";
                example = "you@example.com";
                description = "Contact email for Let's Encrypt (recommended when domain is set).";
              };
            };
            config = lib.mkIf cfg.enable {
              systemd.services.tuimessager = {
                description = "tuimessager server";
                wantedBy = [ "multi-user.target" ];
                after = [ "network-online.target" ];
                environment = {
                  TUIMESSAGER_BIND = cfg.bind;
                  TUIMESSAGER_DATA_DIR = cfg.dataDir;
                  TUIMESSAGER_ALLOW_REGISTRATION = if cfg.allowRegistration then "true" else "false";
                };
                serviceConfig = {
                  ExecStart = "${cfg.package}/bin/tuimessager-server";
                  StateDirectory = "tuimessager";
                  DynamicUser = true;
                  Restart = "always";
                };
              };

              # Free public HTTPS via Caddy + Let's Encrypt when a domain is set.
              services.caddy = lib.mkIf (cfg.domain != null) {
                enable = true;
                virtualHosts.${cfg.domain}.extraConfig = ''
                  reverse_proxy ${cfg.bind}
                '';
              };
              security.acme = lib.mkIf (cfg.domain != null) {
                acceptTerms = true;
                defaults.email = lib.mkIf (cfg.acmeEmail != "") cfg.acmeEmail;
              };
              networking.firewall.allowedTCPPorts = lib.mkIf (cfg.domain != null) [ 80 443 ];
            };
          };
      };
}
