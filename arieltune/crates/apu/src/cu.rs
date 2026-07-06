// SPDX-License-Identifier: GPL-2.0-only
//! Compute-unit topology: read the live CU bitmap and render the harvest map.
//!
//! We talk to amdgpu directly via the DRM `AMDGPU_INFO` ioctl (query
//! `DEV_INFO` = 0x16) — no libdrm dependency. The returned
//! `drm_amdgpu_info_device` layout (offsets validated on a real BC-250 by the
//! original `cu_map.sh`):
//!   num_shader_engines            @ 20
//!   num_shader_arrays_per_engine  @ 24
//!   cu_active_number              @ 48
//!   cu_bitmap[4][4]              @ 56
//!
//! Each shader array on Cyan Skillfish has 5 WGPs = 10 CU slots.

use std::fs::{self, OpenOptions};
use std::os::fd::AsRawFd;
use std::path::Path;

use anyhow::{Context, Result};

/// modprobe.d drop-in that persists the 40-CU liberation across reboots.
pub const MODPROBE_CONF: &str = "/etc/modprobe.d/aputune-40cu.conf";
const MODPROBE_LINE: &str = "options amdgpu bc250_cc_write_mode=3\n";

const AMDGPU_INFO_DEV_INFO: u32 = 0x16;
const DRM_IOCTL_BASE: u32 = 0x64; // 'd'
const DRM_COMMAND_BASE: u32 = 0x40;

// _IOWR(0x64, 0x40 + 0x05, struct drm_amdgpu_info) with struct = 32 bytes.
const fn iowr(nr: u32, size: u32) -> u64 {
    let dir = 3u32; // _IOC_READ | _IOC_WRITE
    ((dir << 30) | (size << 16) | (DRM_IOCTL_BASE << 8) | nr) as u64
}
const DRM_IOCTL_AMDGPU_INFO: u64 = iowr(DRM_COMMAND_BASE + 0x05, 32);

#[repr(C)]
struct DrmAmdgpuInfo {
    return_pointer: u64,
    return_size: u32,
    query: u32,
    _union: [u32; 4],
}

const RENDER_NODES: &[&str] = &["/dev/dri/renderD128", "/dev/dri/renderD129"];

/// Raw `DEV_INFO` buffer from the first render node that answers.
fn dev_info() -> Option<[u8; 1024]> {
    for node in RENDER_NODES {
        let Ok(f) = OpenOptions::new().read(true).write(true).open(node) else {
            continue;
        };
        let mut buf = [0u8; 1024];
        let mut req = DrmAmdgpuInfo {
            return_pointer: buf.as_mut_ptr() as u64,
            return_size: buf.len() as u32,
            query: AMDGPU_INFO_DEV_INFO,
            _union: [0; 4],
        };
        let rc = unsafe {
            libc::ioctl(
                f.as_raw_fd(),
                DRM_IOCTL_AMDGPU_INFO as libc::c_ulong,
                &mut req as *mut _,
            )
        };
        if rc == 0 {
            return Some(buf);
        }
    }
    None
}

fn u32_at(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

/// One shader array's CU layout.
pub struct ArrayMap {
    pub se: u32,
    pub sh: u32,
    pub bitmap: u32,
    pub active: u32,
    /// "full" | "contiguous" | "scattered"
    pub pattern: &'static str,
}

pub struct CuMap {
    // se/sh counts are parsed for the DEV_INFO layout's sake and future
    // multi-engine rendering; the current render keys off `arrays` directly.
    #[allow(dead_code)]
    pub num_se: u32,
    #[allow(dead_code)]
    pub num_sh: u32,
    pub active: u32,
    pub possible: u32,
    pub arrays: Vec<ArrayMap>,
}

const SLOTS_PER_ARRAY: usize = 10; // 5 WGP x 2 CU

fn classify(bitmap: u32) -> &'static str {
    let disabled: Vec<usize> = (0..SLOTS_PER_ARRAY)
        .filter(|i| bitmap & (1 << i) == 0)
        .collect();
    if disabled.is_empty() {
        "full"
    } else if disabled.windows(2).all(|w| w[1] == w[0] + 1) {
        "contiguous"
    } else {
        "scattered"
    }
}

