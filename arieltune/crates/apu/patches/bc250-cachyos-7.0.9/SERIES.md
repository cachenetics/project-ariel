# BC-250 amdgpu liberation patches (curated)

The kernel-patch series aputune embeds and builds into the system. Authored on
`linux-cachyos-bore-7.0.2`; the amdgpu/SMU source files are structurally
identical through `7.0.9`, so the same patches apply unchanged. Applied via the
`makepkg` flow (driven by `aputune build`), not `make M=...`.

**Kernel: pin to `linux-cachyos-bore-7.0.9`** (the current known-good target).
Do **not** build against `7.0.11+` yet - those kernels regress the BC-250 SDMA
path. The folder is named for the validated kernel (`bc250-cachyos-7.0.9`).

This set is **curated for portability** - only patches that are safe and useful
on any BC-250 ship, numbered **01-28**. Four patches are on disk but NOT applied,
in the order listed: `12` (Studebaker's Vulkan-only CU unlock, kept as an
alternate to `16`), `21` (KIQ PASID-flush disable - superseded by patch
`14(e)`, which already sets `flush_pasid_uses_kiq = false` for gfx10.1.x),
`26` and `27` (the SDMA firmware fix and its companion boot trap fix - staged
for the SDMA validation round, then patch `19` retires). All four are tracked
in the `aputune` TUI under "on disk, not applied". See "Excluded" below for
what was deliberately dropped.

Patch `28` is the 8-core CPU-unlock telemetry fix (see `arieltune-core.md`).
GabriWar's companion `0002-bc250-rocm-vm-flush.patch` was reviewed and is
**deliberately not staged**: applied patch `14(e)` already forces
`flush_pasid_uses_kiq = false` for gfx10.1.x on the exact line `0002` touches.


## Detailed patch list

