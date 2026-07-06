// SPDX-License-Identifier: GPL-2.0-only
//! Sysfs-only telemetry for the DPM control loop.
//!
//! The daemon must NOT read the SMU — that contends with amdgpu's own metric
//! polling on the shared MP1 mailbox. So every signal here comes from sysfs /
//! debugfs nodes amdgpu already maintains:
//!   * fence_info ring sequence deltas — the activity signal (clock-independent)
//!   * hwmon junction temperature — the thermal signal
//!   * amdgpu_pm_info SoC watts — sanity / thermal-budget input (best-effort)

use std::fs;
use std::path::{Path, PathBuf};

/// Sum of all fence sequence numbers across amdgpu rings. The *delta* between
/// two reads, divided by elapsed seconds, is the activity rate. Absolute value
/// is meaningless; only the rate matters.
pub fn fence_sum(dbg: &Path) -> Option<u64> {
    let txt = fs::read_to_string(dbg.join("amdgpu_fence_info")).ok()?;
    let mut sum: u64 = 0;
    for tok in txt.split_whitespace() {
        if let Some(hex) = tok.strip_prefix("0x") {
            if let Ok(v) = u64::from_str_radix(hex, 16) {
                sum = sum.wrapping_add(v);
            }
        }
    }
    Some(sum)
}

/// Hottest amdgpu HWMON temperature in degrees C (max of temp*_input on the
/// amdgpu hwmon). NOTE: despite the name, on this silicon the hwmon exposes the
/// filtered EDGE sensor, not the true junction/hotspot — the real gfx-die
/// temperature (which runs much hotter) is [`gfx_temp_c`] from gpu_metrics.
/// None if no amdgpu hwmon is found.
pub fn junction_temp_c() -> Option<f64> {
    let entries = fs::read_dir("/sys/class/hwmon").ok()?;
    for e in entries.flatten() {
        let dir = e.path();
        let name = fs::read_to_string(dir.join("name")).unwrap_or_default();
        if name.trim() != "amdgpu" {
            continue;
        }
        let mut hottest: Option<f64> = None;
        if let Ok(files) = fs::read_dir(&dir) {
            for f in files.flatten() {
                let fname = f.file_name();
                let fname = fname.to_string_lossy();
                if fname.starts_with("temp") && fname.ends_with("_input") {
                    if let Ok(s) = fs::read_to_string(f.path()) {
                        if let Ok(milli) = s.trim().parse::<f64>() {
                            let c = milli / 1000.0;
                            hottest = Some(hottest.map_or(c, |h| h.max(c)));
                        }
                    }
                }
            }
        }
        if hottest.is_some() {
            return hottest;
        }
    }
    None
}

/// The nct6686 (carrier Super-I/O in EC mode) hwmon directory, matched by name.
/// Present only when the `nct6683` driver is loaded with `force=1`.
fn nct6686_dir() -> Option<PathBuf> {
    for e in fs::read_dir("/sys/class/hwmon").ok()?.flatten() {
        let dir = e.path();
        if fs::read_to_string(dir.join("name"))
            .map(|s| s.trim() == "nct6686")
            .unwrap_or(false)
        {
            return Some(dir);
        }
    }
    None
}

fn read_num(dir: &Path, file: &str) -> Option<i64> {
    fs::read_to_string(dir.join(file)).ok()?.trim().parse().ok()
}

/// Carrier-board sensors read from the nct6686 (needs `nct6683 force=1`).
#[derive(Default)]
pub struct Carrier {
    /// AMD TSI (SoC) temperature, °C.
    pub soc_c: Option<f64>,
    /// Hottest board thermistor, °C.
    pub board_c: Option<f64>,
    /// Fastest fan, RPM.
    pub fan_rpm: Option<u32>,
    /// EC fan duty (read-only monitor), percent.
    pub duty_pct: Option<u32>,
}

