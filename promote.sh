#!/bin/bash
# ============================================================================
# AUTO-PROMOTE WINNERS — turn cheap --test proof stills into full --hero renders.
# ----------------------------------------------------------------------------
# After a `--test` sweep, workers drop proof stills in
#   $FARM_ROOT/done/proofs/<ID>_seed<SEED>.png
# This tool lets you cherry-pick the good ones and re-enqueue EXACTLY those
# seeds as full --hero video renders — closing the test -> hero loop without
# hand-copying seeds. It just shells out to enqueue.sh for each pick.
#
#   # interactive (default when run in a terminal): numbered list, pick + prompt
#   ./promote.sh
#
#   # promote every proof, non-interactive, one prompt for all
#   ./promote.sh --all --prompt "storm clouds over a QLD roof, cinematic"
#
#   # only certain IDs or seeds
#   ./promote.sh --ids "hail_hero milk" --prompt "..."
#   ./promote.sh --seeds "1000 1003" --prompt "..."
#
#   # see what it WOULD do, queue nothing
#   ./promote.sh --all --dry-run
#
#   # heroes usually jump the queue
#   ./promote.sh --all --prompt "..." --priority high
#
#   # carry the LoRA / still-prompt / dims the proof used into the hero
#   ./promote.sh --all --prompt "..." --lora Elijah_lora.safetensors --lora-scale 0.9
#
# Each pick is queued as:  enqueue.sh --id "<ID>_hero" --seed <SEED> --hero --prompt "..."
# (plus any --priority/--lora/--lora-scale/--still-prompt/-W/-H/-f/--fps you pass).
# ============================================================================
set -euo pipefail

# --- resolve our own dir robustly (so we can find enqueue.sh beside us) -----
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
ENQUEUE="$SCRIPT_DIR/enqueue.sh"

# --- config ----------------------------------------------------------------
FARM_ROOT="${FARM_ROOT:-/Volumes/RenderFarm}"
DONE="$FARM_ROOT/done"
PROOFS="$DONE/proofs"

# --- flags -----------------------------------------------------------------
DO_ALL=0; DRY_RUN=0; PROMPT=""; PRIORITY="normal"
FILTER_IDS=""; FILTER_SEEDS=""
LORA=""; LORA_SCALE=""; STILL_PROMPT=""; WIDTH=""; HEIGHT=""; FRAMES=""; FPS=""
while [ $# -gt 0 ]; do case "$1" in
  --all)          DO_ALL=1; shift;;
  --dry-run)      DRY_RUN=1; shift;;
  --prompt)       PROMPT="$2"; shift 2;;
  --ids)          FILTER_IDS="$2"; shift 2;;
  --seeds)        FILTER_SEEDS="$2"; shift 2;;
  --priority)     PRIORITY="$2"; shift 2;;
  --lora)         LORA="$2"; shift 2;;
  --lora-scale)   LORA_SCALE="$2"; shift 2;;
  --still-prompt) STILL_PROMPT="$2"; shift 2;;
  -W)             WIDTH="$2"; shift 2;;
  -H)             HEIGHT="$2"; shift 2;;
  -f)             FRAMES="$2"; shift 2;;
  --fps)          FPS="$2"; shift 2;;
  -h|--help)      sed -n '2,30p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0;;
  *) echo "!! unknown arg: $1"; exit 1;;
esac; done

# --- guards ----------------------------------------------------------------
[ -f "$ENQUEUE" ]  || { echo "!! enqueue.sh not found beside promote.sh: $ENQUEUE"; exit 1; }
[ -x "$ENQUEUE" ]  || { echo "!! enqueue.sh is not executable: $ENQUEUE  (chmod +x it)"; exit 1; }

# --- collect proofs, newest first ------------------------------------------
# ls -t sorts by mtime desc; guard the nullglob-less case with a check.
PROOF_FILES=()
if [ -d "$PROOFS" ]; then
  while IFS= read -r p; do [ -n "$p" ] && PROOF_FILES+=("$p"); done \
    < <(ls -1t "$PROOFS"/*.png 2>/dev/null || true)
fi
if [ "${#PROOF_FILES[@]}" -eq 0 ]; then
  echo "No proof stills in $PROOFS — run a --test sweep first, then come back. Nothing to promote."
  exit 0
fi

# --- parse ID + SEED out of  <ID>_seed<SEED>.png  (ID may contain _) --------
# strip dir + .png, then peel the trailing _seed<digits>.
parse_id(){   local b; b="$(basename "$1" .png)"; echo "${b%_seed*}"; }
parse_seed(){ local b; b="$(basename "$1" .png)"; echo "${b##*_seed}"; }

# best-effort prompt recovery: hero sidecar at $DONE/<ID>.json (proofs won't
# have one — a sibling agent writes these for hero renders). Don't block on it.
recover_prompt(){
  local id="$1" sc="$DONE/$1.json"
  [ -f "$sc" ] || return 1
  # pull "prompt": "...."  (single-line json from farm_worker.sh)
  sed -n 's/.*"prompt"[[:space:]]*:[[:space:]]*"\(.*\)".*/\1/p' "$sc" | head -n1
}

# in-set membership over a space-separated list
in_list(){ local needle="$1"; shift; local x; for x in $*; do [ "$x" = "$needle" ] && return 0; done; return 1; }

# --- build the working set (apply --ids / --seeds filters if given) --------
SET_FILES=(); SET_IDS=(); SET_SEEDS=()
i=0
for p in "${PROOF_FILES[@]}"; do
  id="$(parse_id "$p")"; seed="$(parse_seed "$p")"
  if [ -n "$FILTER_IDS" ]   && ! in_list "$id"   "$FILTER_IDS";   then continue; fi
  if [ -n "$FILTER_SEEDS" ] && ! in_list "$seed" "$FILTER_SEEDS"; then continue; fi
  SET_FILES+=("$p"); SET_IDS+=("$id"); SET_SEEDS+=("$seed")
  i=$((i+1))
