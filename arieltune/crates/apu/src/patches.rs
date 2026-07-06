// SPDX-License-Identifier: GPL-2.0-only
//! The BC-250 liberation kernel-patch series, embedded into the binary.
//!
//! aputune *owns* the silicon-liberation surface: the curated 12-patch CachyOS amdgpu
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
//! The patches themselves are GPL-2.0 (kernel diffs), the same license as the
//! rest of arieltune (GPL-2.0-only). arieltune carries them as build assets the
//! way a package manager carries the patches it applies - they are not linked
//! into the binary.

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
        "12",
        "12-unlock-all-40-compute-units.patch",
        "amdgpu.bc250_cc_write_mode: CC + SPI(0x1F) + RLC(0x1F) -> 40 CUs",
        "Re-enables all 40 CUs on the Sony-binned board (default 24). Clears \
         the CC_GC_SHADER_ARRAY_CONFIG harvest mask, sets the per-shader-array \
         SPI dispatch mask to 0x1F, and the RLC power-gate override to 0x1F — \
         all three are required; any one alone is a no-op. Gated on PCI \
         0x13FE and off by default (bc250_cc_write_mode; mode 3 = full \
         liberation, measured +69% FP32).",
        "gfx_v10_0.c",
        Tell::ModParam("bc250_cc_write_mode")
    ),
];

/// Number of patches in the embedded series.
pub fn count() -> usize {
    SERIES.len()
}
