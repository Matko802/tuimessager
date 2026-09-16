#!/usr/bin/env bash
# Quick run without compose (data in ./data).
# Set TUIMESSAGER_DOMAIN to enable Caddy + free TLS (needs ports 80/443 free).
set -euo pipefail
mkdir -p data
podman run -d --replace --name tuimessager \
  -p 3000:3000 -p 80:80 -p 443:443 -p 443:443/udp \
  -v ./data:/data:U \
  -e TUIMESSAGER_DATA_DIR=/data \
  -e TUIMESSAGER_ALLOW_REGISTRATION=true \
  -e TUIMESSAGER_DOMAIN="${TUIMESSAGER_DOMAIN:-}" \
  localhost/tuimessager:latest
podman logs -f tuimessager
