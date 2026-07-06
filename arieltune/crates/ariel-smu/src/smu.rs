// SPDX-License-Identifier: GPL-2.0-only
//! SMU actuation — race-free clock/voltage control via the patched amdgpu
//! debugfs surface.
//!
//! The BC-250 PMFW shares one MP1 mailbox with amdgpu's own metric polling;
//! poking it from a second context (out-of-tree module, PCI-cfg backdoor)
//! wedges the box. The liberation series solves this with
//! `amdgpu_smu_send_raw`: a debugfs node that sends a raw MP1 message *under
//! amdgpu's `message_lock`*, so it can never race the driver's polls. Every
//! actuation here routes through that node (or the typed `cyan_skillfish_*`
//! nodes that wrap specific msgids), never a raw SMN write.
//!
//! Write format of `amdgpu_smu_send_raw` (patch 0008b):
//!     "<msgid_hex> <param_hex> [<extra_hex>]\n"
//! The typed cclk / sdpm nodes take a single decimal value.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

/// Authoritative PMFW 88.6.0 msgid table (`smu_v11_8_ppsmc.h`, as mapped by
/// the liberation series). Kept complete as the documented reference for the
/// patched surface even where aputune doesn't (yet) send a given msg.
#[allow(dead_code)]
pub mod msg {
    pub const QUERY_GFXCLK: u16 = 0x0F;
    pub const REQUEST_ACTIVE_WGP: u16 = 0x18;
    pub const QUERY_ACTIVE_WGP: u16 = 0x1E;
    pub const START_TELEMETRY: u16 = 0x1B;
    pub const SET_SOFT_MIN_CCLK: u16 = 0x35;
    pub const SET_SOFT_MAX_CCLK: u16 = 0x36;
    pub const GET_GFX_FREQUENCY: u16 = 0x37;
    pub const GET_GFX_VID: u16 = 0x38;
    pub const FORCE_GFX_FREQ: u16 = 0x39;
    pub const UNFORCE_GFX_FREQ: u16 = 0x3A;
    pub const FORCE_GFX_VID: u16 = 0x3B;
    pub const UNFORCE_GFX_VID: u16 = 0x3C;
}

/// Hard safety ceilings shared with the CPU/GPU tuners.
pub const GFX_VID_CEILING_MV: u32 = 1325; // never exceed 1.325 V on this silicon
/// Reference floor for the DEAD raw-Vid API below only. The LIVE clock-aware
/// voltage floor is `telemetry::min_gfx_vddc` (scaled per target clock); this
/// constant is just the absolute never-below bound the dead guard keeps.
#[allow(dead_code)]
pub const GFX_VID_MIN_MV: u32 = 650;
pub const SCLK_MIN_MHZ: u32 = 350;
pub const SCLK_MAX_MHZ: u32 = 2500; // raised by patch 0003

/// Handle to the patched amdgpu debugfs surface.
pub struct Smu {
    dbg: PathBuf,
}

impl Smu {
    /// Open the actuation surface. Requires the liberation series live and the
    /// process to be root (debugfs nodes are 0600 root-owned).
    pub fn open() -> Result<Self> {
        let dbg = ariel_hal::amdgpu_dbg_dir()
            .context("amdgpu debugfs dir not found (debugfs mounted? running as root?)")?;
        let s = Smu { dbg };
        if !s.send_raw_node().exists() {
            bail!(
                "amdgpu_smu_send_raw not present at {} — kernel not liberated (run `arieltune apu build`)",
                s.send_raw_node().display()
            );
        }
        Ok(s)
    }

    fn send_raw_node(&self) -> PathBuf {
        self.dbg.join("amdgpu_smu_send_raw")
    }
    fn node(&self, name: &str) -> PathBuf {
        self.dbg.join(name)
    }

    fn write(path: &Path, contents: &str) -> Result<()> {
        fs::write(path, contents)
            .with_context(|| format!("write {} <- {:?}", path.display(), contents))
    }

    /// Send a raw MP1 message (serialized by the kernel against amdgpu polls).
    pub fn send_raw(&self, msgid: u16, param: u32, extra: u32) -> Result<()> {
        let line = format!("{:x} {:x} {:x}\n", msgid, param, extra);
        Self::write(&self.send_raw_node(), &line)
    }

    // ---- GPU clock ----

    /// Pin the GFX clock to `mhz` (clamped to the silicon's valid range).
    /// Returns the frequency actually applied after clamping.
    pub fn force_gfx_freq(&self, mhz: u32) -> Result<u32> {
        let mhz = mhz.clamp(SCLK_MIN_MHZ, SCLK_MAX_MHZ);
        self.send_raw(msg::FORCE_GFX_FREQ, mhz, 0)?;
        Ok(mhz)
    }
    /// Release the GFX clock lock (PMFW BAPM resumes control).
    pub fn unforce_gfx_freq(&self) -> Result<()> {
        self.send_raw(msg::UNFORCE_GFX_FREQ, 0, 0)
    }

    // ---- GPU voltage ----
    //
    // DO NOT WIRE THESE UP. Raw FORCE_GFX_VID writes the MP1 mailbox directly and
    // RACES amdgpu's own metric polling on the same mailbox — under load it hard-
    // hangs gfx1013 and wedges the whole box (verified: two power-cycles). GPU
    // voltage MUST go through amdgpu's overdrive interface instead
    // (telemetry::od_set_vddc / pp_od_clk_voltage), which serializes the SMU write
    // and is safe live. These are kept only for reference; they have no callers
    // and must stay that way.

    /// DANGER — wedges the box (MP1 race). Use telemetry::od_set_vddc instead.
    #[allow(dead_code)]
    pub fn force_gfx_vid(&self, mv: u32) -> Result<()> {
        if mv > GFX_VID_CEILING_MV {
            bail!("refusing GFX Vid {mv} mV (> {GFX_VID_CEILING_MV} mV ceiling)");
        }
        if mv < GFX_VID_MIN_MV {
            bail!("refusing GFX Vid {mv} mV (< {GFX_VID_MIN_MV} mV floor)");
        }
        self.send_raw(msg::FORCE_GFX_VID, mv, 0)
    }
    /// DANGER — see `force_gfx_vid`. Use telemetry::od_reset instead.
    #[allow(dead_code)]
    pub fn unforce_gfx_vid(&self) -> Result<()> {
        self.send_raw(msg::UNFORCE_GFX_VID, 0, 0)
    }

    // NOTE: the typed cclk_soft_min/max debugfs setters (patch 0009) were
    // REMOVED from this API: CPU clock control lives on the SMU queue-3 mailbox
    // (ocq3), and a second CPU-clock actuator here is the dual-governor hazard
    // (two writers fighting over the same knob — the class that crippled a test board).

    /// Read the telemetry dump node (patch 0016), if present.
    pub fn telemetry(&self) -> Option<String> {
        fs::read_to_string(self.node("cyan_skillfish_telemetry")).ok()
    }
}

/// Coarse sysfs fallback when only patch 0004 (set_performance_level) is live
/// and the raw send node is not. Writes `power_dpm_force_performance_level`.
pub fn set_performance_level(level: &str) -> Result<()> {
    let entries = fs::read_dir("/sys/class/drm").context("read /sys/class/drm")?;
    for e in entries.flatten() {
        let p = e.path().join("device/power_dpm_force_performance_level");
        if p.exists() {
            return fs::write(&p, format!("{level}\n"))
                .with_context(|| format!("write {}", p.display()));
        }
    }
    bail!("power_dpm_force_performance_level not found")
}
