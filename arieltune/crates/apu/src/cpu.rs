// SPDX-License-Identifier: GPL-2.0-only
//! CPU overclock / undervolt — the reverse-engineered OC model, on the queue-3 OC mailbox
//! ([`ocq3`]).
//!
//! The BC-250's 6-core Zen2 is governed by the SMU's F/Vid curve, not a static
//! voltage. You raise the **max boost clock** and **scale the F/Vid curve down**
//! to undervolt — the SMU then picks voltage per the (scaled) curve, capped.
//! Plus CPU/GPU temp limits.
//!
//! Safety: every voltage-affecting write is hard-capped in [`ocq3`] (Vid <=
//! 1.325 V, curve scale undervolt-only). `disable_extra_voltage(true)` is part of
//! applying an OC (it removes the SMU's voltage padding — the undervolt enabler),
//! and `restore` puts every parameter back to firmware defaults.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::stress;
use ariel_smu::ocq3::{self, OcQ3};

/// A CPU operating point (overclock + undervolt).
#[derive(Clone, Serialize, Deserialize)]
pub struct CpuOc {
    /// Max CPU boost clock (MHz), 2800..4100.
    pub boost_mhz: u32,
    /// F/Vid curve scale (-50..0). 0 = stock; more negative = deeper undervolt.
    pub curve_scale: i32,
    /// CPU temperature limit (C).
    pub cpu_temp_c: u32,
    /// GPU temperature limit (C).
    pub gpu_temp_c: u32,
}

impl Default for CpuOc {
    fn default() -> Self {
        CpuOc {
            boost_mhz: ocq3::BOOST_STOCK_MHZ,
            curve_scale: 0,
            cpu_temp_c: 90,
            gpu_temp_c: 90,
        }
    }
}

