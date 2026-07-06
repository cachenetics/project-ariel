// SPDX-License-Identifier: GPL-2.0-only
//! Per-setting risk classification — the "will this brick a stranger's board?" gate.
//!
//! arieltune ships to people who did not do the reverse-engineering and cannot
//! recover a bricked board without an external SPI programmer. Most BIOS settings
//! are harmless or outright decorative, but a handful can prevent POST, need a
//! power-cycle/CMOS-clear to undo, or destabilise a rail. This module tags each
//! setting from the empirical BC-250 evidence so the write paths can refuse or
//! warn accordingly.
//!
//! Classification is data-first (an explicit table keyed by the setting name /
//! varstore+offset for the settings we actually tested) then falls back to
//! keyword heuristics for the dangerous *classes* (memory training, voltage
//! loadlines, PCIe lane equalisation, MCA/security masks, core topology) so it
//! generalises across the ~1245-entry catalogue instead of only the tested few.

use crate::catalog::Setting;

/// How dangerous is changing this setting on someone else's board.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Risk {
    /// Reversible, no boot or hardware hazard. Write freely.
    Safe,
    /// Can misbehave (black-screen, link/VRM instability) but is recoverable by
    /// re-setting the value or an NVRAM clear. Needs an explicit acknowledgement.
    Caution,
    /// Can prevent POST. Recovery may need a CMOS clear or an external SPI
    /// programmer. Needs an explicit acknowledgement + the recovery warning.
    Brick,
    /// Applies but does NOT revert by re-setting the value — the change latches
    /// in a power-domain/CMOS store and only a power-cycle (or CMOS clear) undoes
    /// it. Reversible, but not the way a user expects. Needs acknowledgement.
    OneWay,
}

impl Risk {
    /// Anything but `Safe` requires the operator to pass `--force`.
    pub fn requires_force(self) -> bool {
        self != Risk::Safe
    }

    /// Short tag for the TUI / listings.
    pub fn tag(self) -> &'static str {
        match self {
            Risk::Safe => "",
            Risk::Caution => "CAUTION",
            Risk::Brick => "BRICK-RISK",
            Risk::OneWay => "ONE-WAY",
        }
    }
}

/// The verdict for one setting: its risk, why, and how to recover if it goes wrong.
pub struct Assessment {
    pub risk: Risk,
    pub reason: &'static str,
    pub recovery: &'static str,
}

const RECOVER_RESET: &str = "revert by setting the value back (or an NVRAM clear) and rebooting";
const RECOVER_POWERCYCLE: &str =
    "this latches beyond a reflash — revert needs a full AC power-cycle (set the value \
     back to default FIRST, then power-cycle), or a CMOS clear";
const RECOVER_PROGRAMMER: &str =
    "a bad value can stop the board POSTing — recovery may need a CMOS clear or an \
     external SPI programmer (SOIC-8 clip). Have a flash backup and a rig before using this";

fn has(hay: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| hay.contains(n))
}