| # | Filename | Source | Purpose |
|---|---|---|---|
| `01` | `01-declare-20-smu-message-enums.patch` | `smu_types.h` | Declare the new `SMU_MSG_*` enum values the msg map needs |
| `02` | `02-map-23-pmfw-messages-raise-sclk-max.patch` | `cyan_skillfish_ppt.c` | Map 23 msgids (11->34); raise `CYAN_SKILLFISH_SCLK_MAX` 2000->2500 |
| `03` | `03-gfx-clock-force-and-dpm-levels.patch` | `cyan_skillfish_ppt.c` | `set_performance_level` + `ForceGfxFreq`/`UnForceGfxFreq` |
| `04` | `04-start-pmfw-telemetry-reporting.patch` | `cyan_skillfish_ppt.c` | `StartTelemetryReporting` so `SmuMetrics_t` populates (temp) |
| `05` | `05-raceless-direct-gfxclk-query.patch` | `cyan_skillfish_ppt.c` | GFXCLK sensor reads direct `QueryGfxclk` (metrics path races) |
| `06` | `06-read-cac-weight-baselines.patch` | `cyan_skillfish_ppt.c` | CAC weight read helper (dep of the read-only CAC nodes) |
| `07` | `07-cac-weight-and-sendraw-debugfs.patch` | `cyan_skillfish_ppt.c`, `amdgpu_smu.c`, `smu_cmn.h` | Read-only `*_cac_weight` debugfs + the `smu_send_raw` foundation |
| `08` | `08-smu-cmn-send-raw-debugfs-definitions.patch` | `smu_cmn.c` | `smu_cmn_send_raw` definitions + `amdgpu_smu_send_raw` node |
| `09` | `09-cpu-cclk-soft-limits-debugfs.patch` | `cyan_skillfish_ppt.c` | `cclk_soft_min/max` debugfs (CPU clock control) |
| `10` | `10-print-full-32bit-cac-value.patch` | `cyan_skillfish_ppt.c` | CAC print widened to 32-bit (correct CAC-node output) |
| `11` | `11-full-telemetry-dump-debugfs.patch` | `cyan_skillfish_ppt.c` | `cyan_skillfish_telemetry` node (clocks/pstates/voltages) |
| `12` | `12-unlock-all-40-compute-units.patch` | `gfx_v10_0.c` | *(on disk, NOT applied)* Studebaker's CU unlock: CC + SPI(0x1F) + RLC(0x1F) → all 40 CUs (⚠️ Vulkan-only, hangs ROCm/HSA) — superseded by `16` |
| `13` | `13-gfxoff-disable-gfx1013.patch` | `gfx_v10_0.c` | **NEW** GFXOFF disabled for gfx1013 — prevents GPU power-state hangs (c) Fabian, used with permission |
| `14` | `14-gmc-kiq-bypass-dead-gpu.patch` | `gmc_v10_0.c` | **NEW** KIQ bypass + dead-GPU detection in gmc_v10_0 TLB flush (5 sub-patches) (c) Fabian, used with permission |
| `15` | `15-amdgpu-gmc-kiq-bypass.patch` | `amdgpu_gmc.c` | **NEW** KIQ bypass + dead-GPU detection in centralized GMC code (2 sub-patches) (c) Fabian, used with permission |
| `16` | `16-cu-unlock-cc-spi-safe-no-rlc.patch` | `gfx_v10_0.c` | **NEW** BC-250 40 CU unlock — CC+SPI only, NO RLC_PG (safe for ROCm+HSA) |
| `17` | `17-bc250-gfx1013-fault-probe.patch` | `gmc_v10_0.c` | **NEW** gfx1013 instruction-fetch fault probe — diagnostic, report-only. Marked *optional* in detection: kernels without it (like the deployed build2) still report fully patched. |
| `18` | `18-ttm-guard-null-pages-on-unpopulate.patch` | `amdgpu_ttm.c` | **NEW** Guard NULL `ttm->pages[]` on unpopulate — survive compute faults |
| `19` | `19-bc250-kfd-skip-sdma0.patch` | `kfd_device_queue_manager.c` | **NEW** BC-250 SDMA0 skip — restrict user queues to SDMA1 (SDMA0 completion IRQ is lost at boot) |
| `20` | `20-amdgpu-ttm-populate-null-guard.patch` | `amdgpu_ttm.c` | **NEW** READ_ONCE + `return -ENOMEM` NULL guard on the TTM *populate* path — completes patch 18 |
| `21` | `21-amdgpu-gmc-flush-pasid-kiq.patch` | `gmc_v10_0.c` | *(on disk, NOT applied)* KIQ PASID-flush disable — superseded by patch `14(e)`, which applies the same change on gfx10.1.x |
| `22` | `22-amdgpu-ttm-fno-lto.patch` | `Makefile` | **NEW** `CFLAGS_amdgpu_ttm.o += -fno-lto` — prevents ThinLTO from eliding NULL guards from patches 18+20 |
| `23` | `23-gb-addr-config-num-se.patch` | `gfx_v10_0.c` | **APPLIED** GB_ADDR_CONFIG 0x00000044→0x00100044 in the gc_10_1_2 golden table — deployed form; GabriWar later retracted it upstream, but the production build carries it |
| `24` | `24-gmc-v10-flush-all-vmids.patch` | `gmc_v10_0.c` | **NEW** TLB flush all mapped VMIDs on BC-250 — direct MMIO path, no KIQ. Fixes GPU aliasing bug. **Active on snap-a0af1eeb.** |
| `25` | `25-bc250-flush-tlb-by-runlist.patch` | `kfd_device_queue_manager.c`, `kfd_chardev.c`, `kfd_device_queue_manager.h` | **NEW** Rebuild the runlist on unmap so the firmware really invalidates the compute TLB. **Active on snap-8fe794e8** — the patch that made PyTorch work. |
| `26` | `26-bc250-sdma-firmware-override.patch` | `amdgpu_sdma.c` | *(on disk, NOT applied)* SDMA firmware override — the cyan_skillfish2 blob never drives user queues (GabriWar docs/28+29); navi10/navi12 blobs work. Gated by `amdgpu.bc250_sdma_fw=<base>`. Validation path to retiring patch 19. |
| `27` | `27-bc250-early-sdma-trap.patch` | `sdma_v5_0.c` | *(on disk, NOT applied)* Write SDMA TRAP_ENABLE in gfx_resume — removes the two 500 ms "Fence fallback" stalls at boot. Companion to patch 26. |
| `28` | `28-bc250-8core-telemetry.patch` | `cyan_skillfish_ppt.c`, `smu11_driver_if_cyan_skillfish.h` | **APPLIED** 8-core hybrid SMU metrics layout. After the 8-core CPU unlock the firmware redistributes the 116-byte metrics table and `GfxclkFrequency` loses its 0x44 slot (`C0Residency[6]` now) — this reinterprets the table with the empirically mapped hybrid layout and queries gfxclk direct from the SMU (`QueryGfxclk`) with a table fallback. Auto-detects via topology; `amdgpu.cs_eight_core_map=1` forces. Safe on 6-core blades. Ported from GabriWar `0001-bc250-8core-telemetry`. |

