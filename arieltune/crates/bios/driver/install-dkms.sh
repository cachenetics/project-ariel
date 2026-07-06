#!/usr/bin/env bash
# Register + build + install the smiflash module via DKMS, so it is rebuilt
# automatically on every kernel upgrade. Run as root (or it will use sudo).
#
# Requires: dkms, kernel headers for the running kernel, gcc/make.
# On a BC-250 the module builds ON THE BOARD (prepare.sh handles the stripped
# CachyOS headers) — no cross-build host needed.
set -euo pipefail

VER="0.1.0"
NAME="smiflash"
SRC="/usr/src/${NAME}-${VER}"
HERE="$(cd "$(dirname "$0")" && pwd)"

SUDO=""
[ "$(id -u)" -ne 0 ] && SUDO="sudo"

command -v dkms >/dev/null || {
	echo "dkms not found — install it (e.g. 'pacman -S dkms' / 'apt install dkms') and re-run." >&2
	echo "Alternatively build once with: make -C '$HERE' && sudo insmod '$HERE/smiflash.ko' smi_port=0xB0" >&2
	exit 1
}

echo "== staging $SRC =="
$SUDO rm -rf "$SRC"
$SUDO install -d "$SRC"
$SUDO install -m644 "$HERE/smiflash.c" "$HERE/Makefile" "$HERE/dkms.conf" "$SRC/"
$SUDO install -m755 "$HERE/prepare.sh" "$SRC/"

echo "== dkms add/build/install =="
$SUDO dkms remove "${NAME}/${VER}" --all 2>/dev/null || true
$SUDO dkms add "${NAME}/${VER}"
$SUDO dkms build "${NAME}/${VER}"
$SUDO dkms install "${NAME}/${VER}"

echo
dkms status "$NAME"
echo
echo "installed. Load it with:  sudo arieltune bios driver load"
echo "(DKMS will rebuild it automatically when you upgrade your kernel.)"
