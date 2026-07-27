#!/bin/bash
# ============================================================================
# LTX RENDER FARM — worker node
# ----------------------------------------------------------------------------
# One of these runs on each Mac. All workers watch the SAME shared queue folder
# (an SMB share off the coordinator Mac, mounted at $FARM_ROOT on every node).
#
# Each worker loops:
#   1. Find the oldest *.job in  $FARM_ROOT/queue/
#   2. ATOMICALLY claim it  (mv queue/x.job -> running/x.job.<host>) — only ONE
#      worker wins the mv; the losers grab the next job. No collisions, no server.
#   3. Render it LOCALLY with this Mac's own GPU + RAM (one job at a time).
#   4. Drop the MP4 in  $FARM_ROOT/done/  and move the job to done/ (or failed/).
#   5. Repeat until the queue is empty, then poll every $POLL_SECS.
#
# This is JOB-level parallelism (a render farm), NOT splitting one clip across
# machines. 4 Macs = ~4x throughput. Only tiny job files + finished MP4s cross
# the network, so the gigabit switch (or even WiFi) is plenty.
#
# PERFORMANCE PROFILES (per-worker default via $PERF, per-job override via PERF=):
#   full  = use everything, fastest. NO --low-ram, no tiling, nice -n 5.
#           Use on a Mac DEDICATED to rendering. Peaks ~10-16GB.
#   light = capped so the Mac stays usable for other work. --low-ram + temporal
#           tiling + nice -n 15. Peak stays low single-digit GB (well under 16),
#           slower. Use on someone's daily-driver Mac.
#
# Job TYPES:
#   t2v       text -> video
#   i2v       image -> video          (IMAGE = file in assets/, or absolute)
#   lora_i2v  z-image still w/ a character LoRA -> LTX i2v   (Elijah etc.)
#
# Rules matched from overnight_run.sh (M4 Max): LTX-2.3 MLX q4, --distilled,
# frames = 8k+1, HF_HUB_OFFLINE=1, ONE GPU job at a time (this loop is serial),
# caffeinate -ims keeps the machine awake.
# ============================================================================
set -uo pipefail

# --- config (override via env / the .command launcher) --------------------
FARM_ROOT="${FARM_ROOT:-/Volumes/RenderFarm}"     # the mounted shared folder
ROOT_DIR="${ROOT_DIR:-/Users/aidenwood/Desktop/00 - Aidxn/Social Video Creation}"
LTX_DIR="${LTX_DIR:-$ROOT_DIR/LTX2-MLX}"
LORA_DIR="${LORA_DIR:-$HOME/farm-loras}"          # local LoRAs (provision.command fills this)
MODEL="${MODEL:-dgrauet/ltx-2.3-mlx-q4}"
PERF="${PERF:-full}"                               # per-worker default profile
POLL_SECS="${POLL_SECS:-15}"
MIN_FREE_GB="${MIN_FREE_GB:-15}"                   # pause rendering below this much free disk on $DONE volume
HOST="$(scutil --get LocalHostName 2>/dev/null || hostname -s)"

QUEUE="$FARM_ROOT/queue"; RUNNING="$FARM_ROOT/running"
DONE="$FARM_ROOT/done";   FAILED="$FARM_ROOT/failed"
ASSETS="$FARM_ROOT/assets"

# --- preflight ------------------------------------------------------------
[ -d "$FARM_ROOT" ] || { echo "!! FARM_ROOT not mounted: $FARM_ROOT — connect the share first."; exit 1; }
mkdir -p "$QUEUE" "$QUEUE/hi" "$RUNNING" "$DONE" "$FAILED" "$ASSETS"

log(){ echo "[$(date +%H:%M:%S)][$HOST] $*"; }
notify(){ osascript -e "display notification \"$1\" with title \"Render farm — $HOST\"" 2>/dev/null || true; }

log "worker online. profile=$PERF  model=$MODEL  queue=$QUEUE"
notify "worker online ($PERF)"

# --- per-worker concurrency lock: enforce ONE GPU job per Mac --------------
# Prefer the share (farm-wide visible), fall back to local tmp if the share
# can't hold a lock (some SMB mounts choke on flock-style ops — we use a plain
# PID file so it works either way).
LOCKFILE="$RUNNING/.worker.$HOST.lock"
if ! ( : > "$LOCKFILE" ) 2>/dev/null; then
  LOCKFILE="${TMPDIR:-/tmp}/ltxfarm.$HOST.lock"
fi
if [ -f "$LOCKFILE" ]; then
  oldpid="$(cat "$LOCKFILE" 2>/dev/null || true)"
  if [ -n "$oldpid" ] && kill -0 "$oldpid" 2>/dev/null; then
    log "!! another worker already running on $HOST — refusing to double-run (violates one-GPU-job rule)"
    exit 1
  fi
  log "reclaiming stale lock (pid $oldpid not alive)"
