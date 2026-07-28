#!/bin/bash
# ============================================================================
# MEASURE THE REAL MEMORY CURVE — run this on the 64GB Mac.
# ----------------------------------------------------------------------------
# Closes the open item in docs/MEMORY-INCIDENT-2026-07-28.md §6.
#
# WHY THIS MATTERS RIGHT NOW: the still coefficient is measured (896x1216
# --low-ram = 27.5GB), but the VIDEO one is extrapolated. On that extrapolation
# a hero 1080x1920 t2v prices at ~49GB, which puts it out of reach of all three
# 32GB Macs and reserves it for the single 64GB box. If the real number is
# lower, that guess is costing you three quarters of the farm.
#
# This script renders progressively larger jobs on THIS Mac with the farm's own
# instrumentation attached, reads back the true peak, and prints the
# coefficients to paste into farm.conf. It never guesses.
#
#   ./measure_peak.sh              # video curve (the open item)
#   ./measure_peak.sh --stills     # re-verify the still curve
#   ./measure_peak.sh --quick      # two points instead of four
#
# Runs jobs SERIALLY and honours the one-GPU-job rule. Expect ~10 min per point
# for stills and considerably longer for video — start it and walk away.
# ============================================================================
set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="${ROOT_DIR:-/Users/aidenwood/Desktop/00 - Aidxn/Social Video Creation}"
LTX_DIR="${LTX_DIR:-$ROOT_DIR/LTX2-MLX}"
LORA_DIR="${LORA_DIR:-$HOME/farm-loras}"
MODEL="${MODEL:-dgrauet/ltx-2.3-mlx-q4}"
OUT="${OUT:-${TMPDIR:-/tmp}/farm_peak_probe}"
MODE="video"; QUICK=0

while [ $# -gt 0 ]; do case "$1" in
  --stills) MODE="stills"; shift;;
  --video)  MODE="video";  shift;;
  --quick)  QUICK=1; shift;;
  *) echo "unknown arg: $1"; exit 1;;
esac; done

# shellcheck disable=SC1091
. "$HERE/farm_mem.sh"
mkdir -p "$OUT"

RAM="$(mem_total_gb)"
echo "── measuring on a ${RAM}GB Mac (budget ${MEM_BUDGET_PCT}% = $(mem_budget_gb)GB) ──"
if [ "$RAM" -lt 48 ]; then
  echo "⚠️  This is a ${RAM}GB Mac. The larger probes will OOM here and skew the curve."
  echo "   Run it on the 64GB machine. Continuing anyway in 5s — ^C to stop."
  sleep 5
fi

SITE="$HERE/farm_sitecustomize"
# Deliberately NO memory cap during measurement — we want the honest peak, not
# a number shaped by the ceiling we're trying to calibrate.
probe_env=( "PYTHONPATH=${SITE}${PYTHONPATH:+:$PYTHONPATH}" "FARM_MLX_VERBOSE=1" )

# peak_of <logfile> -> GB, from the atexit hook in farm_sitecustomize
peak_of(){ awk '/\[farm\] MLX peak:/ {v=$(NF-1)} END{print (v==""?"":v)}' "$1"; }
mp_of(){ awk -v w="$1" -v h="$2" 'BEGIN{printf "%.3f", (w*h)/1000000}'; }

RESULTS=""

run_still(){
  local w="$1" h="$2" log="$OUT/still_${w}x${h}.log"
  local largs=()
  if [ -f "$LORA_DIR/Elijah_lora.safetensors" ]; then
    largs=(--lora-paths "$LORA_DIR/Elijah_lora.safetensors" --lora-scales 0.9)
  fi
  echo "  still ${w}x${h} ($(mp_of "$w" "$h") MP) ..."
  HF_HUB_OFFLINE=1 env "${probe_env[@]}" caffeinate -ims nice \
    mflux-generate-z-image-turbo -q 4 --low-ram ${largs[@]+"${largs[@]}"} \
    --steps 9 --guidance 1.0 --width "$w" --height "$h" --seed 9301 \
    --prompt "eljhwd man, photorealistic test" \
    --output "$OUT/still_${w}x${h}.png" >"$log" 2>&1
  local rc=$? peak; peak="$(peak_of "$log")"
  if mem_is_oom "$rc" "$log"; then echo "    ❌ OOM — this Mac cannot measure this point"; return; fi
  [ -z "$peak" ] && { echo "    ! no peak reported (rc=$rc), see $log"; return; }
  echo "    peak ${peak} GB"
  RESULTS="$RESULTS still ${w} ${h} 1 ${peak}"$'\n'
}

