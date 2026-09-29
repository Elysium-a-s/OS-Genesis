#!/bin/sh
set -eu

if [ -z "${SUPERVISOR_TOKEN:-}" ]; then
  echo "Genesis: Supervisor token is unavailable" >&2
  exit 1
fi

if [ ! -r /data/options.json ]; then
  echo "Genesis: app options are unavailable" >&2
  exit 1
fi

read_token="$(jq -er '.read_token | select(type == "string" and length >= 32)' /data/options.json)" || {
  echo "Genesis: configure a read token of at least 32 characters" >&2
  exit 1
}

write_token="$(jq -er '.write_token | select(type == "string" and length >= 32)' /data/options.json)" || {
  echo "Genesis: configure a write token of at least 32 characters" >&2
  exit 1
}

member_token="$(jq -r '.member_token // empty' /data/options.json)"
guest_token="$(jq -r '.guest_token // empty' /data/options.json)"
if [ -n "$member_token" ] && [ "${#member_token}" -lt 32 ]; then
  echo "Genesis: member token must have at least 32 characters" >&2
  exit 1
fi
if [ -n "$guest_token" ] && [ "${#guest_token}" -lt 32 ]; then
  echo "Genesis: guest token must have at least 32 characters" >&2
  exit 1
fi

export GENESIS_HOUSEHOLD_ID="pilot-home"
export GENESIS_WRITE_TOKEN="$write_token"
export GENESIS_MEMBER_TOKEN="$member_token"
export GENESIS_GUEST_TOKEN="$guest_token"
export GENESIS_BIND_ADDR="0.0.0.0:8080"
export GENESIS_LEDGER_PATH="/data/genesis-ledger.sqlite3"
export GENESIS_HA_WS_URL="ws://supervisor/core/websocket"
export GENESIS_HA_TOKEN="$SUPERVISOR_TOKEN"
export GENESIS_READ_TOKEN="$read_token"
unset SUPERVISOR_TOKEN read_token write_token member_token guest_token

exec /usr/local/bin/genesis-core
