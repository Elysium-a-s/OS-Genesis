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

sensitive_devices="$(jq -r '.sensitive_devices // empty' /data/options.json)"
behavior_key_id="$(jq -r '.behavior_key_id // empty' /data/options.json)"
behavior_secret="$(jq -r '.behavior_secret // empty' /data/options.json)"
central_unit_id="$(jq -r '.central_unit_id // empty' /data/options.json)"

# Polovične nastavený kanál je chyba, nie vypnutý kanál: niekto ho zapnúť chcel.
# Core to pri štarte tiež odmietne, ale povedať to menom možnosti v app je
# zrozumiteľnejšie než menom premennej, ktorú prevádzkovateľ nikdy nevidel.
if [ -n "$behavior_key_id" ] && [ -z "$behavior_secret" ]; then
  echo "Genesis: behavior_key_id is set without behavior_secret" >&2
  exit 1
fi
if [ -z "$behavior_key_id" ] && [ -n "$behavior_secret" ]; then
  echo "Genesis: behavior_secret is set without behavior_key_id" >&2
  exit 1
fi
if [ -n "$behavior_secret" ] && [ "${#behavior_secret}" -lt 32 ]; then
  echo "Genesis: behavior secret must have at least 32 characters" >&2
  exit 1
fi

export GENESIS_HOUSEHOLD_ID="pilot-home"
export GENESIS_WRITE_TOKEN="$write_token"
export GENESIS_MEMBER_TOKEN="$member_token"
export GENESIS_GUEST_TOKEN="$guest_token"
export GENESIS_BIND_ADDR="0.0.0.0:8080"
export GENESIS_INGRESS_ONLY="${GENESIS_INGRESS_ONLY:-true}"
export GENESIS_LEDGER_PATH="/data/genesis-ledger.sqlite3"
export GENESIS_PANEL_DIR="/usr/share/genesis-panel"
export GENESIS_HA_WS_URL="ws://supervisor/core/websocket"
export GENESIS_HA_TOKEN="$SUPERVISOR_TOKEN"
export GENESIS_READ_TOKEN="$read_token"
# Záloha je `VACUUM INTO` do perzistentného `/data`, takže prežije reštart aj
# aktualizáciu app. Bez tejto premennej `POST /v1/backup` odpovedá 503 — kód bol
# v obraze od 0.1.3, ale zapnúť sa nedal.
export GENESIS_BACKUP_DIR="/data/backups"
export GENESIS_SENSITIVE_DEVICES="$sensitive_devices"

# Nenastavené možnosti sa **neexportujú ako prázdne**. Core číta tieto premenné
# cez `env::var(...).ok()`, takže prázdny reťazec preň nie je „nenastavené", ale
# nastavená prázdna hodnota: prázdny `GENESIS_BEHAVIOR_KEY_ID` by zhodil štart a
# prázdny `GENESIS_CENTRAL_UNIT_ID` by odmietol každé rozhodnutie Behavior.
if [ -n "$behavior_key_id" ]; then
  export GENESIS_BEHAVIOR_KEY_ID="$behavior_key_id"
  export GENESIS_BEHAVIOR_SECRET="$behavior_secret"
fi
if [ -n "$central_unit_id" ]; then
  export GENESIS_CENTRAL_UNIT_ID="$central_unit_id"
fi
unset SUPERVISOR_TOKEN read_token write_token member_token guest_token \
  sensitive_devices behavior_key_id behavior_secret central_unit_id

exec /usr/local/bin/genesis-core