/// Read the carrier-board sensor set. All fields None if the nct6686 hwmon
/// isn't present (driver not loaded).
pub fn carrier() -> Carrier {
    let mut c = Carrier::default();
    let Some(dir) = nct6686_dir() else { return c };
    c.soc_c = read_num(&dir, "temp1_input").map(|m| m as f64 / 1000.0);
    for i in 2..=6 {
        if let Some(m) = read_num(&dir, &format!("temp{i}_input")) {
            let t = m as f64 / 1000.0;
            c.board_c = Some(c.board_c.map_or(t, |b| b.max(t)));
        }
    }
    // Report the actively-spinning cooler, not the max across every channel.
    // The BC-250 exposes 6-8 fan channels but only the Pump-Fan header (ch2)
    // has a fan on it; the unused "System Fan" headers idle at a nonzero PWM
    // (~65%) yet report 0 RPM. Keying off RPM means `Fan` and `Duty` describe
    // the same physical fan — the one we also control — instead of a phantom.
    let mut best_rpm = 0u32;
    let mut best_ch = 0u8;
    for i in 1..=8u8 {
        if let Some(r) = read_num(&dir, &format!("fan{i}_input")) {
            if r > 0 && (r as u32) > best_rpm {
                best_rpm = r as u32;
                best_ch = i;
            }
        }
    }
    if best_rpm > 0 {
        c.fan_rpm = Some(best_rpm);
        if let Some(p) = read_num(&dir, &format!("pwm{best_ch}")) {
            c.duty_pct = Some((p.clamp(0, 255) as u32) * 100 / 255);
        }
    }
    c
}

/// True when the carrier (nct6686) sensors are available.
pub fn carrier_present() -> bool {
    nct6686_dir().is_some()
}

/// The prebuilt writable driver, embedded in the binary, and the exact kernel
/// its vermagic matches. The BC-250 is x86-64-v3 but the CachyOS kernel headers
/// ship x86-64-v4 host build tools (`fixdep` aborts with "CPU ISA level is lower
/// than required"), so the module CANNOT be built on the board. aputune carries
/// the prebuilt `.ko` and installs it itself. Source + patch + rebuild script:
/// `kmod/nct6687-bc250/` — rebuild there for a different kernel.
const NCT6687_KO: &[u8] =
    include_bytes!("../kmod/nct6687-bc250/prebuilt/nct6687-7.0.9-1-cachyos-bore.ko");
const NCT6687_KVER: &str = "7.0.9-1-cachyos-bore";

/// Running kernel release (`uname -r`).
fn running_kver() -> String {
    fs::read_to_string("/proc/sys/kernel/osrelease")
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// Install the embedded writable module into the running kernel's module tree
/// if it isn't already resolvable by modprobe. Returns true if the module is
/// available to load afterwards. When the running kernel doesn't match the
/// prebuilt vermagic we return false rather than force-load a mismatched module
/// (there's no way to rebuild on the board). Best-effort; needs root.
fn install_writable_module() -> bool {
    let kver = running_kver();
    let dst = PathBuf::from(format!("/lib/modules/{kver}/updates/nct6687.ko"));
    if dst.exists() {
        return true;
    }
    // Already resolvable elsewhere (user-built / packaged)?
    if std::process::Command::new("modinfo")
        .arg("nct6687")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return true;
    }
    if kver != NCT6687_KVER {
        return false; // prebuilt blob won't load on a different kernel
    }
    if let Some(parent) = dst.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if fs::write(&dst, NCT6687_KO).is_err() {
        return false;
    }
    let _ = std::process::Command::new("depmod").arg("-a").status();
    true
}

