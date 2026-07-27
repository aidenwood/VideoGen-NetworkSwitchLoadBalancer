#!/bin/bash
# ============================================================================
# SEED FARM ASSETS — run ONCE on the COORDINATOR (the Mac that already has the
# models + LoRAs). Copies everything in MANIFEST.txt onto the shared folder so
# the other Macs pull it over the gigabit switch (~minutes) instead of
# re-downloading from HuggingFace (throttled here to ~86KB/s = days).
# ============================================================================
set -euo pipefail
FARM_ROOT="${FARM_ROOT:-/Volumes/RenderFarm}"
ROOT_DIR="${ROOT_DIR:-/Users/aidenwood/Desktop/00 - Aidxn/Social Video Creation}"
HERE="$(cd "$(dirname "$0")" && pwd)"
MANIFEST="$HERE/MANIFEST.txt"
HF_HUB="$HOME/.cache/huggingface/hub"

mkdir -p "$FARM_ROOT/models" "$FARM_ROOT/loras" "$FARM_ROOT/assets"
[ -f "$MANIFEST" ] || { echo "!! no MANIFEST.txt"; exit 1; }
cp "$MANIFEST" "$FARM_ROOT/MANIFEST.txt"   # publish the manifest to the share

expand(){ echo "${1/#\~/$HOME}"; }

while read -r kind src _; do
  [ -z "${kind:-}" ] && continue
  case "$kind" in \#*) continue;; esac
  src="$(expand "$src")"; [ "${src:0:1}" = "/" ] || src="$ROOT_DIR/$src"
  case "$kind" in
    HFMODEL)
      [ -d "$src" ] || { echo "  skip (missing): $src"; continue; }
      echo "staging model: $(basename "$src")  ($(du -sh "$src" | cut -f1))"
      rsync -a --info=progress2 "$src" "$FARM_ROOT/models/" ;;
    LORA)
      [ -f "$src" ] || { echo "  skip (missing): $src"; continue; }
      echo "staging LoRA:  $(basename "$src")"
      rsync -a "$src" "$FARM_ROOT/loras/" ;;
    *) echo "  ?? unknown kind: $kind";;
  esac
done < <(sed -E 's/[[:space:]]*#.*$//' "$MANIFEST")

echo "✅ seeded -> $FARM_ROOT/{models,loras}. Now run provision.command on each worker."
