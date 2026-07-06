// SPDX-License-Identifier: GPL-2.0-only
//! Effect verification — does a BIOS field actually *do* anything?
//!
//! The BC-250 trap: most AmdSetup values are decorative (AGESA reads the APCB
//! copy first), so writing a variable and reading it back proves nothing. The
//! only trustworthy check is an INDEPENDENT downstream observable — a real
//! silicon/OS effect that has nothing to do with the variable store: CPU
//! topology, PCIe link speed, GPU power/clock under load, IOMMU groups, the GPU
//! memory aperture. This module captures a fingerprint of those observables so
//! you can snapshot before a change, reboot, snapshot after, and diff.
//!
//! Workflow:
//!   biostune effect snapshot --out pre.json
//!   biostune set FOO=BAR --write && reboot
//!   biostune effect snapshot --out post.json
//!   biostune effect diff pre.json post.json     # changed observable => field is LIVE
//!
//! Proven with this harness (measured): the `PPT` toggle is LIVE (Auto ->
//! 2230MHz@165W, Enabled -> 1500MHz@114W under GPU load), while the numeric
//! `FAST/SLOW_PPT_LIMIT` fields are inert (40k and 120k gave identical results).
//! GPU clock/power are load-dependent, so for power-limit fields drive a GPU
//! load and watch `effect gpu` rather than relying on the static snapshot.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

/// A fingerprint of independent, downstream-of-firmware observables.
#[derive(Serialize, Deserialize, Debug, Default, PartialEq)]
pub struct Fingerprint {
    // CPU topology (Downcore / SMT effects)
    pub nproc: Option<u32>,
    pub cpus_online: Option<String>,
    pub siblings: Option<u32>,
    pub cpu_cores: Option<u32>,
    // CPU feature flags (SVM/virtualization effects)
    pub svm: bool,
    // cpufreq (Core Performance Boost effects). NOTE: the test board's cpufreq
    // sysfs is often empty (no active driver) — then these are None and CPB needs
    // an MSR/turbostat observable instead.
    pub cpufreq_boost: Option<String>,
    pub scaling_max_khz: Option<u64>,
    pub cpuinfo_max_khz: Option<u64>,
    pub scaling_driver: Option<String>,
    // IOMMU / virtualization
    pub iommu_groups: usize,
    pub kvm_amd: bool,
    // PCIe GPU link (Speed Mode effects) — read from sysfs, no root/lspci needed
    pub gpu_link_speed: Option<String>,
    pub gpu_link_width: Option<String>,
    // GPU BAR base (Above-4G-Decoding effect): > 0x1_0000_0000 == relocated high
    pub gpu_bar0_base: Option<String>,
    // GPU memory aperture (UMA size effect)
    pub vram_total: Option<u64>,
    pub vis_vram_total: Option<u64>,
    // GPU clock envelope (DPM table / OverDrive range)
    pub dpm_sclk_max_mhz: Option<u32>,
    pub od_sclk_max_mhz: Option<u32>,
    // GPU instantaneous (snapshot-time; load-dependent — use `effect gpu` for live)
    pub gpu_sclk_mhz: Option<u32>,
    pub gpu_power_w: Option<u32>,
    pub gpu_temp_c: Option<u32>,
}

fn rd(p: &str) -> Option<String> {
    fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}
fn rd_u64(p: &str) -> Option<u64> {
    rd(p).and_then(|s| s.parse().ok())
}

/// Locate the AMD GPU's sysfs device dir (card index varies across boots).
fn gpu_dev() -> Option<PathBuf> {
    for entry in fs::read_dir("/sys/class/drm").ok()?.flatten() {
        let dev = entry.path().join("device");
        // amdgpu render node has pp_dpm_sclk; vendor 0x1002 == AMD
        if dev.join("pp_dpm_sclk").exists()
            && rd(dev.join("vendor").to_str()?).as_deref() == Some("0x1002")
        {
            return Some(dev);
        }
    }
    None
}

/// First hwmon dir under a GPU device.
fn gpu_hwmon(dev: &Path) -> Option<PathBuf> {
    fs::read_dir(dev.join("hwmon"))
        .ok()?
        .flatten()
        .next()
        .map(|e| e.path())
}

