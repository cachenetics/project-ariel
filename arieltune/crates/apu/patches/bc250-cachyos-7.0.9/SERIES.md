# BC-250 amdgpu liberation patches (curated)

The kernel-patch series aputune embeds and builds into the system. Authored on
`linux-cachyos-bore-7.0.2`; the amdgpu/SMU source files are structurally
identical through `7.0.9`, so the same patches apply unchanged. Applied via the
`makepkg` flow (driven by `aputune build`), not `make M=...`.

**Kernel: pin to `linux-cachyos-bore-7.0.9`** (the current known-good target).
Do **not** build against `7.0.11+` yet - those kernels regress the BC-250 SDMA
path. The folder is named for the validated kernel (`bc250-cachyos-7.0.9`).

This set is **curated for portability** - only patches that are safe and useful
on any BC-250 ship, renumbered **01-12** in apply order. See "Excluded" below for
what was deliberately dropped.

## Patch list (12)

| # | Source | Purpose |
|---|---|---|
| `01` | `smu_types.h` | Declare the new `SMU_MSG_*` enum values the msg map needs |
| `02` | `cyan_skillfish_ppt.c` | Map 23 msgids (11->34); raise `CYAN_SKILLFISH_SCLK_MAX` 2000->2500 |
| `03` | `cyan_skillfish_ppt.c` | `set_performance_level` + `ForceGfxFreq`/`UnForceGfxFreq` |
| `04` | `cyan_skillfish_ppt.c` | `StartTelemetryReporting` so `SmuMetrics_t` populates (temp) |
| `05` | `cyan_skillfish_ppt.c` | GFXCLK sensor reads direct `QueryGfxclk` (metrics path races) |
| `06` | `cyan_skillfish_ppt.c` | CAC weight read helper (dep of the read-only CAC nodes) |
| `07` | `cyan_skillfish_ppt.c`, `amdgpu_smu.c`, `smu_cmn.h` | Read-only `*_cac_weight` debugfs + the `smu_send_raw` foundation |
| `08` | `smu_cmn.c` | `smu_cmn_send_raw` definitions + `amdgpu_smu_send_raw` node |
| `09` | `cyan_skillfish_ppt.c` | `cclk_soft_min/max` debugfs (CPU clock control) |
| `10` | `cyan_skillfish_ppt.c` | CAC print widened to 32-bit (correct CAC-node output) |
| `11` | `cyan_skillfish_ppt.c` | `cyan_skillfish_telemetry` node (clocks/pstates/voltages) |
| `12` | `gfx_v10_0.c` | `amdgpu.bc250_cc_write_mode`: CC + SPI(0x1F) + RLC(0x1F) -> all 40 CUs |

## What aputune does with them

- **40-CU unlock** (12): `amdgpu.bc250_cc_write_mode=3`, default-off, gated on PCI 0x13FE.
- **Clock control** (01/02/03 + 07/08): `ForceGfxFreq` via the race-free
  `amdgpu_smu_send_raw` node - GPU `force`/`wake`/`deep-sleep`/`autosleep`.
- **CPU cclk** (09): soft min/max.
- **Telemetry** (04/05/11): live clocks, temp, voltages.

## Excluded (deliberately not shipped)

Patches that exist in the research tree but are left out of the curated series:

- **UMC wiring** - only relevant to the memory controller; that's memtune's
  domain, not aputune.
- **Power-brake / DiDT throttle** - experimental BAPM stall tuning; "MGCG
  deadlocks compute". Research-only, risky on other boards.
- **In-kernel SW-DPM ladder** - the firmware exposes no usable GPU load signal
  (released clock is a flat ~1500 MHz idle/light-load), so an in-kernel auto-DPM
  can't drive a ladder reliably. aputune does power **app-driven** instead
  (`gpu autosleep`).
- **Raw-msgid debugfs** - exposes raw SMU pokes (reset/pstate/VMID); a footgun.
  aputune uses the specific msgids it needs via `smu_send_raw`.

## Build

`aputune build` materializes these patches, runs the `makepkg` flow (CC=gcc-15),
installs the package, arms 40-CU via modprobe.d, rebuilds initramfs, and (with
`--target user@host`) deploys + reboots. Verified end-to-end on a real BC-250
running `linux-cachyos-bore-7.0.9`.