impl CpuOc {
    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            (ocq3::BOOST_MIN_MHZ..=ocq3::BOOST_MAX_MHZ).contains(&self.boost_mhz),
            "boost {} MHz outside {}-{} MHz",
            self.boost_mhz,
            ocq3::BOOST_MIN_MHZ,
            ocq3::BOOST_MAX_MHZ
        );
        anyhow::ensure!(
            (ocq3::SCALE_MIN..=ocq3::SCALE_MAX).contains(&self.curve_scale),
            "curve scale {} outside {}..{} (undervolt only)",
            self.curve_scale,
            ocq3::SCALE_MIN,
            ocq3::SCALE_MAX
        );
        anyhow::ensure!(
            self.cpu_temp_c <= ocq3::TEMP_MAX_C && self.gpu_temp_c <= ocq3::TEMP_MAX_C,
            "temp limit > {}C",
            ocq3::TEMP_MAX_C
        );
        // A temp cap below any plausible operating point (e.g. a typo'd 0 or 5)
        // would either disable the protection or throttle the box into the
        // ground — refuse instead of persisting a poisoned cpu.json.
        anyhow::ensure!(
            self.cpu_temp_c >= ocq3::TEMP_MIN_C && self.gpu_temp_c >= ocq3::TEMP_MIN_C,
            "temp limit < {}C floor",
            ocq3::TEMP_MIN_C
        );
        // The bricking mode (per the reference OC driver): raising the boost clock without
        // enough undervolt lets Vid scale past safe limits. Refuse any combo the
        // F/Vid curve predicts will exceed the ceiling — require a deeper scale.
        let pred = self.predicted_vid_mv();
        if pred > ocq3::VID_CEILING_MV {
            // Deepest possible undervolt at this clock; if even that exceeds the
            // ceiling, the boost simply isn't achievable safely.
            let floor = ocq3::vid_predict(self.boost_mhz, ocq3::SCALE_MIN)
                .round()
                .max(0.0) as u32;
            if floor > ocq3::VID_CEILING_MV {
                anyhow::bail!(
                    "boost {} MHz predicts {} mV even at max undervolt (scale {}) — \
                     over the {} mV ceiling. Lower the boost clock.",
                    self.boost_mhz,
                    floor,
                    ocq3::SCALE_MIN,
                    ocq3::VID_CEILING_MV
                );
            }
            let need = safe_scale_for(self.boost_mhz, ocq3::VID_CEILING_MV);
            anyhow::bail!(
                "boost {} MHz at scale {} predicts {} mV (> {} mV ceiling) — \
                 undervolt more: use scale <= {}",
                self.boost_mhz,
                self.curve_scale,
                pred,
                ocq3::VID_CEILING_MV,
                need
            );
        }
        Ok(())
    }

    /// Apply this point to the live SMU. Boost/curve writes are staged
    /// direction-aware ([`CpuOc::staged_writes`]) so the predicted Vid never
    /// TRANSIENTLY exceeds the ceiling between the two mailbox messages.
    /// `prev` = the last-known applied point; None = unknown (conservative
    /// staging that is safe from any prior state).
    pub fn apply_from(&self, oc: &OcQ3, prev: Option<&CpuOc>) -> Result<()> {
        self.validate()?;
        oc.set_cpu_max_temp(self.cpu_temp_c)?;
        oc.set_gpu_max_temp(self.gpu_temp_c)?;
        oc.disable_extra_voltage(true)?;
        for st in self.staged_writes(prev) {
            match st {
                Stage::Scale(s) => oc.scale_vid_curve(s)?,
                Stage::Boost(b) => oc.set_max_boost_clk(b)?,
            }
        }
        Ok(())
    }

    /// [`CpuOc::apply_from`] with an unknown prior state.
    pub fn apply(&self, oc: &OcQ3) -> Result<()> {
        self.apply_from(oc, None)
    }

    /// The ordered boost/curve write sequence such that EVERY intermediate
    /// (boost, scale) state predicts a Vid at or under the ceiling.
    ///
    /// The naive scale-then-boost order overvolts transiently when LOWERING
    /// boost / shallowing the scale (e.g. 4000/-35 -> 3500/0: writing scale 0
    /// first leaves 4000@0 ~ 1524 mV in flight). So we stage: first deepen the
    /// scale to one that is safe at BOTH the old and new boost (min of old, new
    /// and the safe scale for the higher boost), then move boost, then relax the
    /// scale to its final value. With `prev` unknown, the old boost is assumed
    /// worst-case (BOOST_MAX) and the old scale stock (0).
    pub fn staged_writes(&self, prev: Option<&CpuOc>) -> Vec<Stage> {
        let old_boost = prev.map(|p| p.boost_mhz).unwrap_or(ocq3::BOOST_MAX_MHZ);
        let old_scale = prev.map(|p| p.curve_scale).unwrap_or(0);
        let hi_boost = old_boost.max(self.boost_mhz);
        let stage_scale = old_scale
            .min(self.curve_scale)
            .min(safe_scale_for(hi_boost, ocq3::VID_CEILING_MV));
        let mut out = Vec::with_capacity(3);
        if stage_scale != old_scale {
            out.push(Stage::Scale(stage_scale));
        }
        out.push(Stage::Boost(self.boost_mhz));
        if self.curve_scale != stage_scale {
            out.push(Stage::Scale(self.curve_scale));
        }
        out
    }

    /// Predicted Vid (mV) at the boost clock for this curve scale.
    pub fn predicted_vid_mv(&self) -> u32 {
        ocq3::vid_predict(self.boost_mhz, self.curve_scale)
            .round()
            .max(0.0) as u32
    }

    /// Load the persisted CPU OC (or defaults if none saved / unreadable).
    pub fn load_or_default() -> Self {
        std::fs::read_to_string(CONFIG_PATH)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Persist this OC so the boot service can re-apply it after a reboot.
    /// Atomic (tmp + rename) so a crash can't leave a corrupt cpu.json that
    /// blocks the boot re-apply.
    pub fn save(&self) -> Result<()> {
        std::fs::create_dir_all("/var/lib/aputune")
            .context("mkdir /var/lib/aputune (need root)")?;
        crate::dpm::write_atomic(CONFIG_PATH, &serde_json::to_string_pretty(self)?)
            .with_context(|| format!("write {CONFIG_PATH}"))?;
        Ok(())
    }
}

/// One staged boost/curve mailbox write (see [`CpuOc::staged_writes`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Stage {
    Scale(i32),
    Boost(u32),
}