/// Ensure the carrier board has the WRITABLE fan driver bound so both sensors
/// and fan control work — automatically installing the prebuilt `nct6687`
/// module if it isn't present. Persists the policy (blacklist the read-only
/// nct6683, autoload nct6687) so it survives reboots. Falls back to read-only
/// nct6683 sensors if the writable driver can't be brought up (wrong kernel /
/// no root) so the card still shows temps + RPM. Best-effort; silent no-op
/// without root.
///
/// nct6687 (Fred78290's driver + the BC-250 EC-firmware attach patch) exposes
/// the same NCT6686D sensors AND honours PWM writes on this EC (verified: fan
/// RPM tracks the written duty), whereas the in-kernel nct6683 is read-only.
pub fn ensure_carrier_sensors() -> bool {
    if fan_writable() {
        persist_nct6687_policy();
        return true;
    }
    // Install the module if needed, then hand it the chip: a read-only nct6683
    // must be unloaded first or it keeps the EC I/O window and nct6687 can't bind.
    if install_writable_module() {
        if carrier_present() && !fan_writable() {
            let _ = std::process::Command::new("modprobe")
                .args(["-r", "nct6683"])
                .status();
        }
        let _ = std::process::Command::new("modprobe")
            .args(["nct6687", "force=true"])
            .status();
    }
    if fan_writable() {
        // Persist the writable-driver policy ONLY once the driver has actually
        // bound: blacklisting nct6683 on a box where nct6687 can't come up
        // (kernel mismatch, no prebuilt) would leave the NEXT boot with no
        // carrier sensors at all.
        persist_nct6687_policy();
        return true;
    }

    // Fallback: read-only sensors so the card still shows temps + fan RPM. Make
    // sure OUR blacklist isn't left behind blocking nct6683 on future boots.
    remove_nct6687_policy();
    if !carrier_present() {
        let _ = std::process::Command::new("modprobe")
            .args(["nct6683", "force=true"])
            .status();
    }
    carrier_present()
}

/// Persist the writable-driver policy (blacklist read-only nct6683, autoload
/// nct6687) for future boots. Call only after `fan_writable()` confirms the
/// driver binds on this kernel.
fn persist_nct6687_policy() {
    let _ = fs::write(
        "/etc/modprobe.d/bc250-nct6687.conf",
        "blacklist nct6683\noptions nct6687 force=true\n",
    );
    let _ = fs::write("/etc/modules-load.d/bc250-nct6687.conf", "nct6687\n");
    let _ = fs::remove_file("/etc/modprobe.d/nct6683.conf");
    let _ = fs::remove_file("/etc/modules-load.d/nct6683.conf");
}

/// Remove our persisted nct6687 policy (used when falling back to nct6683 so
/// the blacklist can't orphan the sensors on the next boot).
fn remove_nct6687_policy() {
    let _ = fs::remove_file("/etc/modprobe.d/bc250-nct6687.conf");
    let _ = fs::remove_file("/etc/modules-load.d/bc250-nct6687.conf");
}

/// True when the carrier fan PWM is writable (the nct6687 driver is bound, not
/// the read-only nct6683). Checks that a pwm node exists and the backing driver
/// module is nct6687.
pub fn fan_writable() -> bool {
    let Some(dir) = nct6686_dir() else {
        return false;
    };
    // The writable driver is nct6687; its module symlink resolves to .../nct6687.
    match fs::read_link(dir.join("device/driver/module")) {
        Ok(p) => p.file_name().map(|n| n == "nct6687").unwrap_or(false),
        Err(_) => false,
    }
}

/// Current pwm_enable mode for a fan channel: 1 = manual (software duty),
/// 2 = EC automatic curve, other = chip-specific. None if unreadable.
pub fn fan_enable(channel: u8) -> Option<u8> {
    let dir = nct6686_dir()?;
    read_num(&dir, &format!("pwm{channel}_enable")).and_then(|v| u8::try_from(v).ok())
}

/// Set the carrier fan duty (0..=100 percent) on the given fan channel (1-based,
/// e.g. 2 = the Pump-Fan header that drives the main BC-250 cooler). Switches
/// the channel to manual mode first. Returns false if the writable driver isn't
/// bound or the write fails (needs root). Passing `None` for `pct` restores the
/// EC automatic curve (pwm_enable = 2).
pub fn set_fan_duty(channel: u8, pct: Option<u8>) -> bool {
    if !fan_writable() {
        return false;
    }
    let Some(dir) = nct6686_dir() else {
        return false;
    };
    let en = dir.join(format!("pwm{channel}_enable"));
    let pwm = dir.join(format!("pwm{channel}"));
    match pct {
        Some(p) => {
            let duty = (p.min(100) as u32 * 255 / 100).min(255);
            fs::write(&en, "1\n").is_ok() && fs::write(&pwm, format!("{duty}\n")).is_ok()
        }
        // Restore EC automatic control.
        None => fs::write(&en, "2\n").is_ok(),
    }
}