fi
echo "$$" > "$LOCKFILE"

# --- perf profile -> flags + nice level -----------------------------------
# echoes:  "<nice>|<ltx extra flags>|<mflux extra flags>"
perf_spec(){
  case "$1" in
    light) echo "15|--low-ram --tile-frames 2|--low-ram" ;;
    *)     echo "5||"                          ;;   # full
  esac
}

# --- atomic claim: only one worker wins the mv ----------------------------
claim(){
  local base; base="$(basename "$1")"
  local claimed="$RUNNING/${base}.${HOST}.$$"
  mv "$1" "$claimed" 2>/dev/null && { echo "$claimed"; return 0; }
  return 1
}

# --- render one job -------------------------------------------------------
render(){
  local jobfile="$1"
  # per-job defaults, then source
  local ID="" TYPE="t2v" PROMPT="" IMAGE="" WIDTH=1080 HEIGHT=1920 FRAMES=97 SEED=42 FPS=24 EXTRA=""
  local LORA="" LORA_SCALE=1.0 STILL_PROMPT="" STILL_W=768 STILL_H=1280
  local MODE="hero"      # hero = full video | test = cheap z-image still to cherry-pick
  local PERF="$PERF"     # inherit worker default; job may override below
  # shellcheck disable=SC1090
  source "$jobfile"

  [ -n "$ID" ]     || ID="job_$(date +%H%M%S)"
  [ -n "$PROMPT" ] || { log "  !! $ID has no PROMPT"; return 2; }

  local spec nice_l ltx_extra mflux_extra
  spec="$(perf_spec "$PERF")"
  nice_l="${spec%%|*}"; local rest="${spec#*|}"
  ltx_extra="${rest%%|*}"; mflux_extra="${rest#*|}"

  local out="$DONE/${ID}.mp4"
  log ">>> $ID  mode=$MODE type=$TYPE perf=$PERF ${WIDTH}x${HEIGHT} f=${FRAMES} seed=${SEED}"
  notify "$MODE: $ID ($PERF)"

  # ---- TEST mode: cheap cherry-pick proof (z-image still only, no video) --
  if [ "$MODE" = "test" ]; then
    mkdir -p "$DONE/proofs"
    local proof="$DONE/proofs/${ID}_seed${SEED}.png"
    local lp="" largs=()
    if [ -n "$LORA" ]; then
      lp="$LORA"; [ -f "$lp" ] || lp="$LORA_DIR/$LORA"
      [ -f "$lp" ] || { log "  !! LoRA missing: $LORA"; return 2; }
      largs=(--lora-paths "$lp" --lora-scales "$LORA_SCALE")
    fi
    local sp="${STILL_PROMPT:-$PROMPT}"
    log "  TEST proof still -> proofs/$(basename "$proof")"
    caffeinate -ims nice -n "$nice_l" mflux-generate-z-image-turbo -q 4 $mflux_extra \
      "${largs[@]}" --steps 9 --guidance 1.0 --width "$STILL_W" --height "$STILL_H" \
      --seed "$SEED" --prompt "$sp" --output "$proof"
    return $?
  fi

  local imgarg=()

  # ---- lora_i2v: generate a still with the character LoRA first ----------
  if [ "$TYPE" = "lora_i2v" ]; then
    [ -n "$LORA" ] || { log "  !! lora_i2v needs LORA"; return 2; }
    local lorapath="$LORA"; [ -f "$lorapath" ] || lorapath="$LORA_DIR/$LORA"
    [ -f "$lorapath" ] || { log "  !! LoRA missing: $LORA (run provision.command)"; return 2; }
    local still="$FARM_ROOT/assets/_still_${ID}_${HOST}.png"
    local sp="${STILL_PROMPT:-$PROMPT}"
    log "  still: z-image + $(basename "$lorapath") -> $(basename "$still")"
    if ! caffeinate -ims nice -n "$nice_l" mflux-generate-z-image-turbo -q 4 $mflux_extra \
          --lora-paths "$lorapath" --lora-scales "$LORA_SCALE" \
          --steps 9 --guidance 1.0 --width "$STILL_W" --height "$STILL_H" --seed "$SEED" \
          --prompt "$sp" --output "$still"; then
      log "  !! still gen failed"; return 3
    fi
    imgarg=(--image "$still")
  elif [ "$TYPE" = "i2v" ] || [ -n "$IMAGE" ]; then
    local src="$IMAGE"; [ -f "$src" ] || src="$ASSETS/$IMAGE"
    [ -f "$src" ] || { log "  !! i2v source missing: $IMAGE"; return 2; }
    imgarg=(--image "$src")
  fi

  # ---- LTX-2.3 MLX render (exact invocation from overnight_run.sh + perf) -
  local _t0 _t1 _dur
  _t0="$(date +%s)"
  ( cd "$LTX_DIR" && export HF_HUB_OFFLINE=1 HF_HUB_ENABLE_HF_TRANSFER=0 && \
    caffeinate -ims nice -n "$nice_l" uv run ltx-2-mlx generate \
      --model "$MODEL" --distilled $ltx_extra \
      "${imgarg[@]}" \
      -W "$WIDTH" -H "$HEIGHT" -f "$FRAMES" --frame-rate "$FPS" --seed "$SEED" \
      --prompt "$PROMPT" $EXTRA \
      -o "$out" )
  local _rc=$?
  [ $_rc -eq 0 ] || return $_rc
  _t1="$(date +%s)"; _dur=$(( _t1 - _t0 ))

  # ---- metadata sidecar (success only, real mp4 — never in test mode) -----
  local _pj="${PROMPT//\\/\\\\}"; _pj="${_pj//\"/\\\"}"   # escape backslashes then quotes
  {
    printf '{\n'
    printf '  "id": "%s",\n'       "$ID"
    printf '  "mode": "%s",\n'     "$MODE"
    printf '  "type": "%s",\n'     "$TYPE"
    printf '  "prompt": "%s",\n'   "$_pj"
    printf '  "seed": %s,\n'       "$SEED"
    printf '  "model": "%s",\n'    "$MODEL"
    printf '  "lora": "%s",\n'     "$LORA"
    printf '  "width": %s,\n'      "$WIDTH"
    printf '  "height": %s,\n'     "$HEIGHT"
    printf '  "frames": %s,\n'     "$FRAMES"
    printf '  "fps": %s,\n'        "$FPS"
    printf '  "perf": "%s",\n'     "$PERF"
    printf '  "worker": "%s",\n'   "$HOST"
    printf '  "duration_secs": %s\n' "$_dur"
    printf '}\n'
  } > "$DONE/${ID}.json"
  return 0
}

