#!/bin/bash
# ============================================================================
# ADD JOBS TO THE FARM QUEUE (run on the coordinator, or any node).
# ----------------------------------------------------------------------------
#  # text -> video
#  ./enqueue.sh --id hail_hero --prompt "storm clouds over a QLD roof" --seed 8804
#
#  # image -> video  (still already in /Volumes/RenderFarm/assets/)
#  ./enqueue.sh --id milk --image elijah.png --prompt "he lifts the glass" --seed 8804
#
#  # Elijah LoRA -> still -> i2v  (needs provision.command run on workers)
#  ./enqueue.sh --id elijah_taste --lora Elijah_lora.safetensors \
#       --still-prompt "eljhwd man tasting hail, kitchen" \
#       --prompt "he sniffs then licks, cinematic" --seed 8804
#
#  # seed sweep — the farm splits all N across the Macs
#  ./enqueue.sh --id dragon --sweep 12 --prompt "black dragon on a rooftop at dusk"
#
#  # force the light profile on a job (default = the worker's own PERF)
#  ./enqueue.sh --id bg_clip --perf light --prompt "..."
#
#  # jump the queue — workers pick high-priority jobs first
#  ./enqueue.sh --id urgent --priority high --prompt "..."
# ============================================================================
set -euo pipefail
FARM_ROOT="${FARM_ROOT:-/Volumes/RenderFarm}"
QUEUE="$FARM_ROOT/queue"; mkdir -p "$QUEUE"

ID=""; TYPE="t2v"; PROMPT=""; IMAGE=""; WIDTH=1080; HEIGHT=1920; FRAMES=97; SEED=42; FPS=24; EXTRA=""; SWEEP=0
LORA=""; LORA_SCALE="1.0"; STILL_PROMPT=""; PERF=""; MODE=""; PRIORITY="normal"; MIN_RAM_GB=0
while [ $# -gt 0 ]; do case "$1" in
  --id)          ID="$2"; shift 2;;
  --type)        TYPE="$2"; shift 2;;
  --prompt)      PROMPT="$2"; shift 2;;
  --image)       IMAGE="$2"; TYPE="i2v"; shift 2;;
  --lora)        LORA="$2"; TYPE="lora_i2v"; shift 2;;
  --lora-scale)  LORA_SCALE="$2"; shift 2;;
  --still-prompt) STILL_PROMPT="$2"; shift 2;;
  --perf)        PERF="$2"; shift 2;;
  --mode)        MODE="$2"; shift 2;;
  --test)        MODE="test"; shift;;
  --hero)        MODE="hero"; shift;;
  -W)            WIDTH="$2"; shift 2;;
  -H)            HEIGHT="$2"; shift 2;;
  -f)            FRAMES="$2"; shift 2;;
  --seed)        SEED="$2"; shift 2;;
  --fps)         FPS="$2"; shift 2;;
  --extra)       EXTRA="$2"; shift 2;;
  --sweep)       SWEEP="$2"; shift 2;;
  --priority)    PRIORITY="$2"; shift 2;;
  --min-ram)     MIN_RAM_GB="$2"; shift 2;;   # only Macs with >= this many GB may claim it
  --hi)          PRIORITY="high"; shift;;
  *) echo "unknown arg: $1"; exit 1;;
esac; done
[ -n "$PROMPT" ] || { echo "!! --prompt is required"; exit 1; }

# --- ask test-vs-hero at prompt time (only if interactive + not preset) ----
if [ -z "$MODE" ]; then
  if [ -t 0 ]; then
    echo "How should the farm run \"${ID:-this}\"?"
    echo "  1) test  — quick z-image still(s) to cherry-pick first (cheap, seconds)"
    echo "  2) hero  — straight to full ${WIDTH}x${HEIGHT} video render"
    printf "choose [1/2] (default 2): "; read -r ans
    case "$ans" in 1|test|t) MODE="test";; *) MODE="hero";; esac
  else
    MODE="hero"   # non-interactive default
  fi
fi
echo "mode: $MODE"

write_job(){
  local id="$1" seed="$2"
  local stamp; stamp="$(date +%Y%m%d_%H%M%S)_$RANDOM"
  local dest="$QUEUE"
  if [ "$PRIORITY" = "high" ]; then dest="$QUEUE/hi"; mkdir -p "$dest"; fi
  local f="$dest/${stamp}__${id}.job"
  {
    echo "ID=\"$id\""
    echo "TYPE=\"$TYPE\""
    echo "PROMPT=\"$PROMPT\""
    echo "IMAGE=\"$IMAGE\""
    echo "LORA=\"$LORA\""
    echo "LORA_SCALE=$LORA_SCALE"
    echo "STILL_PROMPT=\"$STILL_PROMPT\""
    echo "WIDTH=$WIDTH"; echo "HEIGHT=$HEIGHT"; echo "FRAMES=$FRAMES"
    echo "SEED=$seed"; echo "FPS=$FPS"; echo "EXTRA=\"$EXTRA\""
    echo "MODE=\"$MODE\""
    [ -n "$PERF" ] && echo "PERF=\"$PERF\""
    [ "$MIN_RAM_GB" -gt 0 ] 2>/dev/null && echo "MIN_RAM_GB=$MIN_RAM_GB"
  } > "$f"
  echo "queued: $(basename "$f")  (seed=$seed mode=$MODE${PERF:+ perf=$PERF}$([ "$PRIORITY" = "high" ] && echo " [HIGH PRIORITY]"))"
}

if [ "$SWEEP" -gt 0 ]; then
  base="${ID:-sweep}"
  for i in $(seq 0 $((SWEEP-1))); do write_job "${base}_s$((1000+i))" "$((1000+i))"; done
  echo "-> $SWEEP jobs queued for the farm."
else
  write_job "${ID:-job}" "$SEED"
fi