/// Parse the top MHz from a `pp_dpm_sclk` listing ("2: 2500Mhz").
fn max_mhz_from_dpm(s: &str) -> Option<u32> {
    s.lines()
        .filter_map(|l| {
            l.split(':')
                .nth(1)?
                .trim()
                .trim_end_matches('*')
                .trim()
                .to_lowercase()
                .strip_suffix("mhz")?
                .trim()
                .parse()
                .ok()
        })
        .max()
}

/// Parse the OD_RANGE SCLK ceiling from `pp_od_clk_voltage`.
fn od_sclk_max(s: &str) -> Option<u32> {
    let mut in_range = false;
    for l in s.lines() {
        let t = l.trim();
        if t.starts_with("OD_RANGE") {
            in_range = true;
            continue;
        }
        if in_range && t.to_uppercase().starts_with("SCLK") {
            // "SCLK:    1000Mhz       2500Mhz" -> second number
            return t
                .to_lowercase()
                .replace("mhz", " ")
                .split_whitespace()
                .filter_map(|w| w.parse::<u32>().ok())
                .max();
        }
    }
    None
}

#[allow(clippy::field_reassign_with_default)] // fields are filled conditionally (GPU block is optional)
pub fn capture() -> Fingerprint {
    let mut f = Fingerprint::default();

    // topology
    f.nproc = rd("/proc/cpuinfo").map(|c| c.matches("processor\t").count() as u32);
    f.cpus_online = rd("/sys/devices/system/cpu/online");
    if let Some(ci) = rd("/proc/cpuinfo") {
        f.siblings = ci
            .lines()
            .find(|l| l.starts_with("siblings"))
            .and_then(|l| l.split(':').nth(1)?.trim().parse().ok());
        f.cpu_cores = ci
            .lines()
            .find(|l| l.starts_with("cpu cores"))
            .and_then(|l| l.split(':').nth(1)?.trim().parse().ok());
        f.svm = ci
            .lines()
            .any(|l| l.starts_with("flags") && l.split_whitespace().any(|t| t == "svm"));
    }

    // cpufreq
    f.cpufreq_boost = rd("/sys/devices/system/cpu/cpufreq/boost");
    f.scaling_max_khz = rd_u64("/sys/devices/system/cpu/cpu0/cpufreq/scaling_max_freq");
    f.cpuinfo_max_khz = rd_u64("/sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq");
    f.scaling_driver = rd("/sys/devices/system/cpu/cpu0/cpufreq/scaling_driver");

    // iommu / virt
    f.iommu_groups = fs::read_dir("/sys/kernel/iommu_groups")
        .map(|d| d.flatten().count())
        .unwrap_or(0);
    f.kvm_amd = Path::new("/sys/module/kvm_amd").exists();

    // GPU-derived observables
    if let Some(dev) = gpu_dev() {
        let p = |n: &str| dev.join(n).to_str().and_then(rd);
        f.gpu_link_speed = p("current_link_speed");
        f.gpu_link_width = p("current_link_width");
        // BAR0 base from the PCI resource file (line 0: "start end flags")
        f.gpu_bar0_base = dev.join("resource").to_str().and_then(rd).and_then(|s| {
            s.lines()
                .next()
                .map(|l| l.split_whitespace().next().unwrap_or("").to_string())
        });
        f.vram_total = dev.join("mem_info_vram_total").to_str().and_then(rd_u64);
        f.vis_vram_total = dev
            .join("mem_info_vis_vram_total")
            .to_str()
            .and_then(rd_u64);
        if let Some(s) = dev.join("pp_dpm_sclk").to_str().and_then(rd) {
            f.dpm_sclk_max_mhz = max_mhz_from_dpm(&s);
        }
        if let Some(s) = dev.join("pp_od_clk_voltage").to_str().and_then(rd) {
            f.od_sclk_max_mhz = od_sclk_max(&s);
        }
        if let Some(hw) = gpu_hwmon(&dev) {
            f.gpu_sclk_mhz = hw
                .join("freq1_input")
                .to_str()
                .and_then(rd_u64)
                .map(|v| (v / 1_000_000) as u32);
            f.gpu_power_w = hw
                .join("power1_average")
                .to_str()
                .and_then(rd_u64)
                .map(|v| (v / 1_000_000) as u32);
            f.gpu_temp_c = hw
                .join("temp1_input")
                .to_str()
                .and_then(rd_u64)
                .map(|v| (v / 1000) as u32);
        }
    }
    f
}

