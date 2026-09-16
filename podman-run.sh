#!/usr/bin/env bash
# Quick run without compose (data in ./data).
set -euo pipefail
mkdir -p data
podman run -d --replace --name tuimessager-server \
  -p 3000:3000 \
  -v ./data:/data:U \
  -e TUIMESSAGER_DATA_DIR=/data \
  -e TUIMESSAGER_ALLOW_REGISTRATION=true \
  localhost/tuimessager-server:latest
podman logs -f tuimessager-server
