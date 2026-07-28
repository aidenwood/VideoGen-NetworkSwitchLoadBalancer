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
#   auto  = pick from this Mac's RAM. The default. Leave it alone.
#   full  = NO --low-ram, no tiling, nice -n 5. 64GB Macs ONLY.
#   light = --low-ram + temporal tiling, nice -n 15. Everything smaller.
#
# ⚠️ MEMORY NUMBERS: an earlier version of this header claimed 'full' peaks
# ~10-16GB and 'light' stays in "low single-digit GB". BOTH WERE WRONG by 3-5x.
# One 896x1216 z-image still WITH --low-ram measured 27.5GB peak MLX memory.
# That is why 'full' is now 64GB-only and why the worker prices every job
# against a 90%-of-RAM budget before claiming it. Do not re-derive these
# numbers from intuition — see docs/MEMORY-INCIDENT-2026-07-28.md.
#
# Also: MLX memory is INVISIBLE to `ps` RSS (that 27.5GB job reported 4.9GB),
# so admission control is done by PRICING the job, never by observing free RAM.
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
HOST="$(scutil --get LocalHostName 2>/dev/null || hostname -s)"
HERE="$(cd "$(dirname "$0")" && pwd)"

QUEUE="$FARM_ROOT/queue"; RUNNING="$FARM_ROOT/running"
DONE="$FARM_ROOT/done";   FAILED="$FARM_ROOT/failed"
ASSETS="$FARM_ROOT/assets"; LOGS="$FARM_ROOT/logs"

# --- preflight ------------------------------------------------------------
[ -d "$FARM_ROOT" ] || { echo "!! FARM_ROOT not mounted: $FARM_ROOT — connect the share first."; exit 1; }
mkdir -p "$QUEUE" "$QUEUE/hi" "$RUNNING" "$DONE" "$FAILED" "$ASSETS" "$LOGS"

log(){ echo "[$(date +%H:%M:%S)][$HOST] $*"; }
notify(){ osascript -e "display notification \"$1\" with title \"Render farm — $HOST\"" 2>/dev/null || true; }

# --- farm-wide config: one file on the share drives every Mac -------------
# Reloaded every poll, so editing $FARM_ROOT/farm.conf on the coordinator
# reaches all workers within ~$POLL_SECS — nobody has to touch each machine.
#
# Every entry in those files uses `: "${VAR:=x}"` (assign only if unset), so
# the FIRST source to set a variable wins. That means we source in reverse
# priority order — host override, then farm-wide, then built-in defaults —
# and reset the whole set to the launcher's env first so env always wins.
CONF_VARS="PERF AUTO_PERF_MIN_GB MEM_BUDGET_PCT ADMISSION STILL_GB_PER_MP
           STILL_GB_BASE VIDEO_GB_PER_MP VIDEO_GB_BASE MLX_CAP OOM_BACKOFF
           OOM_MAX_RETRY MEM_GUARD MIN_FREE_MEM_PCT MAX_PRESSURE_LEVEL
           MAX_SWAP_USED_MB MIN_FREE_GB POLL_SECS MODEL"
for _v in $CONF_VARS; do eval "ENV_${_v}=\${${_v}:-}"; done

load_conf(){
  local _v
  for _v in $CONF_VARS; do eval "${_v}=\$ENV_${_v}"; done
  # CONFIG comes from the share (edit once, every Mac picks it up).
  # shellcheck disable=SC1090,SC1091
  [ -f "$FARM_ROOT/farm.conf.$HOST" ] && . "$FARM_ROOT/farm.conf.$HOST"
  # shellcheck disable=SC1090,SC1091
  [ -f "$FARM_ROOT/farm.conf" ]       && . "$FARM_ROOT/farm.conf"
  # CODE comes from the local checkout first — a flaky SMB mount must never be
  # able to half-source the library that every guard depends on. The share is
  # only a fallback for a Mac that hasn't been provisioned yet.
  # shellcheck disable=SC1090,SC1091
  if   [ -f "$HERE/farm_mem.sh" ];       then . "$HERE/farm_mem.sh"
  # shellcheck disable=SC1090,SC1091
  elif [ -f "$FARM_ROOT/farm_mem.sh" ];  then . "$FARM_ROOT/farm_mem.sh"
  fi
  : "${MODEL:=dgrauet/ltx-2.3-mlx-q4}"
  : "${PERF:=auto}"
  : "${MLX_CAP:=1}"
  : "${ADMISSION:=block}"
  : "${OOM_BACKOFF:=90}"
  : "${OOM_MAX_RETRY:=2}"
  : "${POLL_SECS:=15}"
  : "${MIN_FREE_GB:=15}"
  # resolve PERF=auto against THIS Mac's installed RAM
  WORKER_PERF="$PERF"
  [ "$WORKER_PERF" = "auto" ] && WORKER_PERF="$(mem_auto_perf 2>/dev/null || echo light)"
}
load_conf