/// Real GFX-die temperature (degrees C) from amdgpu's `gpu_metrics` sysfs blob —
/// `temperature_gfx` at offset 4, hundredths of a degree in every gpu_metrics
/// v2_x. This is the hotspot the SMU thermally governs, and runs much hotter
/// than the filtered hwmon `edge` sensor (which reads ~67C while gfx is ~94C).
/// Reading gpu_metrics is a plain sysfs read of amdgpu's cached table — it does
/// NOT touch the MP1 mailbox, so it's safe inside the control loop.
pub fn gfx_temp_c() -> Option<f64> {
    let mut path: Option<PathBuf> = None;
    for e in fs::read_dir("/sys/class/drm").ok()?.flatten() {
        let p = e.path().join("device/gpu_metrics");
        if p.exists() {
            path = Some(p);
            break;
        }
    }
    let b = fs::read(path?).ok()?;
    if b.len() < 6 {
        return None;
    }
    let t = u16::from_le_bytes([b[4], b[5]]) as f64 / 100.0;
    // Sanity gate: reject 0 / absurd values (blob not populated yet).
    (5.0..130.0).contains(&t).then_some(t)
}

/// The amdgpu DRM device sysfs dir (…/cardN/device), located by the presence of
/// the overdrive node. None if amdgpu isn't up.
fn drm_device_dir() -> Option<PathBuf> {
    for e in fs::read_dir("/sys/class/drm").ok()?.flatten() {
        let d = e.path().join("device");
        if d.join("pp_od_clk_voltage").exists() {
            return Some(d);
        }
    }
    None
}

/// Live GFX rail voltage (mV) from amdgpu's hwmon `in0_input` (vddgfx) — the
/// "voltage meter". A plain sysfs read; does NOT touch the SMU mailbox.
pub fn vddgfx_mv() -> Option<u32> {
    for e in fs::read_dir("/sys/class/hwmon").ok()?.flatten() {
        let dir = e.path();
        if fs::read_to_string(dir.join("name"))
            .map(|s| s.trim() == "amdgpu")
            .unwrap_or(false)
        {
            if let Ok(s) = fs::read_to_string(dir.join("in0_input")) {
                if let Ok(mv) = s.trim().parse::<u32>() {
                    return Some(mv);
                }
            }
        }
    }
    None
}

/// The current overdrive operating point (sclk MHz, vddc mV) parsed from
/// `pp_od_clk_voltage`. This is the amdgpu-mediated, SMU-safe voltage interface —
/// point 0 is the high-clock operating point.
pub fn od_point() -> Option<(u32, u32)> {
    let txt = fs::read_to_string(drm_device_dir()?.join("pp_od_clk_voltage")).ok()?;
    let (mut sclk, mut vddc, mut sect) = (None, None, "");
    for line in txt.lines() {
        let t = line.trim();
        if t.starts_with("OD_SCLK") {
            sect = "s";
        } else if t.starts_with("OD_VDDC") {
            sect = "v";
        } else if t.starts_with("OD_") {
            sect = "";
        } else if let Some((_, rest)) = t.split_once(':') {
            // "0: 1500Mhz *"  /  "0: 906mV *"
            let n: String = rest
                .trim()
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if let Ok(v) = n.parse::<u32>() {
                match sect {
                    "s" => sclk = Some(v),
                    "v" => vddc = Some(v),
                    _ => {}
                }
            }
        }
    }
    Some((sclk?, vddc?))
}

