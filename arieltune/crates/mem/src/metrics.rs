// SPDX-License-Identifier: GPL-2.0-only
//! Live GPU/memory telemetry from the amdgpu `gpu_metrics` sysfs blob.
//!
//! The BC-250 exposes an APU metrics table (`gpu_metrics_v2_2`). We read the few
//! fields that are reliable on this silicon: the GFX core clock (which drives the
//! latency/random numbers — it boosts under load), the UMC/memory clock (locked
//! ~450 MHz here), and the GFX die temperature. Safe sysfs read, no SMN/ioctl.
//!
//! NOTE: reading this while a GPU compute job is in flight can race (a measured
//! gpu-metrics race during compute), so callers should sample it
//! on idle, not during a bench dispatch.

use std::fs;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default)]
pub struct Telemetry {
    pub gfxclk_mhz: u16,
    pub uclk_mhz: u16,
    pub temp_c: f64,
}

/// Read current telemetry, or None if the metrics blob isn't readable.
pub fn read() -> Option<Telemetry> {
    let b = fs::read(gpu_metrics_path()?).ok()?;
    if b.len() < 82 {
        return None;
    }
    let u16at = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
    // header: structure_size(2), format_revision(1), content_revision(1)
    let (fmt, content) = (b[2], b[3]);
    // temperature_gfx is at offset 4 in every v2_x; it's in hundredths of a degree.
    let temp_c = u16at(4) as f64 / 100.0;
    // current_gfxclk / current_uclk land at 76 / 80 in v2_2.
    let (gfxclk_mhz, uclk_mhz) = if fmt == 2 && content >= 2 {
        (u16at(76), u16at(80))
    } else {
        (0, 0)
    };
    Some(Telemetry {
        gfxclk_mhz,
        uclk_mhz,
        temp_c,
    })
}

fn gpu_metrics_path() -> Option<PathBuf> {
    for entry in fs::read_dir("/sys/class/drm").ok()?.flatten() {
        let dev = entry.path().join("device");
        let f = dev.join("gpu_metrics");
        // pp_dpm_sclk confirms this card is the amdgpu GPU.
        if f.exists() && dev.join("pp_dpm_sclk").exists() {
            return Some(f);
        }
    }
    None
}
