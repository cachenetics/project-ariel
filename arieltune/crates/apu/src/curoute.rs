// SPDX-License-Identifier: GPL-2.0-only
//! Live CU/WGP routing via UMR — a faithful port of bc250-cu-live-manager's
//! proven register sequence:
//!
//!   * `CC_GC_SHADER_ARRAY_CONFIG` = 0 (clear the harvest mask → the driver
//!     enumerates all CUs, so RADV issues work to the routed WGPs)
//!   * per shader array: `SPI_PG_ENABLE_STATIC_WGP_MASK` = the WGP dispatch mask
//!     (bit w = WGP w; 5 WGP / 10 CU per array)
//!   * `RLC_PG_ALWAYS_ON_WGP_MASK` = the union (power-gate override)
//!
//! Validated on a BC-250: live, reversible register writes, no wedge. Requires
//! `umr` (same dependency as cu-live-manager) — gated so the rest of aputune
//! works without it.

use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};

const ASIC: &str = "cyan_skillfish.gfx1013";
const REG_CC: &str = "mmCC_GC_SHADER_ARRAY_CONFIG";
const REG_SPI: &str = "mmSPI_PG_ENABLE_STATIC_WGP_MASK";
const REG_RLC: &str = "mmRLC_PG_ALWAYS_ON_WGP_MASK";

/// All 5 WGP routed = 40 CU.
pub const FULL_MASK: u32 = 0x1f;
/// Factory dispatch = WGP 0-2 = 24 CU.
pub const FACTORY_MASK: u32 = 0x07;
/// The four shader arrays, in grid order: (SE, SH).
pub const ARRAYS: [(u32, u32); 4] = [(0, 0), (0, 1), (1, 0), (1, 1)];

fn umr_bin() -> Result<&'static str> {
    for p in [
        "/usr/bin/umr",
        "/usr/local/bin/umr",
        "/opt/umr/build/src/app/umr",
    ] {
        if Path::new(p).exists() {
            return Ok(p);
        }
    }
    bail!("umr not found — install it for live CU routing (e.g. pacman -S umr)")
}

/// DRI instance number for the amdgpu device (matches the debugfs dir).
fn instance() -> String {
    ariel_hal::amdgpu_dbg_dir()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "0".into())
}

fn reg(name: &str) -> String {
    format!("{ASIC}.{name}")
}

