#!/bin/bash
# ============================================================================
# FARM STATUS + REAPER — run on the coordinator any time.
#   ./farm_status.sh          show counts + what each worker is rendering
#   ./farm_status.sh --reap   requeue jobs whose worker died (stuck > $STALE_MIN)
# ============================================================================
set -uo pipefail
FARM_ROOT="${FARM_ROOT:-/Volumes/RenderFarm}"
STALE_MIN="${STALE_MIN:-45}"     # a running job older than this = presumed dead
Q="$FARM_ROOT/queue"; R="$FARM_ROOT/running"; D="$FARM_ROOT/done"; F="$FARM_ROOT/failed"

c(){ ls -1 "$1"/*.job* 2>/dev/null | wc -l | tr -d ' '; }
echo "── LTX render farm @ $FARM_ROOT ──"
echo "  queued : $(c "$Q")"
echo "  running: $(c "$R")"
echo "  done   : $(c "$D")"
echo "  failed : $(c "$F")"

if [ "$(c "$R")" -gt 0 ]; then
  echo "  in flight:"
  for j in "$R"/*.job*; do
    [ -e "$j" ] || continue
    host="$(basename "$j" | sed -E 's/.*\.job\.([^.]+)\..*/\1/')"
    echo "    - $(basename "$j" | sed -E 's/\.job\..*//')  on  $host"
  done
fi

if [ "${1:-}" = "--reap" ]; then
  echo "── reaping stale running jobs (> ${STALE_MIN}min) ──"
  n=0
  while IFS= read -r j; do
    [ -e "$j" ] || continue
    # strip the .<host>.<pid> suffix back to a plain queue name
    orig="$(basename "$j" | sed -E 's/(\.job)\..*/\1/')"
    mv "$j" "$Q/REQUEUED_${orig}"
    echo "  requeued: $orig"
    n=$((n+1))
  done < <(find "$R" -name '*.job.*' -mmin +"$STALE_MIN" 2>/dev/null)
  echo "  reaped $n."
fi