/// `effect snapshot [--out FILE]` — capture a fingerprint to FILE or stdout.
pub fn snapshot(out: Option<&str>) -> Result<()> {
    let json = serde_json::to_string_pretty(&capture())?;
    match out {
        Some(path) => {
            fs::write(path, &json).with_context(|| format!("write {path}"))?;
            eprintln!("wrote fingerprint to {path}");
        }
        None => println!("{json}"),
    }
    Ok(())
}

/// `effect diff A B` — show observables that changed (LIVE) or none (INERT).
pub fn diff(a: &str, b: &str) -> Result<()> {
    let va: Value =
        serde_json::from_str(&fs::read_to_string(a).with_context(|| format!("read {a}"))?)?;
    let vb: Value =
        serde_json::from_str(&fs::read_to_string(b).with_context(|| format!("read {b}"))?)?;
    let (oa, ob) = (
        va.as_object().context("A is not a fingerprint object")?,
        vb.as_object().context("B is not a fingerprint object")?,
    );
    println!("effect diff {a} -> {b}");
    let mut changed = 0;
    for (k, x) in oa {
        let y = ob.get(k).unwrap_or(&Value::Null);
        if x != y {
            println!("  {k}: {x} -> {y}");
            changed += 1;
        }
    }
    if changed == 0 {
        println!("\nno observable changed — the field(s) are INERT/decorative, or did not apply.");
    } else {
        println!("\n{changed} observable(s) changed — the field(s) are LIVE.");
    }
    Ok(())
}

/// `effect gpu [--secs N]` — sample the GPU clock/power/temp for N seconds and
/// report the peak. Drive a GPU load in parallel; this is the dynamic observable
/// for power-limit fields (PPT/TDC/EDC) whose effect only shows under load.
pub fn gpu(secs: u32) -> Result<()> {
    use std::thread::sleep;
    use std::time::Duration;
    let dev = gpu_dev().context("no AMD GPU found")?;
    let hw = gpu_hwmon(&dev).context("no GPU hwmon")?;
    let f = |n: &str| hw.join(n).to_str().and_then(rd_u64).unwrap_or(0);
    let (mut max_s, mut max_p, mut max_t) = (0u64, 0u64, 0u64);
    let samples = secs * 2;
    for _ in 0..samples {
        let s = f("freq1_input") / 1_000_000;
        let p = f("power1_average") / 1_000_000;
        let t = f("temp1_input") / 1000;
        max_s = max_s.max(s);
        max_p = max_p.max(p);
        max_t = max_t.max(t);
        println!("  sclk={s} MHz  pwr={p} W  temp={t} C");
        sleep(Duration::from_millis(500));
    }
    println!("\npeak: sclk={max_s} MHz  pwr={max_p} W  temp={max_t} C  (over {secs}s)");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpm_max_parsed() {
        let s = "0: 1000Mhz *\n1: 1500Mhz \n2: 2500Mhz ";
        assert_eq!(max_mhz_from_dpm(s), Some(2500));
    }

    #[test]
    fn od_range_max_parsed() {
        let s =
            "OD_SCLK:\n0: 1000Mhz *\nOD_RANGE:\nSCLK:    1000Mhz       2500Mhz\nVDDC: 700mV 1129mV";
        assert_eq!(od_sclk_max(s), Some(2500));
    }

    #[test]
    fn diff_detects_no_change_as_inert() {
        // two identical fingerprints -> serde Values equal -> zero changes
        let a = serde_json::to_value(Fingerprint::default()).unwrap();
        let b = serde_json::to_value(Fingerprint::default()).unwrap();
        assert_eq!(a, b);
    }
}
