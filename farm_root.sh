#!/bin/bash
# ============================================================================
# ensure_farm_root — get this Mac to a usable farm folder, whatever its role.
# ----------------------------------------------------------------------------
# Sourced by start_worker.command, setup.command and provision.command.
#
# It lives in its own file because the same bug was fixed in start_worker.command
# and then shipped again in the other two: each had its own copy of "mount the
# share" logic, hardcoding smb://COORDINATOR.local and assuming /Volumes. One
# function, three callers, one place to be wrong.
#
# The rule the copies kept getting wrong:
#   coordinator — HOSTS the folder. It's a local dir (~/RenderFarm). Nothing to
#                 mount; if it doesn't exist yet, just make it.
#   worker      — MOUNTS the coordinator's folder at /Volumes/<name>.
# /Volumes is root-owned, so a coordinator pointed there can't even mkdir.
#
# Usage:  . "$(dirname "$0")/farm_root.sh"; ensure_farm_root || exit 1
# Honours: $FARM_ROOT (where), $COORDINATOR (who to mount from).
# ============================================================================

ensure_farm_root() {
  FARM_ROOT="${FARM_ROOT:-/Volumes/RenderFarm}"
  export FARM_ROOT

  [ -d "$FARM_ROOT" ] && { echo "farm: $FARM_ROOT"; return 0; }

  case "$FARM_ROOT" in
    /Volumes/*)
      # Worker: this is a mount point, so something has to be mounted onto it.
      local host share
      host="$(printf '%s' "${COORDINATOR:-}" | tr -cd 'A-Za-z0-9._-')"
      share="$(basename "$FARM_ROOT")"
      if [ -z "$host" ] || [ "$host" = "COORDINATOR" ]; then
        cat <<EOF
!! Don't know which Mac to connect to.

   FARM_ROOT is $FARM_ROOT but COORDINATOR isn't set, so there's no server to
   mount. Fix it in ONE of these ways:

     * Open the LTX Mac Farm app -> Setup. It finds the coordinator over
       Bonjour and you pick it from a list — then use the buttons there.
     * Or set the name yourself:
         COORDINATOR=<mac-name> "\$0"

   If this Mac IS the coordinator, FARM_ROOT should be its own folder
   (e.g. \$HOME/$share), NOT a path under /Volumes — /Volumes is where OTHER
   Macs' shares get mounted, and it isn't writable by you.
EOF
        return 1
      fi
      echo "Share not mounted — connecting to smb://$host.local/$share ..."
      open "smb://$host.local/$share" 2>/dev/null || true
      printf "Approve the mount in Finder, then press Return here to continue... "
      read -r _
      ;;
    *)
      # Coordinator: a local folder it owns. Make it rather than failing.
      echo "Creating farm folder $FARM_ROOT ..."
      mkdir -p "$FARM_ROOT" || { echo "!! Couldn't create $FARM_ROOT"; return 1; }
      ;;
  esac

  [ -d "$FARM_ROOT" ] || { echo "!! Still no farm folder at $FARM_ROOT"; return 1; }
  echo "farm: $FARM_ROOT"
  return 0
}

# ---------------------------------------------------------------------------
# rsync progress flag — macOS 15+ ships OPENRSYNC ("rsync 2.6.9 compatible"),
# which does NOT understand --info=progress2. Using it there doesn't degrade,
# it hard-fails with a usage dump, so provisioning died on every worker. GNU
# rsync 3.x (brew) does support it. Detect once, use everywhere.
# ---------------------------------------------------------------------------
if rsync --info=progress2 --version >/dev/null 2>&1; then
  RSYNC_PROGRESS="--info=progress2"
else
  RSYNC_PROGRESS="--progress"
fi
export RSYNC_PROGRESS
