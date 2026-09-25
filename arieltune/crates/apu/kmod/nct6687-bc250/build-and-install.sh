#!/usr/bin/env sh
# Build the BC-250-patched nct6687 fan-control driver and install it on a
# BC-250 carrier board. Writable PWM fan control (the in-kernel nct6683 is
# read-only; the BC-250 EC ignores its FAN_CFG-handshake writes).
#
# The BC-250 CPU is x86-64-v3 but CachyOS host build tools are x86-64-v4, so
# the kernel module CANNOT be built on the board itself (fixdep aborts with
# "CPU ISA level is lower than required"). Build on an x86-64-v4 host (a modern
# x86-64 build host) against the board's kernel build tree, then copy the .ko over.
#
# Usage:
#   On the board:   ./build-and-install.sh install <path-to-nct6687.ko>
#   On a v4 host:   ./build-and-install.sh build <kernel-build-tree> <upstream-nct6687d-src>
set -eu
UPSTREAM=https://github.com/Fred78290/nct6687d.git
UPSTREAM_COMMIT=cd735225a95e04dda3e2befd94ba77e1f7609dcc
HERE=$(cd "$(dirname "$0")" && pwd)

# Detect kernel build tree — checks common paths across distros.
# Sets KBUILD_TREE to the first valid tree found.
detect_kbuild_tree() {
  local kver="$(uname -r)"
  local cands="
    /lib/modules/${kver}/build
    /lib/modules/${kver}/source
    /usr/src/linux-${kver}
    /usr/src/linux
  "
  for c in $cands; do
    if [ -f "$c/Makefile" ]; then
      KBUILD_TREE="$c"
      return 0
    fi
  done
  return 1
}

# Resolve KBUILD_TREE: user-passed arg > auto-detected > error.
resolve_kbuild_tree() {
  if [ -n "${1:-}" ]; then
    KBUILD_TREE="$1"
  elif detect_kbuild_tree; then
    : # found above
  else
    echo "Error: no kernel build tree found." >&2
    echo "Expected at one of:" >&2
    echo "  /lib/modules/$(uname -r)/build" >&2
    echo "  /lib/modules/$(uname -r)/source" >&2
    echo "  /usr/src/linux-$(uname -r)" >&2
    echo "  /usr/src/linux" >&2
   echo "Install kernel headers (e.g. apk add linux-headers) or pass the path:" >&2
   echo "  $0 build /path/to/kbuild-tree" >&2
  exit 1
  fi
}

# Detect privilege elevation: sudo (most distros) or doas (Alpine)
if command -v sudo >/dev/null 2>&1; then
    SUDO_CMD=sudo
elif command -v doas >/dev/null 2>&1; then
    SUDO_CMD=doas
else
    echo "No privilege elevation found (need sudo or doas)" >&2
    exit 1
fi

case "${1:-}" in
build)
  resolve_kbuild_tree "${2:-}"
  KBUILD="$KBUILD_TREE"
  SRC=${3:-/tmp/nct6687d}
  [ -d "$SRC/.git" ] || git clone "$UPSTREAM" "$SRC"
  git -C "$SRC" checkout "$UPSTREAM_COMMIT"
  git -C "$SRC" apply "$HERE/0001-nct6687-bc250-ec-firmware-attach.patch"
  git -C "$SRC" apply "$HERE/0002-nct6687-silence-secondary-port-open-bus.patch"
  # The board's linux-headers package may ship a trimmed tree missing
  # non-x86 arch Kconfigs; stub them so syncconfig can generate autoconf.h.
  for i in $(seq 1 40); do
    m=$(make -C "$KBUILD" syncconfig 2>&1 | sed -n 's/.*can.t open file "\([^"]*\)".*/\1/p' | head -1)
    [ -z "$m" ] && break
    mkdir -p "$KBUILD/$(dirname "$m")"; : > "$KBUILD/$m"
  done
  # The board's kernel is clang-built (see include/config/CC_IS_CLANG); its
  # flags are clang-only, so pass LLVM=1. gcc-built kernels build as before.
  LLVM_ARG=
  [ -e "$KBUILD/include/config/CC_IS_CLANG" ] && LLVM_ARG="LLVM=1"
  make -C "$KBUILD" M="$SRC" $LLVM_ARG modules
  echo "built: $SRC/nct6687.ko  (copy to the board and run: $0 install nct6687.ko)"
  ;;
install)
  KO=${2:?path to prebuilt nct6687.ko}
  K=$(uname -r)
  "$SUDO_CMD" install -Dm644 "$KO" "/lib/modules/$K/updates/nct6687.ko"
  "$SUDO_CMD" depmod -a
  printf 'blacklist nct6683\noptions nct6687 force=true\n' | "$SUDO_CMD" tee /etc/modprobe.d/bc250-nct6687.conf >/dev/null
  echo nct6687 | "$SUDO_CMD" tee /etc/modules-load.d/bc250-nct6687.conf >/dev/null
  "$SUDO_CMD" rm -f /etc/modprobe.d/nct6683.conf /etc/modules-load.d/nct6683.conf 2>/dev/null || true
  lsmod | grep -q nct6687 && "$SUDO_CMD" rmmod nct6687 || true
  lsmod | grep -q nct6683 && "$SUDO_CMD" rmmod nct6683 || true
  "$SUDO_CMD" modprobe nct6687
  echo "installed + loaded. verify: sensors | grep -A3 nct6686"
  ;;
*)
  echo "usage: $0 build [kbuild-tree] [upstream-src] | install <nct6687.ko>"; exit 1;;
esac
