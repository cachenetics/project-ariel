#!/bin/sh
# Project Ariel — convenience installer (Alpine, Arch, Debian/Ubuntu, Fedora, etc.).
#
# Forwards to arieltune/install.sh so you can build + install straight from the
# repo root. All arguments are passed through, e.g.:
#
#  ./install.sh  build (release) + install to /usr/local/bin
#  ./install.sh --with-units  also install the APU GPU/route units
#  ./install.sh --with-driver  also build the BIOS smiflash DKMS driver
#
# On Alpine Linux: uses apk + doas (no sudo required). Automatically enables
# the community repo if needed for rust/cargo packages.
# Needs a Rust toolchain (cargo) to build and sudo/doas to install. See README.md.

# Check root privileges — prompt to re-exec with sudo/doas if needed
if [ "$(id -u)" -ne 0 ]; then
	if command -v doas >/dev/null 2>&1; then
		exec doas "$0" "$@"
	elif command -v sudo >/dev/null 2>&1; then
		exec sudo "$0" "$@"
	else
		echo "error: root privileges required but neither sudo nor doas is installed" >&2
		exit 1
	fi
fi

set -euo pipefail
exec "$(dirname "$0")/arieltune/install.sh" "$@"
