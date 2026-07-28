#!/bin/bash
# ============================================================================
# PROVISION A WORKER — double-click on each Mac BEFORE its first render.
# Pulls the exact models + LoRAs (from MANIFEST.txt on the share) onto THIS Mac
# over the gigabit switch, into the same local paths the tools expect. Idempotent
# (rsync skips what's already current), so re-run any time the manifest changes.
# ============================================================================
set -euo pipefail
export FARM_ROOT="${FARM_ROOT:-/Volumes/RenderFarm}"
LORA_DIR="${LORA_DIR:-$HOME/farm-loras}"
HF_HUB="$HOME/.cache/huggingface/hub"
cd "$(dirname "$0")"

# shellcheck disable=SC1091
. "$(cd "$(dirname "$0")" && pwd)/farm_root.sh"
ensure_farm_root || { read -r -t 30 _ || true; exit 1; }
MANIFEST="$FARM_ROOT/MANIFEST.txt"; [ -f "$MANIFEST" ] || MANIFEST="./MANIFEST.txt"

mkdir -p "$HF_HUB" "$LORA_DIR"
echo "Provisioning $(scutil --get LocalHostName 2>/dev/null || hostname -s) from $FARM_ROOT ..."

# models (HF hub cache) — copy from share/models into local HF cache.
# On the COORDINATOR the share's models are hardlinks to this very cache, so
# there is nothing to pull; comparing inodes catches that without guessing.
if [ -d "$FARM_ROOT/models" ]; then
  for m in "$FARM_ROOT"/models/models--*; do
    [ -e "$m" ] || continue
    name="$(basename "$m")"
    src_ino="$(find "$m" -type f -print -quit 2>/dev/null)"
    if [ -n "$src_ino" ] && [ -e "$HF_HUB/$name" ]; then
      a="$(stat -f '%d:%i' "$src_ino" 2>/dev/null)"
      b="$(stat -f '%d:%i' "${src_ino/#$FARM_ROOT\/models/$HF_HUB}" 2>/dev/null)"
      if [ -n "$a" ] && [ "$a" = "$b" ]; then
        echo "  model: $name — already the same files on this Mac (hardlinked), skipping"
        continue
      fi
    fi
    echo "  model: $name"
    rsync -a $RSYNC_PROGRESS "$m" "$HF_HUB/"
  done
fi

# LoRAs — copy from share/loras into the local farm LoRA dir
if [ -d "$FARM_ROOT/loras" ]; then
  echo "  loras -> $LORA_DIR"
  rsync -a $RSYNC_PROGRESS "$FARM_ROOT"/loras/ "$LORA_DIR/"
fi

# --- memory/OOM control files ----------------------------------------------
# Split by kind, deliberately:
#   farm.conf            CONFIG — lives on the share, edited once, read live by
#                        every worker each poll. Never overwritten if present.
#   farm_mem.sh          CODE   — must be local so a flaky SMB mount can't take
#   farm_sitecustomize/  CODE     the guards down mid-render. Newest wins.
# Net effect: tuning a limit is a one-file edit on the coordinator; shipping a
# code fix is one re-run of this script per Mac.
echo "  memory/OOM control files"
if [ ! -f "$FARM_ROOT/farm.conf" ] && [ -f ./farm.conf ]; then
  cp ./farm.conf "$FARM_ROOT/farm.conf"
  echo "    published farm.conf to the share (farm-wide limits live there now)"
elif [ -f "$FARM_ROOT/farm.conf" ]; then
  echo "    farm.conf already on the share — left alone (it is the authority)"
fi
# code: seed the share from this checkout, then take the newer of the two.
# NOTE the `|| true` on every line — this script runs under `set -e`, and a
# short-circuited `a && b` list returns non-zero, which is enough to abort the
# whole provision in some positions. Skipping a copy must never be fatal.
for f in farm_mem.sh; do
  if [ -f "./$f" ] && [ ! -f "$FARM_ROOT/$f" ]; then cp "./$f" "$FARM_ROOT/$f" || true; fi
  if [ -f "$FARM_ROOT/$f" ]; then rsync -au "$FARM_ROOT/$f" "./$f" || true; fi
done
if [ -d ./farm_sitecustomize ] && [ ! -d "$FARM_ROOT/farm_sitecustomize" ]; then
  rsync -a ./farm_sitecustomize/ "$FARM_ROOT/farm_sitecustomize/" || true
fi
if [ -d "$FARM_ROOT/farm_sitecustomize" ]; then
  rsync -au "$FARM_ROOT"/farm_sitecustomize/ ./farm_sitecustomize/ || true
fi
chmod +x ./farm_mem.sh 2>/dev/null || true

# report what THIS Mac will actually do, so a bad tier is obvious at setup time
# shellcheck disable=SC1091
if [ -f ./farm_mem.sh ]; then
  # shellcheck disable=SC1090
  [ -f "$FARM_ROOT/farm.conf" ] && . "$FARM_ROOT/farm.conf"
  . ./farm_mem.sh
  _perf="${PERF:-auto}"
  if [ "$_perf" = "auto" ]; then _perf="$(mem_auto_perf)"; fi
  echo "    this Mac: $(mem_total_gb)GB RAM · tier $(mem_tier) · profile $_perf · budget $(mem_budget_gb)GB"
  if [ "$(mem_total_gb)" -le 8 ]; then
    echo "    ⚠️  8GB or less — hero video renders will likely OOM. See docs/OOM_LIMITS.md."
  fi
fi

# sanity: is the LTX tool importable?
LTX_DIR="${LTX_DIR:-/Users/aidenwood/Desktop/00 - Aidxn/Social Video Creation/LTX2-MLX}"
if [ -x "$LTX_DIR/.venv/bin/ltx-2-mlx" ]; then
  echo "  ✅ ltx-2-mlx present"
else
  echo "  ⚠️  ltx-2-mlx not built at $LTX_DIR — run: (cd \"$LTX_DIR\" && uv sync --all-extras)"
fi
command -v mflux-generate-z-image-turbo >/dev/null 2>&1 && echo "  ✅ mflux z-image present" || echo "  ⚠️  mflux z-image not on PATH (only needed for lora_i2v jobs)"

echo "✅ provisioned. This Mac now has the same models + LoRAs as everyone. Launch start_worker.command."
read -r -t 5 _ || true
