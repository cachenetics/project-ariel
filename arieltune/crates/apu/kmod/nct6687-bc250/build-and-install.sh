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
# Supported distros: CachyOS, Alpine, Debian/Ubuntu, Arch, Fedora.
#
# Usage:
#  On the board:  ./build-and-install.sh install <path-to-nct6687.ko>
#  On a v4 host:  ./build-and-install.sh build [kernel-build-tree] [upstream-nct6687d-src]
#  Combined mode: ./build-and-install.sh all [kbuild-tree] [upstream-src]
#  Both args auto-detect when omitted; pass nothing for defaults.
set -eu
UPSTREAM=https://github.com/Fred78290/nct6687d.git
UPSTREAM_COMMIT=cd735225a95e04dda3e2befd94ba77e1f7609dcc
HERE=$(cd "$(dirname "$0")" && pwd)

# ── OS detection ─────────────────────────────────────────────────────────────
detect_os() {
		# /etc/os-release is standard on virtually all modern distros
		if [ -f /etc/os-release ]; then
			OS_ID="$(. /etc/os-release && echo "${ID:-}")" || true
		fi
		# Alpine check (also has /etc/alpine-release)
		if [ -f /etc/alpine-release ]; then
			OS_ID=alpine
		fi
		case "$OS_ID" in
			alpine)  OS_NAME=alpine  ;;
			cachyos) OS_NAME=cachyos ;;
			ubuntu|debian) OS_NAME=debian ;;
			fedora)  OS_NAME=fedora  ;;
			arch)    OS_NAME=arch    ;;
			*)       OS_NAME="$OS_ID" ;;
		esac
		if [ -z "${OS_NAME:-}" ]; then OS_NAME=generic; fi
}

# ── Privilege elevation ──────────────────────────────────────────────────────
detect_elevation() {
		# Detect sudo (most distros) or doas (Alpine, OpenBSD)
		if command -v sudo >/dev/null 2>&1; then
			SUDO_CMD=sudo
		elif command -v doas >/dev/null 2>&1; then
			SUDO_CMD=doas
		else
			echo "No privilege elevation found (need sudo or doas)" >&2
			exit 1
		fi
}

# ── Package manager ─────────────────────────────────────────────────────────
detect_pkgmgr() {
		if command -v apt >/dev/null 2>&1; then PKG_MGR=apt; fi
		if command -v apk >/dev/null 2>&1; then PKG_MGR=apk; fi
		if command -v pacman >/dev/null 2>&1; then PKG_MGR=pacman; fi
		if command -v dnf >/dev/null 2>&1; then PKG_MGR=dnf; fi
		if command -v yum >/dev/null 2>&1; then PKG_MGR=yum; fi
}

# ── Install kernel headers / kbuild tree ─────────────────────────────────────
install_headers() {
		local kver_base="${1:-}"
		# Alpine: linux-headers (kernel versioned) + build tools
		if [ "$OS_NAME" = "alpine" ]; then
			apk add --no-cache \
				"linux-headers-${kver_base}" \
				"build" \
				"gcc" \
				"make" \
				"bash" 2>/dev/null || true
		# Debian/Ubuntu
		elif [ "$OS_NAME" = "debian" ]; then
			apt install -y -q \
				"linux-headers-${kver_base}" \
				"linux-modules-${kver_base}" 2>/dev/null || true
		# Arch / CachyOS: pacman kernel headers
		elif [ "$OS_NAME" = "cachyos" ] || [ "$OS_NAME" = "arch" ]; then
			pacman -y --noconfirm linux-headers 2>/dev/null || true
		# Fedora: dnf kernel-devel
		elif [ "$OS_NAME" = "fedora" ]; then
			dnf install -y kernel-devel kernel-headers 2>/dev/null || true
		fi
}

