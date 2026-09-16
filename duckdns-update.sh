#!/usr/bin/env bash
# DuckDNS dynamic-IP updater (free) for tuimessager.
# Keeps e.g. tuimessager.duckdns.org pointed at your home public IP.
#
# Setup:
#   1. Log in at https://www.duckdns.org and copy your account token.
#   2. Run once by hand to verify:
#        TOKEN=your-token-here ./duckdns-update.sh tuidns
#   3. Cron (runs every 5 minutes):
#        crontab -e
#        */5 * * * * TOKEN=your-token-here /home/pi/tuimessager/duckdns-update.sh tuidns >> /home/pi/duckdns.log 2>&1
set -euo pipefail
TOKEN="${TOKEN:?set TOKEN to your duckdns.org account token}"
DOMAINS="${1:-${DUCKDNS_DOMAINS:-tuidns}}"
OUT="$(curl -s "https://www.duckdns.org/update?domains=${DOMAINS}&token=${TOKEN}&ip=")"
if [ "$OUT" = "OK" ]; then
  echo "$(date -Is) duckdns ${DOMAINS}: OK"
else
  echo "$(date -Is) duckdns ${DOMAINS}: FAILED (${OUT})" >&2
  exit 1
fi
