#!/usr/bin/env bash
# Build the combined tuimessager image (server + Caddy).
# Usage:
#   ./podman-build.sh            # native arch (PC x86_64 or RPi5 aarch64)
#   ./podman-build.sh arm64      # cross for Raspberry Pi 5 from PC
#   ./podman-build.sh amd64      # cross for PC from Pi/other
set -euo pipefail
ARCH="${1:-native}"
case "$ARCH" in
  arm64|aarch64) PLATFORM="linux/arm64" ;;
  amd64|x86_64)  PLATFORM="linux/amd64" ;;
  native)        PLATFORM="" ;;
  *) echo "unknown arch: $ARCH (want arm64|amd64|native)"; exit 1 ;;
esac
if [ -n "$PLATFORM" ]; then
  podman build --platform "$PLATFORM" -f Containerfile.server -t tuimessager:latest .
else
  podman build -f Containerfile.server -t tuimessager:latest .
fi
echo "built tuimessager:latest ($ARCH)"
