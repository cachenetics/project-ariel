// SPDX-License-Identifier: GPL-2.0-only
//! GPU power control — app-driven setpoints.
//!
//! BC-250's firmware exposes no usable GPU-load signal: released, the clock sits
//! at a flat ~1500 MHz whether idle or lightly loaded, and `QueryGfxclk` only
//! moves under heavy sustained load. So automatic in-kernel/userspace DPM is
//! unreliable here (every attempt — fence-rate, kdpm, peek — fought that weak
//! signal). The reliable model is **app-driven**: the workload, which knows when
//! it has GPU work, forces the top clock on demand and a deep-sleep clock when
//! idle.
//!
//! Measured idle power: forced 350 MHz = 36 W vs 56 W released — a ~20 W
//! deep-sleep win (GFXOFF does not engage on this silicon). Actuation is the
//! race-free `amdgpu_smu_send_raw` ForceGfxFreq; voltage is left to BAPM.
//!
//! Two setpoints: `top` (active — your thermal cap) and `deep` (idle).
//! `autosleep` is a poke-driven loop: an app touches the poke file on each unit
//! of work; if it goes stale past the idle timeout, drop to `deep`; on a fresh
//! poke, force `top`. One SMU write per transition.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use ariel_smu::smu::{self, Smu};

pub const CONFIG_PATH: &str = "/var/lib/aputune/power.json";
/// Runtime dir for the poke file + the power.json RMW lock. Namespaced to
/// `arieltune` (was `/run/aputune`): its existence is the "an arieltune GPU
/// governor is installed" tell the MEM bench keys off before writing the poke.
/// The persistent state dir (`/var/lib/aputune`) is deliberately left as-is.
pub const RUN_DIR: &str = "/run/arieltune";
/// The single-writer clock poke. The MEM bench WRITES it to hold the top clock
/// during a bench; the governor (the ONE SMU clock writer) READS it. File IPC is
/// the single-writer discipline -- an in-process pin from the bench would race
/// the governor daemon (the double-writer hazard), so it is intentionally NOT done.
pub const POKE_PATH: &str = "/run/arieltune/poke";

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_sig: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

fn install_signal_handlers() {
    unsafe {
        libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
    }
}

/// Which auto controller owns the GPU when NO manual pin (`force_mhz`) is set.
/// Dispatched by `gpu apply-boot` (the single aputune-gpu.service ExecStart) —
/// mode changes edit power.json and restart that one unit; there is no
/// per-mode boot unit to enable/disable (that drift class killed a heat-pin).
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize, Default)]
pub enum AutoMode {
    /// Activity-driven fence-rate governor (the default auto mode).
    #[default]
    Governor,
    /// Poke-driven autosleep daemon (force top while poked, release when idle).
    Autosleep,
    /// No auto controller: clock released to native DPM (BAPM).
    Released,
}

/// GPU power config: the autosleep setpoints, plus an optional persistent MANUAL
/// clock. When `force_mhz` is set the GPU is pinned there across reboots and
/// the auto controllers step aside (manual mode); when None, `auto_mode` picks
/// the controller (governor / autosleep / released).
#[derive(Clone, Serialize, Deserialize)]
pub struct PowerConfig {
    /// Active clock (MHz) — forced under load. This is your thermal cap.
    pub top_mhz: u32,
    /// Deep-sleep clock (MHz) — forced when idle (350 = ~36 W vs ~56 W idle).
    pub deep_mhz: u32,
    /// Persistent manual clock (MHz). Some = pin here across reboots (manual
    /// mode, auto controllers off); None = auto (`auto_mode` governs).
    #[serde(default)]
    pub force_mhz: Option<u32>,
    /// Persistent forced GFX Vid (mV). Some = pin this GPU voltage across reboots
    /// (re-applied by `gpu apply-boot`); None = SMU-managed voltage.
    #[serde(default)]
    pub force_vid_mv: Option<u32>,
    /// The auto controller used when `force_mhz` is None. Defaults to Governor
    /// so a pre-existing power.json (no field) keeps today's behavior.
    #[serde(default)]
    pub auto_mode: AutoMode,
}

impl Default for PowerConfig {
    fn default() -> Self {
        PowerConfig {
            top_mhz: 2230,
            deep_mhz: 350,
            force_mhz: None,
            force_vid_mv: None,
            auto_mode: AutoMode::Governor,
        }
    }
}