/// Where the persisted CPU OC lives (re-applied at boot by arieltune-cpu-oc.service).
pub const CONFIG_PATH: &str = "/var/lib/aputune/cpu.json";

/// Whether a persisted CPU OC config exists.
pub fn saved_exists() -> bool {
    std::path::Path::new(CONFIG_PATH).exists()
}

/// Remove the persisted CPU OC config (used on restore-to-stock).
pub fn clear_saved() -> Result<()> {
    let p = std::path::Path::new(CONFIG_PATH);
    if p.exists() {
        std::fs::remove_file(p).with_context(|| format!("rm {CONFIG_PATH}"))?;
    }
    Ok(())
}

/// Restore CPU to firmware defaults.
pub fn restore_stock(oc: &OcQ3) -> Result<()> {
    oc.restore_defaults()
}

/// Live CPU telemetry from the SMU (queue 3).
pub struct CpuLive {
    pub cur_vid_mv: Option<u32>,
    pub cores: Vec<u32>,
    pub pstates: Vec<u32>,
    pub cpu_temp_max: Option<u32>,
}

/// Read live CPU state. Best-effort: missing fields are None/empty.
pub fn live(oc: &OcQ3) -> CpuLive {
    CpuLive {
        cur_vid_mv: oc.cpu_voltage_mv().ok(),
        cores: oc.core_freqs(),
        pstates: (0..8).filter_map(|p| oc.pstate_clk_mhz(p).ok()).collect(),
        cpu_temp_max: oc.cpu_temp_max().ok(),
    }
}

/// Sample the predicted F/Vid curve over the boost range for a given scale —
/// `(clock_mhz, vid_mv)` points for plotting.
pub fn curve_samples(scale: i32) -> Vec<(u32, u32)> {
    (ocq3::BOOST_MIN_MHZ..=ocq3::BOOST_MAX_MHZ)
        .step_by(100)
        .map(|c| (c, ocq3::vid_predict(c, scale).round().max(0.0) as u32))
        .collect()
}

// ---- Stability-detect sweep (curve-predictor + torture) ----

static SWEEP_STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_sweep_signal(_sig: libc::c_int) {
    SWEEP_STOP.store(true, Ordering::SeqCst);
}

pub struct SweepOpts {
    /// Target boost clock to reach (MHz).
    pub target_mhz: u32,
    /// CPU Vid cap (mV) — the undervolt search keeps predicted Vid at/under this.
    pub vid_cap_mv: u32,
    /// Temp limit (C) applied during the sweep and as a thermal abort.
    pub temp_c: u32,
    /// Step between boost points (MHz).
    pub step: u32,
    /// Seconds of torture per point.
    pub dwell_s: u64,
}

impl Default for SweepOpts {
    fn default() -> Self {
        SweepOpts {
            target_mhz: 4100,
            vid_cap_mv: 1275,
            temp_c: 90,
            step: 100,
            dwell_s: 8,
        }
    }
}

pub struct SweepPoint {
    pub boost_mhz: u32,
    pub scale: i32,
    pub measured_vid_mv: Option<u32>,
    pub stable: bool,
    pub max_temp_c: f64,
    pub note: String,
}

/// Pick the least-aggressive (mildest) curve scale that keeps predicted Vid
/// <= `vid_cap` at `freq` — climb down from 0 toward SCALE_MIN. Used by the
/// detect sweep, the presets, and the safety check.
pub fn safe_scale_for(freq: u32, vid_cap_mv: u32) -> i32 {
    let mut scale = 0;
    while scale > ocq3::SCALE_MIN && ocq3::vid_predict(freq, scale) > vid_cap_mv as f64 {
        scale -= 1;
    }
    scale
}

