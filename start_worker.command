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
# Shared with setup.command and provision.command — see farm_root.sh.
# shellcheck disable=SC1091
. "$(cd "$(dirname "$0")" && pwd)/farm_root.sh"
ensure_farm_root || { read -r -t 30 _ || true; exit 1; }

# keep the whole worker awake + run it
exec caffeinate -ims ./farm_worker.sh