/// Atomically replace `path` with `contents`: write `<path>.tmp` then rename
/// (atomic on the same filesystem), so a crash mid-write can never leave a
/// truncated/corrupt config behind.
pub fn write_atomic(path: &str, contents: &str) -> Result<()> {
    let tmp = format!("{path}.tmp");
    std::fs::write(&tmp, contents).with_context(|| format!("write {tmp}"))?;
    std::fs::rename(&tmp, path).with_context(|| format!("rename {tmp} -> {path}"))?;
    Ok(())
}

/// Best-effort exclusive lock serializing power.json read-modify-write cycles
/// across concurrent aputune processes (CLI + TUI + boot unit). Held for the
/// guard's lifetime. Never hard-fails: if /run is unavailable the caller
/// proceeds unlocked (same behavior as before the lock existed).
pub struct ConfigLock {
    _f: Option<std::fs::File>,
}

impl ConfigLock {
    pub fn acquire() -> Self {
        use std::os::fd::AsRawFd;
        let _ = std::fs::create_dir_all(RUN_DIR);
        let f = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open("/run/arieltune/power.lock")
            .ok();
        if let Some(ref f) = f {
            unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) };
        }
        ConfigLock { _f: f }
    }
}

impl Drop for ConfigLock {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        if let Some(ref f) = self._f {
            unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_UN) };
        }
    }
}

/// power.json state as seen by the BOOT path, distinguishing a missing file
/// (fresh install — defaults are correct) from a CORRUPT one (a config that
/// existed but doesn't parse — its contents, e.g. a 350 MHz heat-pin, are
/// unknown and must be treated fail-SAFE, never defaulted to governor-at-top).
pub enum BootLoad {
    Missing,
    Loaded(PowerConfig),
    Corrupt,
}

pub fn load_for_boot() -> BootLoad {
    let p = Path::new(CONFIG_PATH);
    if !p.exists() {
        return BootLoad::Missing;
    }
    match std::fs::read_to_string(p)
        .ok()
        .and_then(|t| serde_json::from_str::<PowerConfig>(&t).ok())
    {
        Some(c) => BootLoad::Loaded(c),
        None => BootLoad::Corrupt,
    }
}

/// What `gpu apply-boot` should enact.
#[derive(Debug, PartialEq)]
pub enum BootPlan {
    /// Pin the manual clock (heat-safety invariant: always wins over auto).
    Manual(u32),
    /// Dispatch on the auto controller.
    Auto(AutoMode),
    /// power.json existed but is CORRUPT: pin the deep/idle-safe LOW clock and
    /// skip auto entirely — the lost config may have been a heat-pin, and a
    /// default governor-at-top could cook the box.
    FailSafePin(u32),
}

/// Pure boot-mode decision (unit-testable): map the loaded power.json state to
/// the action apply-boot must take.
pub fn boot_plan(load: &BootLoad) -> BootPlan {
    match load {
        BootLoad::Missing => BootPlan::Auto(PowerConfig::default().auto_mode),
        BootLoad::Loaded(c) => match c.force_mhz {
            Some(m) => BootPlan::Manual(m),
            None => BootPlan::Auto(c.auto_mode),
        },
        BootLoad::Corrupt => BootPlan::FailSafePin(PowerConfig::default().deep_mhz),
    }
}

impl PowerConfig {
    pub fn load_or_default() -> Self {
        let p = Path::new(CONFIG_PATH);
        if p.exists() {
            if let Ok(txt) = std::fs::read_to_string(p) {
                if let Ok(c) = serde_json::from_str::<PowerConfig>(&txt) {
                    return c;
                }
            }
            eprintln!("arieltune apu: bad {CONFIG_PATH}; using defaults");
        }
        Self::default()
    }

    pub fn save(&self) -> Result<()> {
        std::fs::create_dir_all("/var/lib/aputune")
            .context("mkdir /var/lib/aputune (need root)")?;
        write_atomic(CONFIG_PATH, &serde_json::to_string_pretty(self)?)
            .with_context(|| format!("write {CONFIG_PATH}"))?;
        Ok(())
    }