run_video(){
  local w="$1" h="$2" f="$3" log="$OUT/video_${w}x${h}_f${f}.log"
  echo "  video ${w}x${h} f=${f} ($(mp_of "$w" "$h") MP) ..."
  ( cd "$LTX_DIR" && HF_HUB_OFFLINE=1 HF_HUB_ENABLE_HF_TRANSFER=0 \
    env "${probe_env[@]}" caffeinate -ims nice \
    uv run ltx-2-mlx generate --model "$MODEL" --distilled --low-ram \
      -W "$w" -H "$h" -f "$f" --frame-rate 24 --seed 9301 \
      --prompt "a slow push in on a suburban rooftop, overcast" \
      -o "$OUT/video_${w}x${h}_f${f}.mp4" ) >"$log" 2>&1
  local rc=$? peak; peak="$(peak_of "$log")"
  if mem_is_oom "$rc" "$log"; then echo "    ❌ OOM — this Mac cannot measure this point"; return; fi
  [ -z "$peak" ] && { echo "    ! no peak reported (rc=$rc), see $log"; return; }
  echo "    peak ${peak} GB"
  RESULTS="$RESULTS video ${w} ${h} ${f} ${peak}"$'\n'
}

if [ "$MODE" = "stills" ]; then
  echo "── still curve ──"
  run_still 512 704
  [ "$QUICK" = "1" ] || run_still 640 896
  [ "$QUICK" = "1" ] || run_still 768 1024
  run_still 896 1216          # the known point: expect ~27.5GB
else
  echo "── video curve (the open item) ──"
  run_video 768 1280 65
  [ "$QUICK" = "1" ] || run_video 768 1280 97
  [ "$QUICK" = "1" ] || run_video 1080 1920 65
  run_video 1080 1920 97      # the hero job currently priced at ~49GB
fi

# --- fit -------------------------------------------------------------------
# peak = MP * PER_MP + BASE, video additionally scaled by frames/97. Least
# squares over whatever points survived; a single point degenerates to a
# through-the-origin fit, which is still better than the current extrapolation.
echo
echo "── results ──"
[ -z "$RESULTS" ] && { echo "  no usable measurements — check the logs in $OUT"; exit 1; }
printf '%s' "$RESULTS" | awk '
  { kind=$1; w=$2; h=$3; f=$4; p=$5
    x = (w*h)/1000000; if (kind=="video") x = x * (f/97)
    printf "  %-6s %5dx%-5d f=%-4d %6.3f MP-eq  ->  %6.2f GB\n", kind, w, h, f, x, p
    n++; sx+=x; sy+=p; sxx+=x*x; sxy+=x*p; K=kind }
  END {
    if (n<2) { printf "\n  only %d point(s) — assuming base 3GB\n", n
               printf "  %s_GB_PER_MP = %d\n", toupper(K), int((sy-3)/sx + 0.5); exit }
    d = n*sxx - sx*sx
    if (d==0) { print "\n  degenerate fit"; exit }
    m = (n*sxy - sx*sy)/d; b = (sy - m*sx)/n
    printf "\n  fit: peak = %.1f * MP-eq + %.1f\n", m, b
    printf "\n  paste into farm.conf:\n"
    printf "    : \"${%s_GB_PER_MP:=%d}\"\n", toupper(K), int(m+0.5)
    printf "    : \"${%s_GB_BASE:=%d}\"\n",   toupper(K), (b<0?0:int(b+0.5))
  }'
echo
echo "  logs: $OUT"
echo "  then: edit \$FARM_ROOT/farm.conf on the coordinator — every Mac picks it up"
echo "        within one poll, and re-prices its queue automatically."