/// Classify a setting. Ordering matters: the most dangerous class that matches
/// wins, so e.g. a DRAM *timing* field is Brick even though it is also "memory".
pub fn assess(s: &Setting) -> Assessment {
    let name = s.name.to_lowercase();
    let cat = s.category.to_lowercase();
    // Telemetry / readback rows are reporting-only (they surface a measured
    // value, they do not drive a rail). Never let the voltage rules gate them.
    let is_telemetry = name.contains("telemetry");
    let a = |risk, reason, recovery| Assessment {
        risk,
        reason,
        recovery,
    };

    // --- BRICK by CATEGORY: whole dangerous classes ------------------------
    // Some catalog categories are dangerous IN THEIR ENTIRETY -- every member
    // either fails DRAM training or pokes a raw UMC register, so a bad value in
    // any of them can corrupt the memory controller / stop the board POSTing.
    // These ship under GDDR6/UMC *categories* but NOT a mem/umc/dram *name*, so
    // the name-token path below misses siblings like TrrdS/TFaw/Trtp/TrcAb and
    // the UMCCONFIG*/UMCCTRL_MISC*/DFE-RX register pokes -- those would classify
    // Safe and write with no --force. Gate the whole category to Brick:
    //   * "I Accept"                            -- GDDR6 timings + memory clock
    //   * "GDDR6 DRAM Timing Configuration"     -- GDDR6 timings + UCLK dividers
    //   * "GDDR6 Diagnostic and Debug Features" -- refresh/calibration/VMEMP/DFE
    //   * "UMCCONFIG Control"                   -- raw UMCCONFIG mask/value + DFE RX
    //   * "UMCCTRL_MISC Control"                -- raw UMCCTRL_MISC mask/value pokes
    // Verified against catalog.json: none of these categories carries a benign /
    // informational / telemetry row (every member is training- or register-class),
    // so the whole-category gate cannot over-classify a harmless setting.
    const BRICK_CATEGORIES: &[&str] = &[
        "i accept",
        "gddr6 dram timing configuration",
        "gddr6 diagnostic and debug features",
        "umcconfig control",
        "umcctrl_misc control",
    ];
    if BRICK_CATEGORIES.contains(&cat.as_str()) {
        return a(
            Risk::Brick,
            "GDDR6 timing / UMC register class -- a bad value fails DRAM training or \
             corrupts the memory controller, and the board won't POST",
            RECOVER_PROGRAMMER,
        );
    }

    // --- BRICK: can prevent POST -------------------------------------------
    // DRAM timing / training user-controls. Setting these Manual and writing a
    // bad value fails memory training -> no-POST (measured: dangerous without
    // empirical per-board testing; brick-class risk per memory).
    //
    // Two classes, deliberately gated differently:
    //  (a) UNAMBIGUOUS DRAM-timing name tokens (trcd/tras/trfc/tcl/twr/trp) are
    //      real GDDR6 timing fields wherever they appear — the actual catalog
    //      fields (Tcl/Tras/Trcdrd/Trfc/TrfcPb/Twrrd...) ship under category
    //      "I Accept" / "GDDR6 DRAM Timing Configuration", NOT a mem/umc/dram
    //      category, so a cat-gate here would misclassify them Safe and let a
    //      bad `bios set Tcl=..` through without --force -> failed training ->
    //      no-POST brick. Classify Brick on the NAME alone.
    //  (b) GENERIC tokens (timing/training) ARE ambiguous (fan timing, boot
    //      timing, link training, etc.) — keep those AND-gated with a memory
    //      category to avoid over-classifying unrelated "timing" settings.
    if has(&name, &["trcd", "tras", "trfc", "tcl", "twr", "trp"])
        || (has(&name, &["timing", "training"]) && has(&cat, &["mem", "umc", "dram", "ddr"]))
        || has(
            &name,
            &["dram timing", "memory training", "apudfe training"],
        )
    {
        return a(
            Risk::Brick,
            "DRAM timing/training — a bad value fails memory training and the board won't POST",
            RECOVER_PROGRAMMER,
        );
    }

    // --- BRICK: direct FIXED-RAIL voltage override -------------------------
    // A fixed-rail override (VCORE / GFX / MEMIO "Fix Voltage") drives a rail to
    // an ABSOLUTE value: too high can DAMAGE the silicon, too low won't POST.
    // Unlike an offset or a loadline this is a hard override -- brick-class.
    // (Telemetry rows are reporting-only and excluded.)
    if !is_telemetry && name.contains("fix voltage") {
        return a(
            Risk::Brick,
            "fixed-rail voltage override — too high can damage the silicon, too low won't POST",
            RECOVER_PROGRAMMER,
        );
    }

    // --- ONE-WAY: latches beyond a reflash ---------------------------------
    // SMT / downcore / core-topology. Proven on a test BC-250:
    // SMT-disable latches in an always-on power-domain register that survives
    // warm reset and a full SPI reflash; only a real power loss clears it.
    // Only genuine core-*topology* latches, not per-core power knobs (DPM /
    // watchdog / MSR-access), which are ordinary NVRAM-recoverable settings.
    if has(
        &name,
        &[
            "smt",
            "downcore",
            "core control",
            "cores per",
            "core count",
            "ccd control",
            "ccx control",
        ],
    ) {
        return a(
            Risk::OneWay,
            "CPU core topology (SMT/downcore) latches in a power-domain register",
            RECOVER_POWERCYCLE,
        );
    }

    // --- CAUTION: recoverable but can misbehave ----------------------------
    // IOMMU can black-screen the BC-250 (no display output path if AMD-Vi
    // reroutes the GPU). Measured live consumer; shipped CLI-only.
    if has(&name, &["iommu"]) {
        return a(
            Risk::Caution,
            "IOMMU can black-screen the BC-250 (headless, no display output)",
            RECOVER_RESET,
        );
    }
    // Voltage loadlines — default impedance is correct; manual risks VRM stability
    // (measured: change risks VRM stability).
    if has(&name, &["loadline", "load line", "ll trim"]) {
        return a(
            Risk::Caution,
            "voltage loadline — a manual value can destabilise the VRM",
            RECOVER_RESET,
        );
    }
    // Direct voltage overrides — offsets (VDDCR_VDD/GFX Voltage Offset), DC-BTC
    // set-points (CPU/GFX DC BTC Set voltage), and the non-fixed "Voltage
    // Configuration" entries (Voltage(mV)/Voltage). A manual value can destabilise
    // a rail: too low no-POST, too high stresses the silicon. These are offsets /
    // calibration points, not a hard fixed rail (Fix Voltage is Brick above), so
    // Caution -- but they still require --force. Telemetry rows are excluded.
    if !is_telemetry
        && (has(&name, &["voltage offset", "set voltage"]) || cat == "voltage configuration")
    {
        return a(
            Risk::Caution,
            "direct voltage override — a manual value can destabilise the rail \
             (too low can fail to POST, too high stresses the silicon)",
            RECOVER_RESET,
        );
    }
    // GFX / DF clock overrides — GfxCLKFreq (Mode Config), GfxClkDfll,
    // GFXCLK_EFFECTIVE_FREQ, DF Clock Override. Forcing a clock off the
    // AGESA-tuned point can wedge the GPU or fail to POST; recoverable by clearing
    // the override, but needs acknowledgement.
    if has(
        &name,
        &[
            "gfxclkfreq",
            "gfxclkdfll",
            "gfxclk_effective_freq",
            "df clock override",
        ],
    ) || cat == "mode config"
    {
        return a(
            Risk::Caution,
            "GFX/DF clock override — forcing the clock off the AGESA-tuned point can \
             wedge the GPU or prevent POST",
            RECOVER_RESET,
        );
    }
    // PCIe lane equalisation — 255=Auto is AGESA-tuned per silicon; manual values
    // risk PCIe link instability (measured).
    if has(
        &name,
        &[
            "txeq",
            "rxeq",
            "de-emphasis",
            "deemphasis",
            "lane eq",
            "preset",
            "swing",
            "rx ctle",
        ],
    ) && has(&cat, &["pcie", "nbio", "dxio", "gpp", "gnb", "port"])
    {
        return a(
            Risk::Caution,
            "PCIe lane equalisation — a manual value can make the PCIe link unstable",
            RECOVER_RESET,
        );
    }
    // MCA masks + memory-error controls — masking incorrectly hides real silicon
    // errors (measured).
    if has(&name, &["mca", "machine check", "error threshold", "ecc"])
        && has(&name, &["mask", "disable", "control", "threshold"])
    {
        return a(
            Risk::Caution,
            "machine-check/ECC control — masking can hide real silicon errors",
            RECOVER_RESET,
        );
    }
    // Security-processor / memory-encryption toggles — not compute-relevant and
    // can change the trust/boot path.
    if has(
        &name,
        &["sev", "sme", "tsme", "psb", "platform secure boot", "smee"],
    ) {
        return a(
            Risk::Caution,
            "security/encryption feature — off-scope for tuning and changes the boot/trust path",
            RECOVER_RESET,
        );
    }
    // Memory clock / voltage overrides in OTHER categories (uclk/vmemp/memclk/
    // gddr/vddio/memory-voltage names not already caught by the GDDR6/UMC
    // category gate above). memtune owns the real memory path; flag here so a
    // stray write is acknowledged. FIX 1e: recovery is RECOVER_PROGRAMMER, NOT
    // RECOVER_RESET — if UCLK/VMEMP fails training the board won't POST, so you
    // can't boot to "set the value back and reboot"; recovery needs an external
    // programmer. (The named UclkDiv1M*/VMEMP/Memory Clock Speed settings ship
    // under the GDDR6/"I Accept" categories and are already Brick above; this
    // branch is the belt-and-suspenders net for any stragglers elsewhere.)
    if has(
        &name,
        &[
            "memclk",
            "memory clock",
            "mem clock",
            "uclk",
            "vmemp",
            "gddr",
            "memory voltage",
            "vddio",
        ],
    ) {
        return a(
            Risk::Caution,
            "memory clock/voltage — an aggressive value can fail training and the \
             board won't POST (you cannot boot to revert)",
            RECOVER_PROGRAMMER,
        );
    }
    // Spread spectrum — no measured benefit, can break EMI compliance / marginal
    // clocks (measured).
    if has(&name, &["spread spectrum", "spread-spectrum"]) {
        return a(
            Risk::Caution,
            "spread spectrum — no measured benefit and can affect clock/EMI margins",
            RECOVER_RESET,
        );
    }
    // Fast Boot skips USB enumeration, making recovery from a bad setting harder
    // (explicitly excluded for this reason).
    if has(&name, &["fast boot", "fastboot", "quiet boot skip"]) {
        return a(
            Risk::Caution,
            "Fast Boot skips USB init — it makes recovering from a later bad setting harder",
            RECOVER_RESET,
        );
    }

    a(
        Risk::Safe,
        "reversible, no boot or hardware hazard known",
        RECOVER_RESET,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(name: &str, cat: &str, varstore: &str) -> Setting {
        Setting {
            category: cat.into(),
            name: name.into(),
            offset: 0,
            bits: 8,
            default: None,
            options: vec![(0, "Disabled".into()), (1, "Enabled".into())],
            range: None,
            varstore: varstore.into(),
        }
    }

    #[test]
    fn dram_timing_is_brick() {
        assert_eq!(
            assess(&s("Tcl Ctrl", "Umc Common", "AmdSetup")).risk,
            Risk::Brick
        );
        assert_eq!(
            assess(&s("DRAM Timing Configuration", "DDR4 Common", "AmdSetup")).risk,
            Risk::Brick
        );
    }

    #[test]
    fn bare_dram_timing_names_under_i_accept_are_brick() {
        // H1 regression: the real catalog fields ship under category "I Accept"
        // (or "GDDR6 DRAM Timing Configuration"), NOT a mem/umc category. Before
        // the fix these classified Safe (cat-gate missed) and a `bios set` would
        // proceed without --force -> failed memory training -> no-POST brick.
        // They must be Brick on the NAME alone.
        for name in [
            "Tcl", "Tras", "Trfc", "Trcdrd", "Trcdwr", "TrfcPb", "Twrrd", "TrpAb",
        ] {
            assert_eq!(
                assess(&s(name, "I Accept", "AmdSetup")).risk,
                Risk::Brick,
                "{name} (I Accept) should be Brick on the name alone"
            );
        }
    }

    #[test]
    fn sibling_gddr6_timings_are_brick() {
        // FIX 1 (H1): GDDR6 timing SIBLINGS that carry no trcd/tras/trfc/tcl/twr/
        // trp name token still ship under "I Accept" / "GDDR6 DRAM Timing
        // Configuration" -- before the category gate they classified Safe and a
        // `bios set` proceeded with no --force -> failed training -> no-POST brick.
        for (name, cat) in [
            ("TrrdS", "I Accept"),
            ("TFaw", "I Accept"),
            ("Trtp", "I Accept"),
            ("TrcAb", "I Accept"),
            ("Tref", "I Accept"),
            ("Trdwr", "GDDR6 DRAM Timing Configuration"),
            ("TrdrdSc", "GDDR6 DRAM Timing Configuration"),
            ("UclkDiv1M0", "GDDR6 DRAM Timing Configuration"),
            (
                "Additional CAS-CAS Delay Cycles",
                "GDDR6 Diagnostic and Debug Features",
            ),
            (
                "VMEMP Voltage Control",
                "GDDR6 Diagnostic and Debug Features",
            ),
        ] {
            let r = assess(&s(name, cat, "AmdSetup"));
            assert_eq!(r.risk, Risk::Brick, "{name} ({cat}) should be Brick");
            assert!(r.risk.requires_force(), "{name} must require --force");
        }
    }

    #[test]
    fn raw_umc_register_pokes_are_brick() {
        // FIX 1 (H1): raw UMC register pokes are maximally brick-capable and ship
        // under "UMCCONFIG Control" / "UMCCTRL_MISC Control" with no mem/umc name
        // token -> must be gated Brick by category.
        for (name, cat) in [
            ("UMCCONFIG0 Value", "UMCCONFIG Control"),
            ("UMCCONFIG0 Mask", "UMCCONFIG Control"),
            ("DFE RX Value UMC0 Channel0", "UMCCONFIG Control"),
            ("UMCCTRL_MISC0 Value", "UMCCTRL_MISC Control"),
        ] {
            assert_eq!(
                assess(&s(name, cat, "AmdSetup")).risk,
                Risk::Brick,
                "{name} ({cat}) should be Brick"
            );
        }
    }

    #[test]
    fn fixed_rail_voltage_is_brick_offsets_and_clocks_caution() {
        // FIX 1 (H1): a fixed-rail override is damage-capable -> Brick; offsets /
        // DC-BTC set-points / non-fixed Voltage Configuration rows -> Caution;
        // GFX/DF clock overrides -> Caution.
        for name in ["VCORE Fix Voltage", "GFX Fix Voltage", "MEMIO Fix Voltage"] {
            assert_eq!(
                assess(&s(name, "Voltage Configuration", "Setup")).risk,
                Risk::Brick,
                "{name} should be Brick"
            );
        }
        for (name, cat) in [
            ("VDDCR_GFX Voltage Offset", "SMU Debug"),
            ("CPU DC BTC Set voltage in mV", "SMU Debug"),
            ("Voltage(mV)", "Voltage Configuration"),
            ("GfxCLKFreq Control", "Mode Config"),
            ("Native Mode GfxCLKFreq", "Mode Config"),
            ("DF Clock Override", "SMU Debug"),
            ("GfxClkDfll", "Gfx Config"),
            ("GFXCLK_EFFECTIVE_FREQ", "SMU Features"),
        ] {
            let r = assess(&s(name, cat, "AmdSetup"));
            assert_eq!(r.risk, Risk::Caution, "{name} ({cat}) should be Caution");
            assert!(r.risk.requires_force(), "{name} must require --force");
        }
    }

    #[test]
    fn no_over_classification_benign_and_telemetry_stay_safe() {
        // Guard against over-classification. A telemetry/readback row is
        // reporting-only and must stay Safe even though its name carries a rail
        // token, and a benign feature toggle must not be dragged into a gate.
        for (name, cat) in [
            ("VDDCR_VDD Telemetry slope", "CBS Telemetry setup"),
            (
                "VDDCR_GFX Telemetry Offset Value (mA)",
                "CBS Telemetry setup",
            ),
            ("Memory Context Restore", "Umc Common"),
            ("Above 4G Decoding", "Advanced"),
            ("DS_GFXCLK", "SMU Features"),
        ] {
            assert_eq!(
                assess(&s(name, cat, "AmdSetup")).risk,
                Risk::Safe,
                "{name} ({cat}) should stay Safe (no over-classification)"
            );
        }
    }

    #[test]
    fn memory_clock_straggler_recovers_via_programmer() {
        // FIX 1e: a mem clock/voltage name in a NON-gated category is Caution but
        // must carry the programmer recovery, NOT the reboot-to-revert string --
        // if UCLK/VMEMP fails training you cannot boot to undo it.
        let r = assess(&s("VDDIO Control", "DRAM Power Options", "AmdSetup"));
        assert_eq!(r.risk, Risk::Caution);
        assert_eq!(r.recovery, RECOVER_PROGRAMMER);
    }

    #[test]
    fn smt_and_downcore_are_one_way() {
        assert_eq!(
            assess(&s("SMT Control", "CPU Common", "AmdSetup")).risk,
            Risk::OneWay
        );
        assert_eq!(
            assess(&s("Downcore Control", "CPU Common", "AmdSetup")).risk,
            Risk::OneWay
        );
    }

    #[test]
    fn iommu_and_loadline_are_caution() {
        assert_eq!(
            assess(&s("IOMMU", "CPU Configuration", "Setup")).risk,
            Risk::Caution
        );
        assert_eq!(
            assess(&s("VCORE Loadline", "Voltage", "Setup")).risk,
            Risk::Caution
        );
    }

    #[test]
    fn known_safe_effective_settings_are_safe() {
        // The proven-effective, reversible knobs must not trip a gate.
        for name in [
            "Above 4G Decoding",
            "SVM Mode",
            "Cool'n'Quiet",
            "C6 Mode",
            "GPP Link ASPM",
            "Extended Tag",
            "Above 4G Memory",
        ] {
            assert_eq!(
                assess(&s(name, "Advanced", "Setup")).risk,
                Risk::Safe,
                "{name} should be Safe"
            );
        }
    }

    #[test]
    fn memory_is_not_blanket_brick() {
        // A plain memory *feature* toggle (not a timing) is not brick-class.
        assert_eq!(
            assess(&s("Memory Context Restore", "Umc Common", "AmdSetup")).risk,
            Risk::Safe
        );
    }

    #[test]
    fn force_needed_for_nonsafe() {
        assert!(!Risk::Safe.requires_force());
        assert!(Risk::Caution.requires_force());
        assert!(Risk::Brick.requires_force());
        assert!(Risk::OneWay.requires_force());
    }
}
