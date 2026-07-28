#!/bin/bash
# ============================================================================
# FARM STATUS + REAPER — run on the coordinator any time.
#   ./farm_status.sh          counts, what each worker is rendering, memory
#   ./farm_status.sh --reap   requeue jobs whose worker died (stuck > $STALE_MIN)
# ============================================================================
set -uo pipefail
FARM_ROOT="${FARM_ROOT:-/Volumes/RenderFarm}"
STALE_MIN="${STALE_MIN:-45}"     # a running job older than this = presumed dead
Q="$FARM_ROOT/queue"; HQ="$FARM_ROOT/queue/hi"; R="$FARM_ROOT/running"; D="$FARM_ROOT/done"; F="$FARM_ROOT/failed"

# count .job* files, never counting worker heartbeat files as jobs
c(){ ls -1 "$1"/*.job* 2>/dev/null | grep -v '\.heartbeat$' | wc -l | tr -d ' '; }
echo "── LTX render farm @ $FARM_ROOT ──"
echo "  queued : $(( $(c "$Q") + $(c "$HQ") ))"
echo "  hi-queue: $(c "$HQ")"
echo "  running: $(c "$R")"
echo "  done   : $(c "$D")"
echo "  failed : $(c "$F")"
echo "  oom-retried: $(ls -1 "$Q"/OOMRETRY_* 2>/dev/null | wc -l | tr -d ' ')"

# --- per-worker memory, straight off the share -----------------------------
# Each worker rewrites running/.worker.<host>.info every poll. Reading those is
# how you spot the Mac that's about to OOM without SSHing into four machines.
if ls "$R"/.worker.*.info >/dev/null 2>&1; then
  echo "  workers:"
  printf "    %-16s %6s %-14s %-6s %7s %6s %5s %s\n" HOST RAM TIER PERF BUDGET FREE PRESS STATE
  for i in "$R"/.worker.*.info; do
    [ -e "$i" ] || continue
    HOST=""; RAM_GB=""; TIER=""; PERF=""; BUDGET_GB=""; FREE_PCT=""; PRESSURE=""; SWAP_MB=""; STATE=""; UPDATED=""
    # shellcheck disable=SC1090
    . "$i" 2>/dev/null || continue
    flag=""
    [ -n "$FREE_PCT" ] && [ "$FREE_PCT" -lt 20 ] 2>/dev/null && flag="  ⚠ low RAM"
    [ -n "$PRESSURE" ] && [ "$PRESSURE" -ge 4 ] 2>/dev/null && flag="  ⚠ CRITICAL pressure"
    printf "    %-16s %5sG %-14s %-6s %6sG %5s%% %5s %s%s\n" \
      "$HOST" "$RAM_GB" "$TIER" "$PERF" "$BUDGET_GB" "$FREE_PCT" "$PRESSURE" "$STATE" "$flag"
  done
fi

if [ "$(c "$R")" -gt 0 ]; then
  echo "  in flight:"
  for j in "$R"/*.job*; do
    [ -e "$j" ] || continue
    case "$j" in *.heartbeat) continue;; esac
    host="$(basename "$j" | sed -E 's/.*\.job\.([^.]+)\..*/\1/')"
    echo "    - $(basename "$j" | sed -E 's/\.job\..*//')  on  $host"
  done
fi

if [ "${1:-}" = "--reap" ]; then
  echo "── reaping stale running jobs (> ${STALE_MIN}min) ──"
  n=0
  # walk every running jobfile (never the .heartbeat files themselves)
  for j in "$R"/*.job.*; do
    [ -e "$j" ] || continue
    case "$j" in *.heartbeat) continue;; esac
    hb="${j}.heartbeat"
    stale=0
    if [ -e "$hb" ]; then
      # heartbeat-aware: dead only if the worker stopped touching it > $STALE_MIN
      if [ -n "$(find "$hb" -mmin +"$STALE_MIN" 2>/dev/null)" ]; then stale=1; fi
    else
      # no heartbeat (older worker / never started): fall back to jobfile mtime
      if [ -n "$(find "$j" -mmin +"$STALE_MIN" 2>/dev/null)" ]; then stale=1; fi
    fi
    [ "$stale" -eq 1 ] || continue
    # strip the .<host>.<pid> suffix back to a plain queue name
    orig="$(basename "$j" | sed -E 's/(\.job)\..*/\1/')"
    mv "$j" "$Q/REQUEUED_${orig}"
    rm -f "$hb"
    echo "  requeued: $orig"
    n=$((n+1))
  done
  echo "  reaped $n."
fi
