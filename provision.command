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

[ -d "$FARM_ROOT" ] || { echo "!! share not mounted at $FARM_ROOT — connect it first (smb://COORDINATOR.local/RenderFarm)"; read -r _; exit 1; }
MANIFEST="$FARM_ROOT/MANIFEST.txt"; [ -f "$MANIFEST" ] || MANIFEST="./MANIFEST.txt"

mkdir -p "$HF_HUB" "$LORA_DIR"
echo "Provisioning $(scutil --get LocalHostName 2>/dev/null || hostname -s) from $FARM_ROOT ..."

# models (HF hub cache) — copy from share/models into local HF cache
if [ -d "$FARM_ROOT/models" ]; then
  for m in "$FARM_ROOT"/models/models--*; do
    [ -e "$m" ] || continue
    echo "  model: $(basename "$m")"
    rsync -a --info=progress2 "$m" "$HF_HUB/"
  done
fi

# LoRAs — copy from share/loras into the local farm LoRA dir
if [ -d "$FARM_ROOT/loras" ]; then
  echo "  loras -> $LORA_DIR"
  rsync -a --info=progress2 "$FARM_ROOT"/loras/ "$LORA_DIR/"
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
