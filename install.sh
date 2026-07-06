#!/usr/bin/env bash
# Project Ariel — convenience installer.
#
# Forwards to arieltune/install.sh so you can build + install straight from the
# repo root. All arguments are passed through, e.g.:
#
#   ./install.sh                 build (release) + install to /usr/local/bin
#   ./install.sh --with-units    also install the APU GPU/route systemd units
#   ./install.sh --with-driver   also build the BIOS smiflash DKMS driver
#
# Needs a Rust toolchain (cargo) to build and sudo to install. See README.md.
set -euo pipefail
exec "$(dirname "$0")/arieltune/install.sh" "$@"
