#!/bin/sh
# Build and install arieltune — the unified BC-250 tuning suite (WIKI|BIOS|APU|MEM).
#
#  ./install.sh  build (release) + install to /usr/local/bin
#  ./install.sh --with-units  also install the APU GPU/route units
#  ./install.sh --with-driver  also build+install the BIOS smiflash DKMS driver
#
# Installs ONE binary (arieltune) plus:
#  * at  short alias -> arieltune
#  * aputune / memtune / biostune / wikitune compat symlinks -> arieltune
#  (argv[0] dispatch keeps old commands + any units working)
#
# Cross-distro support (Alpine, Arch, Debian/Ubuntu, Fedora, openSUSE, etc.).
# On Alpine: uses apk + doas. On others: apt/pacman/dnf + sudo.
# POSIX / BusyBox sh compatible.
set -eu

cd "$(dirname "$0")"

PREFIX="${PREFIX:-/usr/local}"
BIN="$PREFIX/bin/arieltune"
WITH_UNITS=0
WITH_DRIVER=0
for a in "$@"; do
  case "$a" in
  --with-units)   WITH_UNITS=1 ;;
  --with-driver)  WITH_DRIVER=1 ;;
  -h|--help)     sed -n '2,12p' "$0"; exit 0 ;;
  *)  echo "unknown arg: $a" >&2; exit 2 ;;
  esac
done

# ---------------------------------------------------------------------------
#  Package-manager / superuser detection
# ---------------------------------------------------------------------------
detect_os() {
  DISTRO=""

  # Try lsb_release first (if present)
  if command -v lsb_release >/dev/null 2>&1; then
    DISTRO=$(lsb_release -si 2>/dev/null || true)
  fi

  # Fallback: /etc/os-release (Debian, Ubuntu, Fedora, Alpine, openSUSE, etc.)
  if [ -z "$DISTRO" ] && [ -f /etc/os-release ]; then
    DISTRO=$(grep '^ID=' /etc/os-release | head -1 | cut -d= -f2 | tr -d '"') || true
  fi

  # Fallback by probing package managers
  if [ -z "$DISTRO" ]; then
    if command -v apk >/dev/null 2>&1; then
      DISTRO="alpine"
    elif command -v pacman >/dev/null 2>&1; then
      DISTRO="arch"
    elif command -v dnf >/dev/null 2>&1; then
      DISTRO="fedora"
    elif command -v zypper >/dev/null 2>&1; then
      DISTRO="opensuse"
    else
      DISTRO="debian"   # default: assume Debian/Ubuntu
    fi
  fi

  echo "Detected distro: $DISTRO"
}

detect_os

# Detect the sudo-like tool to use
SUDO_CMD="sudo"
if [ "$DISTRO" = "alpine" ]; then
  if command -v doas >/dev/null 2>&1; then
    SUDO_CMD="doas"
  fi
fi

# ---------------------------------------------------------------------------
#  Alpine community repo — ensure it's enabled (needed for rust/cargo)
# ---------------------------------------------------------------------------
enable_alpine_community() {
  if [ "$DISTRO" != "alpine" ]; then
    return
  fi

  # Check if community repo is already listed
  if grep '^.*community' /etc/apk/repositories >/dev/null 2>&1; then
    echo "Alpine community repo already enabled."
    return
  fi

  echo "Enabling Alpine community repo..."

  # Read the release version (e.g. "3.20.0") and take the first two parts
  ALPINE_VER=""
  if [ -f /etc/alpine-release ]; then
    ALPINE_VER=$(head -1 /etc/alpine-release) || true
  fi
  if [ -z "$ALPINE_VER" ]; then
    ALPINE_VER="3.20"
  fi

  # Truncate to major.minor (strip patch number)
  ALPINE_MAJMIN=$(printf '%s' "$ALPINE_VER" | tr '.' ' ' | awk '{print $1"."$2}')

  echo "https://dl-cdn.alpinelinux.org/alpine/$ALPINE_MAJMIN/community" >> /etc/apk/repositories
  echo "Community repo added to /etc/apk/repositories"
}

# ---------------------------------------------------------------------------
#  Install helper — ensures the Rust toolchain is present
# ---------------------------------------------------------------------------
install_rust() {
  if command -v cargo >/dev/null 2>&1; then
    return
  fi

  echo "Rust toolchain (cargo) not found — installing it..."

  case "$DISTRO" in
  alpine)
    enable_alpine_community
    apk add --no-cache rust cargo || {
      echo "error: apk install failed. Try: apk add --no-cache rust cargo" >&2
      exit 1
    }
    ;;
  arch)
    pacman -Sy --noconfirm rust || {
      echo "error: pacman install failed. Try: sudo pacman -Sy rust" >&2
      exit 1
    }
    ;;
  debian|ubuntu)
    apt-get update -qq >/dev/null 2>&1 && \
      apt-get install -y -qq rustc cargo || {
      echo "error: apt install failed. Try: sudo apt install rustc cargo" >&2
      exit 1
    }
    ;;
  fedora)
    dnf install -y rust cargo || {
      echo "error: dnf install failed. Try: sudo dnf install rust cargo" >&2
      exit 1
    }
    ;;
  opensuse)
    zypper install -y rust cargo || {
      echo "error: zypper install failed. Try: sudo zypper install rust cargo" >&2
      exit 1
    }
    ;;
  *)
    echo "Installing Rust via rustup (universal)..."
    curl --proto "=https" --tlsv1.2 -sSf -L https://sh.rustup.rs -o rustup-init.sh
    chmod +x rustup-init.sh
    ./rustup-init.sh -y
    rm -f rustup-init.sh
    export PATH="$HOME/.cargo/bin:$PATH"
    ;;
  esac
}

# ---------------------------------------------------------------------------
#  Main: build + install
# ---------------------------------------------------------------------------
install_rust

echo "building release binary..."
cargo build --release

echo "installing $BIN..."
$SUDO_CMD install -d "$PREFIX/bin"
$SUDO_CMD install -m755 target/release/arieltune "$BIN"

echo "symlinks: at + compat (aputune/memtune/biostune/wikitune)"
$SUDO_CMD ln -sf arieltune "$PREFIX/bin/at"
for name in aputune memtune biostune wikitune; do
  $SUDO_CMD ln -sf arieltune "$PREFIX/bin/$name"
done

echo "installed: $("$BIN" --version)"

if [ "$WITH_UNITS" = 1 ]; then
  echo "APU units: pick a power mode to lay + enable the unit, e.g."
  echo "  sudo arieltune apu gpu autosleep-on"
fi
if [ "$WITH_DRIVER" = 1 ]; then
  echo "building the BIOS smiflash DKMS driver (on-board)..."
  $SUDO_CMD "$BIN" bios driver build || echo "  (driver build needs a BC-250 + kernel headers)"
fi

echo
echo "Done. Launch the TUI:  arieltune  (opens on WIKI)"
echo "  jump straight to a tab: arieltune apu  (or bios | mem | wiki)"
echo "  per-app CLI (unchanged):  arieltune apu gpu apply-boot  == aputune gpu apply-boot"
echo "  migrate an old box:  sudo arieltune migrate --apply"