/// Read the full harvest map. None if amdgpu can't be queried.
pub fn map() -> Option<CuMap> {
    let buf = dev_info()?;
    let num_se = u32_at(&buf, 20);
    let num_sh = u32_at(&buf, 24);
    let active = u32_at(&buf, 48);
    if num_se == 0 || num_se > 8 || num_sh == 0 || num_sh > 8 {
        return None; // implausible — wrong node / not amdgpu
    }
    let mut arrays = Vec::new();
    for se in 0..num_se {
        for sh in 0..num_sh {
            let bm = u32_at(&buf, 56 + ((se * 4 + sh) * 4) as usize);
            arrays.push(ArrayMap {
                se,
                sh,
                bitmap: bm,
                active: bm.count_ones(),
                pattern: classify(bm),
            });
        }
    }
    let possible = num_se * num_sh * SLOTS_PER_ARRAY as u32;
    Some(CuMap {
        num_se,
        num_sh,
        active,
        possible,
        arrays,
    })
}

/// Active CU count from amdgpu, if available. Used by patch detection (12).
pub fn active_cu_count() -> Option<u32> {
    map().map(|m| m.active)
}

/// Per-shader-array DRIVER WGP enumeration — how the APU came up at boot: the
/// harvest topology amdgpu read from the silicon (fuses + CC), including any
/// fuse-broken WGPs. A WGP counts as driver-enabled if amdgpu enumerated either
/// of its two CUs. This is the boot ground truth (DEV_INFO's `cu_bitmap` is set
/// at gfx init and is NOT changed by live CC writes), so it's distinct from the
/// live SPI dispatch mask (`curoute`): a WGP can be enumerated but SPI-gated, or
/// — on a marginal APU — never enumerated at all (a broken WGP).
///
/// Indices match `curoute::ARRAYS`: [SE0.SH0, SE0.SH1, SE1.SH0, SE1.SH1].
pub fn driver_wgp_masks() -> Option<[u32; 4]> {
    let m = map()?;
    let mut out = [0u32; 4];
    for a in &m.arrays {
        let idx = (a.se * 2 + a.sh) as usize;
        if idx >= 4 {
            continue;
        }
        let mut wm = 0u32;
        for w in 0..5u32 {
            // Two CU slots per WGP: bits (2w, 2w+1).
            if a.bitmap & (0x3 << (w * 2)) != 0 {
                wm |= 1 << w;
            }
        }
        out[idx] = wm;
    }
    Some(out)
}

/// Whether the 40-CU liberation is currently armed via the module parameter.
pub fn liberation_armed() -> bool {
    fs::read_to_string("/sys/module/amdgpu/parameters/bc250_cc_write_mode")
        .map(|s| s.trim() != "0" && !s.trim().is_empty())
        .unwrap_or(false)
}

/// Persist the 40-CU liberation: write the modprobe.d drop-in so the patched
/// amdgpu loads with `bc250_cc_write_mode=3`. Takes effect on the next boot
/// (after `mkinitcpio -P`). Requires root and the 40-CU patch (12) in the kernel.
pub fn enable_persist() -> Result<()> {
    fs::write(MODPROBE_CONF, MODPROBE_LINE)
        .with_context(|| format!("write {MODPROBE_CONF} (need root)"))?;
    Ok(())
}

/// Remove the persisted liberation (reverts to stock 24 CU on next boot).
pub fn disable_persist() -> Result<()> {
    if Path::new(MODPROBE_CONF).exists() {
        fs::remove_file(MODPROBE_CONF)
            .with_context(|| format!("remove {MODPROBE_CONF} (need root)"))?;
    }
    Ok(())
}

/// Whether the persisted drop-in is in place.
pub fn persist_present() -> bool {
    Path::new(MODPROBE_CONF).exists()
}

/// Render the harvest map as ASCII (filled vs harvested per slot).
pub fn render(m: &CuMap) -> String {
    let mut out = String::new();
    for a in &m.arrays {
        let bar: String = (0..SLOTS_PER_ARRAY)
            .map(|i| if a.bitmap & (1 << i) != 0 { '#' } else { '.' })
            .collect();
        out.push_str(&format!(
            "SE{} SH{}: {}  {:>2} CU  {}\n",
            a.se, a.sh, bar, a.active, a.pattern
        ));
    }
    let harvested = m.possible - m.active;
    out.push_str(&format!(
        "{}/{} CUs active, {} harvested\n",
        m.active, m.possible, harvested
    ));
    out
}