fn run(args: &[String]) -> Result<String> {
    let bin = umr_bin()?;
    let out = Command::new(bin)
        .args(args)
        .output()
        .with_context(|| format!("spawn {bin}"))?;
    if !out.status.success() {
        bail!(
            "umr failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Read a GRBM-banked register's value for shader array (se, sh).
fn read_bank(name: &str, se: u32, sh: u32) -> Result<u32> {
    let inst = instance();
    let out = run(&[
        "-i".into(),
        inst,
        "-b".into(),
        se.to_string(),
        sh.to_string(),
        "x".into(),
        "-r".into(),
        reg(name),
    ])?;
    // line looks like "...mmSPI_PG_ENABLE_STATIC_WGP_MASK => 0x0000001f"
    let hex = out
        .rsplit("=>")
        .next()
        .and_then(|s| s.trim().strip_prefix("0x"))
        .context("no value in umr output")?;
    u32::from_str_radix(hex.trim(), 16).context("parse umr hex")
}

fn write_bank(name: &str, val: u32, se: u32, sh: u32) -> Result<()> {
    let inst = instance();
    run(&[
        "-i".into(),
        inst,
        "-w".into(),
        reg(name),
        format!("0x{val:08x}"),
        "-b".into(),
        se.to_string(),
        sh.to_string(),
        "0xffffffff".into(),
    ])
    .map(|_| ())
}

fn write_global(name: &str, val: u32) -> Result<()> {
    let inst = instance();
    run(&[
        "-i".into(),
        inst,
        "-w".into(),
        reg(name),
        format!("0x{val:08x}"),
    ])
    .map(|_| ())
}

/// Whether live CU routing is available (umr present).
pub fn available() -> bool {
    umr_bin().is_ok()
}

/// Current per-array SPI dispatch masks (the live routing).
pub fn current_masks() -> Result<[u32; 4]> {
    let mut m = [0u32; 4];
    for (i, (se, sh)) in ARRAYS.iter().enumerate() {
        m[i] = read_bank(REG_SPI, *se, *sh)? & FULL_MASK;
    }
    Ok(m)
}

/// True if any of the 4 shader arrays has an all-zero WGP mask. Dispatching
/// compute with a whole array gated off HANGS gfx1013 and wedges the box (the
/// GPU-reset path is broken), so this shape is refused at the actuation
/// chokepoints unless explicitly forced.
pub fn has_empty_array(masks: &[u32; 4]) -> bool {
    masks.iter().any(|m| m & FULL_MASK == 0)
}

/// Refuse an empty-shader-array shape (the gfx1013 compute-hang wedge class).
fn ensure_no_empty_array(masks: &[u32; 4]) -> Result<()> {
    if has_empty_array(masks) {
        let names: Vec<String> = (0..4)
            .filter(|&i| masks[i] & FULL_MASK == 0)
            .map(|i| {
                let (se, sh) = ARRAYS[i];
                format!("SE{se}.SH{sh}")
            })
            .collect();
        bail!(
            "refusing empty-shader-array route ({} empty): dispatching compute with a \
             whole array gated off hangs gfx1013 and wedges the box. Populate every \
             array, or pass the explicit unsafe override.",
            names.join(", ")
        );
    }
    Ok(())
}

/// Apply per-array WGP dispatch masks (cu-live-manager's `apply_target_masks`):
/// clear CC harvest, write each array's SPI mask, set RLC to the union.
///
/// SAFETY: refuses a shape with any empty shader array (compute on it wedges
/// gfx1013). Use [`apply_forced`] to bypass — only for explicit operator override.
pub fn apply(masks: [u32; 4]) -> Result<()> {
    ensure_no_empty_array(&masks)?;
    apply_forced(masks)
}

/// [`apply`] WITHOUT the empty-array safety refusal. Only for an explicit,
/// operator-acknowledged unsafe override (`--force-unsafe`).
pub fn apply_forced(masks: [u32; 4]) -> Result<()> {
    let _ = write_global(REG_CC, 0); // best-effort global clear
    let mut union = 0u32;
    for (i, (se, sh)) in ARRAYS.iter().enumerate() {
        let m = masks[i] & FULL_MASK;
        write_bank(REG_CC, 0, *se, *sh)?;
        write_bank(REG_SPI, m, *se, *sh)?;
        union |= m;
    }
    let _ = write_global(REG_RLC, union);
    Ok(())
}

/// Route all 40 CUs.
pub fn enable_all() -> Result<()> {
    apply([FULL_MASK; 4])
}

/// Restore factory dispatch (24 CU).
pub fn factory() -> Result<()> {
    apply([FACTORY_MASK; 4])
}

/// Toggle one WGP (array index 0..3, wgp 0..4) in the live routing. Refuses a
/// toggle that would empty a shader array unless `force` (the wedge class).
pub fn toggle_wgp(array: usize, wgp: u32, force: bool) -> Result<[u32; 4]> {
    // Guard: 4 shader arrays, <=32-bit mask (WGP 0..31). An out-of-range index
    // would panic (array) or shift-overflow (`1 << wgp`); refuse instead.
    if array >= 4 || wgp >= 32 {
        anyhow::bail!("toggle_wgp: array {array} / wgp {wgp} out of range (array 0..3, wgp 0..31)");
    }
    let mut m = current_masks()?;
    m[array] ^= 1 << wgp;
    m[array] &= FULL_MASK;
    if force {
        apply_forced(m)?;
    } else {
        apply(m)?;
    }
    Ok(m)
}

/// CU count for a WGP mask (2 CU per routed WGP).
pub fn cu_count(masks: &[u32; 4]) -> u32 {
    masks.iter().map(|m| (m & FULL_MASK).count_ones() * 2).sum()
}

/// Diagnostic of a per-array WGP mask set.
///
/// REAL MATH (measured, KAT compute GFLOPS on gfx1013; 12-config sweep with any
/// GPU inference server stopped). Compute throughput is gated by the TWO
/// LEAST-populated shader arrays:
///     effective_CU = 4 × (w₁ + w₂)     w₁,w₂ = the two smallest per-array WGP
///     GFLOPS ≈ 44 × effective_CU       at 1500 MHz  (≈ 0.029 × eff_CU × clock)
/// Fits all 12 configs exactly. For a BALANCED shape the two smallest equal the
/// rest, so eff_CU == routed CU and throughput is linear in routed CU (verified
/// 8/16/24/32/40 CU -> 357/710/1058/1408/1750 GFLOPS). For an UNBALANCED shape the
/// two biggest arrays are capped to the two smallest, so the penalty is large, not
/// small: e.g. 5/5/1/1 routes 24 CU but delivers 8 (357 GFLOPS). WGPs are
/// homogeneous (2 CU each). This supersedes the old "populated × shallowest depth"
/// model (which only counted the single minimum) and is a distinct, compute-only
/// law — memory-bound LLM inference weights array population differently.
///
/// An EMPTY shader array is also a SAFETY concern: dispatching compute with a whole
/// array gated off has hung this silicon; the empty-array compute case is untested,
/// so it stays flagged (and the eff_CU formula is validated only for 4-populated).
#[derive(Debug, Clone, PartialEq)]
pub struct RouteShape {
    /// Routed CU (2 per routed WGP).
    pub cu: u32,
    /// WGP count per shader array (grid order).
    pub per_array_wgp: [u32; 4],
    /// Arrays with >= 1 WGP.
    pub populated: usize,
    /// Shallowest populated array's WGP count (0 if none populated).
    pub min_populated_wgp: u32,
    /// Effective CU = 4 × (two smallest per-array WGP) — the MEASURED throughput
    /// driver: GFLOPS ≈ 44 × this at 1500 MHz. Equals `cu` when balanced; less when
    /// the two biggest arrays out-run the two smallest.
    pub effective_cu: u32,
    /// Indices of fully-gated (empty) arrays.
    pub empty_arrays: Vec<usize>,
    /// Unequal per-array WGP — the two biggest arrays are wasted down to the two
    /// smallest (see `effective_cu`).
    pub unbalanced: bool,
}

/// Analyse a routing shape (see [`RouteShape`]).
pub fn shape(masks: &[u32; 4]) -> RouteShape {
    let per: [u32; 4] = std::array::from_fn(|i| (masks[i] & FULL_MASK).count_ones());
    let pop: Vec<u32> = per.iter().copied().filter(|&c| c > 0).collect();
    let populated = pop.len();
    let min_populated_wgp = pop.iter().copied().min().unwrap_or(0);
    // Effective CU (MEASURED, compute-bound): throughput is gated by the TWO
    // least-populated shader arrays — eff_CU = 4 × (sum of the two smallest
    // per-array WGP counts). Verified exact across a 12-config sweep.
    // Balanced shapes -> eff == routed; unbalanced -> the two biggest arrays are
    // capped to the two smallest.
    let mut sorted = per;
    sorted.sort_unstable();
    let effective_cu = 4 * (sorted[0] + sorted[1]);
    RouteShape {
        cu: cu_count(masks),
        per_array_wgp: per,
        populated,
        min_populated_wgp,
        effective_cu,
        empty_arrays: (0..4).filter(|&i| per[i] == 0).collect(),
        unbalanced: pop.iter().any(|&c| c != min_populated_wgp),
    }
}

impl RouteShape {
    /// Human-readable warnings about this shape (safety + a small efficiency
    /// note), or empty if the shape is balanced and fully populated.
    pub fn warnings(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.empty_arrays.is_empty() {
            let names: Vec<String> = self
                .empty_arrays
                .iter()
                .map(|&i| {
                    let (se, sh) = ARRAYS[i];
                    format!("SE{se}.SH{sh}")
                })
                .collect();
            out.push(format!(
                "UNSAFE: {} shader array(s) empty ({}) — dispatching compute with a \
                 whole array gated off can hang gfx1013 (untested since the legacy-race fix).",
                self.empty_arrays.len(),
                names.join(", ")
            ));
        }
        if self.unbalanced {
            out.push(format!(
                "unequal WGP per array ({:?}) — throughput is gated by the two smallest: \
                 only {} effective CU of {} routed. Balance to recover the rest.",
                self.per_array_wgp, self.effective_cu, self.cu
            ));
        }
        out
    }
}

/// Where the service-routing profile lives (cu-live-manager's service-table
/// idea: save a chosen routing, re-apply it at boot).
pub const PROFILE_PATH: &str = "/var/lib/aputune/route.json";

#[derive(serde::Serialize, serde::Deserialize)]
struct RouteProfile {
    masks: [u32; 4],
}

/// Save the current live routing as the service profile. Returns the masks.
pub fn save_profile() -> Result<[u32; 4]> {
    let masks = current_masks()?;
    if let Some(dir) = Path::new(PROFILE_PATH).parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("mkdir {}", dir.display()))?;
    }
    let json = serde_json::to_string_pretty(&RouteProfile { masks })?;
    std::fs::write(PROFILE_PATH, json).with_context(|| format!("write {PROFILE_PATH}"))?;
    Ok(masks)
}

/// Load the saved service-profile masks.
pub fn load_profile() -> Result<[u32; 4]> {
    let s = std::fs::read_to_string(PROFILE_PATH)
        .with_context(|| format!("no service profile at {PROFILE_PATH} (run `cu route-save`)"))?;
    let p: RouteProfile = serde_json::from_str(&s).context("parse service profile")?;
    Ok(p.masks)
}

/// Apply the saved service-profile routing. Returns the masks applied.
///
/// BOOT SAFETY: a saved profile with an empty shader array is REFUSED (compute
/// on it wedges gfx1013) — the boot path falls back to the factory-24 route
/// instead of enacting the unsafe shape, and logs the substitution.
pub fn apply_saved() -> Result<[u32; 4]> {
    let masks = load_profile()?;
    if has_empty_array(&masks) {
        eprintln!(
            "arieltune apu: saved route {masks:02x?} has an empty shader array (compute-wedge \
             class) — refusing it; falling back to factory-24"
        );
        crate::persist::log_transition(&format!(
            "route-load: unsafe empty-array profile {masks:02x?} refused -> factory-24"
        ));
        let factory = [FACTORY_MASK; 4];
        apply(factory)?;
        return Ok(factory);
    }
    apply(masks)?;
    Ok(masks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shape_full_is_optimal() {
        let s = shape(&[FULL_MASK; 4]);
        assert_eq!(s.cu, 40);
        assert_eq!(s.populated, 4);
        assert_eq!(s.effective_cu, 40);
        assert!(s.empty_arrays.is_empty());
        assert!(!s.unbalanced);
        assert!(s.warnings().is_empty());
    }

    #[test]
    fn shape_skewed_is_flagged_unbalanced() {
        // 4/2/1/1 -> 16 routed CU, but eff = 4 × (two smallest WGP = 1+1) = 8 CU
        // (measured: 357 GFLOPS ≈ 44 × 8). The two biggest arrays are wasted down
        // to the two smallest; the warning flags it.
        let s = shape(&[0x0f, 0x03, 0x01, 0x01]);
        assert_eq!(s.cu, 16);
        assert_eq!(s.effective_cu, 8);
        assert!(s.unbalanced);
        assert!(s.empty_arrays.is_empty());
        assert!(s.warnings().iter().any(|w| w.contains("unequal")));
    }

    #[test]
    fn shape_two_smallest_model_matches_sweep() {
        // Measured (eff = GFLOPS/44): each = 4 × (two smallest WGP).
        assert_eq!(shape(&[0x1f, 0x1f, 0x1f, 0x01]).effective_cu, 24); // 5/5/5/1
        assert_eq!(shape(&[0x1f, 0x0f, 0x03, 0x01]).effective_cu, 12); // 5/4/2/1
        assert_eq!(shape(&[0x1f, 0x1f, 0x01, 0x01]).effective_cu, 8); // 5/5/1/1
        assert_eq!(shape(&[0x0f, 0x0f, 0x0f, 0x0f]).effective_cu, 32); // 4/4/4/4 balanced
    }

    #[test]
    fn shape_two_full_arrays_flags_empty() {
        // 5/0/5/0: two empty arrays. The compute eff formula (two smallest = 0,0)
        // degenerates to 0 here — the empty-array COMPUTE case is untested/unsafe
        // (this shape only ran memory-bound), so we flag it rather than trust eff.
        let s = shape(&[FULL_MASK, 0, FULL_MASK, 0]);
        assert_eq!(s.cu, 20);
        assert_eq!(s.populated, 2);
        assert_eq!(s.effective_cu, 0);
        assert!(!s.unbalanced); // the two populated arrays are equal depth
        assert_eq!(s.empty_arrays, vec![1, 3]);
        assert!(s.warnings().iter().any(|w| w.contains("empty")));
    }

    #[test]
    fn shape_all_empty() {
        let s = shape(&[0; 4]);
        assert_eq!(s.cu, 0);
        assert_eq!(s.populated, 0);
        assert_eq!(s.effective_cu, 0);
        assert_eq!(s.empty_arrays, vec![0, 1, 2, 3]);
    }

    /// WEDGE-SAFETY REGRESSION: apply() must refuse an empty-shader-array shape
    /// BEFORE touching any hardware (the refusal precedes the umr calls, so this
    /// is testable off-box). Compute on an empty array hangs + wedges gfx1013.
    #[test]
    fn apply_rejects_empty_array_shape() {
        for masks in [
            [FULL_MASK, 0, FULL_MASK, FULL_MASK],
            [0, 0, 0, 0],
            [FULL_MASK, FULL_MASK, FULL_MASK, 0],
        ] {
            let err = apply(masks).unwrap_err().to_string();
            assert!(err.contains("empty"), "masks {masks:02x?}: {err}");
        }
    }

    #[test]
    fn empty_array_detection() {
        assert!(has_empty_array(&[FULL_MASK, 0, FULL_MASK, FULL_MASK]));
        assert!(!has_empty_array(&[FACTORY_MASK; 4]));
        assert!(!has_empty_array(&[FULL_MASK; 4]));
        // Only the 5 WGP bits count: 0x20 (bit 5) is not a routed WGP.
        assert!(has_empty_array(&[0x20, FULL_MASK, FULL_MASK, FULL_MASK]));
    }
}