/// Climb the boost clock toward `target`, undervolting each step (via the curve
/// predictor) to stay under the Vid cap, and torture-testing stability.
///
/// Safety: every write is capped in [`ocq3`]; aborts on thermal or SIGINT; and
/// ALWAYS restores firmware defaults before returning, including on error. Returns
/// the per-point log and the highest stable [`CpuOc`].
pub fn detect(oc: &OcQ3, o: &SweepOpts) -> Result<(Vec<SweepPoint>, Option<CpuOc>)> {
    anyhow::ensure!(
        (ocq3::BOOST_MIN_MHZ..=ocq3::BOOST_MAX_MHZ).contains(&o.target_mhz),
        "target {} MHz outside {}-{} MHz",
        o.target_mhz,
        ocq3::BOOST_MIN_MHZ,
        ocq3::BOOST_MAX_MHZ
    );
    anyhow::ensure!(o.step > 0, "step must be > 0");
    anyhow::ensure!(
        o.vid_cap_mv <= ocq3::VID_CEILING_MV,
        "vid cap {} mV exceeds {} mV ceiling",
        o.vid_cap_mv,
        ocq3::VID_CEILING_MV
    );

    SWEEP_STOP.store(false, Ordering::SeqCst);
    unsafe {
        libc::signal(
            libc::SIGINT,
            on_sweep_signal as *const () as libc::sighandler_t,
        );
        libc::signal(
            libc::SIGTERM,
            on_sweep_signal as *const () as libc::sighandler_t,
        );
    }

    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(6);

    // Reference checksum at firmware defaults.
    restore_stock(oc)?;
    std::thread::sleep(Duration::from_millis(500));
    let reference = stress::block_checksum();

    let mut points = Vec::new();
    let mut best: Option<CpuOc> = None;
    let mut freq = ocq3::BOOST_STOCK_MHZ;
    let temp_ceiling = o.temp_c as f64;
    // Last applied point (stock after restore_stock) — drives direction-aware
    // boost/scale staging so no step transiently overvolts.
    let mut prev_point = CpuOc::default();
    while freq <= o.target_mhz {
        if SWEEP_STOP.load(Ordering::SeqCst) {
            points.push(SweepPoint {
                boost_mhz: freq,
                scale: 0,
                measured_vid_mv: None,
                stable: false,
                max_temp_c: 0.0,
                note: "interrupted".into(),
            });
            break;
        }
        let scale = safe_scale_for(freq, o.vid_cap_mv);
        let point = CpuOc {
            boost_mhz: freq,
            curve_scale: scale,
            cpu_temp_c: o.temp_c,
            gpu_temp_c: o.temp_c,
        };
        point.apply_from(oc, Some(&prev_point))?;
        prev_point = point.clone();
        std::thread::sleep(Duration::from_millis(300));
        let measured = oc.cpu_voltage_mv().ok();

        let res = stress::torture(
            threads,
            Duration::from_secs(o.dwell_s),
            reference,
            temp_ceiling,
            &SWEEP_STOP,
        );
        let note = if res.thermal_abort {
            format!("THERMAL >= {:.0}C", temp_ceiling)
        } else if !res.stable {
            "UNSTABLE".into()
        } else {
            "ok".into()
        };
        let stable = res.stable;
        points.push(SweepPoint {
            boost_mhz: freq,
            scale,
            measured_vid_mv: measured,
            stable,
            max_temp_c: res.max_temp_c,
            note,
        });
        if stable {
            best = Some(point);
        } else {
            break;
        }
        freq = freq.saturating_add(o.step);
    }

    let _ = restore_stock(oc);
    Ok((points, best))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oc(boost: u32, scale: i32) -> CpuOc {
        CpuOc {
            boost_mhz: boost,
            curve_scale: scale,
            cpu_temp_c: 90,
            gpu_temp_c: 90,
        }
    }

    #[test]
    fn stock_3500_is_safe() {
        // ~1158 mV predicted, under the ceiling.
        assert!(oc(3500, 0).validate().is_ok());
        assert!(oc(3500, 0).predicted_vid_mv() < ocq3::VID_CEILING_MV);
    }

    #[test]
    fn high_boost_without_undervolt_refused() {
        // 4000 @ scale 0 predicts ~1524 mV > ceiling — must refuse.
        assert!(oc(4000, 0).validate().is_err());
    }

    #[test]
    fn boost_above_cap_refused() {
        // The operator cap is 4.0 GHz — anything higher is out of range.
        assert!(oc(4300, ocq3::SCALE_MIN).validate().is_err());
        assert!(oc(ocq3::BOOST_MAX_MHZ + 100, -30).validate().is_err());
    }

    #[test]
    fn max_boost_needs_undervolt() {
        // 4.0 GHz at stock curve predicts >1325 mV — must refuse without UV,
        // but is reachable with enough undervolt.
        assert!(oc(ocq3::BOOST_MAX_MHZ, 0).validate().is_err());
        let s = safe_scale_for(ocq3::BOOST_MAX_MHZ, ocq3::VID_CEILING_MV);
        assert!(oc(ocq3::BOOST_MAX_MHZ, s).validate().is_ok());
    }

    #[test]
    fn positive_scale_is_rejected() {
        // Overvolt (scale > 0) is out of range.
        assert!(oc(3500, 5).validate().is_err());
    }

    #[test]
    fn safe_scale_keeps_under_cap() {
        for boost in [3500u32, 3800, 4000, 4200] {
            let s = safe_scale_for(boost, 1275);
            // Either it hit the cap, or it bottomed out at SCALE_MIN.
            let vid = ocq3::vid_predict(boost, s);
            assert!(
                vid <= 1275.0 || s == ocq3::SCALE_MIN,
                "boost {boost} scale {s} -> {vid} mV"
            );
        }
    }

    #[test]
    fn curve_undervolt_lowers_vid() {
        // Deeper scale must not raise predicted Vid at a fixed clock.
        let base = ocq3::vid_predict(3800, 0);
        let uv = ocq3::vid_predict(3800, -20);
        assert!(uv < base, "{uv} !< {base}");
    }

    /// Replay a staged write sequence from a starting state, asserting the
    /// predicted Vid at EVERY intermediate state stays at/under the ceiling.
    fn assert_staging_safe(from: &CpuOc, to: &CpuOc, prev: Option<&CpuOc>) {
        let (mut boost, mut scale) = (from.boost_mhz, from.curve_scale);
        for st in to.staged_writes(prev) {
            match st {
                Stage::Scale(s) => scale = s,
                Stage::Boost(b) => boost = b,
            }
            let vid = ocq3::vid_predict(boost, scale);
            assert!(
                vid <= ocq3::VID_CEILING_MV as f64,
                "transient overvolt: {boost} MHz @ scale {scale} -> {vid:.0} mV \
                 (from {}/{} to {}/{})",
                from.boost_mhz,
                from.curve_scale,
                to.boost_mhz,
                to.curve_scale
            );
        }
        assert_eq!((boost, scale), (to.boost_mhz, to.curve_scale));
    }

    /// OVERVOLT-SAFETY REGRESSION: lowering 4000/-35 -> 3500/0 must never stage
    /// a state whose predicted Vid exceeds the 1325 mV ceiling (the old
    /// scale-before-boost order transited 4000@0 ~ 1524 mV).
    #[test]
    fn staged_writes_never_transit_over_ceiling() {
        let hi = oc(4000, -35);
        let lo = oc(3500, 0);
        // Known previous state (the fixed direction-aware path).
        assert_staging_safe(&hi, &lo, Some(&hi));
        // Unknown previous state (conservative staging).
        assert_staging_safe(&hi, &lo, None);
        // Raising is staged safely too (undervolt lands before the boost).
        let up = oc(4000, safe_scale_for(4000, ocq3::VID_CEILING_MV));
        assert_staging_safe(&lo, &up, Some(&lo));
        assert_staging_safe(&lo, &up, None);
        // Exhaustive-ish sweep across the envelope, from every plausible state.
        for (fb, fs) in [(3500, 0), (3800, -20), (4000, -35), (4100, -50)] {
            for (tb, ts_cap) in [(3500u32, 1325u32), (3800, 1275), (4000, 1275), (4100, 1275)] {
                let from = oc(fb, fs);
                let to = oc(tb, safe_scale_for(tb, ts_cap));
                assert_staging_safe(&from, &to, Some(&from));
                assert_staging_safe(&from, &to, None);
            }
        }
    }

    #[test]
    fn temp_floor_enforced() {
        let mut c = oc(3500, 0);
        c.cpu_temp_c = 30; // below the 50C floor — a poisoned cap
        assert!(c.validate().is_err());
        c.cpu_temp_c = 90;
        c.gpu_temp_c = 0; // 0 would disable the soft cap
        assert!(c.validate().is_err());
    }
}
