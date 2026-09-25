#!/bin/sh
# bc250-enable-40cu-alpine.sh — Build and install patched amdgpu for 40 CU on Alpine Linux
#
# Usage:
#	 doas ./bc250-enable-40cu-alpine.sh build  # patch + compile + install
#	 doas ./bc250-enable-40cu-alpine.sh enable # set 40 CU mode and reboot
#	 doas ./bc250-enable-40cu-alpine.sh disable # return to stock 24 CU and reboot
#	 doas ./bc250-enable-40cu-alpine.sh status # show current CU state
#	 doas ./bc250-enable-40cu-alpine.sh restore # restore original amdgpu module

set -e

VERBOSE=0

KVER="$(uname -r)"
CLEAN_KVER="${KVER%%-*}"
MODDIR="/lib/modules/${KVER}"
MODPATH="${MODDIR}/kernel/drivers/gpu/drm/amd/amdgpu/amdgpu.ko"
BUILDDIR="/tmp/bc250-40cu-build"
CONF40="/etc/modprobe.d/bc250-40cu.conf"
BACKUP_SUFFIX=".bc250-backup-$(date +%Y%m%d)"
BC250_PCI_ID="13fe"
BUILDLOG="/tmp/bc250-40cu-build.log"

info()  { printf '\033[0;32m[+]\033[0m %s\n' "$*" >&2; }
warn()  { printf '\033[0;33m[!]\033[0m %s\n' "$*" >&2; }
err()	{ printf '\033[0;31m[E]\033[0m %s\n' "$*" >&2; }
die() 	{ err "$@"; exit 1; }

check_bc250() {
	detected=0
	if vulkaninfo --summary 2>/dev/null | grep -qi "${BC250_PCI_ID}"; then
		detected=1
	elif grep -qr "0x${BC250_PCI_ID}" /sys/class/drm/card*/device/device 2>/dev/null; then
		detected=1
	fi

	if [ "$detected" -eq 0 ]; then
		warn "No BC-250 (PCI ID ${BC250_PCI_ID}) detected."
		printf "Continue anyway? [y/N] " >&2
		read -r ans
		case "$ans" in y|Y) ;; *) exit 1 ;; esac
	else
		info "BC-250 (PCI ID ${BC250_PCI_ID}) successfully verified."
	fi
}

check_deps() {
	pkgs_to_install=""
	command -v gcc >/dev/null 2>&1 || pkgs_to_install="${pkgs_to_install} gcc"
	command -v make >/dev/null 2>&1 || pkgs_to_install="${pkgs_to_install} make"
	command -v python3 >/dev/null 2>&1 || pkgs_to_install="${pkgs_to_install} python3"
	command -v vulkaninfo >/dev/null 2>&1 || pkgs_to_install="${pkgs_to_install} vulkan-tools"
	command -v curl >/dev/null 2>&1 || pkgs_to_install="${pkgs_to_install} curl"

	if ! diff --version 2>/dev/null | grep -q GNU; then
		pkgs_to_install="${pkgs_to_install} diffutils"
	fi
	if ! awk --version 2>/dev/null | grep -q GNU; then
		pkgs_to_install="${pkgs_to_install} gawk"
	fi

	if [ ! -d "${MODDIR}/build" ]; then
		flavor="lts"
		case "${KVER}" in
		*-virt*) flavor="virt" ;;
		*-hardened*) flavor="hardened" ;;
		*-zen*) flavor="zen" ;;
		*) flavor="lts" ;;
		esac
		pkgs_to_install="${pkgs_to_install} linux-${flavor}-dev linux-headers build-base pahole elfutils-dev openssl-dev flex bison bc perl zstd-dev syslinux"
	fi

	if [ -n "$pkgs_to_install" ]; then
		info "Missing dependencies detected. Installing via apk:${pkgs_to_install}..."
		apk update
		# shellcheck disable-SC2086
		apk add --no-cache ${pkgs_to_install}
	fi
}