/// Parse a "LABEL:  <lo>unit  <hi>unit" OD_RANGE line into (min, max).
fn od_range_line(label: &str, unit: &str) -> Option<(u32, u32)> {
    let txt = fs::read_to_string(drm_device_dir()?.join("pp_od_clk_voltage")).ok()?;
    for line in txt.lines() {
        let t = line.trim();
        if t.starts_with(label) {
            let nums: Vec<u32> = t
                .split_whitespace()
                .filter_map(|w| w.trim_end_matches(unit).parse::<u32>().ok())
                .collect();
            if nums.len() == 2 {
                return Some((nums[0], nums[1]));
            }
        }
    }
    None
}

/// The allowed VDDC range (min, max) mV from `pp_od_clk_voltage` OD_RANGE.
pub fn od_vddc_range() -> Option<(u32, u32)> {
    od_range_line("VDDC:", "mV")
}

/// The allowed SCLK range (min, max) MHz from `pp_od_clk_voltage` OD_RANGE.
pub fn od_sclk_range() -> Option<(u32, u32)> {
    od_range_line("SCLK:", "Mhz")
}

/// Minimum SAFE GFX voltage (mV) for a given top clock (MHz) — a voltage floor
/// scaled from the stock AVFS relationship. Undervolting below this for a clock
/// risks an instant crash: the rail can't sustain the frequency. This is the
/// "curve" that keeps a low-clock undervolt from being carried onto a high clock
/// (e.g. 887 mV is fine at 1500 MHz but crashes at 2230). Anchored to the
/// measured stock point (~906 mV @ 1500 MHz) and the OD_RANGE VDDC max at the
/// SCLK max, minus a modest undervolt allowance; clamped to the OD VDDC range.
pub fn min_gfx_vddc(top_mhz: u32) -> u32 {
    const STOCK_MHZ: i64 = 1500;
    const STOCK_MV: i64 = 906;
    // How far below stock an undervolt may reach. 50 mV puts floor(1500)=856,
    // which is the lowest measured-STABLE point at 1500 MHz (887 also stable);
    // 60 would admit 846-855, which nothing has validated.
    const ALLOWANCE_MV: i64 = 50;
    let (vlo, vhi) = od_vddc_range().unwrap_or((700, 1129));
    let (_, smax) = od_sclk_range().unwrap_or((1000, 2500));
    // Linear stock estimate through (1500, 906) and (smax, vhi).
    let slope_num = vhi as i64 - STOCK_MV;
    let slope_den = (smax as i64 - STOCK_MHZ).max(1);
    let stock = STOCK_MV + (top_mhz as i64 - STOCK_MHZ) * slope_num / slope_den;
    (stock - ALLOWANCE_MV).clamp(vlo as i64, vhi as i64) as u32
}

/// Set the GFX voltage (mV) for the given TOP clock (MHz) via the amdgpu
/// overdrive interface: `vc 0 <sclk> <vddc>` then commit. amdgpu serializes the
/// SMU write, so this is safe under load — unlike a raw `force_gfx_vid`, which
/// races amdgpu's MP1 polling and hangs the GPU.
///
/// `top_mhz` MUST be the intended TOP clock (aputune's top setpoint), NOT the
/// transient live clock: OD point 0 IS the top DPM operating point, so filling it
/// from whatever the governor happened to be at (idle 1000 / mid 1250 / a capped
/// high) would PIN the max clock there — the sclk-pinning bug. The clock is
/// clamped into the driver's OD_RANGE. Returns false on failure (needs root /
/// overdrive enabled).
pub fn od_set_vddc(top_mhz: u32, vddc_mv: u32) -> bool {
    let Some(dir) = drm_device_dir() else {
        return false;
    };
    let p = dir.join("pp_od_clk_voltage");
    let sclk = match od_sclk_range() {
        Some((lo, hi)) => top_mhz.clamp(lo, hi),
        None => top_mhz,
    };
    // Final safety net: never drive a voltage below the clock's floor, even if a
    // stale persisted value (set for a lower clock) is re-applied at a higher one.
    let vddc = vddc_mv.max(min_gfx_vddc(sclk));
    // H3 (defense-in-depth): never exceed the silicon ceiling, even if a future or
    // rebinned board reports a higher OD_RANGE VDDC max. Clamp to the hard ceiling
    // AND, when the driver reports one, the OD_RANGE max — whichever is lower.
    let ceiling = match od_vddc_range() {
        Some((_, hi)) => hi.min(ariel_smu::smu::GFX_VID_CEILING_MV),
        None => ariel_smu::smu::GFX_VID_CEILING_MV,
    };
    let vddc = vddc.min(ceiling);
    if fs::write(&p, format!("vc 0 {sclk} {vddc}\n")).is_err() {
        return false;
    }
    if fs::write(&p, "c\n").is_err() {
        // Commit failed with the point still staged — discard it (best-effort)
        // so a later unrelated commit can't flush our half-applied point.
        let _ = fs::write(&p, "r\n");
        return false;
    }
    true
}

