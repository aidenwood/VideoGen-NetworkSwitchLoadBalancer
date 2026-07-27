#!/bin/bash
# ============================================================================
# FARM STATUS + REAPER — run on the coordinator any time.
#   ./farm_status.sh          show counts + what each worker is rendering
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
