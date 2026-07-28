#!/bin/bash
# ============================================================================
# SEED FARM ASSETS — run ONCE on the COORDINATOR (the Mac that already has the
# models + LoRAs). Publishes everything in MANIFEST.txt to the shared folder so
# the other Macs pull it over the gigabit switch (~minutes) instead of
# re-downloading from HuggingFace (throttled here to ~86KB/s = days).
#
# ---------------------------------------------------------------------------
# IT DOES NOT MAKE A SECOND COPY (when it doesn't have to)
# ---------------------------------------------------------------------------
# The models are ~87GB. On the coordinator the share is a local folder on the
# SAME disk as the HuggingFace cache, so copying would burn 87GB duplicating
# files that are already sitting right there.
#
# When source and destination are on the same volume this HARDLINKS instead:
# the share gets its own directory entries pointing at the SAME data on disk.
# Costs kilobytes, is instant, and workers reading it over SMB can't tell the
# difference. Safe because HF blobs are content-addressed and never modified in
# place — nothing can edit one copy and corrupt the other.
#
# Across volumes (a real external/NAS share) it falls back to rsync, because
# hardlinks can't span filesystems.
#
#   ./seed_farm_assets.sh            # hardlink when possible (default)
#   ./seed_farm_assets.sh --copy     # force real copies
#   ./seed_farm_assets.sh --dry-run  # show what it would do
# ============================================================================
set -uo pipefail
FARM_ROOT="${FARM_ROOT:-/Volumes/RenderFarm}"
ROOT_DIR="${ROOT_DIR:-/Users/aidenwood/Desktop/00 - Aidxn/Social Video Creation}"
HERE="$(cd "$(dirname "$0")" && pwd)"
MANIFEST="$HERE/MANIFEST.txt"
# shellcheck disable=SC1091
. "$HERE/farm_root.sh"     # for $RSYNC_PROGRESS (macOS openrsync compatibility)

MODE="link"; DRY=0
while [ $# -gt 0 ]; do case "$1" in
  --copy)    MODE="copy"; shift;;
  --link)    MODE="link"; shift;;
  --dry-run) DRY=1; shift;;
  *) echo "unknown arg: $1"; exit 1;;
esac; done

mkdir -p "$FARM_ROOT/models" "$FARM_ROOT/loras" "$FARM_ROOT/assets"
[ -f "$MANIFEST" ] || { echo "!! no MANIFEST.txt"; exit 1; }
cp "$MANIFEST" "$FARM_ROOT/MANIFEST.txt"   # publish the manifest to the share

expand(){ echo "${1/#\~/$HOME}"; }
dev_of(){ stat -f '%d' "$1" 2>/dev/null; }          # numeric device id
free_kb(){ df -k "$1" 2>/dev/null | awk 'NR==2{print $4}'; }

SRC_DEV=""; DST_DEV="$(dev_of "$FARM_ROOT/models")"
START_FREE="$(free_kb "$FARM_ROOT")"

echo "── seeding $FARM_ROOT (mode: $MODE) ──"

# Publish one directory. Hardlink when we can, copy when we must.
stage_dir(){
  local src="$1" dest_parent="$2" base; base="$(basename "$src")"
  local dest="$dest_parent/$base"
  SRC_DEV="$(dev_of "$src")"

  local how="copy"
  if [ "$MODE" = "link" ] && [ -n "$SRC_DEV" ] && [ "$SRC_DEV" = "$DST_DEV" ]; then
    how="link"
  fi

  local size; size="$(du -sh "$src" 2>/dev/null | cut -f1)"
  if [ "$how" = "link" ]; then
    echo "  link  $base  ($size, no extra disk)"
  else
    echo "  copy  $base  ($size)"
  fi
  [ "$DRY" = "1" ] && return 0

  if [ "$how" = "link" ]; then
    # cp -al needs a clean destination or it nests the tree inside itself.
    # Removing a hardlink farm never touches the source data — the blobs just
    # drop back to one link. (Guarded so a typo'd FARM_ROOT can't nuke a home dir.)
    case "$dest" in
      "$dest_parent"/models--*|"$dest_parent"/*.safetensors) [ -e "$dest" ] && rm -rf "$dest" ;;
      *) [ -e "$dest" ] && rm -rf "$dest" ;;
    esac
    cp -al "$src" "$dest_parent/" || { echo "    !! hardlink failed — falling back to copy"; rsync -a "$src" "$dest_parent/"; }
  else
    rsync -a $RSYNC_PROGRESS "$src" "$dest_parent/"
  fi
}

while read -r kind src _; do
  [ -z "${kind:-}" ] && continue
  case "$kind" in \#*) continue;; esac
  src="$(expand "$src")"; [ "${src:0:1}" = "/" ] || src="$ROOT_DIR/$src"
  case "$kind" in
    HFMODEL)
      [ -d "$src" ] || { echo "  skip (missing): $src"; continue; }
      stage_dir "$src" "$FARM_ROOT/models" ;;
    LORA)
      [ -f "$src" ] || { echo "  skip (missing): $src"; continue; }
      echo "  lora  $(basename "$src")"
      [ "$DRY" = "1" ] || rsync -a "$src" "$FARM_ROOT/loras/" ;;   # tiny, always copy
    *) echo "  ?? unknown kind: $kind";;
  esac
done < <(sed -E 's/[[:space:]]*#.*$//' "$MANIFEST")

[ "$DRY" = "1" ] && { echo "(dry run — nothing written)"; exit 0; }

# --- verify ----------------------------------------------------------------
# HF snapshots are relative symlinks into ../../blobs. A dangling one means a
# worker pulls a model that can't load, so fail loudly here rather than there.
echo "── verifying ──"
bad=0
for d in "$FARM_ROOT"/models/models--*; do
  [ -d "$d" ] || continue
  n=$(find "$d" -type l ! -exec test -e {} \; -print 2>/dev/null | wc -l | tr -d ' ')
  if [ "$n" -gt 0 ]; then
    echo "  ⚠️  $(basename "$d"): $n dangling symlink(s) — that model is incomplete AT THE SOURCE too"
    bad=$((bad+n))
  else
    echo "  ✅ $(basename "$d")"
  fi
done

END_FREE="$(free_kb "$FARM_ROOT")"
if [ -n "$START_FREE" ] && [ -n "$END_FREE" ]; then
  used_mb=$(( (START_FREE - END_FREE) / 1024 ))
  echo "── disk used by this staging: ${used_mb}MB ──"
  [ "$MODE" = "link" ] && [ "$used_mb" -lt 500 ] && \
    echo "   (hardlinked — the share points at the same data, no duplicate)"
fi

echo "✅ seeded -> $FARM_ROOT/{models,loras}. Now run provision.command on each worker."
[ "$bad" -gt 0 ] && echo "   NOTE: $bad dangling link(s) above — re-download those models before workers pull them."
exit 0
