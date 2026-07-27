#!/bin/bash
# ============================================================================
# ONE-SHOT SETUP — double-click on a FRESH Mac to become a render worker.
# Installs the whole toolchain, pulls the models off the share, and leaves you
# ready to run start_worker.command. Idempotent: re-run any time, it skips
# whatever's already done. No prior setup or instructions needed.
#
# What it does:
#   1. Homebrew        (if missing)
#   2. uv              (Python env manager for the LTX toolchain)
#   3. clone LTX2-MLX  + uv sync   (the video model runtime)
#   4. mflux           (z-image still generator, for cherry-pick proofs + LoRA)
#   5. mount the share + provision.command  (pull the exact models + LoRAs)
# ============================================================================
set -uo pipefail

# --- config: change these if your setup differs ---------------------------
export FARM_ROOT="${FARM_ROOT:-/Volumes/RenderFarm}"
COORDINATOR="${COORDINATOR:-COORDINATOR}"                 # the host Mac's name -> smb://<name>.local/RenderFarm
WORKDIR="${WORKDIR:-$HOME/video-gen}"                      # where the toolchain gets installed
LTX_REPO="${LTX_REPO:-https://github.com/dgrauet/ltx-2-mlx.git}"
export LTX_DIR="${LTX_DIR:-$WORKDIR/LTX2-MLX}"
export LORA_DIR="${LORA_DIR:-$HOME/farm-loras}"
HERE="$(cd "$(dirname "$0")" && pwd)"

say(){ printf "\n\033[1;36m==> %s\033[0m\n" "$*"; }
ok(){  printf "   \033[32m✓ %s\033[0m\n" "$*"; }
warn(){ printf "   \033[33m! %s\033[0m\n" "$*"; }

mkdir -p "$WORKDIR"

# 1) Homebrew ---------------------------------------------------------------
say "Homebrew"
if ! command -v brew >/dev/null 2>&1; then
  /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
fi
# make brew visible this session (Apple Silicon path)
[ -x /opt/homebrew/bin/brew ] && eval "$(/opt/homebrew/bin/brew shellenv)"
command -v brew >/dev/null 2>&1 && ok "brew ready" || { warn "brew still missing — install it manually then re-run"; exit 1; }

# 2) uv ---------------------------------------------------------------------
say "uv"
command -v uv >/dev/null 2>&1 || brew install uv
export PATH="$HOME/.local/bin:$PATH"
command -v uv >/dev/null 2>&1 && ok "uv ready" || { warn "uv install failed"; exit 1; }

# 3) LTX2-MLX runtime -------------------------------------------------------
say "LTX2-MLX video runtime"
if [ ! -d "$LTX_DIR/.git" ]; then
  git clone "$LTX_REPO" "$LTX_DIR"
fi
( cd "$LTX_DIR" && uv sync --all-extras )
[ -x "$LTX_DIR/.venv/bin/ltx-2-mlx" ] && ok "ltx-2-mlx built" || warn "ltx-2-mlx not built — check the uv sync output above"

# 4) mflux (z-image stills) -------------------------------------------------
say "mflux (z-image still generator)"
if ! command -v mflux-generate-z-image-turbo >/dev/null 2>&1; then
  uv tool install mflux || warn "mflux install failed — only needed for test proofs + LoRA jobs"
fi
command -v mflux-generate-z-image-turbo >/dev/null 2>&1 && ok "mflux ready" || warn "mflux not on PATH (test/LoRA jobs will fail until installed)"

# 5) mount the share + provision -------------------------------------------
say "shared folder"
if [ ! -d "$FARM_ROOT" ]; then
  warn "share not mounted — opening it, approve in Finder..."
  open "smb://$COORDINATOR.local/RenderFarm" || true
  printf "   press Return once it's mounted at %s ... " "$FARM_ROOT"; read -r _
fi
if [ -d "$FARM_ROOT" ]; then
  ok "share mounted"
  say "provisioning models + LoRAs from the share"
  FARM_ROOT="$FARM_ROOT" LTX_DIR="$LTX_DIR" LORA_DIR="$LORA_DIR" bash "$HERE/provision.command" || warn "provision hit an issue — see above"
else
  warn "share still not mounted — mount it, then run provision.command"
fi

# write this Mac's launcher defaults so start_worker.command just works ------
say "done"
cat <<EOF
This Mac is set up. To join the farm:
  1. Open start_worker.command, check FARM_ROOT / LTX_DIR / PERF near the top.
  2. Double-click start_worker.command.
Toolchain: $LTX_DIR   |   LoRAs: $LORA_DIR   |   Share: $FARM_ROOT
EOF
read -r -t 8 _ || true
