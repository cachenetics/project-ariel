// SPDX-License-Identifier: GPL-2.0-only
//! The BC-250 liberation kernel-patch series, embedded into the binary.
//!
//! aputune *owns* the silicon-liberation surface: the curated CachyOS amdgpu
//! series (authored on `linux-cachyos-bore-7.0.2`, structurally identical through
//! `7.0.9` — the pinned, known-good kernel) ships inside the binary as data, and
//! each patch carries the *runtime tell* that proves it is live on the running
//! kernel. That lets `aputune patches` report a true per-patch state without
//! trusting a version string, and lets the build path (see `kbuild`) reconstruct
//! the exact source tree it was validated against.
//!
//! Pin to `linux-cachyos-bore-7.0.9`. Newer kernels (7.0.11+) regress the BC-250
//! SDMA path — do not build the series against them until that is resolved.
//!
//! The patches themselves are GPL-2.0 (kernel diffs) — the same license as this
//! project (GPL-2.0-only, matching upstream cachenetics/project-ariel).
//! arieltune carries them as build assets applied by the kernel-build path — they
//! are not linked into the binary.

/// How to prove, at runtime, that a given patch is live on the booted kernel.
#[derive(Clone, Copy, Debug)]
pub enum Tell {
    /// A debugfs node by basename under the amdgpu DRI debug dir
    /// (`/sys/kernel/debug/dri/<n>/<name>`).
    Debugfs(&'static str),
    /// An amdgpu module parameter file
    /// (`/sys/module/amdgpu/parameters/<name>`).
    ModParam(&'static str),
    /// `pp_dpm_sclk` advertises a max state >= this many MHz.
    SclkMax(u32),
    /// The driver reports at least this many active CUs. No series member uses
    /// it today (patch 12 detects via ModParam) — kept as a detection
    /// capability for future patch revisions whose only tell is the CU count.
    #[allow(dead_code)]
    CuCount(u32),
    /// No unique runtime fingerprint of its own (header-only, an init code
    /// path, or a log line). Presence is *inferred* when the uniquely
    /// detectable members of the series are live.
    Bundled,
}

/// One member of the series.
pub struct Patch {
    /// Ordinal as it appears in the filename (e.g. "08").
    pub id: &'static str,
    /// Embedded patch text.
    pub body: &'static str,
    /// One-line purpose (mirrors patches/.../SERIES.md).
    pub title: &'static str,
    /// Plain-English description: what the patch does and why it matters
    /// (1-3 sentences, rendered in the TUI patch popup).
    pub desc: &'static str,
    /// Kernel source file(s) it touches.
    pub touches: &'static str,
    /// Runtime fingerprint.
    pub tell: Tell,
}

macro_rules! patch {
    ($id:literal, $file:literal, $title:literal, $desc:literal, $touches:literal, $tell:expr) => {
        Patch {
            id: $id,
            body: include_str!(concat!("../patches/bc250-cachyos-7.0.9/", $file)),
            title: $title,
            desc: $desc,
            touches: $touches,
            tell: $tell,
        }
    };
}

/// The full series, in apply order.
pub const SERIES: &[Patch] = &[
    patch!(
        "01",
        "01-declare-20-smu-message-enums.patch",
        "Declare 20 new SMU_MSG_* enum values",
        "Adds 20 new SMU_MSG_* names (QueryGfxclk, ForceGfxFreq, \
         StartTelemetryReporting, the CAC weight operations, and friends) to \
         the common SMU type header so the rest of the series can reference \
         them. Header-only — no behavior change by itself; every other \
         cyan_skillfish patch builds on these names.",
        "smu_types.h",
        Tell::Bundled
    ),
    patch!(
        "02",
        "02-map-23-pmfw-messages-raise-sclk-max.patch",
        "Map 23 msgids (11->34); raise CYAN_SKILLFISH_SCLK_MAX 2000->2500",
        "Grows the cyan_skillfish message map from 11 to 34 entries, wiring \
         the new enums to their real PMFW msgids — clock force/query, \
         telemetry start/stop, CAC weights, cclk soft limits. Also raises \
         CYAN_SKILLFISH_SCLK_MAX from 2000 to 2500 MHz so the driver accepts \
         overclocked GFX targets instead of rejecting them as out of range.",
        "cyan_skillfish_ppt.c",
        Tell::SclkMax(2500)
    ),
    patch!(
        "03",
        "03-gfx-clock-force-and-dpm-levels.patch",
        "set_soft_freq_limited_range + set_performance_level (ForceGfxFreq)",
        "Implements the two standard clock-control hooks the stock driver \
         leaves empty, backed by ForceGfxFreq (0x39) / UnForceGfxFreq (0x3A): \
         min == max locks the GFX clock at that frequency, anything else \
         releases it back to PMFW DPM. This is what makes \
         power_dpm_force_performance_level and pp_dpm_sclk writes actually \
         do something on the BC-250.",
        "cyan_skillfish_ppt.c",
        Tell::Bundled
    ),
    patch!(
        "04",
        "04-start-pmfw-telemetry-reporting.patch",
        "StartTelemetryReporting(0x1B) so SmuMetrics_t populates",
        "Sends StartTelemetryReporting (0x1B) with a 1 ms sample interval at \
         SMU init — the stock driver never starts it, so the SmuMetrics_t \
         Current/Average fields (including temperature) stay stale zeros. \
         Best-effort: a failure is logged and non-fatal.",
        "cyan_skillfish_ppt.c",
        Tell::Bundled
    ),
    patch!(
        "05",
        "05-raceless-direct-gfxclk-query.patch",
        "GFXCLK sensor reads direct QueryGfxclk (metrics path races)",
        "The stock GFXCLK sensor reads through the SmuMetrics_t table \
         (TransferTableSmu2Dram), which has empirically raced amdgpu's own \
         SMU traffic during compute and returned garbage. This routes the \
         read to a direct QueryGfxclk mailbox message, serialized under \
         msg_ctl.lock, so live clock readings are trustworthy.",
        "cyan_skillfish_ppt.c",
        Tell::Bundled
    ),
    patch!(
        "06",
        "06-read-cac-weight-baselines.patch",
        "CAC weight read helper; logs GFX[0]/L3[0] baselines",
        "Adds a readback helper for the PMFW CAC (power-accounting) weight \
         tables via GfxCacWeightOperation (0x2F) / L3CacWeightOperation \
         (0x30) and logs the slot-0 baselines once at SMU init (GFX[0]=0x17 \
         Sony default, L3[0]=0x00). Smoke-tests the CAC msgid path; the \
         dependency of the read-only CAC debugfs nodes in patch 07.",
        "cyan_skillfish_ppt.c",
        Tell::Bundled
    ),
    patch!(
        "07",
        "07-cac-weight-and-sendraw-debugfs.patch",
        "Read-only *_cac_weight debugfs + smu_send_raw foundation",
        "Adds two read-only debugfs nodes that dump the full CAC weight \
         tables (64 GFX / 80 L3 slots, one per line) and hooks the send-raw \
         debugfs creation into amdgpu's SMU init. Writes are deliberately \
         not implemented — the PMFW write encoding is unverified on 88.6.0; \
         encodings get validated through smu_send_raw first.",
        "cyan_skillfish_ppt.c, amdgpu_smu.c",
        Tell::Debugfs("cyan_skillfish_gfx_cac_weight")
    ),
    patch!(
        "08",
        "08-smu-cmn-send-raw-debugfs-definitions.patch",
        "smu_cmn_send_raw definitions + amdgpu_smu_send_raw node",
        "Supplies smu_cmn_send_raw_smc_msg and the amdgpu_smu_send_raw \
         debugfs node that patch 07 declares but does not define (the series \
         fails to link without it). Raw sends take smu->msg_ctl.lock, so \
         they serialize with amdgpu's own polling and cannot wedge the \
         mailbox — the race-free foundation aputune's GPU actuation rides on.",
        "smu_cmn.h, amdgpu_smu.c",
        Tell::Debugfs("amdgpu_smu_send_raw")
    ),
    patch!(
        "09",
        "09-cpu-cclk-soft-limits-debugfs.patch",
        "cclk_soft_min/max debugfs via SetSoftMin/MaxCclk",
        "Adds cclk_soft_min/max debugfs nodes that send SetSoftMinCclk \
         (0x35) / SetSoftMaxCclk (0x36) with the written MHz value, giving \
         root a CPU clock floor/ceiling knob. Bounded to 0..5000 MHz; reads \
         return the last-written value (PMFW has no query counterpart).",
        "cyan_skillfish_ppt.c",
        Tell::Debugfs("cyan_skillfish_cclk_soft_min")
    ),
    patch!(
        "10",
        "10-print-full-32bit-cac-value.patch",
        "CAC print widened 0x%04x -> 0x%08x",
        "Widens the CAC debugfs print from 16 to 32 bits. PMFW returns \
         meaningful state in the high bits (e.g. GFX slot 23 reads \
         0x000701xx) that the old 0xffff mask silently discarded.",
        "cyan_skillfish_ppt.c",
        Tell::Bundled
    ),
    patch!(
        "11",
        "11-full-telemetry-dump-debugfs.patch",
        "cyan_skillfish_telemetry node (clocks/WGP/pstates/voltages)",
        "Adds a read-only cyan_skillfish_telemetry debugfs node: one read \
         fans out over the Query* msgids (GfxClk, GfxVid, VddcrSocClock, DF \
         and core pstates, enabled SMU features, PMFW version) as a one-shot \
         chip-state snapshot for diagnostics. Every read is a fresh mailbox \
         query under msg_ctl.lock, race-free even during compute.",
        "cyan_skillfish_ppt.c",
        Tell::Debugfs("cyan_skillfish_telemetry")
    ),
    patch!(
        "13",
        "13-gfxoff-disable-gfx1013.patch",
        "GFXOFF disabled for gfx1013 (Cyan Skillfish) — Layer 1 stability",
        "Disables GFXOFF power-saving state for IP 10.1.3 (BC-250/gfx1013). \
         The GPU cannot reliably wake from GFXOFF, and since the BC-250's \
         internal PCIe fabric lacks completion timeout, any MMIO read to a \
         sleeping GPU hangs the CPU indefinitely. Runs early in hw_init, \
         before any GFXOFF transition is attempted. First of the three-layer \
         v3 protection system.",
        "gfx_v10_0.c",
        Tell::Bundled
    ),
    patch!(
        "14",
        "14-gmc-kiq-bypass-dead-gpu.patch",
        "KIQ bypass + dead-GPU detection in gmc_v10_0.c — Layer 2a",
        "Five sub-patches hardening the gfx10.x TLB flush: (a) gfx10.1.x \
         jumps to direct MMIO via goto use_mmio, skipping the KIQ ring; \
         (b) pre-spinlock 0xFFFFFFFF health check bails early if GPU is \
         unreachable; (c) semaphore acquire loop detects dead GPU; \
         (d) ACK-wait loop detects dead GPU and releases semaphore; \
         (e) disables KIQ-based PASID flush in hw_init for gfx10.1.x. \
         Generation-specific counterpart to patch 15.",
        "gmc_v10_0.c",
        Tell::Bundled
    ),
    patch!(
        "15",
        "15-amdgpu-gmc-kiq-bypass.patch",
        "KIQ bypass + dead-GPU detection in amdgpu_gmc.c — Layer 2b",
        "Two sub-patches hardening the centralized GMC: (a) in \
         flush_gpu_tlb_pasid, gfx10.1.x bypasses the KIQ ring and calls \
         the ASIC-specific flush directly via MMIO; (b) in \
         fw_reg_write_reg_wait, gfx10.1.x short-circuits the entire KIQ \
         path with a direct MMIO write + polling loop, including pre-write \
         health check and dead-GPU detection. Together with patch 14, \
         eliminates the BC-250 KIQ hang.",
        "amdgpu_gmc.c",
        Tell::Bundled
    ),
    patch!(
        "16",
        "16-cu-unlock-cc-spi-safe-no-rlc.patch",
        "BC-250 40 CU unlock — CC+SPI only, NO RLC_PG (safe for ROCm/HSA)",
        "Re-enables all 40 CUs using CC_GC_SHADER_ARRAY_CONFIG=0 and \
         SPI_PG_ENABLE_STATIC_WGP_MASK=0x1f per shader array. \
         RLC_PG_ALWAYS_ON_WGP_MASK is deliberately NOT written: doing so \
         while RLC firmware is running hangs the RLC on uninitialized WGP \
         handshake registers, causing a system hang on first rocBLAS/HSA \
         launch. BC-250 already has global PG off, making the RLC write \
         redundant. Safe for both Vulkan/RADV and ROCm/HSA. Controlled \
         via amdgpu.bc250_cc_write_mode=3. Supersedes the excluded patch 12, \
         which also writes RLC_PG and is Vulkan-only.",
        "gfx_v10_0.c",
        Tell::ModParam("bc250_cc_write_mode")
    ),
    patch!(
        "17",
        "17-bc250-gfx1013-fault-probe.patch",
        "gfx1013 instruction-fetch fault probe (diagnostic, report-only)",
        "Diagnostic for the open multi-phase compute defect: the second heavy \
         GPU phase dies on an SQC (inst) GPUVM fault whose address is the \
         right one with bit 47 wrongly set (0x7f.. read back as 0xff..). \
         Logs the raw interrupt vector and the bit-47-cleared candidate for \
         gfx1013 so the candidate can be checked against the faulting \
         process's GPU VA map. Changes no control flow and fixes nothing — \
         it exists to identify the root cause. Set amdgpu.bc250_fault_probe=0 \
         to silence it.",
        "gmc_v10_0.c",
        Tell::ModParam("bc250_fault_probe")
    ),
    patch!(
        "18",
        "18-ttm-guard-null-pages-on-unpopulate.patch",
        "Guard NULL ttm->pages[] on unpopulate — survive compute faults",
        "amdgpu_ttm_tt_unpopulate walks ttm->pages[] and writes \
         pages[i]->mapping without a NULL check, so a buffer left partially \
         populated by an aborted GPU command panics the kernel on cleanup \
         (CR2=0x18). On the BC-250 that turned every compute fault into a \
         dead blade. Containment, not a fix: the faulting process still \
         dies, but the machine survives long enough to read what patch 17 \
         logged. Upstream bug, not distro-specific. By Gabriel Duarte Guerra.",
        "amdgpu_ttm.c",
        Tell::Bundled
    ),
        patch!(
        "20",
        "20-amdgpu-ttm-populate-null-guard.patch",
        "READ_ONCE NULL guard on the TTM *populate* path",
        "Completes patch 18 on the other half of the TTM lifecycle: amdgpu_ttm_tt_populate dereferences ttm->pages[i]->mapping without a NULL check. A BO whose page array has holes (partial population after an aborted GPU command) dies with a kernel panic instead of an error. Guard each entry with READ_ONCE + return -ENOMEM. This is the exact form deployed in the build2 tree on blade 15 (snap-8fe794e8).",
        "amdgpu_ttm.c",
        Tell::Bundled
    ),
    patch!(
        "22",
        "22-amdgpu-ttm-fno-lto.patch",
        "Disable ThinLTO for amdgpu_ttm.o (defeats NULL guard elision)",
        "ThinLTO can prove ttm->pages[i] is never NULL across the call graph and elide the NULL guards from patches 18 and 20, bringing back the CR2=0x18 kernel panic. Build amdgpu_ttm.o with -fno-lto so the guards survive. Note: the deployed build2 tree does not carry this flag — listed here because the guards need it to survive a ThinLTO build.",
        "Makefile",
        Tell::Bundled
    ),
    patch!(
        "23",
        "23-gb-addr-config-num-se.patch",
        "Fix GB_ADDR_CONFIG NUM_SHADER_ENGINES 0→2 (BC-250 community)",
        "The stock mmGB_ADDR_CONFIG=0x00000044 sets NUM_SHADER_ENGINES=0. The BC-250 community reports 0x00100044 (bit 20→NUM_SE=2) as required for ROCm. GB_ADDR_CONFIG controls pipe interleave and shader-engine address swizzle; a wrong SE count causes distinct GPU addresses to collide — the measured aliasing signature. Credit: BC-250 community, documented by GabriWar.",
        "gfx_v10_0.c",
        Tell::Bundled
    ),
    patch!(
        "24",
        "24-gmc-v10-flush-all-vmids.patch",
        "Flush all mapped VMIDs on BC-250 — direct MMIO path, no KIQ",
        "The PASID-to-VMID query in gmc_v10_0_flush_gpu_tlb_pasid() races KFD \
         teardown and the INVALIDATE_TLBS packet wedges KIQ on this board. \
         Instead, invalidate every currently mapped process VMID through the \
         direct register path on every TLB invalidation, and keep the BC-250 \
         off the KIQ branch in gmc_v10_0_flush_gpu_tlb(). Deployed form on \
         snap-a0af1eeb and snap-8fe794e8: logs \"BC-250: %d VMID(s) flushed\". \
         By Gabriel Duarte Guerra.",
        "gmc_v10_0.c",
        Tell::Bundled
    ),
    patch!(
        "25",
        "25-bc250-flush-tlb-by-runlist.patch",
        "Flush the compute TLB by rebuilding the runlist (fixes aliasing)",
        "The compute TLB on gfx1013 is never invalidated: the PASID scan finds \
         zero VMIDs (20/20) because mmATC_VMID*_PASID_MAPPING is never written \
         under HWS, and forced MMIO flushes wedge the GPU. Rebuilding the \
         runlist on unmap makes the firmware invalidate for real: 36 \
         counterbalanced runs go from 13/18 dirty to 0/18 dirty \
         (Fisher p=3.7e-06), verified active via ftrace \
         execute_queues_cpsch counts. This is the patch that made PyTorch \
         work on the BC-250 — deployed as snap-8fe794e8 on blade 15 with \
         amdgpu.bc250_flush_by_runlist=1. By Gabriel Duarte Guerra.",
        "kfd_device_queue_manager.c, kfd_chardev.c, kfd_device_queue_manager.h",
        Tell::ModParam("bc250_flush_by_runlist")
    ),
patch!(
        "26",
        "26-bc250-sdma-firmware-override.patch",
        "SDMA firmware override (navi10/navi12 blob instead of cyan)",
        "The cyan_skillfish2 SDMA firmware never drives the user queues \
         (GabriWar docs/28+29: 0 bytes copied, signal never drops; any other \
         SDMA 5.0 blob copies 4 MiB in 0.04 s with correct data). This is the \
         exit strategy for patch 19: validate with HSA_ENABLE_SDMA=1 round \
         trips, then retire bc250_skip_sdma0. Gated by \
         amdgpu.bc250_sdma_fw=<base>; empty = stock cyan blob.",
        "amdgpu_sdma.c",
        Tell::ModParam("bc250_sdma_fw")
    ),
patch!(
        "27",
        "27-bc250-early-sdma-trap.patch",
        "Write SDMA TRAP_ENABLE during gfx_resume",
        "TRAP_ENABLE is normally written only after amdgpu_device_ip_init() \
         returns in full, so early SDMA0 fence waits always hit the 500 ms \
         fallback timer — the two 'Fence fallback timer expired on ring sdma0' \
         boot lines. Writing the bit in gfx_resume removes them; a readback \
         log proves the write landed (GabriWar docs/28). Ship together with \
         patch 26.",
        "sdma_v5_0.c",
        Tell::ModParam("bc250_early_sdma_trap")
    ),
    patch!(
        "28",
        "28-bc250-8core-telemetry.patch",
        "8-core hybrid SMU metrics layout (fixes GPU clock reporting)",
        "After the 8-core CPU unlock the firmware redistributes the per-core \
         arrays inside the fixed 116-byte metrics table and GfxclkFrequency \
         loses its slot (offset 0x44 is now C0Residency[6]) — pp_dpm_sclk, \
         OD_SCLK, gpu_metrics and hwmon all report garbage. Auto-detects the \
         8-core topology (or amdgpu.cs_eight_core_map=1 to force) and \
         reinterprets the table with the empirically mapped hybrid layout, \
         querying gfxclk direct from the SMU (QueryGfxclk, serialized under \
         msg_ctl.lock) with a table fallback. Safe on 6-core blades: \
         auto-detect keeps the stock layout. Ported from GabriWar's \
         0001-bc250-8core-telemetry (core-count helper by FilippoR).",
        "cyan_skillfish_ppt.c, smu11_driver_if_cyan_skillfish.h",
        Tell::ModParam("cs_eight_core_map")
    ),

];

/// Patches that ship on disk and are tracked in the TUI, but are NOT part of
/// the applied series — `aputune build` does not materialize them. Opt one in
/// by moving its entry into [`SERIES`] (mind apply-order and hunk overlaps).
pub const ON_DISK: &[Patch] = &[
    patch!(
        "12",
        "12-unlock-all-40-compute-units.patch",
        "Vulkan-only 40-CU unlock (Studebaker)",
        "The original 40-CU unlock that also writes RLC_PG_ALWAYS_ON_WGP_MASK. \
         Safe for Vulkan/RADV; hangs ROCm/HSA on the first KFD queue, which is \
         why patch 16 (no RLC write) is the applied one. Kept on disk for \
         Vulkan-only setups. Note: its tell is shared with patch 16, so the \
         TUI cannot tell the two apart on a live kernel.",
        "gfx_v10_0.c",
        Tell::ModParam("bc250_cc_write_mode")
    ),
    patch!(
        "19",
        "19-bc250-kfd-skip-sdma0.patch",
        "BC-250 SDMA0 skip — restrict user queues to SDMA1 (retired)",
        "Retired from the applied series by 26+27 (navi12 SDMA firmware \
         override + early TRAP_ENABLE). Kept on disk as the fallback if the \
         sdma0 engine still misbehaves after the swap — re-arm by moving \
         back into SERIES and re-adding amdgpu.bc250_skip_sdma0=1.",
        "kfd_device_queue_manager.c",
        Tell::ModParam("bc250_skip_sdma0")
    ),
    patch!(
        "21",
        "21-amdgpu-gmc-flush-pasid-kiq.patch",
        "KIQ PASID-flush disable (neoney, verified by GabriWar)",
        "Sets flush_pasid_uses_kiq = false for gfx1013 so PASID TLB flushes go \
         through direct MMIO instead of the KIQ INVALIDATE_TLBS packet that \
         wedges this board. Superseded in the applied series by patch 14(e), \
         which implements the same change — kept on disk for attribution and \
         as a standalone reference.",
        "gmc_v10_0.c",
        Tell::Bundled
    ),
];

/// Number of patches in the embedded series.
pub fn count() -> usize {
    SERIES.len()
}

/// Series members whose absence does not poison the bundled-inference or the
/// "fully patched" verdict: a kernel without them is still reported complete.
/// Patch 17 is the diagnostic fault probe — production builds (e.g. the
/// deployed build2 tree on blade 15) deliberately do not carry it, so its
/// missing tell must not drag every Bundled member to `[??]`.
pub const OPTIONAL: &[&str] = &["17"];
