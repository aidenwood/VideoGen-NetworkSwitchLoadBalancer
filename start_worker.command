#!/bin/bash
# ============================================================================
# DOUBLE-CLICK ME on each Mac to join the render farm.
# ----------------------------------------------------------------------------
# Usually there is NOTHING to edit here. The app passes this Mac's real config
# in as environment variables, and every default below defers to it — note the
# `${VAR:-default}` form. An earlier version assigned these unconditionally,
# which silently overrode the app and sent the coordinator looking for its own
# folder at /Volumes/RenderFarm.
#
#  * FARM_ROOT  = the farm folder ON THIS MAC.
#                   coordinator -> a local dir it hosts, e.g. ~/RenderFarm
#                   worker      -> the mounted share,    e.g. /Volumes/RenderFarm
#  * COORDINATOR = the coordinator Mac's name, for workers that must mount.
#  * LTX_DIR    = this Mac's local LTX2-MLX checkout.
# ============================================================================

# --- config: env (from the app) wins, these are only fallbacks -------------
export FARM_ROOT="${FARM_ROOT:-/Volumes/RenderFarm}"
export LORA_DIR="${LORA_DIR:-$HOME/farm-loras}"
export COORDINATOR="${COORDINATOR:-}"

# setup.command installs to ~/video-gen/LTX2-MLX; the older hand-built checkout
# lived beside Social Video Creation. Prefer whichever actually exists.
if [ -z "${LTX_DIR:-}" ]; then
  for _c in "$HOME/video-gen/LTX2-MLX" \
            "$HOME/Desktop/00 - Aidxn/Social Video Creation/LTX2-MLX"; do
    [ -d "$_c" ] && { LTX_DIR="$_c"; break; }
  done
  LTX_DIR="${LTX_DIR:-$HOME/video-gen/LTX2-MLX}"
fi
export LTX_DIR
#
# PERFORMANCE PROFILE for THIS Mac:
#   auto  = decide from this Mac's installed RAM (the default, and correct on
#           every machine — 64GB+ gets 'full', anything smaller gets 'light').
#   full  = fastest, uses everything. Only on Macs DEDICATED to rendering.
#   light = capped low so you can keep using the Mac. Daily-driver Macs.
#
# Leave this commented out unless this ONE Mac needs to differ. Uncommenting it
# makes an env var, which beats the farm-wide $FARM_ROOT/farm.conf — so you'd
# have to come back to this machine to change it again. Prefer editing
# farm.conf on the coordinator, or dropping a farm.conf.<hostname> beside it.
# export PERF="light"
# ---------------------------------------------------------------------------

cd "$(dirname "$0")"

# --- get to a usable FARM_ROOT ---------------------------------------------
# The coordinator HOSTS the folder, so there is nothing to mount — mounting is
# a worker-only step. Deciding that by "is it already a directory?" also means
# a worker with the share already mounted skips straight through.
if [ ! -d "$FARM_ROOT" ]; then
  case "$FARM_ROOT" in
    /Volumes/*)
      host="$(printf '%s' "$COORDINATOR" | tr -cd 'A-Za-z0-9._-')"
      share="$(basename "$FARM_ROOT")"
      if [ -z "$host" ] || [ "$host" = "COORDINATOR" ]; then
        cat <<EOF
!! Don't know which Mac to connect to.

   FARM_ROOT is $FARM_ROOT but COORDINATOR isn't set, so there's no server to
   mount. Fix it in ONE of these ways:

     * Open the LTX Mac Farm app -> Setup, and pick the coordinator from the
       list (it finds them automatically). Then press "Start worker" there.
     * Or run this with the name set:
         COORDINATOR=<mac-name> "$0"

   If this Mac IS the coordinator, FARM_ROOT should be its own folder
   (e.g. \$HOME/$share), not a path under /Volumes.
EOF
        read -r -t 30 _ || true
        exit 1
      fi
      echo "Share not mounted — connecting to smb://$host.local/$share ..."
      open "smb://$host.local/$share"
      echo "Approve the mount in Finder, then press Return here to continue..."
      read -r _
      ;;
    *)
      # A local path that doesn't exist yet: this is the coordinator's own
      # folder, so just make it rather than failing.
      echo "Creating farm folder $FARM_ROOT ..."
      mkdir -p "$FARM_ROOT" || {
        echo "!! Couldn't create $FARM_ROOT"; read -r -t 30 _ || true; exit 1; }
      ;;
  esac
fi

[ -d "$FARM_ROOT" ] || {
  echo "!! Still no farm folder at $FARM_ROOT — nothing to do."
  read -r -t 30 _ || true; exit 1; }

echo "farm: $FARM_ROOT"

# keep the whole worker awake + run it
exec caffeinate -ims ./farm_worker.sh
