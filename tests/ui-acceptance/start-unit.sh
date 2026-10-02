#!/bin/sh
# Spustí jednotku tak, ako ju spúšťa HA app, s dvoma rozdielmi: panel ide z
# panel/build/web a ingress guard je vypnutý, pretože test nechodí cez Supervisor
# (CI robí to isté: GENESIS_INGRESS_ONLY=false).
#
# Tokeny a tajomstvo Behavior kanála sú testovacie hodnoty pre tento beh. Žijú
# len v prostredí tohto procesu a do logu ani do zálohy sa nedostanú.
#
# PID si zapisuje sám proces, ktorý sa binárkou nahradí. `setsid ... &` vracia
# PID setsidu, ktorý sa odforkuje, takže zvonka zapísané číslo po chvíli nepatrí
# nikomu — a test, ktorý ním jednotku „reštartuje", by nereštartoval nič.
set -eu
work="$1"
root="$(cd "$(dirname "$0")/../.." && pwd)"
mkdir -p "$work"

setsid env -i PATH=/usr/bin:/bin \
  GENESIS_PID_FILE="$work/core.pid" \
  GENESIS_BINARY="$root/core/target/release/genesis-core" \
  GENESIS_BIND_ADDR=127.0.0.1:18080 \
  GENESIS_INGRESS_ONLY=false \
  GENESIS_HOUSEHOLD_ID=pilot-home \
  GENESIS_READ_TOKEN=read-token-abcdefghijklmnopqrstuvw \
  GENESIS_WRITE_TOKEN=write-token-ABCDEFGHIJKLMNOPQRSTU \
  GENESIS_MEMBER_TOKEN=member-token-abcdefghijklmnopqrst \
  GENESIS_GUEST_TOKEN=guest-token-abcdefghijklmnopqrstu \
  GENESIS_LEDGER_PATH="$work/ledger.sqlite3" \
  GENESIS_BACKUP_DIR="$work/backups" \
  GENESIS_SENSITIVE_DEVICES=ha:switch.boiler \
  GENESIS_PANEL_DIR="$root/panel/build/web" \
  GENESIS_HA_WS_URL=ws://127.0.0.1:18123 \
  GENESIS_HA_TOKEN=stub-ha-token \
  GENESIS_BEHAVIOR_KEY_ID=pilot-behavior-key \
  GENESIS_BEHAVIOR_SECRET=pilot-behavior-secret-of-32-chars \
  GENESIS_CENTRAL_UNIT_ID=ad19a578-21e2-453f-a57c-1913350be34e \
  GENESIS_LOG=info \
  /bin/sh -c 'echo $$ > "$GENESIS_PID_FILE"; exec "$GENESIS_BINARY"' \
  >> "$work/core.log" 2>&1 < /dev/null &