# ── Detect kernel build tree ─────────────────────────────────────────────────
detect_kbuild_tree() {
		local kver="$(uname -r)"
		local kver_base="${kver%%-*}"
		local cands="
		/lib/modules/${kver}/build
		/lib/modules/${kver_base}/build
		/lib/modules/${kver}/source
		/lib/modules/${kver_base}/source
		/usr/src/linux-${kver}
		/usr/src/linux-${kver_base}
		/usr/src/linux
		"
		for c in $cands; do
			if [ -f "$c/Makefile" ]; then
				KBUILD_TREE="$c"
				return 0
			fi
		done
		# Try auto-installing headers if root + pkg mgr available
		if [ "$(id -u)" -eq 0 ] && [ -n "${PKG_MGR:-}" ]; then
			install_headers "$kver_base"
			if detect_kbuild_tree; then return 0; fi
		fi
		# Last resort: try installing with elevation
		if [ -n "${PKG_MGR:-}" ]; then
			"$SUDO_CMD" "$PKG_MGR" install -y "linux-headers-${kver_base}" 2>/dev/null || true
			if detect_kbuild_tree; then return 0; fi
			"$SUDO_CMD" "$PKG_MGR" install -y kernel-devel kernel-headers 2>/dev/null || true
			if detect_kbuild_tree; then return 0; fi
		fi
		return 1
}

# ── Resolve kbuild tree: arg > auto-detect > error ───────────────────────────
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
			echo "Install kernel headers for your distro or pass the path:" >&2
			echo "  $0 build /path/to/kbuild-tree" >&2
			exit 1
		fi
}

# ── Shared build logic ──────────────────────────────────────────────────────
do_build() {
		KBUILD="$KBUILD_TREE"
		SRC="${2:-/tmp/nct6687d}"
		[ -d "$SRC/.git" ] || git clone "$UPSTREAM" "$SRC"
		git -C "$SRC" checkout "$UPSTREAM_COMMIT"
		git -C "$SRC" apply "$HERE/0001-nct6687-bc250-ec-firmware-attach.patch"
		git -C "$SRC" apply "$HERE/0002-nct6687-silence-secondary-port-open-bus.patch"

		# The board's linux-headers package may ship a trimmed tree missing
		# non-x86 arch Kconfigs; stub them so syncconfig can generate autoconf.h.
		for i in $(seq 1 40); do
			local m
			m=$(make -C "$KBUILD" syncconfig 2>&1 | sed -n 's/.*can.t open file "\([^"]*\)".*/\1/p' | head -1)
			[ -z "$m" ] && break
			mkdir -p "$KBUILD/$(dirname "$m")"; : > "$KBUILD/$m"
		done

		# The board's kernel is clang-built (see include/config/CC_IS_CLANG); its
		# flags are clang-only, so pass LLVM=1. gcc-built kernels build as before.
		local LLVM_ARG=
		[ -e "$KBUILD/include/config/CC_IS_CLANG" ] && LLVM_ARG="LLVM=1"
		make -C "$KBUILD" M="$SRC" $LLVM_ARG modules
		echo "built: $SRC/nct6687.ko"
}

# ── Shared install logic ────────────────────────────────────────────────────
do_install() {
		local KO="${1:?path to prebuilt nct6687.ko}"
		local K=$(uname -r)
		"$SUDO_CMD" install -Dm644 "$KO" "/lib/modules/$K/updates/nct6687.ko"
		"$SUDO_CMD" depmod -a
		printf 'blacklist nct6683\noptions nct6687 force=true\n' | \
			"$SUDO_CMD" tee /etc/modprobe.d/bc250-nct6687.conf >/dev/null
		echo nct6687 | "$SUDO_CMD" tee /etc/modules-load.d/bc250-nct6687.conf >/dev/null
		"$SUDO_CMD" rm -f /etc/modprobe.d/nct6683.conf \
		                  /etc/modules-load.d/nct6683.conf 2>/dev/null || true
		lsmod | grep -q nct6687 && "$SUDO_CMD" rmmod nct6687 || true
		lsmod | grep -q nct6683 && "$SUDO_CMD" rmmod nct6683 || true
		"$SUDO_CMD" modprobe nct6687
		echo "installed + loaded. verify: sensors | grep -A3 nct6686"
}

# ── Auto-detect everything on startup ──────────────────────────────────────
detect_os
detect_elevation
detect_pkgmgr

# ── Main dispatch ───────────────────────────────────────────────────────────
case "${1:-}" in
build)
		resolve_kbuild_tree "${2:-}"
		do_build "$2" "$3"
		;;
all)
		resolve_kbuild_tree "${2:-}"
		SRC="${3:-/tmp/nct6687d}"
		do_build "$2" "$3"
		# install phase
		do_install "$SRC"
		;;
install)
	do_install "$2"
	;;
*)
	echo "usage: $0 build [kbuild-tree] [upstream-src] | all [kbuild-tree] [upstream-src] | install <nct6687.ko>"; exit 1
	;;
esac
