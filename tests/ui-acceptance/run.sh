#!/bin/sh
# UI akceptačný test (ELYSIUM-362).
#
# Postaví panel aj jednotku, spustí ich proti sebe a prejde panelom v skutočnom
# prehliadači. Jednotka je preložený binárny súbor, nie mock; komunikuje po
# reálnom sockete so `stub_ha.py`, ktorý hovorí protokolom Home Assistanta.
#
# Čo tento test NEoveruje: fyzickú žiarovku, fyzický iPad a skutočný Home
# Assistant Green. To je ELYSIUM-350 a nedá sa to nahradiť.
set -eu

root="$(cd "$(dirname "$0")/../.." && pwd)"
work="${GENESIS_TEST_WORK:-${TMPDIR:-/tmp}/genesis-ui-acceptance}"
flutter="${FLUTTER_BIN:-flutter}"
chromium="${CHROMIUM_BIN:-}"

# Zvyšky z predošlého behu treba zavrieť skôr, než sa adresár zmaže. Inak beží
# stará jednotka s ledgerom, ktorý už na disku nie je, drží port a odpovedá 500 —
# a vyzerá to ako chyba produktu.
for stale in "$work/core.pid" "$work/stub.pid"; do
  [ -f "$stale" ] && kill "$(cat "$stale")" 2>/dev/null || true
done
sleep 1
rm -rf "$work"
mkdir -p "$work"
echo normal > "$work/ha-mode"

log() { echo "run.sh: $*" >&2; }

cleanup() {
  for pidfile in "$work"/core.pid "$work"/stub.pid; do
    [ -f "$pidfile" ] && kill "$(cat "$pidfile")" 2>/dev/null || true
  done
}
trap cleanup EXIT INT TERM

if [ "${GENESIS_SKIP_BUILD:-}" != "1" ]; then
  log "building the unit"
  (cd "$root/core" && cargo build --locked --release)
  log "building the panel"
  # --no-web-resources-cdn je podstatné, nie kozmetické: bez neho si panel ťahá
  # CanvasKit z gstatic.com a v domácnosti bez internetu nenakreslí nič.
  (cd "$root/panel" && "$flutter" build web --release --no-web-resources-cdn)
fi

start_stub() {
  setsid python3 "$root/tests/ui-acceptance/stub_ha.py" \
    --port 18123 --control "$work/ha-mode" --pidfile "$work/stub.pid" \
    > "$work/stub-ha.log" 2>&1 < /dev/null &
}

start_core() {
  "$(dirname "$0")/start-unit.sh" "$work"
}

await() {
  attempt=0
  while [ "$attempt" -lt 60 ]; do
    if curl -fsS -o /dev/null "$1"; then return 0; fi
    attempt=$((attempt + 1))
    sleep 0.5
  done
  log "$1 did not come up"
  return 1
}

log "starting the stub Home Assistant"
start_stub
log "starting the unit"
start_core
await http://127.0.0.1:18080/health

log "driving the panel"
GENESIS_TEST_WORK="$work" \
GENESIS_START_STUB="$root/tests/ui-acceptance/stub_ha.py" \
GENESIS_RESTART_CORE="$root/tests/ui-acceptance/start-unit.sh $work" \
NODE_PATH="${NODE_PATH:-}" \
CHROMIUM_BIN="$chromium" \
  node "$root/tests/ui-acceptance/acceptance.mjs"
