// SPDX-License-Identifier: GPL-2.0-only
//! ocq3 — the BC-250 SMU **queue-3 OC mailbox** (CPU overclock / undervolt).
//!
//! This is the message surface the BC-250 CPU-OC path drives: max CPU boost clock, the
//! F/Vid curve scale (undervolt), CPU/GPU temp limits, Vid offsets, per-core OC,
//! plus live reads (current Vid, per-core frequency, the P-state table).
//!
//! SAFETY — why this is a userspace SMN path and not a footgun:
//!   * Queue 3 is a SEPARATE mailbox from the GPU PMFW mailbox that amdgpu polls
//!     (see `smu.rs`). amdgpu does not manage CPU OC and never touches queue 3,
//!     so writing it cannot race the driver's metric polling — the wedge mode
//!     the GPU doctrine warns about does not apply here.
//!   * The SMN aperture used (PCI-config 0xB8 index / 0xBC data) is independent
//!     of amdgpu's runtime MMIO SMN aperture, so the index latch can't be stomped
//!     by the driver mid-transaction.
//!   * We still `flock(LOCK_EX)` the config fd to serialize our own accesses
//!     (as the reference OC driver does) and probe the test message before trusting it.
//!   * Every voltage-affecting message is hard-capped: Vid never exceeds 1.325 V,
//!     the curve scale is undervolt-only (<= 0), boost clock is range-clamped.
//!
//! GPU clock/voltage stays on the kernel-serialized `amdgpu_smu_send_raw` node
//! (smu.rs) — that one IS amdgpu's mailbox and must not be raced.
//!
//! The raw SMN index/data plumbing (open, flock lock/unlock, the short-transfer-
//! guarded reg access) lives in `ariel_hal::SmnAperture`; this module builds the
//! queue-3 protocol on top of it.

use anyhow::{bail, Result};
use ariel_hal::SmnAperture;

/// Queue-3 mailbox registers (SMN), validated on BC-250 firmware.
const Q3_CMD: u32 = 0x03B1_0A20;
const Q3_RSP: u32 = 0x03B1_0A80;
const Q3_ARG: u32 = 0x03B1_0A88;

// SMU mailbox status codes.
const SMU_OK: u32 = 0x01;
const SMU_FAILED: u32 = 0xFF;
const SMU_UNKNOWN: u32 = 0xFE;
const SMU_REJECTED_PREREQ: u32 = 0xFD;
const SMU_REJECTED_BUSY: u32 = 0xFC;

/// Hard safety ceilings (the documented-safe OC envelope).
pub const VID_CEILING_MV: u32 = 1325; // bricking territory above this
pub const BOOST_MIN_MHZ: u32 = 2800; // underclock floor (cap the CPU lower for power/heat)
pub const BOOST_STOCK_MHZ: u32 = 3500; // stock boost (the default operating point)
pub const BOOST_MAX_MHZ: u32 = 4100; // operator cap (firmware allows 4500; held at 4.1 GHz)
pub const BOOST_WARN_MHZ: u32 = 3900; // above this, stress-test thoroughly
pub const SCALE_MIN: i32 = -50; // deepest documented undervolt
pub const SCALE_MAX: i32 = 0; // 0 = stock; positive (overvolt) is forbidden
pub const TEMP_MAX_C: u32 = 100;
/// Floor for the CPU/GPU temp caps: a cap below any plausible operating point
/// (a typo'd 0/5/30) either disables the protection or throttles the box into
/// the ground — clamp/refuse below this.
pub const TEMP_MIN_C: u32 = 50;
/// Safe floor for the HARDWARE GPU temp cap (SMU 0x8C). On gfx1013 the firmware's
/// temp-cap path can't gently throttle a force-pinned clock — set below this it
/// just lets the die overshoot until an emergency thermal event HARD-WEDGES the
/// GPU (verified: an 85C hardware cap under sustained GPU load reset the box via watchdog).
/// So we never arm the hardware cap below this backstop; a lower user-facing cap
/// is enforced in SOFTWARE by the governor (which throttles the clock to hold it).
pub const HW_TEMP_FLOOR_C: u32 = 95;