## What aputune does with them

- **GPU stability** (13/14/15): Three-layer protection against BC-250 GPU hangs.
  - Layer 1 (13): GFXOFF disabled for gfx1013 — prevents GPU entering unrecoverable power state.
  - Layer 2a (14): gmc_v10_0 KIQ bypass + dead-GPU detection — 5 sub-patches protecting TLB flush.
  - Layer 2b (15): amdgpu_gmc KIQ bypass + dead-GPU detection — centralized GMC protection.
  Together these eliminate the infamous BC-250 KIQ hang and provide graceful
  recovery when the GPU becomes unreachable (0xFFFFFFFF MMIO reads).
- **40-CU unlock** (16): `16` is the one the build applies. Studebaker's `12`
  stays on disk as the Vulkan-only alternate — see the comparison below.
- **Clock control** (01/02/03 + 07/08): `ForceGfxFreq` via the race-free
  `amdgpu_smu_send_raw` node - GPU `force`/`wake`/`deep-sleep`/`autosleep`.
- **CPU cclk** (09): soft min/max.
- **Telemetry** (04/05/11): live clocks, temp, voltages.
- **SDMA reliability** (19): Steers user SDMA queues off engine 0 to engine 1 (lost completion IRQ). Controlled by amdgpu.bc250_skip_sdma0=1. By Gabriel Duarte Guerra.

### CU unlock: patch 12 vs patch 16

`12` is Studebaker's original 40-CU unlock, written for Vulkan/RADV. `16` is the
ROCm-safe derivative, and it is the one `apu build` applies.

| Aspect | Patch 12 (Studebaker, Vulkan) | Patch 16 (applied) |
|---|---|---|---|
| Registers | CC + SPI + **RLC** | CC + SPI only |
| RLC_PG_ALWAYS_ON_WGP_MASK | Written (mode 3) | **NOT written** |
| Vulkan/RADV | ✅ Works | ✅ Works |
| ROCm/HSA (rocBLAS, HIP) | ❌ Hangs on first KFD queue | ✅ Works |
| Module param | `bc250_cc_write_mode=3` | `bc250_cc_write_mode=3` |
| Probe modes | 1 (SE0 only), 4 (all SAs) | 1 (SE0 only), 4 (all SAs) |

**Why no RLC write?** Writing RLC_PG_ALWAYS_ON_WGP_MASK while RLC firmware is
running triggers the WGP bring-up state machine for harvested WGPs 3-4 whose
handshake registers are uninitialized. The RLC stalls waiting for an ACK that
never arrives → any subsequent KFD queue operation hangs. BC-250 already has
RLC_PG_CNTL=0 (PG globally off via ppfeaturemask), so the RLC write is
redundant.

**Which ships:** `16`, because it covers both Vulkan and ROCm. Studebaker's `12`
stays on disk for Vulkan-only setups — swap it in by pointing the `16` registry
entry in `crates/apu/src/patches.rs` at `12-unlock-all-40-compute-units.patch`.
Do NOT use `12` if any ROCm/HSA workload is planned.

## GPU stability architecture (patches 13-15)

The BC-250 has a known hardware issue: when the GPU enters certain power states
(GFXOFF) or paths that require firmware coordination (KIQ ring), it can become
unresponsive. Because the BC-250's internal PCIe fabric lacks completion
timeout, any MMIO read to a dead GPU hangs the CPU indefinitely.

The three-layer protection:

```
Layer 1 (patch 13): Prevent entry into dangerous power states
  └─ gfx_v10_0_check_gfxoff_flag() → disable GFXOFF for gfx1013

Layer 2 (patches 14+15): Bypass dangerous code paths + detect dead GPU
  ├─ gmc_v10_0_flush_gpu_tlb() → goto use_mmio for gfx10.1.x
  ├─ amdgpu_gmc_flush_gpu_tlb_pasid() → direct MMIO callout
  ├─ amdgpu_gmc_fw_reg_write_reg_wait() → direct MMIO poll loop
  └─ All paths: pre-read health check + 0xFFFFFFFF dead-GPU detection

Layer 3 (patch 19): Steer user DMA away from broken SDMA0. Clear even-numbered SDMA queue bits so user queues use engine 1 only. amdgpu.bc250_skip_sdma0=1 (default off, A/B-testable).
```

All three layers are gated on `IP_VERSION(10, 1, x)` so they are no-ops on
non-BC-250 hardware.

## The open compute defect (patches 17-18)

ROCm inference is stable on this part. What is not stable is any workload with a
second heavy GPU phase — a training backward pass, an img2img reverse pass,
multi-step generation. The first phase completes; the next one dies.

When it surfaces as a page fault rather than a HIP error, the fault is always an
`SQC (inst)` read — the shader *instruction* cache. The wave's PC was already
wrong before the fault was raised, so the fault handler is reporting the
symptom, not causing it.

Seven of nine faults collected across separate boots with ASLR active share an
exact shape: top byte `0xff`, low 20 bits `0xbb000`, only bits [39:20] moving.
The process's own code objects live at `0x7f...`. `0x7f` -> `0xff` is one bit —
bit 47, the canonical-form boundary — and the top byte of a 48-bit shader
address is exactly `COMPUTE_PGM_HI`, which is 8 bits wide and carries bits
[47:40]. So this is one wrong bit in a otherwise plausible address.

- **`17` is the measurement.** It logs the raw interrupt vector and the
  bit-47-cleared candidate so the candidate can be checked against the faulting
  process's GPU VA map. It changes no control flow and fixes nothing. Gate with
  `amdgpu.bc250_fault_probe=0`.
- **`18` is containment.** Compute faults on this part were escalating to kernel
  panics via a missing NULL check in TTM cleanup, which made the defect
  expensive to study — you lose the blade instead of getting a log. With the
  guard the process dies and the machine survives.

Neither patch claims a fix. They exist so the defect can be isolated. Deliberately
*not* shipped: fault-handler retry, address rewriting, warmup dispatches, and
`AMD_SERIALIZE_KERNEL`-style throttling — those change timing or hide the symptom
without addressing why bit 47 is set.

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


## Patches 18-25: TTM crash fixes + TLB flush (GabriWar era)

### What they do

- **TTM crash fix** (18/20/22): Patch 18 guards the unpopulate path; patch 20
  guards the populate path with READ_ONCE + `return -ENOMEM`; patch 22 builds
  `amdgpu_ttm.o` with `-fno-lto` so ThinLTO cannot elide either guard. Together
  these eliminate the CR2=0x18 kernel panics on compute faults.
- **KIQ bypass for PASID flush** (21, superseded): Patch `14(e)` already sets
  `flush_pasid_uses_kiq = false` for gfx10.1.x, so patch 21 is kept on disk for
  attribution (neoney, verified by GabriWar) but is NOT in the applied series.
- **GB_ADDR_CONFIG** (23): Applied in the deployed build2 tree and registered in
  the series. GabriWar later retracted the 0x00100044 value upstream (his repo
  keeps stock 0x00000044), so treat this as a divergence to re-test — but the
  production blade runs it and PyTorch is stable with it.
- **All-VMID TLB flush** (24): Instead of matching PASID values (which can race
  with KFD teardown), patch 24 invalidates every currently-mapped process VMID
  through the direct MMIO path on every TLB invalidation. Also guards
  `gmc_v10_0_flush_gpu_tlb()` against BC-250 to skip the FW-register KIQ path.
  **Active on snap-a0af1eeb.** Validated with 4-worker concurrent GPU stress
  (200 iters, 0 corruptions).
- **Runlist rebuild flush** (25): The measured fix for the aliasing bug. The
  compute TLB on gfx1013 is never invalidated: the PASID scan finds zero VMIDs
  (mmATC_VMID*_PASID_MAPPING is never written under HWS), and forced MMIO
  flushes wedge the GPU. Rebuilding the runlist on unmap makes the firmware
  invalidate for real. 36 counterbalanced runs, single boot: stock 13/18 dirty,
  runlist 0/18 dirty (Fisher p = 3.7e-06); verified active via ftrace
  `execute_queues_cpsch` counts (6 -> 68), zero board errors. Off by default:
  `amdgpu.bc250_flush_by_runlist=1`.