/// Restore the stock overdrive curve (SMU-managed voltage): `r` then commit.
pub fn od_reset() -> bool {
    let Some(dir) = drm_device_dir() else {
        return false;
    };
    let p = dir.join("pp_od_clk_voltage");
    if fs::write(&p, "r\n").is_err() {
        return false;
    }
    fs::write(&p, "c\n").is_ok()
}

/// SoC power in watts, parsed from amdgpu_pm_info. Best-effort.
#[allow(dead_code)] // thermal-budget input kept for the governor's future use
pub fn soc_watts(dbg: &Path) -> Option<f64> {
    let txt = fs::read_to_string(dbg.join("amdgpu_pm_info")).ok()?;
    for line in txt.lines() {
        // e.g. "  45.00 W (average SoC)" or "... (SoC)"
        if line.contains("SoC") && line.contains('W') {
            for tok in line.split_whitespace() {
                if let Ok(w) = tok.parse::<f64>() {
                    return Some(w);
                }
            }
        }
    }
    None
}

/// Parse the starred (active) MHz value out of pp_dpm_sclk text.
/// Lines look like "1: 2500Mhz *" — take the MHz after the colon, not the index.
fn parse_active_sclk(txt: &str) -> Option<u32> {
    for line in txt.lines() {
        if !line.trim_end().ends_with('*') {
            continue;
        }
        let Some(mhz) = line.split(':').nth(1) else {
            continue;
        };
        // The token has leading whitespace — trim BEFORE taking digits, or the
        // take_while sees ' ' first and yields nothing (the always-None bug).
        let d: String = mhz
            .trim_start()
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if let Ok(v) = d.parse() {
            return Some(v);
        }
        // Un-parseable starred line: keep scanning instead of giving up.
    }
    None
}

/// Current GFX clock in MHz from pp_dpm_sclk's starred (active) line.
pub fn current_sclk_mhz() -> Option<u32> {
    let entries = fs::read_dir("/sys/class/drm").ok()?;
    for e in entries.flatten() {
        let p = e.path().join("device/pp_dpm_sclk");
        if let Ok(txt) = fs::read_to_string(&p) {
            if let Some(mhz) = parse_active_sclk(&txt) {
                return Some(mhz);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// REGRESSION: the old parser never trimmed the token before take_while, so
    /// it returned None for EVERY real pp_dpm_sclk (the value always has a
    /// leading space after the colon).
    #[test]
    fn parse_active_sclk_real_format() {
        let txt = "0: 350Mhz\n1: 2500Mhz *\n2: 1500Mhz\n";
        assert_eq!(parse_active_sclk(txt), Some(2500));
        // Star on the first line, no trailing newline.
        assert_eq!(parse_active_sclk("0: 350Mhz *"), Some(350));
        // A non-matching starred line must not abort the scan.
        assert_eq!(parse_active_sclk("garbage *\n1: 1000Mhz *\n"), Some(1000));
        assert_eq!(parse_active_sclk("0: 350Mhz\n1: 2500Mhz\n"), None);
    }
}