find_source() {
	src_tar="/tmp/linux-${CLEAN_KVER}.tar.xz"
	if [ ! -d "${BUILDDIR}/linux-${CLEAN_KVER}" ]; then
		info "Downloading vanilla kernel source for ${CLEAN_KVER}..."
		mkdir -p "$BUILDDIR"
		curl -L "https://cdn.kernel.org/pub/linux/kernel/v6.x/linux-${CLEAN_KVER}.tar.xz" -o "$src_tar"
		tar -xf "$src_tar" -C "$BUILDDIR"
	fi
	MODSRC="${BUILDDIR}/linux-${CLEAN_KVER}"
	[ -f "${MODSRC}/drivers/gpu/drm/amd/amdgpu/gfx_v10_0.c" ] || die "Kernel source tree invalid."
}

patch_source() {
	gfx="${MODSRC}/drivers/gpu/drm/amd/amdgpu/gfx_v10_0.c"
	if grep -q 'bc250_cc_write_mode' "$gfx"; then
		info "Source already patched."
		return 0
	fi

	patchdir="$(dirname "$(realpath "$0")")/../patch"

	# Patch descriptions from SERIES.md
	
	patch_nums=""

	# Build list of patch numbers from the patch directory
	for patchfile in "$patchdir"/*.patch; do
		pnum="$(basename "$patchfile" .patch | cut -d'-' -f1)"
		patch_nums="${patch_nums} ${pnum}"
	done

	# Sort patch numbers numerically
	sorted_nums="$(printf '%s\n' $patch_nums | sort -n)"

	# Patches that default to NO (opt-in)
	skip_default="12 17 19 21 28 29 30"

	# Collect selected patches
	selected_patches=""

	info "Selecting patches for BC-250 40 CU unlock..."
	printf '\n' >&2

	for pnum in $sorted_nums; do
		# Extract description from SERIES.md
		case "$pnum" in
		01) desc="01-declare-20-smu-message-enums: Declare the new SMU_MSG enum values the msg map needs" ;;
		02) desc="02-map-23-pmfw-messages-raise-sclk-max: Map 23 msgids; raise CYAN_SKILLFISH_SCLK_MAX 2000->2500" ;;
		03) desc="03-gfx-clock-force-and-dpm-levels: set_performance_level + ForceGfxFreq/UnForceGfxFreq" ;;
		04) desc="04-start-pmfw-telemetry-reporting: StartTelemetryReporting so SmuMetrics_t populates" ;;
		05) desc="05-raceless-direct-gfxclk-query: GFXCLK sensor reads direct QueryGfxclk (metrics path races)" ;;
		06) desc="06-read-cac-weight-baselines: CAC weight read helper (dep of the read-only CAC nodes)" ;;
		07) desc="07-cac-weight-and-sendraw-debugfs: Read-only *_cac_weight debugfs + smu_send_raw foundation" ;;
		08) desc="08-smu-cmn-send-raw-debugfs-definitions: smu_cmn_send_raw definitions + amdgpu_smu_send_raw node" ;;
		09) desc="09-cpu-cclk-soft-limits-debugfs: cclk_soft_min/max debugfs (CPU clock control)" ;;
		10) desc="10-print-full-32bit-cac-value: CAC print widened to 32-bit" ;;
		11) desc="11-full-telemetry-dump-debugfs: cyan_skillfish_telemetry node (clocks/pstates/voltages)" ;;
		12) desc="12-unlock-all-40-compute-units: Studebaker CU unlock CC+SPI+RLC → all 40 CUs ⚠ Vulkan-only, hangs ROCm/HSA" ;;
		13) desc="13-gfxoff-disable-gfx1013: GFXOFF disabled for gfx1013 — prevents GPU power-state hangs" ;;
		14) desc="14-gmc-kiq-bypass-dead-gpu: KIQ bypass + dead-GPU detection in gmc_v10_0 TLB flush" ;;
		15) desc="15-amdgpu-gmc-kiq-bypass: KIQ bypass + dead-GPU detection in centralized GMC code" ;;
		16) desc="16-cu-unlock-cc-spi-safe-no-rlc: BC-250 40 CU unlock — CC+SPI only, NO RLC_PG (safe for ROCm+HSA)" ;;
		17) desc="17-bc250-gfx1013-fault-probe: gfx1013 instruction-fetch fault probe — diagnostic-only, fails to compile on 6.18.53 (missing GMC9 constants), not needed for CU unlock" ;;
		18) desc="18-ttm-guard-null-pages-on-unpopulate: Guard NULL ttm->pages[] on unpopulate — survive compute faults" ;;
		19) desc="19-bc250-kfd-skip-sdma0: BC-250 SDMA0 skip — restrict user queues to SDMA1" ;;
		20) desc="20-amdgpu-ttm-populate-null-guard: READ_ONCE + return -ENOMEM NULL guard on TTM populate path" ;;
		21) desc="21-amdgpu-gmc-flush-pasid-kiq: KIQ PASID-flush disable — superseded by patch 14(e)" ;;
		22) desc="22-amdgpu-ttm-fno-lto: CFLAGS_amdgpu_ttm.o += -fno-lto — prevents ThinLTO eliding NULL guards" ;;
		23) desc="23-gb-addr-config-num-se: GB_ADDR_CONFIG 0x00000044→0x00100044 in gc_10_1_2 golden table" ;;
		24) desc="24-gmc-v10-flush-all-vmids: TLB flush all mapped VMIDs on BC-250 — fixes GPU aliasing bug" ;;
		25) desc="25-bc250-flush-tlb-by-runlist: Rebuild the runlist on unmap — firmware invalidates compute TLB" ;;
		26) desc="26-bc250-sdma-firmware-override: SDMA firmware override — navi10/navi12 blobs work" ;;
		27) desc="27-bc250-early-sdma-trap: Write SDMA TRAP_ENABLE in gfx_resume — removes boot stalls" ;;
		28) desc="28-bc250-8core-telemetry: 8-core hybrid SMU metrics layout — reinterprets firmware table" ;;
		29) desc="29-bc250-tmr-discovery-offset-fix: Honor IFWI-reported discovery TMR location" ;;
		30) desc="30-cyan-skillfish2-hardcoded-fallback: Fallback to hardcoded cyan skillfish IP table" ;;
		*) continue ;;
		esac

		# Determine default and prompt suffix
		if echo "$skip_default" | grep -qw "$pnum"; then
			default="n"
			prompt_suffix="(y/N)"
		else
			default="Y"
			prompt_suffix="(Y/n)"
		fi

		# Prompt — clean and concise
		printf 'Apply patch %s: %s %s: ' \
			"$pnum" "$desc" "$prompt_suffix" >&2
		read -r ans
		ans="$(echo "$ans" | tr '[:upper:]' '[:lower:]')"

		# Parse response
		case "$ans" in
			y|yes)
				selected_patches="${selected_patches} ${pnum}"
				;;
			n|no)
				# Explicit no
				;;
			*)
				# Empty = use default: accept if default Y, reject if default N
				if ! echo "$skip_default" | grep -qw "$pnum"; then
					selected_patches="${selected_patches} ${pnum}"
				fi
				;;
		esac
	done

	# Apply selected patches
	if [ -z "$selected_patches" ]; then
		warn "No patches selected. Skipping build."
		return 1
	fi

	info "Applying $(echo "$selected_patches" | wc -w) selected patches..."
	cd "$MODSRC"
	: > "$BUILDLOG"

	# Sort patch numbers numerically and apply in order
	for pnum in $(echo "$selected_patches" | tr -s ' ' '\n' | sort -n); do
		patchfile="$patchdir/${pnum}-"*".patch"
		# Find the matching file (the glob may match multiple files; use first)
		patchfile="$(find "$patchdir" -maxdepth 1 -name "${pnum}-"*".patch" | head -1)"
		[ -n "$patchfile" ] || die "Patch file for $pnum not found"

		if ! patch -p1 < "$patchfile" >> "$BUILDLOG" 2>&1; then
			err "Patch $pnum failed to apply. See build log: $BUILDLOG"
			exit 1
		fi

		if [ "$VERBOSE" -eq 1 ]; then
			printf '[PATCH %s] selected and applied\n' "$pnum" >&2
		fi
	done

	info "$(echo "$selected_patches" | wc -w) patches applied successfully."
}

build_module() {
	amdgpu_dir="${MODSRC}/drivers/gpu/drm/amd/amdgpu"
	info "Configuring kernel configuration..."
	cp /boot/config-"${KVER}" "${MODSRC}/.config" >> "$BUILDLOG" 2>&1 || true
	make -C "${MODSRC}" oldconfig >> "$BUILDLOG" 2>&1 || true

	info "Preparing kernel source tree..."
	make -C "${MODSRC}" prepare modules_prepare >> "$BUILDLOG" 2>&1
	cp "${MODDIR}/build/Module.symvers" "${MODSRC}/" >> "$BUILDLOG" 2>&1 || true

	info "Compiling amdgpu module with $(nproc) jobs (log: $BUILDLOG)..."
	make -C "${MODSRC}" M="$amdgpu_dir" clean >> "$BUILDLOG" 2>&1
	if ! make -C "${MODSRC}" M="$amdgpu_dir" -j"$(nproc)" modules >> "$BUILDLOG" 2>&1; then
		err "Compilation failed. Check build log at: $BUILDLOG"
		exit 1
	fi

	ko_path="${amdgpu_dir}/amdgpu.ko"
	[ -f "$ko_path" ] || die "Compilation failed: amdgpu.ko not generated."

	# Strictly output ONLY the file path to stdout for command substitution
	printf '%s\n' "$ko_path"
}

install_module() {
	built="$1"
	target="${MODPATH}.gz"
	[ -f "${MODPATH}" ] && target="${MODPATH}"

	if [ ! -f "${target}${BACKUP_SUFFIX}" ]; then
		info "Backing up original module..."
		cp "$target" "${target}${BACKUP_SUFFIX}"
	fi

	info "Compressing and installing module..."
	cd "${MODSRC}/drivers/gpu/drm/amd/amdgpu"
	gzip -9 -c amdgpu.ko > "$target"
	depmod -a "${KVER}"
	mkinitfs -o /boot/initramfs-"${KVER}" "${KVER}"
}

do_build() {
	check_deps
	check_bc250
	find_source
	patch_source
	built="$(build_module)"
	install_module "$built"
	info "Build complete! Run: doas $0 enable"
}

do_enable() {
	printf 'options amdgpu bc250_cc_write_mode=3\n' > "$CONF40"
	mkinitfs -o /boot/initramfs-"${KVER}" "${KVER}"
	info "40 CU mode enabled. Rebooting..."
	sleep 2
	reboot
}

do_disable() {
	rm -f "$CONF40"
	mkinitfs -o /boot/initramfs-"${KVER}" "${KVER}"
	info "40 CU mode disabled. Rebooting..."
	sleep 2
	reboot
}

do_restore() {
	target="${MODPATH}.gz"
	[ -f "${MODPATH}" ] && target="${MODPATH}"
	backup
	backup="$(ls -1 "${target}".bc250-backup-* 2>/dev/null | head -1)"
	[ -n "$backup" ] || die "No backup found"
	cp "$backup" "$target"
	rm -f "$CONF40"
	depmod -a "${KVER}"
	mkinitfs -o /boot/initramfs-"${KVER}" "${KVER}"
	info "Original module restored. Rebooting..."
	sleep 2
	reboot
}

do_status() {
	printf '\033[1m=== BC-250 Alpine CU Status ===\033[0m\n\n'
	if vulkaninfo --summary 2>/dev/null | grep -qi "${BC250_PCI_ID}"; then
		printf '  Vulkan device:  \033[0;32mBC-250 (ID 13fe) detected\033[0m\n'
	else
		printf '  Vulkan device:  \033[0;33mNot responding via vulkaninfo (headless)\033[0m\n'
	fi
	printf '  active CUs:  5  %s\n' "$(dmesg | grep 'active_cu_number' | tail -1 | grep -o 'active_cu_number [0-9]*' | awk '{print $2}')"
	printf '  write_mode:  5  %s\n' "$(cat /sys/module/amdgpu/parameters/bc250_cc_write_mode 2>/dev/null || echo 'N/A')"
}

# Parse --verbose / -v and --patches from args
OPT_PATCHES=""
for _arg in "$@"; do
	case "$_arg" in
		--verbose|-v) VERBOSE=1 ;;
		--patches) OPT_PATCHES="${2:-}" ;;
	esac
done

action="${1:-}"
if [ -n "$action" ] && [ "$action" != "status" ] && [ "$(id -u)" -ne 0 ]; then
	die "This script must be run as root. Please run with: doas $0 $action"
fi

case "$action" in
	build)  do_build ;;
	enable) do_enable ;;
	disable) do_disable ;;
	restore) do_restore ;;
	status) do_status ;;
	*)
		echo "Usage: doas $0 {build|enable|disable|restore|status}"
		;;
esac