### GPU aliasing bug

The BC-250 (gfx1013) exhibits a GPU page-table aliasing pattern where different
VMIDs can see each other's physical pages. This manifests as ILLEGAL_INSTRUCTION
or random data corruption in multi-process GPU workloads. The root cause per
GabriWar's analysis: hipFree unmaps, hipMalloc reuses the same VA with a new
physical address, the PTEs in memory are correct, and the GPU keeps translating
through the previous mapping — because its compute TLB is never actually
invalidated. Patch 24 flushes all mapped VMIDs on every invalidation (direct
MMIO path), and patch 25 finishes the job by rebuilding the runlist, the only
invalidation measured to work on this silicon.

### Production snapshot lineage

```
snap-31bbf471 (19-patch, stable)
  └─ snap-a0af1eeb (19-patch + 24: all-VMID TLB flush)
       └─ snap-8fe794e8 (25-patch: runlist rebuild flush)  ← active on blade 15
```

### Production state on blade 15 (verified 2026-08-17 against the build2 tree)

The deployed 25-patch build (`snap-8fe794e8`, module `amdgpu.ko` srcversion
`C484A6D2`) was diffed against this series applied to the pristine
`cachyos-7.0.9-1` tarball. Every file is byte-identical except:

- `gfx_v10_0.c`: the deployed tree replaces patch 16's no-RLC unlock with the
  blade's own RLC-writing variant (CC all SAs + SPI 0x1F + RLC_PG 0x1F
  broadcast, run before CU inventory) — same `bc250_cc_write_mode=3` semantics.
- `gmc_v10_0.c`: the deployed tree does NOT carry patch 17 (fault probe), and
  its `gmc_v10_0_hw_init()` keeps `flush_pasid_uses_kiq = !amdgpu_emu_mode`
  (the 14(e)/21 hunk was rejected at patch time and never re-applied).
- `amdgpu/Makefile`: the deployed tree does NOT carry patch 22 (`-fno-lto`).

Runtime facts from the blade: `amdgpu.bc250_flush_by_runlist=1`
(`/etc/modprobe.d/bc250-runlist.conf`), `amdgpu.bc250_skip_sdma0=1`,
`amdgpu.bc250_cc_write_mode=3`, `modtree=build2`. No SDMA firmware swap —
the stock cyan_skillfish2 blob is used (GabriWar's docs/29 firmware swap is
NOT in production; patch 19 is the SDMA workaround). The two
"Fence fallback timer expired on ring sdma0" boot messages still appear
(GabriWar's `bc250_early_sdma_trap` fix from his SDMA instrumentation patch
is not in production).

## Roadmap — the SDMA exit and what it retires

The two "Fence fallback timer expired on ring sdma0" lines at every boot are
init-order artifacts, not lost interrupts (GabriWar docs/28), and the user-queue
hang is the firmware itself: AMD's `cyan_skillfish2_sdma.bin` never drives user
queues, while navi10/navi12 blobs copy 4 MiB in 0.04 s with correct data
(docs/29). Patch 19 (`bc250_skip_sdma0=1`) is the current workaround.

Planned sequence, each step measured before the next:

1. Boot a blade with `amdgpu.bc250_sdma_fw=navi12` (patch 26, opt-in param) and
   patch 27. Validate: completion signal drops, `torch.equal` on 16 MiB round
   trips with `HSA_ENABLE_SDMA=1`, magnum bandwidth, RCCL collectives.
2. Retire patch 19 (`bc250_skip_sdma0=1` off) once SDMA0 is proven.
3. Re-test `vm_update_mode` (SDMA page-table updates) against the Navi 1x
   invalidation-vs-translation errata in `amdgpu_gmc.c:743`.
4. Promote 26 + 27 into the applied series.

Known performance work on top of that: the runlist flush (25) currently rebuilds
on every unmap — coalescing or VA-reuse gating is the next tuning step
(GabriWar's own "still owed" list); patch 24's all-VMID loop may be a no-op
behind the same empty ATC query and should be measured before it is trusted;
`amdgpu.noretry=1` removes retry stalls on the rare real fault.