if ! declare -F mem_total_gb >/dev/null; then
  echo "!! farm_mem.sh not found (looked in $FARM_ROOT and $HERE) — no OOM protection."; exit 1
fi

# --- MLX budget: bytes this Mac may hand to a render, per profile ----------
# See docs/OOM_LIMITS.md. Injected into the render process via a sitecustomize
# on PYTHONPATH, so it applies to ltx-2-mlx and mflux without patching either.
MLX_SITE="$HERE/farm_sitecustomize"
[ -d "$MLX_SITE" ] || MLX_SITE="$FARM_ROOT/farm_sitecustomize"

# Sets MLXENV=() — a prefix for `env` that budgets the child process.
mlx_env(){
  MLXENV=()
  [ "$MLX_CAP" = "1" ] || return 0
  [ -d "$MLX_SITE" ] || { log "  ! sitecustomize missing at $MLX_SITE — render runs UNCAPPED"; return 0; }
  MLXENV=(
    "PYTHONPATH=${MLX_SITE}${PYTHONPATH:+:$PYTHONPATH}"
    "FARM_MLX_CAP_BYTES=$(mem_budget_bytes)"
    "FARM_MLX_CACHE_BYTES=$(mem_cache_bytes)"
    "FARM_MLX_VERBOSE=1"
  )
}

RAM_GB="$(mem_total_gb 2>/dev/null || echo '?')"
log "worker online. profile=$PERF->$WORKER_PERF  ram=${RAM_GB}GB ($(mem_tier))  budget=$(mem_budget_gb)GB (${MEM_BUDGET_PCT}%)  admission=$ADMISSION  model=$MODEL"
notify "worker online ($WORKER_PERF, ${RAM_GB}GB)"

# --- price a job WITHOUT running it ----------------------------------------
# Sourced in a subshell so the job file can't clobber the worker's own state.
# echoes "<need_gb>|<min_ram_gb>|<type>|<w>x<h>|<frames>"
price_job(){
  ( set +u
    TYPE="t2v"; WIDTH=1080; HEIGHT=1920; FRAMES=97; MODE="hero"; MIN_RAM_GB=0
    # shellcheck disable=SC1090
    . "$1" 2>/dev/null
    # a 'test' job only makes the cheap still, never the video
    [ "$MODE" = "test" ] && TYPE="test"
    printf '%s|%s|%s|%sx%s|%s\n' \
      "$(est_peak_gb "$TYPE" "$WIDTH" "$HEIGHT" "$FRAMES")" \
      "${MIN_RAM_GB:-0}" "$TYPE" "$WIDTH" "$HEIGHT" "$FRAMES" )
}

# --- release a claimed job back to the queue -------------------------------
# NOT a failure. This Mac simply can't afford it, so it goes back for a bigger
# one. We remember it in $RELEASED so this worker doesn't instantly re-claim
# the same job and spin. In-memory on purpose: no marker files to litter the
# share, and a worker restart re-evaluates (budgets may have changed).
RELEASED=""
release_job(){
  local claimed="$1" why="$2" orig
  orig="$(basename "$claimed" | sed -E 's/(\.job)\..*/\1/')"
  if mv "$claimed" "$QUEUE/$orig" 2>/dev/null; then
    RELEASED="$RELEASED $orig"
    log "↩︎  released $orig — $why"
  else
    log "!! could not release $orig back to the queue — leaving it in running/"
  fi
}

