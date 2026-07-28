#!/bin/bash
# ============================================================================
# MEMORY / OOM LIBRARY — sourced by farm_worker.sh, farm_status.sh, provision.
# ----------------------------------------------------------------------------
# Built from the measured findings in docs/MEMORY-INCIDENT-2026-07-28.md. Read
# that first if you are about to change a number in here.
#
# THE THREE FACTS THAT SHAPE ALL OF THIS
#
# 1. Apple Silicon has UNIFIED memory. No VRAM pool. A render's weights and
#    activations come out of the same RAM macOS and the user's apps are using.
#
# 2. MLX allocations are INVISIBLE to `ps` RSS. A job measured at 27.5GB peak
#    MLX memory reported 4.9GB RSS. So "wait until N GB look free, then launch"
#    is blind to the thing it is gating on — in the original incident three
#    workers all read "plenty free" in the same instant, all launched, and
#    demanded ~100GB on a 36GB box. Jetsam killed the lot.
#    => Gate on a STATIC BUDGET from hw.memsize + a DECLARED COST per job.
#       Admission control, not observation.
#
# 3. The peaks in the old README were 3-5x too low. One 896x1216 z-image still
#    with --low-ram measured 27.5GB, against a documented "low single-digit GB".
#    Every estimate here is anchored to that measurement.
#
# Fleet this must be correct on: 3 x 32GB Macs + 1 x 64GB Mac. Nothing may
# assume the author's own machine.
# ============================================================================

# --- tunables (farm.conf overrides these; env overrides farm.conf) ----------
: "${MEM_BUDGET_PCT:=90}"         # a worker never admits a job priced above this % of RAM
: "${AUTO_PERF_MIN_GB:=64}"       # 'full' (no --low-ram) only on Macs this big
: "${ADMISSION:=block}"           # block | warn | off  — what to do with an unaffordable job
: "${STILL_GB_PER_MP:=25}"        # measured: 896x1216 (1.09MP) --low-ram = 27.5GB
: "${STILL_GB_BASE:=3}"
: "${VIDEO_GB_PER_MP:=22}"        # EXTRAPOLATED, not measured — see docs §"open item"
: "${VIDEO_GB_BASE:=4}"
: "${OOM_BACKOFF:=90}"            # seconds to let memory drain after a jetsam kill
: "${MIN_FREE_MEM_PCT:=10}"       # secondary guard — see the caveat on mem_guard_reason
: "${MAX_PRESSURE_LEVEL:=4}"      # 1 normal, 2 warn, 4 critical
: "${MAX_SWAP_USED_MB:=12288}"
: "${MEM_GUARD:=1}"

# --- raw readings -----------------------------------------------------------
# Total physical RAM in whole GB.
mem_total_gb(){
  local b; b="$(sysctl -n hw.memsize 2>/dev/null)" || return 1
  [ -n "$b" ] || return 1
  echo $(( b / 1024 / 1024 / 1024 ))
}

# Free system memory, whole percent. Same number `memory_pressure -Q` prints.
mem_free_pct(){ sysctl -n kern.memorystatus_level 2>/dev/null; }

# macOS VM pressure: 1 = normal, 2 = warn, 4 = critical.
mem_pressure_level(){ sysctl -n kern.memorystatus_vm_pressure_level 2>/dev/null; }

# Swap in use, whole MB. "used = 3927.12M" -> 3927
mem_swap_used_mb(){
  sysctl -n vm.swapusage 2>/dev/null \
    | awk '{ for(i=1;i<=NF;i++) if($i=="used"){ v=$(i+2); gsub(/[MG]$/,"",v);
             if($(i+2) ~ /G$/) v=v*1024; printf "%d\n", v; exit } }'
}

# --- the budget -------------------------------------------------------------
# ONE number per Mac: 90% of installed RAM. Deliberately not per-profile —
# `full` vs `light` selects LTX/mflux FLAGS, it does not change how much memory
# the machine physically has. 28GB on a 32GB Mac, 57GB on a 64GB Mac.
mem_budget_gb(){
  local g; g="$(mem_total_gb)" || { echo 0; return; }
  echo $(( g * MEM_BUDGET_PCT / 100 ))
}
mem_budget_bytes(){ echo $(( $(mem_budget_gb) * 1024*1024*1024 )); }
# MLX's free-buffer cache gets half the budget; left at its default it hoards.
mem_cache_bytes(){ echo $(( $(mem_budget_bytes) / 2 )); }

mem_tier(){
  local g; g="$(mem_total_gb)" || { echo "unknown"; return; }
  if   [ "$g" -le 8  ]; then echo "8GB-unusable"
  elif [ "$g" -le 18 ]; then echo "16GB-unusable"
  elif [ "$g" -le 26 ]; then echo "24GB-video-only"
  elif [ "$g" -le 40 ]; then echo "32GB-video-only"
  else                       echo "64GB-everything"
  fi
}