/// Queue-3 message ids (reverse-engineered).
mod q3 {
    pub const TEST: u16 = 0x01;
    pub const SET_CPU_GPU_VID: u16 = 0x0F; // (kind<<16)|vid  kind 0=CPU 1=GFX
    pub const UNFORCE_CPU_GPU_VID: u16 = 0x10; // kind<<16
    #[allow(dead_code)] // per-core OC bypasses the curve Vid-safety; not exposed
    pub const SET_OC_CLK: u16 = 0x25; // (core<<16)|mhz ; core 0xFF = all
    #[allow(dead_code)]
    pub const UNSET_OC_CLK: u16 = 0x26; // core<<16
    pub const GET_CPU_VOLTAGE: u16 = 0x36; // -> mV
    pub const GET_GPU_VOLTAGE: u16 = 0x37; // -> mV
    pub const GET_PSTATE_CLK: u16 = 0x3B; // pstate -> MHz
    pub const GET_CPU_TEMP_MAX: u16 = 0x40;
    pub const GET_CORE_FREQ: u16 = 0x43; // core -> MHz
    pub const SCALE_VID_CURVE: u16 = 0x50; // signed 16-bit curve scale
    pub const SET_CPU_MAX_TEMP: u16 = 0x8B;
    pub const SET_GPU_MAX_TEMP: u16 = 0x8C;
    pub const SET_MAX_BOOST_CLK: u16 = 0x8F;
    pub const DISABLE_EXTRA_VOLTAGE: u16 = 0x9A;
}

/// The Vid CODE of the 1325 mV safety ceiling. Codes DECREASE as voltage rises
/// (vid 0 = 1550 mV = max), so any code below this encodes an over-ceiling
/// voltage and must never be sent.
pub const VID_CEILING_CODE: u32 = 36; // (1550 - 1325) / 6.25

/// SMU Vid code <-> millivolts (bc250 codec): mv = vid*-6.25 + 1550.
///
/// SAFETY: an out-of-range mV (> ceiling) clamps to the CEILING code, never to
/// code 0 — code 0 is 1550 mV, the MAXIMUM voltage, i.e. the exact opposite of
/// a safe saturation.
pub fn mv_to_vid(mv: u32) -> u32 {
    (((1.55 - (mv as f64 / 1000.0)) / 0.00625).round() as i64)
        .clamp(VID_CEILING_CODE as i64, 0xFFFF) as u32
}
/// Inverse codec — only the tests need it (production reads voltages in mV
/// directly from the GET_*_VOLTAGE messages).
#[cfg(test)]
pub fn vid_to_mv(vid: u32) -> u32 {
    (((vid as f64 * -0.00625) + 1.55) * 1000.0).round().max(0.0) as u32
}

/// bc250's F/Vid curve predictor: predicted Vid (mV) at `clock_mhz` for a given
/// curve `scale`. Drives the curve plot in the TUI and the detect sweep.
pub fn vid_predict(clock_mhz: u32, scale: i32) -> f64 {
    let c = clock_mhz as f64;
    let s = scale as f64;
    let p = -1.519 + s * 0.004325;
    let q = 2800.0 - s * 10.0;
    0.0003 * c * c + p * c + q
}

/// Handle to the queue-3 OC mailbox.
pub struct OcQ3 {
    smn: SmnAperture,
}

impl OcQ3 {
    /// Open the SMN aperture (root). Does not yet probe — call [`OcQ3::test`].
    ///
    /// Refuses on non-BC-250 hardware: this pokes an SMN mailbox via PCI config
    /// space on `0000:00:00.0`, which on another AMD platform could hit an
    /// unrelated register. Confirm the silicon first.
    pub fn open() -> Result<Self> {
        if !ariel_hal::ariel_apu_present() {
            bail!("not a BC-250 (PCI 1002:13fe absent) — refusing to poke the SMN mailbox");
        }
        let smn = SmnAperture::open()?;
        Ok(OcQ3 { smn })
    }

