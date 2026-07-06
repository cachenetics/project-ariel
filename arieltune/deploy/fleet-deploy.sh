#!/usr/bin/env bash
# Deploy the arieltune suite to one (or many) BC-250 nodes.
#
#   ./deploy/fleet-deploy.sh user@host [user@host ...]
#   ./deploy/fleet-deploy.sh --hosts fleet.txt        # one user@host per line (# comments ok)
#
# Build ON this host (same arch/glibc as the target), then per node:
#   * scp the ONE release binary
#   * install it to /usr/local/bin/arieltune + the `at` alias + the four compat
#     symlinks (aputune/memtune/biostune/wikitune -> arieltune)
#   * run `arieltune migrate --apply` (disable-before-enable: stops/disables any
#     legacy aputune-* GPU units so the SMU never has two clock writers)
#   * set the GPU power setpoints + enable app-driven autosleep as the mode
#
# Because the suite is ONE binary, this is a single scp per node (was four tools).
# REFUSES a node that isn't a BC-250 (PCI 1002:13fe).
#
# Needs: a release binary (`cargo build --release`), SSH + sudo on each node.
set -euo pipefail
cd "$(dirname "$0")/.."

TOP=${ARIELTUNE_TOP_MHZ:-2230}
DEEP=${ARIELTUNE_DEEP_MHZ:-350}
HOSTS=()
while [ "$#" -gt 0 ]; do
  case "$1" in
    --hosts) mapfile -t HOSTS < <(grep -vE '^\s*#|^\s*$' "$2"); shift 2 ;;
    --top) TOP="$2"; shift 2 ;;
    --deep) DEEP="$2"; shift 2 ;;
    -h|--help) sed -n '2,18p' "$0"; exit 0 ;;
    *) HOSTS+=("$1"); shift ;;
  esac
done
[ "${#HOSTS[@]}" -gt 0 ] || { echo "no hosts given (see --help)" >&2; exit 2; }

BIN=target/release/arieltune
[ -x "$BIN" ] || { echo "build first: cargo build --release" >&2; exit 1; }

install_compat() {
  # $1 = target. Install binary + at + compat symlinks.
  ssh "$1" 'sudo install -m755 /tmp/arieltune /usr/local/bin/arieltune \
    && sudo ln -sf arieltune /usr/local/bin/at \
    && for n in aputune memtune biostune wikitune; do sudo ln -sf arieltune /usr/local/bin/$n; done'
}

deploy_one() {
  local t="$1"
  echo "==> $t"
  if ! ssh -o ConnectTimeout=8 "$t" 'grep -qi 0x13fe /sys/bus/pci/devices/*/device 2>/dev/null'; then
    echo "   SKIP: not a BC-250 (PCI 1002:13fe not found)"; return 1
  fi
  scp -q "$BIN" "$t":/tmp/arieltune
  install_compat "$t"
  # disable-before-enable: retire any legacy aputune-* GPU units first.
  ssh "$t" 'sudo /usr/local/bin/arieltune migrate --apply'
  ssh "$t" "sudo /usr/local/bin/arieltune apu gpu set-top $TOP \
    && sudo /usr/local/bin/arieltune apu gpu set-deep $DEEP"
  # NEVER clobber a persisted manual clock pin (e.g. a heat-safety 350 MHz pin):
  # autosleep-on clears force_mhz, so skip it when a pin is saved.
  if ssh "$t" 'sudo grep -q "\"force_mhz\": [0-9]" /var/lib/aputune/power.json 2>/dev/null'; then
    echo "   force_mhz pin present -- leaving it; not enabling autosleep."
  else
    ssh "$t" 'sudo /usr/local/bin/arieltune apu gpu autosleep-on'
  fi
  echo "   [ok] $t on the suite ($(ssh "$t" '/usr/local/bin/arieltune --version'))"
}

rc=0
for h in "${HOSTS[@]}"; do deploy_one "$h" || rc=1; done
exit $rc