# 'full' drops --low-ram. Given --low-ram ALREADY measured 27.5GB, dropping it
# on anything smaller than 64GB is asking for a jetsam kill.
mem_auto_perf(){
  local g; g="$(mem_total_gb)" || { echo "light"; return; }
  [ "$g" -ge "$AUTO_PERF_MIN_GB" ] && echo "full" || echo "light"
}

# --- pricing a job before we claim it ---------------------------------------
# Peak scales with pixel count (x frames, for video), not with the model.
# est_peak_gb <type> <width> <height> <frames>
#
# ONLY the still coefficient is measured. The video one is an extrapolation and
# is why hero 1080x1920 currently prices as 64GB-only. Measure it (see the docs)
# and correct VIDEO_GB_PER_MP in farm.conf — do not just raise it to make jobs fit.
est_peak_gb(){
  local type="$1" w="${2:-0}" h="${3:-0}" f="${4:-97}"
  [ "$w" -gt 0 ] 2>/dev/null || { echo 0; return; }
  case "$type" in
    lora_i2v|still|test)
      awk -v w="$w" -v h="$h" -v c="$STILL_GB_PER_MP" -v b="$STILL_GB_BASE" \
          'BEGIN{ printf "%d", (w*h)/1000000*c + b }' ;;
    *)
      awk -v w="$w" -v h="$h" -v f="$f" -v c="$VIDEO_GB_PER_MP" -v b="$VIDEO_GB_BASE" \
          'BEGIN{ printf "%d", (w*h)/1000000*(f/97)*c + b }' ;;
  esac
}

# Can THIS Mac afford this job? echoes a reason + returns 1 when it cannot.
# mem_can_afford <need_gb> <min_ram_gb>
mem_can_afford(){
  local need="${1:-0}" min_ram="${2:-0}" budget total
  budget="$(mem_budget_gb)"; total="$(mem_total_gb)"
  if [ -n "$min_ram" ] && [ "$min_ram" -gt 0 ] 2>/dev/null && [ "$total" -lt "$min_ram" ]; then
    echo "job is marked MIN_RAM_GB=${min_ram}, this Mac has ${total}GB"; return 1
  fi
  if [ "$need" -gt "$budget" ] 2>/dev/null; then
    echo "needs ~${need}GB, budget is ${budget}GB (${MEM_BUDGET_PCT}% of ${total}GB)"; return 1
  fi
  return 0
}

# --- the observational guard (SECONDARY — read this before trusting it) -----
# Per fact 2 above, free-memory readings CANNOT see MLX's allocations, so this
# will never catch "this job is too big". Admission control does that.
#
# What it does still catch, and why it earns its place: a Mac where the person
# sitting at it has Final Cut and forty tabs open. That memory IS visible, and
# starting a 27GB render on top of it is what turns their machine to treacle.
# Treat it as courtesy to the human, not as OOM protection.
#
# FAILS OPEN: unparseable reading -> job proceeds.
mem_guard_reason(){
  [ "$MEM_GUARD" = "1" ] || return 0
  local free pressure swap
  free="$(mem_free_pct)"
  if [ -n "$free" ] && [ "$free" -lt "$MIN_FREE_MEM_PCT" ] 2>/dev/null; then
    echo "only ${free}% RAM free (need ${MIN_FREE_MEM_PCT}%)"; return 1
  fi
  pressure="$(mem_pressure_level)"
  if [ -n "$pressure" ] && [ "$pressure" -ge "$MAX_PRESSURE_LEVEL" ] 2>/dev/null; then
    echo "macOS memory pressure level ${pressure} (critical)"; return 1
  fi
  swap="$(mem_swap_used_mb)"
  if [ -n "$swap" ] && [ "$swap" -gt "$MAX_SWAP_USED_MB" ] 2>/dev/null; then
    echo "${swap}MB of swap in use — Mac is already thrashing"; return 1
  fi
  return 0
}

# --- OOM classification -----------------------------------------------------
# jetsam SIGKILL surfaces in bash as 137 (128+9); 134 is 128+SIGABRT from a
# std::bad_alloc. (The single-box pipeline.py sees the same event as a NEGATIVE
# returncode, because Python reports signals differently — same kill.)
# Belt and braces: exit code OR a log grep, since a signal doesn't always
# survive the caffeinate -> nice -> uv -> python chain intact.
mem_is_oom(){
  local rc="$1" logfile="${2:-}"
  case "$rc" in 137|134|139) return 0 ;; esac
  [ -n "$logfile" ] && [ -f "$logfile" ] || return 1
  grep -qiE 'out of memory|bad_alloc|metal::malloc|insufficient memory|\[metal\] out of|memory limit|Cannot allocate memory|Killed: 9' \
    "$logfile" 2>/dev/null
}