    /// The highest clock the GPU can actually reach right now: the governor's
    /// top setpoint, or a manual forced clock if that's higher. GPU-voltage
    /// safety floors MUST key off this, not `top_mhz` alone — otherwise
    /// `gpu force 2230` (top still 1500) lets an undervolt sized for 1500 be
    /// applied while the die runs at 2230, which crashes.
    pub fn effective_top_mhz(&self) -> u32 {
        self.top_mhz.max(self.force_mhz.unwrap_or(0))
    }

    /// The highest clock reachable considering the GOVERNOR ladder too: the
    /// governor's high tier is written independently of `top_mhz` (a TUI tier
    /// edit), so voltage floors must key off max(top, force, governor high).
    pub fn effective_floor_clock(&self, gov: &GovernorConfig) -> u32 {
        self.effective_top_mhz().max(gov.high_mhz)
    }

    pub fn set_top(&mut self, mhz: u32) -> Result<()> {
        anyhow::ensure!(
            (smu::SCLK_MIN_MHZ..=smu::SCLK_MAX_MHZ).contains(&mhz),
            "top {mhz} outside {}-{} MHz",
            smu::SCLK_MIN_MHZ,
            smu::SCLK_MAX_MHZ
        );
        anyhow::ensure!(
            mhz >= self.deep_mhz,
            "top {mhz} below deep {}",
            self.deep_mhz
        );
        self.top_mhz = mhz;
        Ok(())
    }

    pub fn set_deep(&mut self, mhz: u32) -> Result<()> {
        anyhow::ensure!(
            (smu::SCLK_MIN_MHZ..=self.top_mhz).contains(&mhz),
            "deep {mhz} outside {}-{} MHz (must be <= top)",
            smu::SCLK_MIN_MHZ,
            self.top_mhz
        );
        self.deep_mhz = mhz;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// HEAT-SAFETY: a pre-refactor power.json (no auto_mode field) carrying a
    /// live manual heat-pin MUST load with the pin intact and auto_mode
    /// defaulting to Governor — apply-boot's manual branch then re-pins at boot.
    #[test]
    fn legacy_power_json_keeps_manual_pin() {
        let legacy = r#"{
            "top_mhz": 1500,
            "deep_mhz": 350,
            "force_mhz": 350,
            "force_vid_mv": null
        }"#;
        let c: PowerConfig = serde_json::from_str(legacy).unwrap();
        assert_eq!(c.force_mhz, Some(350));
        assert!(c.auto_mode == AutoMode::Governor);
        assert_eq!(c.effective_top_mhz(), 1500);
    }