# --- publish this Mac's memory state so the farm can see it ----------------
# One line of KEY=VALUE per worker in running/, refreshed on every poll. Lets
# farm_status.sh (and the dashboard) show which Macs are memory-starved
# without having to SSH anywhere.
publish_info(){
  local f="$RUNNING/.worker.$HOST.info"
  {
    echo "HOST=\"$HOST\""
    echo "RAM_GB=$RAM_GB"
    echo "TIER=\"$(mem_tier)\""
    echo "PERF=\"$WORKER_PERF\""
    echo "BUDGET_GB=$(mem_budget_gb)"
    echo "FREE_PCT=$(mem_free_pct)"
    echo "PRESSURE=$(mem_pressure_level)"
    echo "SWAP_MB=$(mem_swap_used_mb)"
    echo "STATE=\"${1:-idle}\""
    echo "UPDATED=\"$(date +%H:%M:%S)\""
  } > "$f" 2>/dev/null || true
}

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
  local PERF="$WORKER_PERF"  # inherit this Mac's resolved default; job may override
  local OOM_RETRY=0          # bumped by the requeue path after a memory kill
  # shellcheck disable=SC1090
  source "$jobfile"

  [ -n "$ID" ]     || ID="job_$(date +%H%M%S)"
  [ -n "$PROMPT" ] || { log "  !! $ID has no PROMPT"; return 2; }
  # a job may ask for 'auto' too — resolve it against THIS Mac, not the sender's
  [ "$PERF" = "auto" ] && PERF="$(mem_auto_perf)"

  local spec nice_l ltx_extra mflux_extra
  spec="$(perf_spec "$PERF")"
  nice_l="${spec%%|*}"; local rest="${spec#*|}"
  ltx_extra="${rest%%|*}"; mflux_extra="${rest#*|}"

  # per-job memory budget + the log we scan afterwards to classify an OOM
  local MLXENV; mlx_env
  RENDER_LOG="$LOGS/${ID}.${HOST}.log"
  : > "$RENDER_LOG" 2>/dev/null || RENDER_LOG="${TMPDIR:-/tmp}/${ID}.${HOST}.log"

  local out="$DONE/${ID}.mp4"
  log ">>> $ID  mode=$MODE type=$TYPE perf=$PERF ${WIDTH}x${HEIGHT} f=${FRAMES} seed=${SEED} budget=$(mem_budget_gb)GB"
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
    # NOTE: ${arr[@]+"${arr[@]}"} — macOS ships bash 3.2, where a bare
    # "${arr[@]}" on an EMPTY array is an unbound-variable fatal under `set -u`.
    env ${MLXENV[@]+"${MLXENV[@]}"} \
      caffeinate -ims nice -n "$nice_l" mflux-generate-z-image-turbo -q 4 $mflux_extra \
      ${largs[@]+"${largs[@]}"} --steps 9 --guidance 1.0 --width "$STILL_W" --height "$STILL_H" \
      --seed "$SEED" --prompt "$sp" --output "$proof" 2>&1 | tee -a "$RENDER_LOG"
    return "${PIPESTATUS[0]}"
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
    env ${MLXENV[@]+"${MLXENV[@]}"} \
      caffeinate -ims nice -n "$nice_l" mflux-generate-z-image-turbo -q 4 $mflux_extra \
          --lora-paths "$lorapath" --lora-scales "$LORA_SCALE" \
          --steps 9 --guidance 1.0 --width "$STILL_W" --height "$STILL_H" --seed "$SEED" \
          --prompt "$sp" --output "$still" 2>&1 | tee -a "$RENDER_LOG"
    local _src=${PIPESTATUS[0]}
    if [ "$_src" -ne 0 ]; then
      # surface a memory kill with its real rc so the caller can requeue lighter
      mem_is_oom "$_src" "$RENDER_LOG" && { log "  !! still gen OOM (rc=$_src)"; return "$_src"; }
      log "  !! still gen failed (rc=$_src)"; return 3
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
    env ${MLXENV[@]+"${MLXENV[@]}"} \
    caffeinate -ims nice -n "$nice_l" uv run ltx-2-mlx generate \
      --model "$MODEL" --distilled $ltx_extra \
      ${imgarg[@]+"${imgarg[@]}"} \
      -W "$WIDTH" -H "$HEIGHT" -f "$FRAMES" --frame-rate "$FPS" --seed "$SEED" \
      --prompt "$PROMPT" $EXTRA \
      -o "$out" ) 2>&1 | tee -a "$RENDER_LOG"
  local _rc=${PIPESTATUS[0]}
  [ $_rc -eq 0 ] || return $_rc
  _t1="$(date +%s)"; _dur=$(( _t1 - _t0 ))

  # ---- metadata sidecar (success only, real mp4 — never in test mode) -----
  # peak_mem_gb comes from the atexit hook in farm_sitecustomize — it's the
  # ground truth for retuning the budgets in farm.conf. "null" if uncapped.
  local _peak; _peak="$(awk '/\[farm\] MLX peak:/ {v=$(NF-1)} END{print (v==""?"null":v)}' "$RENDER_LOG" 2>/dev/null)"
  [ -n "$_peak" ] || _peak="null"
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
    printf '  "worker_ram_gb": %s,\n' "$RAM_GB"
    printf '  "budget_gb": %s,\n'  "$(mem_budget_gb)"
    printf '  "peak_mem_gb": %s,\n' "$_peak"
    printf '  "oom_retry": %s,\n'  "$OOM_RETRY"
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