    /// Open and verify the queue answers (test echo). Use this before any write.
    pub fn open_checked() -> Result<Self> {
        let s = Self::open()?;
        s.test()?;
        Ok(s)
    }

    /// Raw queue-3 send. Returns (status, arg). Serialized via flock. Bails on
    /// any failed/short SMN transfer (stale-arg hazard) and reports a poll
    /// timeout distinctly — the command may still be in flight, so the caller
    /// must not assume it did NOT land.
    fn send(&self, msg: u16, arg: u32, arg_high: u32) -> Result<(u32, u32)> {
        self.smn.lock();
        let r = self.send_locked(msg, arg, arg_high);
        self.smn.unlock();
        r
    }

    fn send_locked(&self, msg: u16, arg: u32, arg_high: u32) -> Result<(u32, u32)> {
        let io = |e: std::io::Error| anyhow::anyhow!("q3 msg 0x{msg:02x}: SMN I/O failed ({e})");
        self.smn.wreg(Q3_RSP, 0).map_err(io)?;
        self.smn.wreg(Q3_ARG, arg).map_err(io)?;
        self.smn.wreg(Q3_ARG + 4, arg_high).map_err(io)?;
        self.smn.wreg(Q3_CMD, msg as u32).map_err(io)?;
        let mut st = 0;
        let mut answered = false;
        for _ in 0..4000 {
            st = self.smn.rreg(Q3_RSP).map_err(io)?;
            if matches!(
                st,
                SMU_OK | SMU_FAILED | SMU_UNKNOWN | SMU_REJECTED_PREREQ | SMU_REJECTED_BUSY
            ) {
                answered = true;
                break;
            }
        }
        if !answered {
            bail!(
                "q3 msg 0x{msg:02x}: response poll timed out (last status 0x{st:02x}) — \
                 the command MAY STILL BE IN FLIGHT; do not assume it was dropped"
            );
        }
        let a = self.smn.rreg(Q3_ARG).map_err(io)?;
        Ok((st, a))
    }

    fn send_ok(&self, msg: u16, arg: u32, arg_high: u32) -> Result<u32> {
        let (st, a) = self.send(msg, arg, arg_high)?;
        match st {
            SMU_OK => Ok(a),
            SMU_UNKNOWN => bail!("q3 msg 0x{msg:02x}: unknown command (firmware lacks it)"),
            SMU_REJECTED_PREREQ => bail!("q3 msg 0x{msg:02x}: rejected (prerequisite)"),
            SMU_REJECTED_BUSY => bail!("q3 msg 0x{msg:02x}: rejected (busy)"),
            _ => bail!("q3 msg 0x{msg:02x}: status 0x{st:02x}"),
        }
    }

    /// Verify the mailbox: test echo must return arg+1.
    pub fn test(&self) -> Result<()> {
        let a = self.send_ok(q3::TEST, 123, 0)?;
        if a != 124 {
            bail!("queue-3 test echo returned {a}, expected 124 (wrong mailbox?)");
        }
        Ok(())
    }

    // ---- reads ----

    pub fn cpu_voltage_mv(&self) -> Result<u32> {
        self.send_ok(q3::GET_CPU_VOLTAGE, 0, 0)
    }
    pub fn gpu_voltage_mv(&self) -> Result<u32> {
        self.send_ok(q3::GET_GPU_VOLTAGE, 0, 0)
    }
    pub fn core_freq_mhz(&self, core: u32) -> Result<u32> {
        self.send_ok(q3::GET_CORE_FREQ, core, 0)
    }
    pub fn pstate_clk_mhz(&self, pstate: u32) -> Result<u32> {
        self.send_ok(q3::GET_PSTATE_CLK, pstate, 0)
    }
    pub fn cpu_temp_max(&self) -> Result<u32> {
        self.send_ok(q3::GET_CPU_TEMP_MAX, 0, 0)
    }

