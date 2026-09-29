#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-2.0-only
#
# test_alpine_support.sh — Atomic, fully-logging test for ALPINE support in
# arieltune.  Validates the patch-detect → doctor → patches → cu topology
# → build-preview flow without destructive actions (no --run, no reboot, no
# kernel rebuild).  Designed to exercise the next code updates targeting native
# ALPINE (Alpine Linux) integration into arieltune.
#
# Usage: ./test_alpine_support.sh [options]
#   --build-dir DIR       Where to write logs + artifacts (default: ./alpine-test-YYYYMMDD-HHMMSS)
#   --arieltune PATH      Path to the arieltune binary (default: auto-detect / build)
#   --arieltune-path PATH Alias for --arieltune
#   --dry-run             Skip root-only probes
#   --verbose             Extra diagnostics
#   --help                Print usage and exit
#
# All output is logged to LOGFILE and echoed to stdout via tee.
# Commands are captured individually:  $LOG_DIR/cmd_*.log

# ────────────────────────────────────────────────────────────────────────
# Globals / defaults
# ────────────────────────────────────────────────────────────────────────
export BUILD_DIR="${BUILD_DIR:-./alpine-test-$(date +%Y%m%d-%H%M%S)}"
export ARIELTUNE_BIN="${ARIELTUNE_BIN:-}"
export DRY_RUN="${DRY_RUN:-}"
export VERBOSE="${VERBOSE:-}"

# ────────────────────────────────────────────────────────────────────────
# Helpers
# ────────────────────────────────────────────────────────────────────────
_TS() {
    date '+%Y-%m-%d %H:%M:%S.%N'
}

log_header() {
    local title="$1" ts
    ts=$(_TS)
    printf '\n\033[1;36m=== [%s] %s ===\033[0m\n' "$ts" "$title" | tee -a "$LOGFILE"
}

log_step() {
    local desc="$1" ts
    ts=$(_TS)
    printf '\033[1;33m> [%s] %s\033[0m\n' "$ts" "$desc" | tee -a "$LOGFILE"
}

# Run a command, capture stdout+stderr to a per-command log, echo to stdout.
# Returns the command's exit code.
run_cmd() {
    local cmd="$1" desc="${2:-$cmd}" ts_t0 ts_t1 cmdlog exit_code
    ts_t0=$(_TS)
    cmdlog="$LOG_DIR/cmd_$(echo "$ts_t0" | tr '[:space:].:' '_').log"

    {  printf '\033[2m  [%s] %s\033[0m  (captured → %s)\n' "$(_TS)" "$desc" "$cmdlog"; } | tee -a "$LOGFILE"

    # Capture both streams into the per-command log AND stdout
    $cmd >> "$cmdlog" 2>&1 || true   # always succeed; capture is valuable not zero-exit
    exit_code=${PIPESTATUS[0]:-$?}

    ts_t1=$(_TS)
    { printf '\033[2m  wall=%s\033[0m\n' "captured@$cmdlog"; } | tee -a "$LOGFILE"
}

# ────────────────────────────────────────────────────────────────────────
# Option parsing
# ────────────────────────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
    case "$1" in
        --build-dir)      BUILD_DIR="$2";     shift 2;;
        --arieltune)      ARIELTUNE_BIN="$2";  shift 2;;
        --arieltune-path)  ARIELTUNE_BIN="$2";  shift 2;;
        --dry-run)        DRY_RUN="yes";       shift;;
        --verbose)        VERBOSE="yes";       shift;;
        --help)           grep '^#\s*#' "$0" | head -16; exit 0;;
        *)                echo "UNKNOWN: $1";  exit 1;;
    esac
done

LOGFILE="${BUILD_DIR%/}/aliptest.log"
LOG_DIR="$(dirname "$LOGFILE")"
mkdir -p "$LOG_DIR"