# --- OOM requeue: retry a memory-killed job at the light profile -----------
# A job that OOMs isn't a broken job, it's a job that was too big for THIS Mac.
# Dumping it in failed/ loses work and hides the real cause, so instead we pin
# it to PERF=light (the profile that peaks in low single-digit GB) and put it
# back in the queue. Appending to the jobfile is safe: workers `source` it, so
# a later assignment simply overrides the earlier one.
requeue_oom(){
  local claimed="$1" rc="$2" tries="$3" orig bump
  orig="$(basename "$claimed" | sed -E 's/(\.job)\..*/\1/')"
  # The price model just proved itself wrong for this job on this Mac, so raise
  # its floor above what we have. Next time only a genuinely bigger Mac takes
  # it, instead of every 32GB box discovering the same thing the hard way.
  bump=$(( $(mem_total_gb) + 1 ))
  {
    echo ""
    echo "# --- requeued after OOM on $HOST (rc=$rc) at $(date '+%Y-%m-%d %H:%M:%S') ---"
    echo "PERF=\"light\""
    echo "MIN_RAM_GB=$bump"
    echo "OOM_RETRY=$tries"
  } >> "$claimed" 2>/dev/null
  if mv "$claimed" "$QUEUE/OOMRETRY_${orig}" 2>/dev/null; then
    RELEASED="$RELEASED OOMRETRY_${orig}"
    log "♻️  OOM (rc=$rc) — requeued light, now needs >${bump}GB (attempt $tries/$OOM_MAX_RETRY)"
  else
    mv "$claimed" "$FAILED/$(basename "$claimed").rc${rc}"; log "❌ OOM and requeue failed"
  fi
  notify "OOM — requeued for a bigger Mac"
  # Load-bearing. An instant retry reloads a ~27GB model into a machine that is
  # still recovering and gets killed again — that is the runaway loop from the
  # original incident. Let the memory actually drain.
  log "   backing off ${OOM_BACKOFF}s to let memory drain"
  publish_info "backoff:oom"
  sleep "$OOM_BACKOFF"
}