done

if [ "${#SET_FILES[@]}" -eq 0 ]; then
  echo "No proofs matched your filter (--ids \"$FILTER_IDS\" --seeds \"$FILTER_SEEDS\")."
  exit 0
fi

# --- decide which indices to promote ---------------------------------------
# SELECT holds 0-based indices into SET_*.
SELECT=()

select_all(){ SELECT=(); local k=0; while [ $k -lt "${#SET_FILES[@]}" ]; do SELECT+=("$k"); k=$((k+1)); done; }

if [ "$DO_ALL" -eq 1 ] || [ -n "$FILTER_IDS" ] || [ -n "$FILTER_SEEDS" ]; then
  # any non-interactive selector -> take the whole (already-filtered) set
  select_all
elif [ -t 0 ]; then
  # --- interactive picker ---------------------------------------------------
  echo "Proof stills in $PROOFS (newest first):"
  k=0
  while [ $k -lt "${#SET_FILES[@]}" ]; do
    id="${SET_IDS[$k]}"; seed="${SET_SEEDS[$k]}"
    pr="$(recover_prompt "$id" || true)"
    if [ -n "$pr" ]; then
      printf "  %2d) %-28s seed=%-8s  %s\n" "$((k+1))" "$id" "$seed" "${pr:0:60}"
    else
      printf "  %2d) %-28s seed=%-8s\n" "$((k+1))" "$id" "$seed"
    fi
    k=$((k+1))
  done
  echo
  echo "Pick proofs to promote to full --hero renders."
  printf "numbers/ranges (e.g. 1 3 5-7), 'all', or 'q' to quit: "
  read -r picks
  case "$picks" in
    q|Q|"") echo "nothing promoted."; exit 0;;
    all|ALL) select_all;;
    *)
      for tok in $picks; do
        case "$tok" in
          *-*)  lo="${tok%-*}"; hi="${tok#*-}"
                case "$lo$hi" in *[!0-9]*) echo "!! bad range: $tok"; exit 1;; esac
                [ "$lo" -le "$hi" ] || { echo "!! bad range: $tok"; exit 1; }
                n="$lo"; while [ "$n" -le "$hi" ]; do SELECT+=("$((n-1))"); n=$((n+1)); done ;;
          *[!0-9]*) echo "!! not a number: $tok"; exit 1;;
          *)    SELECT+=("$((tok-1))");;
        esac
      done ;;
  esac
else
  echo "!! non-interactive with no selector — pass --all, --ids or --seeds. Nothing promoted."
  exit 1
fi

# validate indices in range + de-dupe (preserve order)
CLEAN=(); seen=" "
for idx in "${SELECT[@]}"; do
  if [ "$idx" -lt 0 ] || [ "$idx" -ge "${#SET_FILES[@]}" ]; then
    echo "!! index out of range: $((idx+1))"; exit 1
  fi
  case "$seen" in *" $idx "*) ;; *) CLEAN+=("$idx"); seen="$seen$idx ";; esac
done
SELECT=("${CLEAN[@]}")
[ "${#SELECT[@]}" -gt 0 ] || { echo "nothing selected."; exit 0; }

# --- prompt: required for hero (proofs don't carry it reliably) -------------
if [ -z "$PROMPT" ]; then
  if [ -t 0 ]; then
    echo
    echo "Proofs don't carry a reliable prompt, so the hero render needs one."
    printf "Enter the hero prompt to use for the %d selected proof(s): " "${#SELECT[@]}"
    read -r PROMPT
    [ -n "$PROMPT" ] || { echo "!! no prompt given — aborting."; exit 1; }
  else
    echo "!! --prompt is required (proofs don't carry a reliable prompt for the hero render)."
    exit 1
  fi
fi

# --- pass-through args for enqueue.sh --------------------------------------
PASS=()
[ "$PRIORITY" = "high" ]  && PASS+=(--priority high)
[ -n "$LORA" ]            && PASS+=(--lora "$LORA")
[ -n "$LORA_SCALE" ]      && PASS+=(--lora-scale "$LORA_SCALE")
[ -n "$STILL_PROMPT" ]    && PASS+=(--still-prompt "$STILL_PROMPT")
[ -n "$WIDTH" ]           && PASS+=(-W "$WIDTH")
[ -n "$HEIGHT" ]          && PASS+=(-H "$HEIGHT")
[ -n "$FRAMES" ]          && PASS+=(-f "$FRAMES")
[ -n "$FPS" ]             && PASS+=(--fps "$FPS")

# --- promote ----------------------------------------------------------------
QNAME="queue"; [ "$PRIORITY" = "high" ] && QNAME="hi-queue"
n=0
for idx in "${SELECT[@]}"; do
  id="${SET_IDS[$idx]}"; seed="${SET_SEEDS[$idx]}"
  if [ "$DRY_RUN" -eq 1 ]; then
    echo "would promote: ${id}_hero  seed=$seed -> $QNAME"
  else
    "$ENQUEUE" --id "${id}_hero" --seed "$seed" --hero --prompt "$PROMPT" "${PASS[@]}"
  fi
  n=$((n+1))
done

if [ "$DRY_RUN" -eq 1 ]; then
  echo "dry-run: would promote $n proof(s) as hero renders -> $QNAME"
else
  echo "promoted $n proof(s) as hero renders -> $QNAME"
fi