# --- disk guard: whole GB available on the volume holding $DONE -----------
# macOS `df -g` -> avail is the 4th column, already in whole GB. If we can't
# parse it, echo nothing and the caller proceeds (fail open, never block forever).
free_gb(){
  df -g "$DONE" 2>/dev/null | awk 'NR==2 {print $4}'
}

# --- heartbeat control: <claimed>.heartbeat touched every 30s -------------
HB_PID=""
start_heartbeat(){
  local hb="${1}.heartbeat"
  ( while :; do touch "$hb" 2>/dev/null; sleep 30; done ) &
  HB_PID=$!
}
stop_heartbeat(){
  [ -n "$HB_PID" ] && kill "$HB_PID" 2>/dev/null
  wait "$HB_PID" 2>/dev/null || true
  HB_PID=""
  [ -n "${1:-}" ] && rm -f "${1}.heartbeat"
}

# --- main loop ------------------------------------------------------------
trap 'log "worker stopping — an in-flight job stays in running/ and is requeued by farm_status.sh --reap"; stop_heartbeat "${claimed:-}"; rm -f "$LOCKFILE" 2>/dev/null; exit 0' INT TERM
trap 'rm -f "$LOCKFILE" 2>/dev/null' EXIT
while true; do
  # disk guard: bail before claiming if the $DONE volume is nearly full
  avail_gb="$(free_gb)"
  if [ -n "$avail_gb" ] && [ "$avail_gb" -lt "$MIN_FREE_GB" ] 2>/dev/null; then
    log "!! low disk (<${avail_gb}GB free) — pausing"
    notify "low disk (${avail_gb}GB) — paused"
    sleep "$POLL_SECS"; continue
  fi

  # priority lane first, then the normal queue (the $QUEUE/*.job glob can't
  # match the hi/ subdir, so no double-scan)
  next="$(ls -1 "$QUEUE"/hi/*.job 2>/dev/null | sort | head -n1 || true)"
  [ -z "$next" ] && next="$(ls -1 "$QUEUE"/*.job 2>/dev/null | sort | head -n1 || true)"
  [ -z "$next" ] && { sleep "$POLL_SECS"; continue; }
  claimed="$(claim "$next")" || continue        # lost the race, try next
  log "claimed $(basename "$claimed")"
  start_heartbeat "$claimed"
  if render "$claimed"; then
    stop_heartbeat "$claimed"
    mv "$claimed" "$DONE/$(basename "$claimed").ok"; log "✅ done -> $DONE"; notify "finished a clip"
  else
    rc=$?; stop_heartbeat "$claimed"
    mv "$claimed" "$FAILED/$(basename "$claimed").rc${rc}"; log "❌ FAILED rc=$rc"; notify "job failed rc=$rc"
  fi
  claimed=""
done