    /// A truly ancient power.json (top/deep only) loads as auto/governor.
    #[test]
    fn legacy_power_json_defaults_auto() {
        let c: PowerConfig = serde_json::from_str(r#"{"top_mhz": 2230, "deep_mhz": 350}"#).unwrap();
        assert_eq!(c.force_mhz, None);
        assert!(c.auto_mode == AutoMode::Governor);
    }

    /// HEAT-SAFETY REGRESSION: a CORRUPT power.json (existed but unparseable —
    /// its contents may have been a heat-pin) must NEVER boot into the default
    /// governor-at-top. The fail-safe is a LOW-clock pin, no auto controller.
    #[test]
    fn corrupt_power_json_fails_safe_not_governor() {
        let plan = boot_plan(&BootLoad::Corrupt);
        assert_eq!(plan, BootPlan::FailSafePin(350));
        assert!(!matches!(plan, BootPlan::Auto(_)));
    }

    /// HEAT-SAFETY: a persisted manual pin always wins the boot plan; a missing
    /// file (fresh install) defaults to the governor.
    #[test]
    fn boot_plan_manual_pin_wins() {
        let cfg = PowerConfig {
            force_mhz: Some(350),
            ..Default::default()
        };
        assert_eq!(boot_plan(&BootLoad::Loaded(cfg)), BootPlan::Manual(350));
        assert_eq!(
            boot_plan(&BootLoad::Missing),
            BootPlan::Auto(AutoMode::Governor)
        );
    }

    /// An inverted tier ladder makes thermal demotion RAISE the clock — refuse.
    #[test]
    fn inverted_tier_ladder_rejected() {
        let mut g = GovernorConfig::default();
        assert!(g.validate_ladder().is_ok());
        g.idle_mhz = 2000; // idle above mid/high — demotion would upclock
        assert!(g.validate_ladder().is_err());
        let g2 = GovernorConfig {
            high_mhz: 300, // high below deep
            ..Default::default()
        };
        assert!(g2.validate_ladder().is_err());
    }
}

/// Touch the poke file — an app calls this on each unit of GPU work so
/// `autosleep` keeps the clock at `top`.
pub fn poke() -> Result<()> {
    std::fs::create_dir_all(RUN_DIR).ok();
    std::fs::write(POKE_PATH, b"").with_context(|| format!("write {POKE_PATH}"))?;
    Ok(())
}

/// Seconds since the last poke, or None if never poked.
fn poke_age_secs() -> Option<f64> {
    let m = std::fs::metadata(POKE_PATH).ok()?.modified().ok()?;
    m.elapsed().ok().map(|d| d.as_secs_f64())
}

/// Poke-driven auto-sleep: force the `top` clock while pokes are fresh, and
/// RELEASE to BAPM after `idle_secs` without a poke.
///
/// Idle deliberately UNFORCES rather than forcing the deep clock: a hard force
/// pins the clock and starves every GPU consumer that doesn't poke (model loads,
/// benchmarks, any other tool) — the observed wedge. Releasing to BAPM lets
/// the GPU idle low but ramp under any load. When poked (a served workload) it forces
/// `top` for a fast, thermally-capped clock. `--deep-force` opts back into the
/// aggressive force-`deep` idle (saves ~20 W but re-introduces the starvation, so
/// only for boxes where EVERY GPU user pokes). Edge-triggered; runs until
/// SIGTERM/SIGINT (then releases to BAPM).
pub fn autosleep(idle_secs: f64, deep_force: bool) -> Result<()> {
    install_signal_handlers();
    let smu = Smu::open()?;
    let cfg = PowerConfig::load_or_default();
    eprintln!(
        "arieltune apu autosleep: top {} MHz / idle {}, timeout {idle_secs:.0}s",
        cfg.top_mhz,
        if deep_force {
            format!("force {} MHz", cfg.deep_mhz)
        } else {
            "release to BAPM".into()
        }
    );

    // Apply the idle state to start.
    let go_idle = |smu: &Smu| -> Result<()> {
        if deep_force {
            smu.force_gfx_freq(cfg.deep_mhz).map(|_| ())
        } else {
            smu.unforce_gfx_freq()
        }
    };
    go_idle(&smu)?;
    let mut active = false;
    while !STOP.load(Ordering::SeqCst) {
        let fresh = poke_age_secs().map(|a| a < idle_secs).unwrap_or(false);
        if fresh != active {
            if fresh {
                smu.force_gfx_freq(cfg.top_mhz)?;
                eprintln!("arieltune apu autosleep: wake -> {} MHz", cfg.top_mhz);
            } else {
                go_idle(&smu)?;
                eprintln!(
                    "arieltune apu autosleep: idle -> {}",
                    if deep_force {
                        format!("{} MHz", cfg.deep_mhz)
                    } else {
                        "BAPM".into()
                    }
                );
            }
            active = fresh;
        }
        // 1s cadence; sleep in short slices so SIGTERM is responsive.
        let until = Instant::now() + Duration::from_secs(1);
        while Instant::now() < until && !STOP.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    let _ = smu.unforce_gfx_freq();
    eprintln!("arieltune apu autosleep: stopped (clock released to BAPM)");
    Ok(())
}

// ---- Activity-driven 3-state governor (+ deep-sleep) ----------------------
//
// The BC-250's native amdgpu DPM barely ramps (parks ~1500 MHz). This governor
// does DVFS "done right" using the ring FENCE-RATE as the load signal (idle
// ~10 f/s, LLM load ~1300 f/s on this box — a clean 100x range), forcing the
// clock via the kernel-serialized SMU send node (no mailbox race). It climbs
// immediately and descends after a short hold, and drops to a deep clock after
// a long idle — waking automatically when fences resume, so no manual wake.

/// Governor clock tier.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GovState {
    Deep,
    Idle,
    Mid,
    High,
}

/// Config for the GPU governor. Any real GPU work jumps straight to the HIGH
/// (top) clock — you never have to pin. The mid/idle/deep tiers are the
/// wind-DOWN ladder used only when the GPU goes quiet.
#[derive(Clone, Serialize, Deserialize)]
pub struct GovernorConfig {
    pub deep_mhz: u32,
    pub idle_mhz: u32,
    pub mid_mhz: u32,
    pub high_mhz: u32,
    /// Fence-rate (fences/sec) above which the GPU counts as "working" and gets
    /// the top clock. Just above idle noise (~10 f/s) so any real work triggers it.
    pub work_rate: f64,
    /// Stay at top this long after work stops, before winding down (anti-flap).
    pub hold_s: u64,
    /// Drop to the idle tier after this much quiet (mid tier covers hold..this).
    pub idle_after_s: u64,
    /// Drop to deep after this much quiet.
    pub deep_after_s: u64,
}

impl Default for GovernorConfig {
    fn default() -> Self {
        // Calibrated on the BC-250: idle averages ~10 f/s but BURSTS to ~32 f/s
        // (amdgpu housekeeping / resident-model keep-alive); real inference is
        // ~775-1300 f/s. work_rate sits well above the idle bursts so the box
        // actually winds down, and well below load so any real work hits high.
        GovernorConfig {
            deep_mhz: 350,
            idle_mhz: 1000,
            mid_mhz: 1250,
            high_mhz: 1500,
            work_rate: 100.0,
            hold_s: 5,
            idle_after_s: 30,
            deep_after_s: 120,
        }
    }
}

pub const GOVERNOR_PATH: &str = "/var/lib/aputune/governor.json";

// Governor thermal + anti-wedge tuning. The temp cap is enforced by COARSE tier
// demotion (high -> idle), never by continuously re-forcing the clock: repeated
// force_gfx_freq under load wedges gfx1013 (verified — 5 hard resets). A minimum
// dwell between clock changes bounds the forcing rate.
const THERM_HYST_C: f64 = 4.0; // degrees under cap before high is allowed again
const MIN_DWELL_S: u64 = 2; // at most one clock change per this many seconds

impl GovernorConfig {
    pub fn load_or_default() -> Self {
        std::fs::read_to_string(GOVERNOR_PATH)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }
    pub fn save(&self) -> Result<()> {
        std::fs::create_dir_all("/var/lib/aputune")
            .context("mkdir /var/lib/aputune (need root)")?;
        write_atomic(GOVERNOR_PATH, &serde_json::to_string_pretty(self)?)
            .with_context(|| format!("write {GOVERNOR_PATH}"))?;
        Ok(())
    }
    /// The tier ladder must be ordered deep <= idle <= mid <= high: an inverted
    /// ladder makes a thermal DEMOTION (high -> idle) RAISE the clock, which
    /// defeats the temp cap. Every tier setter validates through this.
    pub fn validate_ladder(&self) -> Result<()> {
        anyhow::ensure!(
            self.deep_mhz <= self.idle_mhz
                && self.idle_mhz <= self.mid_mhz
                && self.mid_mhz <= self.high_mhz,
            "tier ladder inverted: need deep <= idle <= mid <= high \
             (got deep {} / idle {} / mid {} / high {})",
            self.deep_mhz,
            self.idle_mhz,
            self.mid_mhz,
            self.high_mhz
        );
        anyhow::ensure!(
            (smu::SCLK_MIN_MHZ..=smu::SCLK_MAX_MHZ).contains(&self.deep_mhz)
                && self.high_mhz <= smu::SCLK_MAX_MHZ,
            "tier outside {}-{} MHz",
            smu::SCLK_MIN_MHZ,
            smu::SCLK_MAX_MHZ
        );
        Ok(())
    }
    pub fn clock(&self, st: GovState) -> u32 {
        match st {
            GovState::Deep => self.deep_mhz,
            GovState::Idle => self.idle_mhz,
            GovState::Mid => self.mid_mhz,
            GovState::High => self.high_mhz,
        }
    }
}

/// Run the activity-driven governor. Blocks until SIGTERM/SIGINT (then releases
/// the clock to BAPM). SMU force via the kernel-serialized send node.
pub fn governor(cfg: GovernorConfig) -> Result<()> {
    // M4: a hand-edited/legacy governor.json can carry an inverted ladder
    // (e.g. idle_mhz > high_mhz), which turns the thermal-cap demotion
    // (High -> Idle) into an UPCLOCK under heat and defeats the cap. Refuse to
    // run inverted: fall back to the known-good default ladder.
    let cfg = match cfg.validate_ladder() {
        Ok(()) => cfg,
        Err(e) => {
            eprintln!(
                "arieltune apu governor: [fail] tier ladder invalid ({e}); \
                 falling back to the default ladder"
            );
            GovernorConfig::default()
        }
    };
    install_signal_handlers();
    let smu = Smu::open()?;
    let dbg = ariel_hal::amdgpu_dbg_dir()
        .context("amdgpu debugfs dir not found (need root + a liberated kernel)")?;
    eprintln!(
        "arieltune apu governor: high {} mid {} idle {} deep {} MHz | work>={:.0} f/s | hold {}s mid<{}s deep>{}s | temp-cap demote (dwell {}s)",
        cfg.high_mhz, cfg.mid_mhz, cfg.idle_mhz, cfg.deep_mhz,
        cfg.work_rate, cfg.hold_s, cfg.idle_after_s, cfg.deep_after_s, MIN_DWELL_S
    );
    let mut prev = crate::telemetry::fence_sum(&dbg).unwrap_or(0);
    let mut prev_t = Instant::now();
    let mut cur = GovState::Idle;
    smu.force_gfx_freq(cfg.clock(cur))?;
    let mut last_active = Instant::now();
    let mut last_change = Instant::now();
    // Latching thermal state: once the die reaches the cap we demote high->idle
    // and stay demoted until it cools past cap - hysteresis. One force per
    // transition, never continuous stepping.
    let mut thermal_block = false;
    while !STOP.load(Ordering::SeqCst) {
        let until = Instant::now() + Duration::from_millis(500);
        while Instant::now() < until && !STOP.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(100));
        }
        if STOP.load(Ordering::SeqCst) {
            break;
        }
        let now = crate::telemetry::fence_sum(&dbg).unwrap_or(prev);
        let dt = prev_t.elapsed().as_secs_f64().max(0.001);
        let rate = now.saturating_sub(prev) as f64 / dt;
        prev = now;
        prev_t = Instant::now();
        // Load tier: work -> high; when quiet, wind down high -> mid -> idle -> deep.
        let mut target = if rate >= cfg.work_rate {
            last_active = Instant::now();
            GovState::High
        } else {
            let quiet = last_active.elapsed().as_secs();
            if quiet < cfg.hold_s {
                GovState::High
            } else if quiet < cfg.idle_after_s {
                GovState::Mid
            } else if quiet < cfg.deep_after_s {
                GovState::Idle
            } else {
                GovState::Deep
            }
        };
        // Temp cap: latching demotion off the real gfx-die temp. The cap comes
        // from cpu.json (re-read each tick so a TUI edit applies live); the
        // hardware SMU cap stays a high backstop.
        let cap = crate::cpu::CpuOc::load_or_default().gpu_temp_c;
        if cap > 0 && cap < ariel_smu::ocq3::TEMP_MAX_C {
            if let Some(t) = crate::telemetry::gfx_temp_c() {
                let capf = cap as f64;
                if t >= capf {
                    thermal_block = true;
                } else if t <= capf - THERM_HYST_C {
                    thermal_block = false;
                }
            }
        } else {
            thermal_block = false;
        }
        // Shed heat by dropping the top tier to idle while blocked.
        if thermal_block && target == GovState::High {
            target = GovState::Idle;
        }
        // At most one clock change per MIN_DWELL_S — bounding the force rate is
        // what keeps the SMU from wedging under load.
        if target != cur && last_change.elapsed() >= Duration::from_secs(MIN_DWELL_S) {
            let clk = cfg.clock(target);
            // Only record the transition when the SMU write actually landed: a
            // swallowed error (e.g. a transient debugfs failure on a thermal
            // demotion) must NOT be booked as done, or the governor would sit
            // at the hot tier believing it demoted. On Err, `cur` is left
            // unchanged so the next tick retries.
            match smu.force_gfx_freq(clk) {
                Ok(_) => {
                    eprintln!(
                        "arieltune apu governor: {cur:?} -> {target:?} {clk} MHz ({rate:.0} f/s{})",
                        if thermal_block {
                            format!(", temp-capped {cap} C")
                        } else {
                            String::new()
                        }
                    );
                    cur = target;
                    last_change = Instant::now();
                }
                Err(e) => {
                    eprintln!(
                        "arieltune apu governor: {cur:?} -> {target:?} force FAILED ({e}); retrying"
                    );
                }
            }
        }
    }
    let _ = smu.unforce_gfx_freq();
    eprintln!("arieltune apu governor: stopped (released to BAPM)");
    Ok(())
}
