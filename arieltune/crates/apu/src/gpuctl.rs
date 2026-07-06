// SPDX-License-Identifier: GPL-2.0-only
//! Shared GPU power state-machine transitions — THE one implementation of the
//! force / unforce paths, used by the CLI (`gpu force|unforce`), the TUI, and
//! `profile apply`, so no caller can bypass the safety ordering:
//!
//!   * force: raise a persisted GPU undervolt to the TARGET clock's floor
//!     BEFORE the clock moves (voltage-first — forcing a high clock onto an
//!     undervolt sized for a lower one is the crash class), persist, then make
//!     the single GPU power unit re-enact it.
//!   * unforce: release the clock + voltage, clear ONLY the manual pin
//!     (`auto_mode` is preserved — releasing a pin must not silently flip a
//!     box from autosleep/released to governor), persist, re-enact.

use anyhow::{bail, Result};

use crate::{dpm, persist, telemetry};
use ariel_smu::smu::{self, Smu};

/// Outcome of a [`force`] transition.
pub struct ForceOutcome {
    /// The clock actually pinned (after silicon-range clamping).
    pub set_mhz: u32,
    /// Whether the boot re-apply unit is holding the pin (false = live-only).
    pub held: bool,
    /// A persisted undervolt that had to be raised: (old mV, new floor mV).
    pub vid_raised: Option<(u32, u32)>,
}

/// If the persisted undervolt in `cfg` is below the safe floor for `target_mhz`,
/// return the (old, floor) raise it needs. Pure decision — the caller performs
/// the actual od write.
pub fn required_vid_raise(cfg: &dpm::PowerConfig, target_mhz: u32) -> Option<(u32, u32)> {
    let v = cfg.force_vid_mv?;
    let floor = telemetry::min_gfx_vddc(target_mhz);
    (v < floor).then_some((v, floor))
}

/// Full manual-pin transition. Voltage-first ordering: if a persisted undervolt
/// is below the floor for the TARGET clock, the od raise must LAND before the
/// clock is forced — and if that raise fails, the clock is left untouched and
/// the error surfaced (never "clock up, voltage still sized for the old clock").
pub fn force(mhz: u32) -> Result<ForceOutcome> {
    let _lock = dpm::ConfigLock::acquire();
    let target = mhz.clamp(smu::SCLK_MIN_MHZ, smu::SCLK_MAX_MHZ);
    let mut cfg = dpm::PowerConfig::load_or_default();
    let mut vid_raised = None;
    if let Some((old, floor)) = required_vid_raise(&cfg, target) {
        if !telemetry::od_set_vddc(target, floor) {
            bail!(
                "cannot raise the persisted GPU undervolt {old} -> {floor} mV (safe floor \
                 for {target} MHz): overdrive write failed — clock left UNCHANGED. \
                 Fix overdrive (amdgpu.ppfeaturemask / root) or release the voltage first."
            );
        }
        vid_raised = Some((old, floor));
    }
    let set = Smu::open()?.force_gfx_freq(target)?;
    cfg.force_mhz = Some(set);
    if let Some((_, floor)) = vid_raised {
        // Persist the raise only now that the od write already succeeded.
        cfg.force_vid_mv = Some(floor);
    }
    cfg.save()?;
    persist::log_transition(&format!("force {set}"));
    let held = persist::apply_mode().is_ok();
    Ok(ForceOutcome {
        set_mhz: set,
        held,
        vid_raised,
    })
}

/// Full release-to-auto transition: unforce the clock, restore the stock
/// voltage curve, clear the manual pin + forced voltage, and re-enact the
/// PRESERVED auto mode (governor/autosleep/released stays whatever it was).
pub fn unforce() -> Result<dpm::AutoMode> {
    let _lock = dpm::ConfigLock::acquire();
    Smu::open()?.unforce_gfx_freq()?;
    // Restore the stock voltage curve via amdgpu overdrive (SMU-safe).
    let _ = telemetry::od_reset();
    let mut cfg = dpm::PowerConfig::load_or_default();
    cfg.force_mhz = None;
    cfg.force_vid_mv = None;
    let mode = cfg.auto_mode;
    cfg.save()?;
    persist::log_transition("unforce -> auto (mode preserved)");
    let _ = persist::apply_mode();
    Ok(mode)
}

/// Human tag for an auto mode (status lines).
pub fn mode_name(m: dpm::AutoMode) -> &'static str {
    match m {
        dpm::AutoMode::Governor => "governor",
        dpm::AutoMode::Autosleep => "autosleep",
        dpm::AutoMode::Released => "released (BAPM)",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CRASH-CLASS REGRESSION: forcing a clock above what a persisted undervolt
    /// was sized for must demand a voltage raise; a clock at/below the floor's
    /// envelope must not. (min_gfx_vddc falls back to the stock OD defaults on
    /// a box without amdgpu, so the decision is testable anywhere.)
    #[test]
    fn force_above_undervolt_requires_raise() {
        // an undervolt sized for ~1500 MHz
        let mut cfg = dpm::PowerConfig {
            force_vid_mv: Some(860),
            ..Default::default()
        };
        let raise = required_vid_raise(&cfg, 2230);
        let floor = telemetry::min_gfx_vddc(2230);
        assert!(floor > 860, "floor for 2230 MHz should exceed 860 mV");
        assert_eq!(raise, Some((860, floor)));
        // No persisted undervolt -> nothing to raise.
        cfg.force_vid_mv = None;
        assert_eq!(required_vid_raise(&cfg, 2230), None);
        // A generous voltage needs no raise at the low clock.
        cfg.force_vid_mv = Some(1100);
        assert_eq!(required_vid_raise(&cfg, 1000), None);
    }
}
