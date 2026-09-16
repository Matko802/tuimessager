#!/bin/bash
# Single-container supervisor for tuimessager.
# Always runs tuimessager-server; also runs Caddy (free Let's Encrypt TLS)
# when TUIMESSAGER_DOMAIN is set. Runs under tini (see Containerfile).
set -u

TUIMESSAGER_BIND="${TUIMESSAGER_BIND:-0.0.0.0:3000}"
TUIMESSAGER_DATA_DIR="${TUIMESSAGER_DATA_DIR:-/data}"
export TUIMESSAGER_BIND TUIMESSAGER_DATA_DIR

/usr/local/bin/tuimessager-server &
SERVER_PID=$!

if [ -n "${TUIMESSAGER_DOMAIN:-}" ]; then
  export TUIMESSAGER_DOMAIN
  /usr/local/bin/caddy run --config /etc/caddy/Caddyfile --adapter caddyfile &
  CADDY_PID=$!
else
  CADDY_PID=""
  echo "run.sh: TUIMESSAGER_DOMAIN unset — LAN mode, server only on ${TUIMESSAGER_BIND}"
fi

shutdown() {
  kill -TERM "$SERVER_PID" 2>/dev/null
  [ -n "$CADDY_PID" ] && kill -TERM "$CADDY_PID" 2>/dev/null
}
trap shutdown TERM INT

# Exit when either process dies so Podman restarts a broken half.
if [ -n "$CADDY_PID" ]; then
  wait -n "$SERVER_PID" "$CADDY_PID"
  EXIT=$?
else
  wait "$SERVER_PID"
  EXIT=$?
fi
shutdown
exit "$EXIT"