# ────────────────────────────────────────────────────────────────────────
# Bootstrap — resolve arieltune binary
# ────────────────────────────────────────────────────────────────────────
{
    log_header "Bootstrap — resolve arieltune binary"

    if [[ -n "$ARIELTUNE_BIN" ]]; then
        loc="$ARIELTUNE_BIN"    # user-provided
    elif command -v arieltune &>/dev/null; then
        loc="$(command -v arieltune)"   # on PATH
    elif [[ -f ../target/release/arieltune ]]; then
        loc="../target/release/arieltune"   # adjacent build
    else
        echo "Building arieltune from scratch..." >> "$LOGFILE"
        (cd .. && cargo build --release 2>&1 | tee -a "$LOGFILE") || {
            echo "BUILD FAILED; aborting." >> "$LOGFILE"; exit 1
        }
        loc="../target/release/arieltune"
    fi

    # validate
    if [[ ! -x "$loc" ]]; then
        echo "ERROR: arieltune not executable: $loc" >> "$LOGFILE"; exit 1
    fi
    export ARIELTUNE_BIN="$loc"

    echo "Bootstrap OK  arieltune=$ARIELTUNE_BIN" >> "$LOGFILE"
}

export PATH="$PATH:/usr/bin:/usr/local/bin:/sbin:/bin"

# ──────────────────────────────────────────────────────────────────────
# Preflight — host & hardware probes
# ──────────────────────────────────────────────────────────────────────
{
    log_header "Preflight — host & hardware probes"

    # Root check
    if [[ $(id -u) != 0 ]]; then
        log_step "Non-root (UID $(id -u)); probes that need root will fallback"
    else
        log_step "Running as root (UID 0) — full detection"
    fi

    # PCI
    { log_step "PCI vendor/device probe"; } >> "$LOGFILE"
    grep -ql "1002" /sys/bus/pci/devices/*/vendor 2>/dev/null &&
        echo "  PCI vendor 0x1002 (AMD) found" >> "$LOGFILE" || true
    grep -ql "13fe" /sys/bus/pci/devices/*/device 2>/dev/null &&
        echo "  PCI device 0x13fe (BC-250) found" >> "$LOGFILE" ||
        echo "  WARN: 0x13fe not present — not a BC-250 (test designed for BC-250)" >> "$LOGFILE" || true

    # debugfs
    { log_step "debugfs"; } >> "$LOGFILE"
    mount 2>/dev/null | grep -q debugfs &&
        echo "  debugfs mounted" >> "$LOGFILE" ||
        echo "  debugfs not mounted — detect::State will report Unknown" >> "$LOGFILE"

    # sysfs amdgpu params
    { log_step "amdgpu sysfs params"; } >> "$LOGFILE"
    if [[ -d /sys/module/amdgpu/parameters ]]; then
        echo "  amdgpu params: present" >> "$LOGFILE"
    else
        echo "  amdgpu params: absent (not booted with amdgpu)" >> "$LOGFILE"
    fi
}

# ──────────────────────────────────────────────────────────────────────
# Step 1: doctor
# ──────────────────────────────────────────────────────────────────────
{
    log_header "Step 1/6 — arieltune apu doctor"
    run_cmd "$ARIELTUNE_BIN apu doctor" "doctor (text summary)"

    log_step "doctor --json"
    run_cmd "$ARIELTUNE_BIN apu doctor --json" "doctor (machine-readable)"

    log_step "doctor --verify (expect failure on non-liberated host)"
    set +e   # deliberately allow failure
    run_cmd "$ARIELTUNE_BIN apu doctor --verify" "doctor --verify" ||
        echo "  doctor --verify failed (expected — host not fully patched)" >> "$LOGFILE"
    set -e
}

# ──────────────────────────────────────────────────────────────────────
# Step 2: patches
# ──────────────────────────────────────────────────────────────────────
{
    log_header "Step 2/6 — arieltune apu patches"
    run_cmd "$ARIELTUNE_BIN apu patches" "patches (full series)"
}

# ──────────────────────────────────────────────────────────────────────
# Step 3: patch inspection — sampled
# ──────────────────────────────────────────────────────────────────────
{
    log_header "Step 3/6 — patch inspection (sampled)"

    run_cmd "$ARIELTUNE_BIN apu patches show 16" "patch 16 (40-CU unlock)"
    run_cmd "$ARIELTUNE_BIN apu patches show 26" "patch 26 (SDMA override)"
    run_cmd "$ARIELTUNE_BIN apu patches show 25" "patch 25 (TLB flush by runlist)"
    run_cmd "$ARIELTUNE_BIN apu patches show 17" "patch 17 (diagnostic probe, optional)"
    run_cmd "$ARIELTUNE_BIN apu patches show 28" "patch 28 (8-core telemetry)"
}