# --- main loop ------------------------------------------------------------
trap 'log "worker stopping — an in-flight job stays in running/ and is requeued by farm_status.sh --reap"; stop_heartbeat "${claimed:-}"; rm -f "$LOCKFILE" "$RUNNING/.worker.$HOST.info" 2>/dev/null; exit 0' INT TERM
trap 'rm -f "$LOCKFILE" "$RUNNING/.worker.$HOST.info" 2>/dev/null' EXIT
while true; do
  # pick up any farm.conf edit made on the coordinator since the last poll
  load_conf
  publish_info idle

  # disk guard: bail before claiming if the $DONE volume is nearly full
  avail_gb="$(free_gb)"
  if [ -n "$avail_gb" ] && [ "$avail_gb" -lt "$MIN_FREE_GB" ] 2>/dev/null; then
    log "!! low disk (<${avail_gb}GB free) — pausing"
    notify "low disk (${avail_gb}GB) — paused"
    publish_info "paused:disk"
    sleep "$POLL_SECS"; continue
  fi

  # RAM guard: same idea as the disk guard, for memory. Checked BEFORE we claim
  # so a starved Mac quietly leaves the job for a healthier one instead of
  # claiming it and then dying halfway through.
  if ! mem_why="$(mem_guard_reason)"; then
    log "!! memory guard: $mem_why — pausing"
    notify "low memory — paused"
    publish_info "paused:memory"
    sleep "$POLL_SECS"; continue
  fi

  # priority lane first, then the normal queue (the $QUEUE/*.job glob can't
  # match the hi/ subdir, so no double-scan). Anything this Mac already
  # released as unaffordable is skipped, so we don't claim-and-release in a
  # tight loop while a bigger Mac gets a chance at it.
  next=""
  for cand in "$QUEUE"/hi/*.job "$QUEUE"/*.job; do
    [ -e "$cand" ] || continue
    case " $RELEASED " in *" $(basename "$cand") "*) continue;; esac
    next="$cand"; break
  done
  [ -z "$next" ] && { sleep "$POLL_SECS"; continue; }
  claimed="$(claim "$next")" || continue        # lost the race, try next
  log "claimed $(basename "$claimed")"

  # ---- ADMISSION CONTROL: price it before spending an hour on it ----------
  # This, not the free-RAM guard, is what stops the OOM. MLX memory is
  # invisible to every free-memory reading, so the only reliable question is
  # "what does a job this size cost, and can this Mac afford it?"
  price="$(price_job "$claimed")"
  need="${price%%|*}"; rest="${price#*|}"
  min_ram="${rest%%|*}"; rest="${rest#*|}"
  jtype="${rest%%|*}"; rest="${rest#*|}"; jdim="${rest%%|*}"
  if ! afford_why="$(mem_can_afford "$need" "$min_ram")"; then
    case "$ADMISSION" in
      block)
        release_job "$claimed" "$jtype $jdim $afford_why"
        publish_info idle; claimed=""; continue ;;
      warn)
        log "  ⚠️  $jtype $jdim $afford_why — rendering anyway (ADMISSION=warn)" ;;
    esac
  else
    log "  priced ~${need}GB / $(mem_budget_gb)GB budget"
  fi

  publish_info "rendering"
  start_heartbeat "$claimed"
  RENDER_LOG=""
  if render "$claimed"; then
    stop_heartbeat "$claimed"
    mv "$claimed" "$DONE/$(basename "$claimed").ok"; log "✅ done -> $DONE"; notify "finished a clip"
  else
    rc=$?; stop_heartbeat "$claimed"
    # how many times has this job already been bounced for memory?
    tries="$(sed -n 's/^OOM_RETRY=\([0-9]*\).*/\1/p' "$claimed" 2>/dev/null | tail -n1)"
    tries="${tries:-0}"
    if mem_is_oom "$rc" "$RENDER_LOG" && [ "$tries" -lt "$OOM_MAX_RETRY" ]; then
      requeue_oom "$claimed" "$rc" "$(( tries + 1 ))"
    else
      mv "$claimed" "$FAILED/$(basename "$claimed").rc${rc}"
      if mem_is_oom "$rc" "$RENDER_LOG"; then
        log "❌ FAILED rc=$rc — OOM, out of retries (${tries}/${OOM_MAX_RETRY}). Shrink the job (frames cost most), or re-measure with ./measure_peak.sh."
        notify "job OOMed — out of retries"
      else
        log "❌ FAILED rc=$rc  (log: ${RENDER_LOG:-none})"; notify "job failed rc=$rc"
      fi
    fi
  fi
  claimed=""
done
