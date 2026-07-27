#!/bin/bash
# ============================================================================
# DOUBLE-CLICK ME on each Mac to join the render farm.
# ----------------------------------------------------------------------------
# Edit the two lines below if your setup differs, then just double-click.
#  * FARM_ROOT  = where the coordinator's shared folder is mounted on THIS Mac.
#  * LTX_DIR    = this Mac's local LTX2-MLX checkout.
# ============================================================================

# --- EDIT THESE IF NEEDED --------------------------------------------------
export FARM_ROOT="/Volumes/RenderFarm"
export LTX_DIR="/Users/aidenwood/Desktop/00 - Aidxn/Social Video Creation/LTX2-MLX"
export LORA_DIR="$HOME/farm-loras"          # where provision.command put the LoRAs
#
# PERFORMANCE PROFILE for THIS Mac:
#   full  = fastest, uses everything. Set this on Macs DEDICATED to rendering.
#   light = capped low so you can keep using the Mac. Set on daily-driver Macs.
export PERF="full"
# ---------------------------------------------------------------------------

cd "$(dirname "$0")"

# auto-mount the share if it isn't already (edit smb host/user if you like)
if [ ! -d "$FARM_ROOT" ]; then
  echo "Share not mounted — attempting to mount the coordinator..."
  open "smb://COORDINATOR.local/RenderFarm"   # <- change COORDINATOR to the host Mac's name
  echo "Approve the mount in Finder, then press Return here to continue..."
  read -r _
fi

# keep the whole worker awake + run it
exec caffeinate -ims ./farm_worker.sh