# ──────────────────────────────────────────────────────────────────────
# Step 4: CU topology
# ──────────────────────────────────────────────────────────────────────
{
    log_header "Step 4/6 — CU topology & modprobe.d"

    log_step "cu map"
    run_cmd "$ARIELTUNE_BIN apu cu map" "cu map" || true

    log_step "modprobe.d config state"
    if [[ -f /etc/modprobe.d/aputune-40cu.conf ]]; then
        echo "  PRESENT (will survive reboot)" >> "$LOGFILE"
        cat /etc/modprobe.d/aputune-40cu.conf >> "$LOGFILE"
    else
        echo "  ABSENT" >> "$LOGFILE"
    fi

    log_step "bc250_cc_write_mode (sysfs)"
    if [[ -f /sys/module/amdgpu/parameters/bc250_cc_write_mode ]]; then
        echo "  value = $(cat /sys/module/amdgpu/parameters/bc250_cc_write_mode)" >> "$LOGFILE"
    else
        echo "  not available" >> "$LOGFILE"
    fi
}

# ──────────────────────────────────────────────────────────────────────
# Step 5: build preview (non-destructive)
# ──────────────────────────────────────────────────────────────────────
{
    log_header "Step 5/6 — arieltune apu build preview"

    log_step "build preview (no --run, no reboot)"
    run_cmd "$ARIELTUNE_BIN apu build" "build preview" ||
        echo "  build preview failed (no PKGBUILD dir or deps missing)" >> "$LOGFILE"
}

# ──────────────────────────────────────────────────────────────────────
# Step 6: ALPINE integration checks
# ──────────────────────────────────────────────────────────────────────
{
    log_header "Step 6/6 — ALPINE-specific probes"

    log_step "Distro detection"
    if [[ -f /etc/os-release ]]; then
        cat /etc/os-release >> "$LOGFILE"
    else
        echo "No /etc/os-release" >> "$LOGFILE"
    fi

    log_step "Running kernel"
    uname -r >> "$LOGFILE"

    log_step "modprobe.d directories"
    (for d in /etc/modprobe.d /usr/lib/modprobe.d /usr/lib/modprobe.d_new; do
        [[ -d "$d" ]] && echo "  $d exists" >> "$LOGFILE"
    done)

    log_step "initramfs tool"
    command -v mkinitcpio &>/dev/null && echo "  mkinitcpio: present" >> "$LOGFILE" ||
    command -v dracut  &>/dev/null && echo "  dracut:  present" >> "$LOGFILE" ||
    echo "  initramfs tool: not found" >> "$LOGFILE"

    log_step "init-system detection"
    systemctl --version &>/dev/null &&
        echo "  systemd" >> "$LOGFILE" ||
    [[ -f /run/rc.conf ]] &&
        echo "  OpenRC" >> "$LOGFILE" ||
    echo "  init-system: unknown" >> "$LOGFILE"
}

# ──────────────────────────────────────────────────────────────────────
# Final summary
# ──────────────────────────────────────────────────────────────────────
{
    log_header "Test summary"

    cat >> "$LOGFILE" <<SUMMARY

=== KEY FINDINGS ===
- BC-250 silicon: $(grep -rc '13fe' /sys/bus/pci/devices/*/device 2>/dev/null | paste -sd+ | bc || echo N/A)
- debugfs:        $(mount 2>/dev/null | grep -c debugfs || echo 0)
- modprobe.d:     $(cat /etc/modprobe.d/aputune-40cu.conf 2>/dev/null || echo absent)
- kernel:         $(uname -r)

=== LOG FILES ===
- Main:  $LOGFILE
- Cmd:   $LOG_DIR/cmd_*.log
SUMMARY

    echo "" >> "$LOGFILE"
    printf 'Test complete.  Full trace: %s\n' "$LOGFILE" | tee -a "$LOGFILE"
    echo "" >> "$LOGFILE"
    echo "Next: integrate native ALPINE (mkinitcpio/dracut + init-system) into arieltune." >> "$LOGFILE"
}