    /// Per-core frequency for the ENABLED cores only. The BC-250 is a harvested
    /// 8-core Zen2 die with 2 cores fused off (6 real cores), and the SMU's 0x43
    /// query enumerates all 8 die slots — so query just the slots Linux reports
    /// as present (unique topology core_id) to skip the 2 dead slots.
    pub fn core_freqs(&self) -> Vec<u32> {
        enabled_core_ids()
            .into_iter()
            .filter_map(|c| self.core_freq_mhz(c).ok())
            .collect()
    }

    // ---- writes (capped) ----

    /// Max CPU boost clock (MHz). Clamped to the silicon range.
    pub fn set_max_boost_clk(&self, mhz: u32) -> Result<()> {
        let m = mhz.clamp(BOOST_MIN_MHZ, BOOST_MAX_MHZ);
        self.send_ok(q3::SET_MAX_BOOST_CLK, m, 0).map(|_| ())
    }

    /// Scale the F/Vid curve (undervolt). Clamped to [-50, 0]; positive
    /// (overvolt) is refused — that is how you destroy this silicon.
    pub fn scale_vid_curve(&self, scale: i32) -> Result<()> {
        if scale > SCALE_MAX {
            bail!("refusing positive curve scale {scale} (overvolt); use <= 0");
        }
        let s = scale.clamp(SCALE_MIN, SCALE_MAX);
        let packed = ((s as i16) as u16) as u32; // signed 16-bit in low half
        self.send_ok(q3::SCALE_VID_CURVE, packed, 0).map(|_| ())
    }

    pub fn set_cpu_max_temp(&self, c: u32) -> Result<()> {
        self.send_ok(q3::SET_CPU_MAX_TEMP, c.clamp(TEMP_MIN_C, TEMP_MAX_C), 0)
            .map(|_| ())
    }
    pub fn set_gpu_max_temp(&self, c: u32) -> Result<()> {
        // Clamp to a SAFE hardware backstop — never arm the wedge-prone emergency
        // path below HW_TEMP_FLOOR_C. Lower caps are enforced by the governor.
        let hw = c.clamp(HW_TEMP_FLOOR_C, TEMP_MAX_C);
        self.send_ok(q3::SET_GPU_MAX_TEMP, hw, 0).map(|_| ())
    }

    /// Disable the SMU's extra CPU/GPU voltage padding (the undervolt enabler).
    pub fn disable_extra_voltage(&self, on: bool) -> Result<()> {
        self.send_ok(q3::DISABLE_EXTRA_VOLTAGE, on as u32, 0)
            .map(|_| ())
    }

    /// Force an absolute CPU Vid (mV). Refuses anything over the 1.325 V ceiling.
    pub fn force_cpu_vid_mv(&self, mv: u32) -> Result<()> {
        if mv > VID_CEILING_MV {
            bail!("refusing CPU Vid {mv} mV (> {VID_CEILING_MV} mV ceiling)");
        }
        let vid = mv_to_vid(mv) & 0xFFFF; // kind 0 = CPU in the high half
        self.send_ok(q3::SET_CPU_GPU_VID, vid, 0).map(|_| ())
    }
    pub fn unforce_cpu_vid(&self) -> Result<()> {
        self.send_ok(q3::UNFORCE_CPU_GPU_VID, 0, 0).map(|_| ())
    }

    /// Per-core (or all-core with core=0xFF) OC target clock (MHz).
    ///
    /// CAUTION: this raises clock WITHOUT the F/Vid-curve Vid cap that the CPU-OC
    /// curve path enforces, so it can drive Vid past safe limits. Kept as
    /// internal API; the CLI/TUI drive the curve-capped path instead.
    #[allow(dead_code)]
    pub fn set_oc_clk(&self, core: u32, mhz: u32) -> Result<()> {
        let param = ((core & 0xFF) << 16) | (mhz & 0xFFFF);
        self.send_ok(q3::SET_OC_CLK, param, 0).map(|_| ())
    }
    #[allow(dead_code)]
    pub fn unset_oc_clk(&self, core: u32) -> Result<()> {
        self.send_ok(q3::UNSET_OC_CLK, (core & 0xFF) << 16, 0)
            .map(|_| ())
    }

    /// Restore the firmware defaults (mirrors the reference OC driver's revert). Boost is
    /// lowered FIRST so relaxing the curve can't transiently overvolt, and any
    /// forced absolute CPU Vid is released too — "restore" must undo the whole
    /// OC surface, not just the curve path.
    pub fn restore_defaults(&self) -> Result<()> {
        self.set_max_boost_clk(BOOST_STOCK_MHZ)?;
        self.scale_vid_curve(0)?;
        self.unforce_cpu_vid()?;
        self.disable_extra_voltage(false)?;
        self.set_cpu_max_temp(TEMP_MAX_C)?;
        self.set_gpu_max_temp(TEMP_MAX_C)?;
        Ok(())
    }
}

/// Physical core indices Linux reports present (unique topology core_id), sorted.
/// The BC-250 has core_ids 0,1,2,4,5,6 (slots 3 & 7 fused off). Falls back to
/// 0..6 if sysfs is unreadable.
fn enabled_core_ids() -> Vec<u32> {
    let mut ids = std::collections::BTreeSet::new();
    if let Ok(rd) = std::fs::read_dir("/sys/devices/system/cpu") {
        for e in rd.flatten() {
            let name = e.file_name();
            let name = name.to_string_lossy();
            if let Some(n) = name.strip_prefix("cpu") {
                if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) {
                    if let Ok(s) = std::fs::read_to_string(e.path().join("topology/core_id")) {
                        if let Ok(id) = s.trim().parse::<u32>() {
                            ids.insert(id);
                        }
                    }
                }
            }
        }
    }
    if ids.is_empty() {
        (0..6).collect()
    } else {
        ids.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vid_codec_roundtrips() {
        for mv in [800u32, 1000, 1158, 1200, 1325] {
            let back = vid_to_mv(mv_to_vid(mv));
            assert!((back as i64 - mv as i64).abs() <= 7, "{mv} -> {back}");
        }
    }

    #[test]
    fn vid_codec_matches_bc250_reference() {
        // bc250 codec: mv = vid*-6.25 + 1550.
        assert_eq!(vid_to_mv(56), 1200); // 56*-6.25+1550 = 1200
    }

    /// OVERVOLT-SAFETY REGRESSION: an out-of-range mV must clamp to the CEILING
    /// code, never to code 0 — vid 0 encodes 1550 mV (the silicon-destroying
    /// maximum), so saturating there would turn a bad input into a max-volt
    /// request.
    #[test]
    fn mv_to_vid_out_of_range_clamps_to_ceiling_not_zero() {
        for mv in [1326u32, 1500, 1550, 2000, u32::MAX] {
            let code = mv_to_vid(mv);
            assert!(code >= VID_CEILING_CODE, "{mv} mV -> code {code}");
            assert!(vid_to_mv(code) <= VID_CEILING_MV, "{mv} mV -> code {code}");
        }
        // The ceiling itself encodes exactly.
        assert_eq!(mv_to_vid(VID_CEILING_MV), VID_CEILING_CODE);
        assert_eq!(vid_to_mv(VID_CEILING_CODE), VID_CEILING_MV);
    }

    #[test]
    fn curve_is_monotonic_in_clock() {
        let mut prev = 0.0;
        for c in (3500..=4500).step_by(100) {
            let v = vid_predict(c, 0);
            assert!(v > prev, "not monotonic at {c}");
            prev = v;
        }
    }

    #[test]
    fn stock_3500_matches_known_vid() {
        // ~1158 mV at 3.5 GHz stock (bc250 says ~1180 stock; predictor ~1158).
        let v = vid_predict(3500, 0);
        assert!((1130.0..1190.0).contains(&v), "3500@0 = {v}");
    }
}
