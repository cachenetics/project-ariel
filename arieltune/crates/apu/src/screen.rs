// SPDX-License-Identifier: GPL-2.0-only
//! Interactive control-center TUI — one screen, styled to match memtune/biostune.
//!
//! A single dashboard with FOUR panels: the system card (fan control), CPU
//! overclock, GPU clock, and CU routing. [tab]/[shift-tab] move input focus
//! across the panels (bare 1-4 are the suite's tab-switch shortcut, not panel
//! jumps); the focused panel's border lights up and takes the arrow/adjust/apply
//! keys. The everyday BC-250 owner drives the
//! whole card from here instead of UMR/bash (CU routing) and python (CPU OC).
//! Silicon writes are draft-then-apply with a status line.

use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::Result;
use arieltune_tui_kit::{Outcome, Screen};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Flex;
use ratatui::prelude::*;
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use crate::cpu::CpuOc;
use crate::detect::{self, State};
use crate::{cpu, cu, curoute, cutest, dpm, patches, telemetry};
use ariel_smu::ocq3::{self, OcQ3};
use ariel_smu::smu::{self, Smu};

// Palette — shared with memtune/biostune.
const ACCENT: Color = Color::Cyan;
const GOOD: Color = Color::Green;
const WARN: Color = Color::Yellow;
const BAD: Color = Color::Red;
const DIM: Color = Color::DarkGray;
const KEY: Color = Color::Magenta;

/// A panel border that lights up (accent, bold title) when it holds input focus.
fn panel(title: &str, focused: bool) -> Block<'_> {
    let border = if focused { ACCENT } else { DIM };
    let mark = if focused {
        format!(" ▸ {title} ")
    } else {
        format!(" {title} ")
    };
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border))
        .title(Span::styled(
            mark,
            Style::default()
                .fg(if focused { ACCENT } else { DIM })
                .add_modifier(Modifier::BOLD),
        ))
}

fn key_line(items: &[(&str, &str)]) -> Line<'static> {
    let mut spans: Vec<Span> = Vec::new();
    for (i, (key, label)) in items.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(
            (*key).to_string(),
            Style::default().fg(KEY).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(format!(" {label}"), Style::default().fg(DIM)));
    }
    Line::from(spans)
}

/// Which panel has input focus on the single-screen dashboard.
#[derive(Clone, Copy, PartialEq)]
enum Focus {
    Fan,
    Cu,
    Cpu,
    Gpu,
}
const FOCUS_ORDER: [Focus; 4] = [Focus::Fan, Focus::Cpu, Focus::Gpu, Focus::Cu];

/// The carrier fan channel that drives the main BC-250 cooler (the Pump-Fan
/// header — pwm2/fan2 in sysfs). Verified: writing its duty moves the cooler.
const FAN_CHANNEL: u8 = 2;

struct Snapshot {
    is_bc250: bool,
    kernel: String,
    fully: bool,
    present: usize,
    total: usize,
    gfxclk: Option<u32>,
    /// Hottest amdgpu hwmon temp (the filtered EDGE sensor) — the system card's
    /// GPU cell. Gathered once per tick so draw() does no fs work.
    temp: Option<f64>,
    top_set: u32,
    force_mhz: Option<u32>,
    governor_on: bool,
    /// Is the one GPU power unit actually active? Cross-checked against
    /// power.json so the mode row can't claim "governor" while nothing runs.
    gpu_unit_active: bool,
    gpu_unit_installed: bool,
    /// Governor tier setpoints (governor.json) — read here, not in draw.
    gov: dpm::GovernorConfig,
    /// Carrier-board sensors (nct6686 hwmon) — read once per tick.
    carrier: telemetry::Carrier,
    /// Carrier fan: whether the writable driver (nct6687) is bound, the current
    /// pwm_enable mode (1 manual / 2 auto), measured duty %, and RPM.
    fan_writable: bool,
    fan_enable: Option<u8>,
    /// Overdrive-set GFX voltage (mV) at the high-clock point (pp_od_clk_voltage
    /// OD_VDDC), and the high-clock sclk (MHz) it applies to. The editable "gpu
    /// volt" setpoint — distinct from the live vddgfx meter.
    od_vddc: Option<u32>,
    od_sclk: Option<u32>,
    /// OD_RANGE VDDC (min, max) mV — scales the vdd gauge without a per-frame
    /// sysfs read.
    od_range: (u32, u32),
}

/// Cheap data — sysfs/debugfs/DRM ioctl (+ one systemctl is-active), refreshed
/// every second. draw() renders exclusively from this snapshot.
fn gather() -> Snapshot {
    let rep = detect::report();
    let present = rep
        .rows
        .iter()
        .filter(|r| matches!(r.state, State::Present | State::Inferred))
        .count();
    let cfg = dpm::PowerConfig::load_or_default();
    let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let od = telemetry::od_point();

    Snapshot {
        is_bc250: rep.is_bc250,
        kernel,
        fully: rep.fully_patched(),
        present,
        total: patches::count(),
        gfxclk: telemetry::current_sclk_mhz(),
        temp: telemetry::junction_temp_c(),
        top_set: cfg.top_mhz,
        force_mhz: cfg.force_mhz,
        // Mode comes from power.json (the single source the one GPU unit
        // dispatches on), not from which systemd unit happens to be enabled.
        governor_on: cfg.force_mhz.is_none() && cfg.auto_mode == dpm::AutoMode::Governor,
        gpu_unit_active: crate::persist::gpu_unit_active(),
        gpu_unit_installed: crate::persist::gpu_unit_installed(),
        gov: dpm::GovernorConfig::load_or_default(),
        carrier: telemetry::carrier(),
        fan_writable: telemetry::fan_writable(),
        fan_enable: telemetry::fan_enable(FAN_CHANNEL),
        od_vddc: od.map(|(_, v)| v),
        od_sclk: od.map(|(s, _)| s),
        od_range: telemetry::od_vddc_range().unwrap_or((700, 1129)),
    }
}

/// SMU telemetry node — refreshed only while the GPU panel holds focus, to
/// avoid contending amdgpu's shared MP1 mailbox from the render loop.
fn telemetry_lines() -> Vec<String> {
    Smu::open()
        .ok()
        .and_then(|s| s.telemetry())
        .map(|t| {
            t.lines()
                .filter(|l| !l.starts_with("==="))
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

enum Edit {
    None,
    Value(u32),
}

/// The [p] patch-detail modal: scroll offset plus the per-patch live states
/// captured once when it opened (detect::report() probes debugfs/sysfs, and
/// draw() must stay fs-read-free).
struct PatchPopup {
    scroll: u16,
    states: Vec<State>,
}

/// M5: result of the health-test worker — the full-40 KAT plus, only when that
/// faulted, the subtractive localize sweep. Computed off the UI thread.
struct HealthOutcome {
    full: cutest::KatResult,
    sweep: Option<Vec<cutest::KatResult>>,
}

pub struct ApuScreen {
    focus: Focus,
    snap: Snapshot,
    last: Instant,
    status: String,
    edit: Edit,
    /// True once the user has started changing the edited value (arrow or digit),
    /// so the first typed digit replaces the shown value instead of appending.
    edit_typed: bool,
    gpu_sel: usize,
    gpu_force: u32,
    cpu: CpuOc,
    cpu_sel: usize,
    cpu_live: Option<cpu::CpuLive>,
    cpu_live_last: Instant,
    oc: Option<OcQ3>,
    cu_live: [u32; 4],
    cu_draft: [u32; 4],
    cu_driver: Option<[u32; 4]>,
    cu_sel: usize,
    cu_ok: bool,
    /// True once the user has edited the CU draft (space/f/t) and not yet applied
    /// or reverted. While set, the draft is NOT resynced from live, so an external
    /// (CLI) route change can't clobber an in-progress edit. When clear, the draft
    /// follows live so CLI route changes show up cleanly (no false "pending").
    cu_dirty: bool,
    /// A health-test was requested this frame (set by [`cu_key`]). The deferral to
    /// `cu_test_armed` on the next tick draws the "testing…" status BEFORE the KAT
    /// is dispatched -- aputune's pre-draw-then-run idiom, kept in the Screen model
    /// where the pane can't force its own redraw.
    cu_test: bool,
    /// The health-test is armed to spawn on this tick (its status was drawn last
    /// frame). See [`ApuScreen::tick_refresh`].
    cu_test_armed: bool,
    /// M5: the KAT (test_full + subtractive localize) runs on a WORKER thread so
    /// the multi-second GPU sweep never freezes the UI. Polled in `tick_refresh`.
    cu_health_job: Option<JoinHandle<Result<HealthOutcome>>>,
    cu_health: Vec<(String, bool)>,
    /// Pending on-the-fly bench trigger + its last result: (routed CU, effective
    /// CU, GFLOPS, correct). Benches the current draft so you can measure a shape
    /// before committing it.
    cu_bench: bool,
    /// The bench is armed to spawn on this tick (its status was drawn last frame).
    cu_bench_armed: bool,
    /// M5: the on-the-fly bench runs on a WORKER thread (see `cu_health_job`).
    cu_bench_job: Option<JoinHandle<Result<cutest::KatResult>>>,
    /// The draft shape the in-flight bench worker is measuring (for shape() at
    /// completion, since the live draft may drift while the worker runs).
    cu_bench_draft: [u32; 4],
    cu_bench_result: Option<(u32, u32, f64, bool)>,
    masks_last: Instant,
    tele: Vec<String>,
    tele_last: Instant,
    gpu_vid: Option<u32>,
    gpu_power_w: Option<u32>,
    cpu_temp: Option<f64>,
    cpu_util: Vec<u8>,
    cpu_prev: Vec<(u64, u64)>,
    /// Fan target the user is dialing in the system card: `None` = the EC
    /// automatic curve, `Some(pct)` = a manual duty. Committed to the EC on
    /// Enter. Dialing down past the minimum manual duty lands on Auto.
    fan_target: Option<u8>,
    /// Two-step guard for GPU `[u]` (release-to-auto): abandoning a manual clock
    /// pin (e.g. a heat-safety 350 MHz pin) needs a second `[u]` to confirm, so a
    /// single stray keystroke can't silently drop the pin.
    armed_unforce: bool,
    /// The [p] liberation patch-detail popup; `None` = closed. While open it
    /// owns all keys (modal).
    patch_popup: Option<PatchPopup>,
}

/// SoC package power (W) from the amdgpu hwmon, if exposed. Filtered to the
/// amdgpu sensor specifically — the box also has nvme/k10temp hwmons, and
/// grabbing the first one with a power node could read the wrong rail.
fn gpu_power_w() -> Option<u32> {
    for e in std::fs::read_dir("/sys/class/hwmon").ok()?.flatten() {
        let dir = e.path();
        if std::fs::read_to_string(dir.join("name"))
            .unwrap_or_default()
            .trim()
            != "amdgpu"
        {
            continue;
        }
        if let Ok(s) = std::fs::read_to_string(dir.join("power1_average")) {
            if let Ok(uw) = s.trim().parse::<u64>() {
                return Some((uw / 1_000_000) as u32);
            }
        }
    }
    None
}

/// Live CPU package temperature (k10temp Tctl), in °C. Read from the k10temp
/// hwmon specifically so it isn't confused with the amdgpu (GPU) sensor.
fn cpu_temp_c() -> Option<f64> {
    for e in std::fs::read_dir("/sys/class/hwmon").ok()?.flatten() {
        let dir = e.path();
        if std::fs::read_to_string(dir.join("name"))
            .unwrap_or_default()
            .trim()
            != "k10temp"
        {
            continue;
        }
        if let Ok(s) = std::fs::read_to_string(dir.join("temp1_input")) {
            if let Ok(milli) = s.trim().parse::<f64>() {
                return Some(milli / 1000.0);
            }
        }
    }
    None
}

/// Shared temperature colour: green normal, yellow warm, red hot.
fn temp_color(t: f64) -> Color {
    if t >= 90.0 {
        BAD
    } else if t >= 75.0 {
        WARN
    } else {
        GOOD
    }
}

/// Per-logical-CPU (idle, total) jiffies from /proc/stat (cpu0..cpuN in order).
fn read_cpu_jiffies() -> Vec<(u64, u64)> {
    let mut out = Vec::new();
    if let Ok(s) = std::fs::read_to_string("/proc/stat") {
        for line in s.lines() {
            let b = line.as_bytes();
            if b.len() > 3 && &b[0..3] == b"cpu" && b[3].is_ascii_digit() {
                let v: Vec<u64> = line
                    .split_whitespace()
                    .skip(1)
                    .filter_map(|x| x.parse().ok())
                    .collect();
                if v.len() >= 4 {
                    let idle = v[3] + v.get(4).copied().unwrap_or(0); // idle + iowait
                    let total: u64 = v.iter().sum();
                    out.push((idle, total));
                }
            }
        }
    }
    out
}

/// Per-CPU utilization % from two /proc/stat snapshots of equal length.
fn cpu_util_pct(prev: &[(u64, u64)], now: &[(u64, u64)]) -> Vec<u8> {
    now.iter()
        .zip(prev.iter())
        .map(|((i1, t1), (i0, t0))| {
            let dt = t1.saturating_sub(*t0);
            let di = i1.saturating_sub(*i0);
            if dt == 0 {
                0
            } else {
                (((dt - di) as f64 / dt as f64) * 100.0)
                    .round()
                    .clamp(0.0, 100.0) as u8
            }
        })
        .collect()
}

fn refresh_masks(app: &mut ApuScreen) {
    match curoute::current_masks() {
        Ok(m) => {
            app.cu_live = m;
            app.cu_ok = true;
        }
        Err(_) => app.cu_ok = false,
    }
    // Boot enumeration (driver topology) — static post-boot, but harmless to
    // re-read; lets the view light up once amdgpu is reachable.
    app.cu_driver = cu::driver_wgp_masks();
    app.masks_last = Instant::now();
}

impl ApuScreen {
    /// Build the APU screen. Cheap only: NO SMU access here (design R5 — a
    /// backgrounded tab must not touch the SMU). The queue-3 handle is opened in
    /// [`Screen::on_enter`] and dropped in [`Screen::on_exit`]; `snap` seeds from
    /// sysfs/debugfs (mailbox-free) so the first frame is populated.
    pub fn new() -> Self {
        let now = Instant::now();
        ApuScreen {
            focus: Focus::Fan,
            gpu_sel: 0,
            gpu_force: 0, // seeded on focus from the snapshot (pin or top setpoint)
            cpu: CpuOc::load_or_default(),
            cpu_sel: 0,
            cpu_live: None,
            cpu_live_last: now,
            // No SMU access until focused (on_enter opens the mailbox).
            oc: None,
            cu_live: [curoute::FULL_MASK; 4],
            cu_draft: [curoute::FULL_MASK; 4],
            cu_driver: None,
            cu_sel: 0,
            cu_ok: false,
            cu_dirty: false,
            cu_test: false,
            cu_test_armed: false,
            cu_health_job: None,
            cu_health: Vec::new(),
            cu_bench: false,
            cu_bench_armed: false,
            cu_bench_job: None,
            cu_bench_draft: [curoute::FULL_MASK; 4],
            cu_bench_result: None,
            masks_last: now,
            edit: Edit::None,
            edit_typed: false,
            status: "ready".into(),
            last: now,
            snap: gather(),
            tele: Vec::new(),
            tele_last: now,
            gpu_vid: None,
            gpu_power_w: None,
            cpu_temp: None,
            cpu_util: Vec::new(),
            cpu_prev: read_cpu_jiffies(),
            // The board boots on the EC automatic curve, so the dial starts on Auto.
            fan_target: None,
            armed_unforce: false,
            patch_popup: None,
        }
    }

    /// Focus gained: START live polling — open the SMU queue-3 mailbox and seed
    /// every live gauge so the first frame is not blank. This is the ONLY place
    /// SMU access begins (design R5).
    fn enter(&mut self) {
        // Ensure the carrier-board sensors are available so the system section can
        // show temps/fan AND drive the fan: loads the writable `nct6687 force=true`
        // driver (and persists it) if the nct6686 hwmon isn't present. Best-effort —
        // needs root, silent no-op otherwise.
        telemetry::ensure_carrier_sensors();
        self.oc = OcQ3::open_checked().ok();
        let now = Instant::now();
        refresh_masks(self);
        self.cu_draft = self.cu_live;
        self.cpu_live = self.oc.as_ref().map(cpu::live);
        self.gpu_vid = telemetry::vddgfx_mv();
        self.gpu_power_w = gpu_power_w();
        self.cpu_temp = cpu_temp_c();
        self.cpu_prev = read_cpu_jiffies();
        self.snap = gather();
        // Seed the force field from the REAL state (an active pin, else the top
        // setpoint) — never a hardcoded constant a stray double-Enter could apply.
        self.gpu_force = self.snap.force_mhz.unwrap_or(self.snap.top_set);
        self.last = now;
        self.tele_last = now;
        self.masks_last = now;
        self.cpu_live_last = now;
    }

    /// Focus lost: STOP live polling — drop the SMU queue-3 handle so a
    /// backgrounded APU tab does NO SMU access (design R5). `tick` is only called
    /// on the active screen, so dropping the handle is what makes "no background
    /// SMU" true.
    fn exit(&mut self) {
        self.oc = None;
        self.tele.clear();
    }

    /// Throttled background refresh (was the old blocking event loop's body, minus
    /// draw/poll/key). Also runs any ARMED blocking health-test/bench — armed on
    /// the key press, its "testing…/benching…" status has since been drawn (the
    /// shell draws before each tick), so running it here preserves aputune's
    /// pre-draw-then-run idiom without a nested draw.
    fn tick_refresh(&mut self) {
        // M5: collect any finished GPU worker first (non-blocking).
        self.poll_cu_jobs();
        // Spawn an action armed on a PREVIOUS tick (status already shown) onto a
        // WORKER thread, so the multi-second KAT/bench never freezes the UI. Only
        // one GPU worker at a time (they share the process-wide compute lock).
        let cu_busy = self.cu_worker_running();
        if self.cu_test_armed && !cu_busy {
            self.cu_test_armed = false;
            self.cu_health_job = Some(std::thread::spawn(|| -> Result<HealthOutcome> {
                // Full-40 KAT; only drill down with the subtractive localize sweep
                // when the full run faulted (matches the old inline logic).
                let full = cutest::test_full()?;
                let sweep = if full.ok {
                    None
                } else {
                    Some(cutest::test_localize(|_| {})?)
                };
                Ok(HealthOutcome { full, sweep })
            }));
            self.last = Instant::now();
        } else if self.cu_bench_armed && !cu_busy {
            self.cu_bench_armed = false;
            let draft = self.cu_draft;
            self.cu_bench_draft = draft;
            self.cu_bench_job = Some(std::thread::spawn(move || cutest::bench_masks(draft)));
            self.last = Instant::now();
        }
        // Arm an action requested by this frame's key press; the "testing…" status
        // it set is drawn next frame, and the tick after that spawns the worker.
        if self.cu_test {
            self.cu_test = false;
            self.cu_test_armed = true;
        }
        if self.cu_bench {
            self.cu_bench = false;
            self.cu_bench_armed = true;
        }
        // One screen — every panel is always visible, so refresh them all
        // (throttled independently). The SMU/umr reads are cheap and gated on
        // the handles being present.
        if self.last.elapsed() >= Duration::from_secs(1) {
            self.snap = gather();
            // Reflect external edits to cpu.json (CLI `arieltune apu cpu set`,
            // another session) live — but never while the user is mid-edit, or it
            // would clobber what they're typing. Governor tiers re-read on draw.
            if matches!(self.edit, Edit::None) {
                self.cpu = CpuOc::load_or_default();
            }
            let now = read_cpu_jiffies();
            if now.len() == self.cpu_prev.len() && !self.cpu_prev.is_empty() {
                self.cpu_util = cpu_util_pct(&self.cpu_prev, &now);
            }
            self.cpu_prev = now;
            self.last = Instant::now();
        }
        if self.tele_last.elapsed() >= Duration::from_secs(3) {
            // The SMU telemetry node read (Smu::open().telemetry()) touches the
            // shared MP1 mailbox. Only poll it while the GPU panel holds focus (its
            // only consumer) AND no CU worker is running: a health-test/bench
            // worker drives compute on the ONE shared device and its ClockGuard
            // already owns the MP1 mailbox (force_gfx_freq). A concurrent mailbox
            // read from this UI thread races that worker and can hard-wedge
            // gfx1013 (the gpu_metrics-during-compute race). The MEM screen guards
            // its gpu_metrics read the same way (skip while Bench::Running). When
            // skipped we keep the last-known telemetry lines. NOTE: gather() +
            // vddgfx_mv/gpu_power_w/cpu_temp below are PLAIN sysfs/hwmon reads
            // (pp_dpm_sclk, amdgpu in0_input, power1_average, k10temp) that do NOT
            // touch the MP1 mailbox, so they always refresh (verified in
            // telemetry.rs: only telemetry_lines opens the SMU).
            if self.focus == Focus::Gpu && !self.cu_worker_running() {
                self.tele = telemetry_lines();
            }
            self.gpu_vid = telemetry::vddgfx_mv();
            self.gpu_power_w = gpu_power_w();
            self.cpu_temp = cpu_temp_c();
            self.tele_last = Instant::now();
        }
        // Skip the umr routing read while a CU worker is running: test_localize /
        // bench_masks are actively REPROGRAMMING the routing masks (curoute::apply)
        // on the worker thread, so a concurrent current_masks() read would race the
        // umr writes and, worse, resync the draft to a TRANSIENT sweep shape rather
        // than the user's real route. poll_cu_jobs()/apply_bench/apply_health
        // refresh_masks() once the worker completes, so the view resyncs cleanly
        // then. (current_masks is a umr register read; gating it also removes any
        // umr read/write contention during the sweep.)
        if self.masks_last.elapsed() >= Duration::from_secs(2) && !self.cu_worker_running() {
            refresh_masks(self);
            // Reflect external (CLI) route changes: while the user has no pending
            // edit, keep the draft tracking live so a CLI route shows cleanly and
            // isn't mistaken for a pending edit. A live edit (cu_dirty) is held.
            if !self.cu_dirty && self.cu_ok {
                self.cu_draft = self.cu_live;
            }
            self.masks_last = Instant::now();
        }
        if self.cpu_live_last.elapsed() >= Duration::from_secs(1) {
            self.cpu_live = self.oc.as_ref().map(cpu::live);
            self.cpu_live_last = Instant::now();
        }
    }

    /// True while a CU health-test or bench WORKER thread is live. Such a worker
    /// holds the process-wide compute lock and drives the GPU (its ClockGuard owns
    /// the SMU MP1 mailbox + it reprograms routing), so `tick_refresh` MUST NOT do
    /// any SMU-mailbox read or umr routing read concurrently — either can race the
    /// worker and hard-wedge gfx1013. Recomputed at each use site (a worker may be
    /// spawned mid-tick, after the top-of-tick `cu_busy` snapshot).
    fn cu_worker_running(&self) -> bool {
        self.cu_health_job.is_some() || self.cu_bench_job.is_some()
    }

    /// M5: harvest finished GPU workers without blocking the UI thread.
    fn poll_cu_jobs(&mut self) {
        if self.cu_health_job.as_ref().is_some_and(|h| h.is_finished()) {
            match self.cu_health_job.take().unwrap().join() {
                Ok(Ok(outcome)) => self.apply_health(outcome),
                Ok(Err(e)) => self.status = format!("health-test error: {e}"),
                Err(_) => self.status = "health-test worker panicked".into(),
            }
            self.snap = gather();
        }
        if self.cu_bench_job.as_ref().is_some_and(|h| h.is_finished()) {
            match self.cu_bench_job.take().unwrap().join() {
                Ok(Ok(r)) => self.apply_bench(r),
                Ok(Err(e)) => self.status = format!("bench error: {e}"),
                Err(_) => self.status = "bench worker panicked".into(),
            }
        }
    }

    /// Apply a completed health-test (formerly the tail of the blocking `run_health`).
    fn apply_health(&mut self, o: HealthOutcome) {
        self.cu_health.clear();
        let full = o.full;
        self.cu_health.push((
            format!(
                "40-CU: {}  {:.0} GFLOPS  {} mismatch",
                full.verdict(),
                full.gflops,
                full.mismatches
            ),
            full.ok,
        ));
        if full.ok {
            self.status = "all 40 CUs healthy".into();
            return;
        }
        // ok == true for a swept config => the fault cleared with that WGP removed
        // => it's the bad one. No sweep (full-40 passed) leaves the list as-is.
        if let Some(sweep) = o.sweep {
            let suspects: Vec<String> = sweep
                .iter()
                .filter(|h| h.ok)
                .map(|h| h.label.trim_start_matches('-').to_string())
                .collect();
            if suspects.is_empty() {
                self.cu_health
                    .push(("could not isolate a single WGP".into(), false));
                self.status = "fault present; WGP not isolated (multiple/intermittent)".into();
            } else {
                for s in &suspects {
                    self.cu_health.push((format!("bad WGP: {s}"), false));
                }
                self.status = format!("bad WGP: {} — route around it", suspects.join(", "));
            }
        }
    }

    /// Apply a completed bench (formerly the tail of the blocking `run_bench`).
    fn apply_bench(&mut self, r: cutest::KatResult) {
        let draft = self.cu_bench_draft;
        let s = curoute::shape(&draft);
        self.cu_bench_result = Some((s.cu, s.effective_cu, r.gflops, r.ok));
        self.status = format!(
            "bench: {} CU routed / {} effective — {:.0} GFLOPS ({})",
            s.cu,
            s.effective_cu,
            r.gflops,
            if r.hung {
                "HUNG"
            } else if r.ok {
                "correct"
            } else {
                "MISMATCH"
            }
        );
        // Benching applied the draft then restored live; keep the view in sync.
        refresh_masks(self);
        if !self.cu_dirty {
            self.cu_draft = self.cu_live;
        }
    }
}

impl Default for ApuScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl Screen for ApuScreen {
    fn title(&self) -> &'static str {
        "APU"
    }

    fn draw(&mut self, f: &mut Frame, area: Rect) {
        draw(f, area, self);
    }

    fn on_key(&mut self, key: KeyEvent) -> Outcome {
        handle_key(self, key)
    }

    fn tick(&mut self) {
        self.tick_refresh();
    }

    /// ~250 ms: keeps the live CPU/GPU gauges moving (aputune's cadence) while
    /// leaving keys instant (a keypress returns from the poll immediately).
    fn tick_hint(&self) -> Duration {
        Duration::from_millis(250)
    }

    fn on_enter(&mut self) {
        self.enter();
    }

    fn on_exit(&mut self) {
        self.exit();
    }

    /// The [p] patch popup and any in-progress field edit are modal sub-states:
    /// while in one the shell must NOT steal global switch/quit keys.
    fn modal(&self) -> bool {
        self.patch_popup.is_some() || matches!(self.edit, Edit::Value(_))
    }

    fn status_hint(&self) -> Option<String> {
        Some(self.status.clone())
    }
}

/// Pane-first key handling. The four-panel focus keys (arrows / Tab) stay
/// PANE-INTERNAL (Consumed). Bare `q` quits the suite (Outcome::Quit) when not
/// mid-edit/modal. Keys carrying Ctrl/Alt, and the function keys, are the shell's
/// global bindings (tab switch / quit) — the pane leaves them Ignored so the
/// shell sees them. Everything the pane acts on is Consumed.
fn handle_key(app: &mut ApuScreen, key: KeyEvent) -> Outcome {
    use crossterm::event::KeyModifiers;
    let code = key.code;
    // While modal (popup open OR mid-edit) the pane owns every key — never leak
    // to the shell's global bindings.
    let is_modal = app.patch_popup.is_some() || matches!(app.edit, Edit::Value(_));
    if !is_modal {
        // The shell's global keys reach it only via Ignored: modified keys
        // (Ctrl-Q quit, Ctrl-Tab cycle, Alt-1..4 jump), the function keys (F1-F4),
        // and bare 1-4 (the suite's tab-switch shortcut). Panels are reached via
        // bare Tab/BackTab; the pane uses only the remaining UNMODIFIED keys, so
        // hand anything modified / function-key / a bare tab-digit back.
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            || matches!(code, KeyCode::F(_))
            || matches!(code, KeyCode::Char('1'..='4'))
        {
            return Outcome::Ignored;
        }
        // Bare `q` (not editing) quits the whole suite.
        if matches!(app.edit, Edit::None) && code == KeyCode::Char('q') {
            return Outcome::Quit;
        }
    }
    dispatch_key(app, code);
    Outcome::Consumed
}

/// The former `handle_key` body: routes one (already-classified) key to the modal
/// popup, the global focus binds, and the focused panel. Every branch is
/// pane-internal (the caller returns Consumed).
fn dispatch_key(app: &mut ApuScreen, code: KeyCode) {
    // Modal patch popup: while open it owns EVERY key — nothing falls through
    // to the global or per-panel handlers, and [q] closes rather than quits.
    if app.patch_popup.is_some() {
        match code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('p') => app.patch_popup = None,
            _ => {
                if let Some(pop) = app.patch_popup.as_mut() {
                    let max = (patch_popup_lines(&pop.states).len() as u16).saturating_sub(1);
                    match code {
                        KeyCode::Up | KeyCode::Char('k') => {
                            pop.scroll = pop.scroll.saturating_sub(1)
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            pop.scroll = (pop.scroll + 1).min(max)
                        }
                        KeyCode::PageUp => pop.scroll = pop.scroll.saturating_sub(10),
                        KeyCode::PageDown => pop.scroll = (pop.scroll + 10).min(max),
                        KeyCode::Home | KeyCode::Char('g') => pop.scroll = 0,
                        _ => {}
                    }
                }
            }
        }
        return;
    }
    if matches!(app.edit, Edit::None) {
        match code {
            // Bare `q` is handled by the caller (quits the suite). Panel focus is
            // by Tab/BackTab (bare 1-4 are the suite's tab-switch, handed to the
            // shell above, so they no longer quick-jump panels).
            KeyCode::Tab | KeyCode::BackTab => {
                let i = FOCUS_ORDER
                    .iter()
                    .position(|f| *f == app.focus)
                    .unwrap_or(0);
                let n = FOCUS_ORDER.len();
                let step = if code == KeyCode::BackTab { n - 1 } else { 1 };
                app.focus = FOCUS_ORDER[(i + step) % n];
            }
            // The liberation "button" in the system card: open the patch-detail
            // popup. One detect probe at open; the popup renders from the
            // capture so draw() stays fs-read-free.
            KeyCode::Char('p') => {
                app.patch_popup = Some(PatchPopup {
                    scroll: 0,
                    states: detect::report().rows.iter().map(|r| r.state).collect(),
                });
                return;
            }
            _ => {}
        }
    }
    match app.focus {
        Focus::Gpu => gpu_key(app, code),
        Focus::Cpu => cpu_key(app, code),
        Focus::Cu => cu_key(app, code),
        Focus::Fan => fan_key(app, code),
    }
}

/// Minimum manual fan duty (percent). Dialing below this lands on Auto rather
/// than letting the user set a dangerously slow/stopped fan.
const FAN_MIN_PCT: u8 = 20;

/// Commit the current fan target to the EC and report it.
fn fan_commit(app: &mut ApuScreen) {
    let ok = telemetry::set_fan_duty(FAN_CHANNEL, app.fan_target);
    app.status = match (ok, app.fan_target) {
        (true, None) => "fan → auto (EC curve)".into(),
        (true, Some(p)) => format!("fan → manual {p}%"),
        (false, _) => "fan set failed (need root?)".into(),
    };
}

/// Fan control in the system card. Everything is in percent + Auto — no raw PWM
/// values. Arrows dial the target (Auto sits just below the minimum duty),
/// Enter commits it, and `[a]` jumps straight back to the EC automatic curve.
fn fan_key(app: &mut ApuScreen, code: KeyCode) {
    if !app.snap.fan_writable {
        app.status = "fan not writable — nct6687 driver not bound".into();
        return;
    }
    match code {
        KeyCode::Up | KeyCode::Right => {
            app.fan_target = Some(match app.fan_target {
                // First step off Auto: seed from the fan's CURRENT duty (max'd
                // with the floor), so Up-then-Enter can never cut a fan running
                // at e.g. 80% under load down to the 20% minimum.
                None => app
                    .snap
                    .carrier
                    .duty_pct
                    .map(|d| d.min(100) as u8)
                    .unwrap_or(FAN_MIN_PCT)
                    .max(FAN_MIN_PCT),
                Some(p) => (p + 5).min(100),
            });
        }
        KeyCode::Down | KeyCode::Left => {
            app.fan_target = match app.fan_target {
                None => None,
                Some(p) if p > FAN_MIN_PCT => Some(p - 5),
                Some(_) => None, // step below the floor → Auto
            };
        }
        KeyCode::Enter | KeyCode::Char('m') => fan_commit(app),
        KeyCode::Char('a') => {
            app.fan_target = None;
            fan_commit(app);
        }
        _ => {}
    }
}

fn gpu_key(app: &mut ApuScreen, code: KeyCode) {
    // Per-field inclusive range + ←/→ step: temp cap (sel 5) in C by 1, GPU Vid
    // (sel 6) in mV by 5, everything else a clock in MHz by 10.
    let (lo, hi, step) = match app.gpu_sel {
        5 => (60, ocq3::TEMP_MAX_C, 1u32),
        6 => {
            // GPU voltage: floor at the safe minimum for the current top clock
            // (can't undervolt into a crash), ceiling at the OD_RANGE VDDC max.
            let (_, hi) = app.snap.od_range;
            // Floor at the effective top clock (a manual force can exceed the
            // governor top), so you can't dial an undervolt that crashes.
            let clk = app
                .snap
                .top_set
                .max(app.snap.force_mhz.unwrap_or(0))
                .max(app.snap.gov.high_mhz);
            let lo = telemetry::min_gfx_vddc(clk);
            (lo, hi, 5)
        }
        _ => (smu::SCLK_MIN_MHZ, smu::SCLK_MAX_MHZ, 10),
    };
    // Any key other than a second [u] disarms the release-to-auto confirm.
    if !matches!(code, KeyCode::Char('u')) {
        app.armed_unforce = false;
    }
    match (&app.edit, code) {
        (Edit::None, KeyCode::Up) => app.gpu_sel = app.gpu_sel.saturating_sub(1),
        (Edit::None, KeyCode::Down) => app.gpu_sel = (app.gpu_sel + 1).min(6),
        (Edit::None, KeyCode::Enter) => {
            let g = &app.snap.gov;
            let cur = match app.gpu_sel {
                0 => g.high_mhz,
                1 => g.mid_mhz,
                2 => g.idle_mhz,
                3 => g.deep_mhz,
                // sel 4 = force: seed from the REAL pin (else the top setpoint),
                // never a constant — a bare Enter-Enter must not invent a clock.
                4 => app.snap.force_mhz.unwrap_or(app.snap.top_set),
                5 => app.cpu.gpu_temp_c,
                // sel 6 = GPU voltage (mV): start from the OD setpoint, else live.
                _ => app.snap.od_vddc.or(app.gpu_vid).unwrap_or(1000),
            };
            app.edit = Edit::Value(cur);
            app.edit_typed = false;
            app.status = "type a value or ←/→ to adjust · Enter apply · Esc cancel".into();
        }
        (Edit::None, KeyCode::Char('u')) => {
            // Abandoning a manual clock pin (e.g. a heat-safety 350 MHz pin) OR
            // a persisted voltage undervolt is a two-step confirm so a stray
            // keystroke can't silently drop either.
            let guarded = app.snap.force_mhz.is_some() || app.snap.od_vddc.is_some();
            if guarded && !app.armed_unforce {
                app.armed_unforce = true;
                app.status = match (app.snap.force_mhz, app.snap.od_vddc) {
                    (Some(m), _) => {
                        format!("manual pin at {m} MHz active — press [u] again to release to auto")
                    }
                    (None, Some(v)) => format!(
                        "GPU voltage set at {v} mV — press [u] again to release it to stock"
                    ),
                    (None, None) => unreachable!(),
                };
            } else {
                app.armed_unforce = false;
                act(app, "unforce", |s| {
                    // The shared unforce transition: release clock + voltage,
                    // clear the pin, PRESERVE the persisted auto mode.
                    let mode = crate::gpuctl::unforce()?;
                    *s = format!(
                        "auto mode — clock + voltage released, {} resumed",
                        crate::gpuctl::mode_name(mode)
                    );
                    Ok(())
                });
            }
        }
        (Edit::Value(v), KeyCode::Left) => {
            app.edit = Edit::Value(v.saturating_sub(step).max(lo));
            app.edit_typed = true;
        }
        (Edit::Value(v), KeyCode::Right) => {
            app.edit = Edit::Value((v + step).min(hi));
            app.edit_typed = true;
        }
        // Typable: digits build the number (first digit replaces the shown value),
        // Backspace deletes a digit. Clamp to the field max while typing; the min
        // is applied on Enter so partial values aren't jammed up mid-entry.
        (Edit::Value(v), KeyCode::Char(c)) if c.is_ascii_digit() => {
            let dgt = c.to_digit(10).unwrap();
            let nv = if app.edit_typed {
                v.saturating_mul(10).saturating_add(dgt)
            } else {
                dgt
            };
            app.edit = Edit::Value(nv.min(hi));
            app.edit_typed = true;
        }
        (Edit::Value(v), KeyCode::Backspace) => {
            app.edit = Edit::Value(v / 10);
            app.edit_typed = true;
        }
        (Edit::Value(v), KeyCode::Enter) => {
            let (v, sel) = ((*v).clamp(lo, hi), app.gpu_sel);
            // The force (4) and voltage (6) fields commit hardware changes whose
            // seed is only a best guess (top setpoint / fluctuating live mV) —
            // a bare Enter-Enter with nothing typed must be a no-op, not an
            // accidental pin/undervolt.
            if !app.edit_typed && matches!(sel, 4 | 6) {
                app.edit = Edit::None;
                app.status = "no change (nothing typed) — edit cancelled".into();
                return;
            }
            match sel {
                6 => {
                    // GPU voltage at the high-clock point, via amdgpu overdrive
                    // (pp_od_clk_voltage `vc 0 <sclk> <mV>`). amdgpu mediates the
                    // SMU write, so it's safe live under load — no governor
                    // coordination, no MP1 race. Persist so it re-applies at boot.
                    act(app, "gpu-volt", move |s| {
                        // Apply at the highest configured clock (top / force /
                        // governor high), never the live one, so setting voltage
                        // never pins the max clock low.
                        let _lock = dpm::ConfigLock::acquire();
                        let mut c = dpm::PowerConfig::load_or_default();
                        let clk = c.effective_floor_clock(&dpm::GovernorConfig::load_or_default());
                        if !telemetry::od_set_vddc(clk, v) {
                            *s = "voltage set failed (overdrive off / need root?)".into();
                            return Ok(());
                        }
                        c.force_vid_mv = Some(v);
                        c.save()?;
                        // Boot re-apply rides the single GPU power unit
                        // (apply-boot applies the saved voltage first); a live
                        // set needs no unit restart.
                        *s = format!(
                            "GPU voltage {v} mV (high clock){}",
                            if crate::persist::gpu_unit_installed() {
                                " — persisted"
                            } else {
                                " (live only until a mode command installs the unit)"
                            }
                        );
                        Ok(())
                    });
                }
                5 => {
                    // GPU temp cap via SMU queue-3; persist alongside the CPU OC.
                    app.cpu.gpu_temp_c = v;
                    let cfg = app.cpu.clone();
                    act(app, "gpu-temp", move |s| {
                        ocq3::OcQ3::open_checked()?.set_gpu_max_temp(v)?;
                        cfg.save()?;
                        crate::persist::enable_cpu_oc()?;
                        *s = format!("GPU temp cap {v} C — persisted");
                        Ok(())
                    });
                }
                4 => {
                    app.gpu_force = v;
                    act(app, "force", move |s| {
                        // The shared force transition: voltage floor raised
                        // BEFORE the clock (raise failure = clock untouched),
                        // persist, single-unit re-enact.
                        let out = crate::gpuctl::force(v)?;
                        *s = format!(
                            "forced {} MHz{}{}",
                            out.set_mhz,
                            match out.vid_raised {
                                Some((old, floor)) =>
                                    format!(" (undervolt raised {old} -> {floor} mV)"),
                                None => String::new(),
                            },
                            if out.held {
                                " — manual, sticks across reboots"
                            } else {
                                " (live only)"
                            }
                        );
                        Ok(())
                    });
                }
                tier => {
                    // A governor tier (0 high / 1 mid / 2 idle / 3 deep): save it +
                    // restart the governor so the new tier takes effect immediately.
                    act(app, "tier", move |s| {
                        let mut g = dpm::GovernorConfig::load_or_default();
                        let name = match tier {
                            0 => {
                                g.high_mhz = v;
                                "high"
                            }
                            1 => {
                                g.mid_mhz = v;
                                "mid"
                            }
                            2 => {
                                g.idle_mhz = v;
                                "idle"
                            }
                            _ => {
                                g.deep_mhz = v;
                                "deep"
                            }
                        };
                        // An inverted ladder would make thermal demotion RAISE
                        // the clock — refuse before saving.
                        g.validate_ladder()?;
                        g.save()?;
                        // Restart the single unit only in auto (no manual pin)
                        // so the running governor rereads the tier.
                        crate::persist::reload_if_auto();
                        *s = format!("{name} tier = {v} MHz (persisted)");
                        Ok(())
                    });
                }
            }
            app.edit = Edit::None;
        }
        (Edit::Value(_), KeyCode::Esc) => {
            app.edit = Edit::None;
            app.status = "cancelled".into();
        }
        _ => {}
    }
}

const CPU_FIELDS: usize = 3;

fn cpu_key(app: &mut ApuScreen, code: KeyCode) {
    // Per-field step + inclusive range. Curve is edited as the undervolt
    // MAGNITUDE (0..=50, shown as -N) so it fits the unsigned edit buffer.
    let (step, lo, hi) = match app.cpu_sel {
        0 => (10u32, ocq3::BOOST_MIN_MHZ, ocq3::BOOST_MAX_MHZ), // boost clk, ±10 MHz
        1 => (1u32, 0, (-ocq3::SCALE_MIN) as u32),              // curve undervolt magnitude
        _ => (1u32, 60, 100),                                   // cpu temp cap, C
    };
    match (&app.edit, code) {
        (Edit::None, KeyCode::Up) => app.cpu_sel = app.cpu_sel.saturating_sub(1),
        (Edit::None, KeyCode::Down) => app.cpu_sel = (app.cpu_sel + 1).min(CPU_FIELDS - 1),
        (Edit::None, KeyCode::Enter) => {
            let cur = match app.cpu_sel {
                0 => app.cpu.boost_mhz,
                1 => (-app.cpu.curve_scale) as u32,
                _ => app.cpu.cpu_temp_c,
            };
            app.edit = Edit::Value(cur);
            app.edit_typed = false;
            app.status = "type a value or ←/→ to adjust · Enter apply · Esc cancel".into();
        }
        (Edit::None, KeyCode::Char('r')) => {
            match app.oc.as_ref() {
                Some(oc) => match cpu::restore_stock(oc) {
                    Ok(()) => {
                        let _ = cpu::clear_saved();
                        crate::persist::disable_cpu_oc();
                        app.cpu = CpuOc::default();
                        app.status =
                            "CPU restored to firmware defaults (persistence removed)".into();
                    }
                    Err(e) => app.status = format!("restore error: {e}"),
                },
                None => app.status = "no queue-3 mailbox".into(),
            }
            app.cpu_live = app.oc.as_ref().map(cpu::live);
        }
        // The curve field (sel 1) is a NEGATIVE undervolt shown as -N, so the
        // arrows follow the number line: Left = more undervolt (bigger magnitude),
        // Right = less. Boost/temp keep the usual Left=down / Right=up.
        (Edit::Value(v), KeyCode::Left) => {
            app.edit = Edit::Value(if app.cpu_sel == 1 {
                (v + step).min(hi)
            } else {
                v.saturating_sub(step).max(lo)
            });
            app.edit_typed = true;
        }
        (Edit::Value(v), KeyCode::Right) => {
            app.edit = Edit::Value(if app.cpu_sel == 1 {
                v.saturating_sub(step).max(lo)
            } else {
                (v + step).min(hi)
            });
            app.edit_typed = true;
        }
        (Edit::Value(v), KeyCode::Char(c)) if c.is_ascii_digit() => {
            let dgt = c.to_digit(10).unwrap();
            let nv = if app.edit_typed {
                v.saturating_mul(10).saturating_add(dgt)
            } else {
                dgt
            };
            app.edit = Edit::Value(nv.min(hi));
            app.edit_typed = true;
        }
        (Edit::Value(v), KeyCode::Backspace) => {
            app.edit = Edit::Value(v / 10);
            app.edit_typed = true;
        }
        (Edit::Value(v), KeyCode::Enter) => {
            let v = (*v).clamp(lo, hi);
            let prev = app.cpu.clone();
            match app.cpu_sel {
                0 => app.cpu.boost_mhz = v,
                1 => app.cpu.curve_scale = -(v as i32),
                _ => app.cpu.cpu_temp_c = v,
            }
            app.edit = Edit::None;
            // Apply the whole CpuOc via queue-3 + persist (same path as CLI cpu
            // set). Direction-aware staging vs the pre-edit point, so lowering
            // boost / shallowing the curve never transiently overvolts.
            match app.oc.as_ref() {
                Some(oc) => match app.cpu.apply_from(oc, Some(&prev)) {
                    Ok(()) => {
                        let persisted =
                            app.cpu.save().is_ok() && crate::persist::enable_cpu_oc().is_ok();
                        app.status = format!(
                            "applied: {} MHz, scale {} (~{} mV){}",
                            app.cpu.boost_mhz,
                            app.cpu.curve_scale,
                            app.cpu.predicted_vid_mv(),
                            if persisted {
                                " — persisted"
                            } else {
                                " — live only"
                            }
                        );
                    }
                    Err(e) => app.status = format!("apply refused: {e}"),
                },
                None => app.status = "no queue-3 mailbox (need root + BC-250)".into(),
            }
            app.cpu_live = app.oc.as_ref().map(cpu::live);
        }
        (Edit::Value(_), KeyCode::Esc) => {
            app.edit = Edit::None;
            app.status = "cancelled".into();
        }
        _ => {}
    }
}

fn cu_key(app: &mut ApuScreen, code: KeyCode) {
    match code {
        KeyCode::Left => app.cu_sel = app.cu_sel.saturating_sub(1),
        KeyCode::Right => app.cu_sel = (app.cu_sel + 1).min(19),
        KeyCode::Up => app.cu_sel = app.cu_sel.saturating_sub(5),
        KeyCode::Down => app.cu_sel = (app.cu_sel + 5).min(19),
        KeyCode::Char(' ') => {
            let (a, w) = (app.cu_sel / 5, (app.cu_sel % 5) as u32);
            app.cu_draft[a] ^= 1 << w;
            app.cu_draft[a] &= curoute::FULL_MASK;
            app.cu_dirty = true;
            app.status = "draft edited — [a] apply  [esc] revert".into();
        }
        KeyCode::Char('f') => {
            app.cu_draft = [curoute::FULL_MASK; 4];
            app.cu_dirty = true;
            app.status = "draft: all 40 CU — [a] apply".into();
        }
        KeyCode::Char('t') => {
            app.cu_draft = [curoute::FACTORY_MASK; 4];
            app.cu_dirty = true;
            app.status = "draft: factory 24 CU — [a] apply".into();
        }
        KeyCode::Esc => {
            app.cu_draft = app.cu_live;
            app.cu_dirty = false;
            app.status = "draft reverted to live".into();
        }
        KeyCode::Char('a') => {
            let draft = app.cu_draft;
            // WEDGE GUARD: an empty shader array under compute hangs gfx1013.
            // The TUI has no unsafe override — use the CLI's --force-unsafe.
            if curoute::has_empty_array(&draft) {
                app.status =
                    "refused: draft leaves a shader array EMPTY (compute would wedge the box)"
                        .into();
                return;
            }
            act(app, "route", move |s| {
                curoute::apply(draft)?;
                // Persist: save the routing profile + install the boot re-apply
                // service so the route survives reboots.
                let persisted =
                    curoute::save_profile().is_ok() && crate::persist::enable_route().is_ok();
                *s = format!(
                    "routed {} CUs live{}",
                    curoute::cu_count(&draft),
                    if persisted {
                        " — persisted (survives reboot)"
                    } else {
                        " — live only"
                    }
                );
                Ok(())
            });
            refresh_masks(app);
            app.cu_draft = app.cu_live;
            app.cu_dirty = false;
        }
        KeyCode::Char('h') => {
            if cutest::available() {
                app.cu_test = true;
                app.cu_health.clear();
                app.status = "health-testing 40 CU (KAT compute, ~a few seconds)…".into();
            } else {
                app.status = "health-test needs umr + a Vulkan ICD (RADV)".into();
            }
        }
        KeyCode::Char('b') => {
            if curoute::has_empty_array(&app.cu_draft) {
                // The bench DISPATCHES compute — exactly the wedge. No override.
                app.status =
                    "refused: draft leaves a shader array EMPTY (bench would wedge the box)".into();
            } else if cutest::available() {
                app.cu_bench = true;
                let s = curoute::shape(&app.cu_draft);
                app.status = format!("benching draft ({} CU) — KAT compute…", s.cu);
            } else {
                app.status = "bench needs umr + a Vulkan ICD (RADV)".into();
            }
        }
        _ => {}
    }
}

// M5: the on-the-fly bench and the full/localize health-test now run on WORKER
// threads (spawned in `ApuScreen::tick_refresh`, harvested by `poll_cu_jobs`,
// applied by `apply_bench` / `apply_health`) instead of blocking the UI thread
// for the multi-second GPU sweep. The GPU-side logic (test_full / test_localize /
// bench_masks) is unchanged and still serialized by the process-wide compute lock;
// only the wrapping moved off the UI thread so keys stay live and the freeze is
// gone. A hung GPU can no longer wedge the UI permanently (M3 also bounds the
// underlying fence waits).

fn act(app: &mut ApuScreen, name: &str, f: impl FnOnce(&mut String) -> Result<()>) {
    let mut msg = String::new();
    match f(&mut msg) {
        Ok(()) => app.status = msg,
        Err(e) => app.status = format!("{name} error: {e}"),
    }
    app.snap = gather();
    app.last = Instant::now();
}

fn draw(f: &mut Frame, area: Rect, app: &ApuScreen) {
    // The suite shell owns the tab bar + global status line; this pane draws its
    // own title/body/footer INSIDE the content `area` it is handed (same layout
    // aputune used, just rooted at `area` not the whole frame).
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(2),
        ])
        .split(area);

    // Header: title only — the lit panel border shows which panel has focus.
    let bar = vec![
        Span::styled(
            " arieltune apu ",
            Style::default().fg(KEY).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  BC-250 APU liberation + tuner", Style::default().fg(DIM)),
        Span::styled("   [tab] move focus", Style::default().fg(DIM)),
    ];
    f.render_widget(Paragraph::new(Line::from(bar)), root[0]);

    // One screen: system identity card, then CPU (full width, with per-thread
    // activity), then CU routing | GPU clock. Live gauges live in their panels
    // now (no separate "live" card).
    let body = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(6),  // system card + carrier sensor row
            Constraint::Length(12), // CPU (controls + F/Vid curve | per-thread activity)
            Constraint::Min(12),    // CU routing | GPU clock
        ])
        .split(root[1]);

    draw_system(f, body[0], app, app.focus == Focus::Fan);
    draw_cpu(f, body[1], app, app.focus == Focus::Cpu);

    let botrow = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
        .split(body[2]);
    draw_gpu(f, botrow[0], app, app.focus == Focus::Gpu);
    draw_cu(f, botrow[1], app, app.focus == Focus::Cu);

    // Footer: status + keys for the focused panel. While a field is being edited,
    // show the EDIT keys — most importantly [esc] cancel, so you can back out of
    // e.g. a `force` edit without committing a clock you didn't want.
    let editing = matches!(app.edit, Edit::Value(_));
    let keys: &[(&str, &str)] = if app.patch_popup.is_some() {
        &[
            ("[up/dn]", "scroll"),
            ("[pgup/pgdn]", "page"),
            ("[home]", "top"),
            ("[esc]", "close"),
        ]
    } else if editing {
        &[
            ("[←→]", "adjust"),
            ("[0-9]", "type"),
            ("[enter]", "apply"),
            ("[esc]", "cancel"),
        ]
    } else {
        match app.focus {
            Focus::Fan => &[
                ("[↑↓]", "duty"),
                ("[enter]", "manual"),
                ("[a]", "auto"),
                ("[p]", "patches"),
                ("[tab]", "panel"),
                ("[q]", "quit"),
            ],
            Focus::Cu => &[
                ("[arrows]", "select"),
                ("[space]", "toggle"),
                ("[f]", "all"),
                ("[t]", "factory"),
                ("[a]", "apply"),
                ("[b]", "bench"),
                ("[h]", "test"),
                ("[tab]", "panel"),
                ("[q]", "quit"),
            ],
            Focus::Cpu => &[
                ("[↑↓]", "field"),
                ("[enter]", "edit"),
                ("[r]", "restore"),
                ("[tab]", "panel"),
                ("[q]", "quit"),
            ],
            Focus::Gpu => &[
                ("[↑↓]", "field"),
                ("[enter]", "edit"),
                ("[u]", "auto"),
                ("[tab]", "panel"),
                ("[q]", "quit"),
            ],
        }
    };
    f.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                app.status.clone(),
                Style::default().fg(GOOD).add_modifier(Modifier::BOLD),
            )),
            key_line(keys),
        ]),
        root[2],
    );

    // Modal overlay LAST so it paints on top of every panel.
    if app.patch_popup.is_some() {
        draw_patch_popup(f, app);
    }
}

/// Greedy word-wrap: break `text` on whitespace into lines of at most
/// `width` columns. A word longer than `width` gets a line of its own
/// rather than being split mid-word. The popup scroll is line-based, so
/// descriptions are pre-wrapped here instead of relying on Paragraph wrap.
fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut line = String::new();
    let mut cols = 0usize;
    for word in text.split_whitespace() {
        let w = word.chars().count();
        if cols > 0 && cols + 1 + w > width {
            out.push(std::mem::take(&mut line));
            cols = 0;
        }
        if cols > 0 {
            line.push(' ');
            cols += 1;
        }
        line.push_str(word);
        cols += w;
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

/// Render the patch-popup body: a short series intro, then one block per
/// patch — live-state glyph, ordinal, purpose, description, touched kernel
/// files, and the runtime tell. `states` come from the popup (captured at
/// open) and zip against SERIES (same order/length). Also the scroll-clamp
/// length source.
fn patch_popup_lines(states: &[State]) -> Vec<Line<'static>> {
    let intro = Style::default().fg(DIM);
    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(
            " The curated 12-patch amdgpu series arieltune embeds and builds into",
            intro,
        )),
        Line::from(Span::styled(
            " the kernel (arieltune apu build). Each patch carries a runtime tell that",
            intro,
        )),
        Line::from(Span::styled(
            " proves it is live on the booted kernel, checked when this opened.",
            intro,
        )),
        Line::from(""),
    ];
    for (i, (p, st)) in patches::SERIES.iter().zip(states.iter()).enumerate() {
        let state_color = match st {
            State::Present | State::Inferred => GOOD,
            State::Absent => BAD,
            State::Unknown => DIM,
        };
        let tell = match p.tell {
            patches::Tell::ModParam(n) => format!("module param amdgpu.{n}"),
            patches::Tell::Debugfs(n) => format!("debugfs node {n}"),
            patches::Tell::SclkMax(m) => format!("pp_dpm_sclk >= {m} MHz"),
            patches::Tell::CuCount(n) => format!(">= {n} active CUs"),
            patches::Tell::Bundled => "inferred (no unique tell)".into(),
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{:>2}. ", i + 1), Style::default().fg(DIM)),
            Span::styled(
                format!(" {} ", st.glyph()),
                Style::default()
                    .fg(state_color)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                p.title.to_string(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ]));
        for seg in wrap_words(p.desc, 64) {
            lines.push(Line::from(Span::styled(format!("        {seg}"), intro)));
        }
        lines.push(Line::from(vec![
            Span::styled("        touches: ", intro),
            Span::styled(p.touches.to_string(), Style::default().fg(ACCENT)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("        tell:    ", intro),
            Span::raw(tell),
        ]));
        lines.push(Line::from(""));
    }
    lines
}

/// The [p] modal: a centered scrollable popup detailing every member of the
/// liberation series. Renders only from the open-time capture — no fs reads.
fn draw_patch_popup(f: &mut Frame, app: &ApuScreen) {
    let Some(pop) = &app.patch_popup else { return };
    // Centered ~72% x ~80% rect via Flex on each axis.
    let horiz = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(72)])
        .flex(Flex::Center)
        .split(f.area());
    let rect = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(80)])
        .flex(Flex::Center)
        .split(horiz[0])[0];
    let live = pop
        .states
        .iter()
        .filter(|s| matches!(s, State::Present | State::Inferred))
        .count();
    let title = format!(
        " liberation series — {live}/{} live  [up/down scroll  esc close] ",
        patches::count()
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(ACCENT))
        .title(Span::styled(
            title,
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(rect);
    f.render_widget(Clear, rect);
    f.render_widget(block, rect);
    f.render_widget(
        Paragraph::new(patch_popup_lines(&pop.states)).scroll((pop.scroll, 0)),
        inner,
    );
}

/// The top "system" section: identity facts + an evenly-spaced carrier-board
/// sensor row (temps / fan / duty from the nct6686, plus the GPU edge temp), and
/// an interactive cooling control (focus with [1]) that speaks percent + mode,
/// never raw PWM.
fn draw_system(f: &mut Frame, area: Rect, app: &ApuScreen, focused: bool) {
    let s = &app.snap;
    let block = panel("system", focused);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(inner);

    // Identity facts (top).
    let facts: Vec<(String, String, Color)> = vec![
        (
            "device".into(),
            if s.is_bc250 {
                "BC-250 (Cyan Skillfish)".into()
            } else {
                "not a BC-250".into()
            },
            if s.is_bc250 { GOOD } else { BAD },
        ),
        ("kernel".into(), s.kernel.clone(), ACCENT),
        (
            "liberation".into(),
            format!(
                "{}/{} patches{}",
                s.present,
                s.total,
                if s.fully { " (full)" } else { "" }
            ),
            if s.fully { GOOD } else { WARN },
        ),
    ];
    let mut lines: Vec<Line> = facts
        .iter()
        .map(|(l, v, c)| {
            Line::from(vec![
                Span::styled(format!("  {l:<9} "), Style::default().fg(DIM)),
                Span::styled(
                    v.clone(),
                    Style::default().fg(*c).add_modifier(Modifier::BOLD),
                ),
            ])
        })
        .collect();
    // The liberation fact (last) doubles as a button: [p] opens the per-patch
    // detail popup. Lit accent when the card holds focus so it reads as
    // selectable.
    if let Some(line) = lines.last_mut() {
        let hint = if focused {
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(DIM)
        };
        line.spans.push(Span::styled("  [p: patch details]", hint));
    }
    f.render_widget(Paragraph::new(lines), rows[0]);

    // Carrier-board sensors — one evenly-spaced cell per reading across the
    // width. All from the throttled snapshot: draw() does no fs reads.
    let car = &s.carrier;
    let gpu = s.temp;
    let dash = || "—".to_string();
    let cells: Vec<(&str, String, Color)> = vec![
        (
            "SoC",
            car.soc_c.map(|t| format!("{t:.0}°C")).unwrap_or_else(dash),
            car.soc_c.map(temp_color).unwrap_or(DIM),
        ),
        (
            "GPU",
            gpu.map(|t| format!("{t:.0}°C")).unwrap_or_else(dash),
            gpu.map(temp_color).unwrap_or(DIM),
        ),
        (
            "Board",
            car.board_c
                .map(|t| format!("{t:.0}°C"))
                .unwrap_or_else(dash),
            car.board_c.map(temp_color).unwrap_or(DIM),
        ),
        (
            "Fan",
            car.fan_rpm.map(|r| format!("{r} rpm")).unwrap_or_else(dash),
            ACCENT,
        ),
        // Interactive cooling control — logical mode + live duty of the actual
        // cooler (never a raw PWM value, never a phantom idle header). This is
        // the single cooling readout; a separate "Duty" cell would just repeat
        // the percent shown here.
        cooling_cell(app, focused),
    ];
    let n = cells.len() as u32;
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(vec![Constraint::Ratio(1, n); cells.len()])
        .split(rows[1]);
    let last = cells.len() - 1;
    for (i, (l, v, c)) in cells.iter().enumerate() {
        // Light up the cooling-control label when the card holds focus.
        let label_color = if i == last && focused { ACCENT } else { DIM };
        // Centered in each equal-width column so the readings are evenly spaced.
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!("{l} "), Style::default().fg(label_color)),
                Span::styled(
                    v.clone(),
                    Style::default().fg(*c).add_modifier(Modifier::BOLD),
                ),
            ]))
            .alignment(Alignment::Center),
            cols[i],
        );
    }
}

/// The cooling-control cell for the sensor row: a logical view of fan state and
/// the interactive control. Percent + mode only — never a raw 0..255 PWM value.
///   * driver read-only (nct6683)  -> "read-only"
///   * focused                     -> "set N%" (the target Enter will apply)
///   * manual override active      -> "Manual N%"
///   * EC automatic curve          -> "Auto N%"
fn cooling_cell(app: &ApuScreen, focused: bool) -> (&'static str, String, Color) {
    let s = &app.snap;
    if !s.fan_writable {
        return ("Cool", "read-only".into(), DIM);
    }
    if focused {
        let t = match app.fan_target {
            None => "Auto".to_string(),
            Some(p) => format!("{p}%"),
        };
        return ("Cool", format!("set {t}"), ACCENT);
    }
    let duty = s.carrier.duty_pct;
    match s.fan_enable {
        // Manual software override — flag it (WARN) so it's clear the EC curve
        // is not in charge.
        Some(1) => (
            "Cool",
            duty.map(|d| format!("Manual {d}%"))
                .unwrap_or_else(|| "Manual".into()),
            WARN,
        ),
        _ => (
            "Cool",
            duty.map(|d| format!("Auto {d}%"))
                .unwrap_or_else(|| "Auto".into()),
            GOOD,
        ),
    }
}

fn draw_cu(f: &mut Frame, area: Rect, app: &ApuScreen, focused: bool) {
    // Bench-driven view: the grid is the routing map (WGP on/off per shader
    // array). The shape's effective CU follows the MEASURED two-smallest law
    // (eff = 4 x sum of the two smallest per-array WGP counts — imbalance is
    // EXPENSIVE, not a few percent); [b] measures the real GFLOPS.
    let dim = Style::default().fg(DIM);
    let bd = |s: &str| Span::styled(s.to_string(), dim);
    let pending = app.cu_draft != app.cu_live;
    let sh = curoute::shape(&app.cu_draft);

    let title = if app.cu_ok {
        "compute units (40 CU)"
    } else {
        "compute units — umr unavailable"
    };
    let block = panel(title, focused);
    let inner = block.inner(area);
    f.render_widget(block, area);
    // Vertical: note (top) · [die map | bench] (middle) · details (bottom).
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(7),
            Constraint::Min(0),
        ])
        .split(inner);

    // ---- top: the real-math note ----
    f.render_widget(
        Paragraph::new(vec![
            Line::from(bd(" GFLOPS ≈ 44 × eff-CU @1500 (eff=4×two-smallest WGP)")),
            Line::from(bd(" balanced eff=routed; unequal gated · [b] measures")),
        ]),
        rows[0],
    );

    // ---- middle-left: stacked-engines die map. Each WGP is its TWO CUs (a pair),
    // tight within the pair, spaced between WGPs — a WGP (2 CU) is the smallest
    // routable unit. SE0 on top, SE1 below; SH0/SH1 rows per engine. ----
    let sh_row = |ai: usize, shl: &str| -> Line<'static> {
        let mut spans: Vec<Span<'static>> = vec![Span::styled(
            format!("  {shl}  "),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        )];
        for w in 0..5u32 {
            let on = app.cu_draft[ai] & (1 << w) != 0;
            let live_on = app.cu_live[ai] & (1 << w) != 0;
            let sel = focused && ai * 5 + w as usize == app.cu_sel;
            let (glyph, mut color) = if on { ("██", GOOD) } else { ("··", DIM) };
            if on != live_on {
                color = ACCENT; // unapplied edit
            }
            let mut st = Style::default().fg(color).add_modifier(Modifier::BOLD);
            if sel {
                st = st.add_modifier(Modifier::REVERSED);
            }
            spans.push(Span::styled(glyph.to_string(), st));
            spans.push(Span::styled("  ".to_string(), Style::default().fg(DIM)));
            // inter-WGP gap
        }
        let cus = sh.per_array_wgp[ai] * 2;
        spans.push(Span::styled(
            format!("{cus:>3} CU"),
            Style::default()
                .fg(if cus == 0 { BAD } else { DIM })
                .add_modifier(Modifier::BOLD),
        ));
        Line::from(spans)
    };
    let border = |s: &str| Line::from(Span::styled(s.to_string(), Style::default().fg(DIM)));
    // Engine header with the name colored (box-drawing stays dim).
    let eng = |left: &str, name: &str, right: &str| -> Line<'static> {
        Line::from(vec![
            Span::styled(left.to_string(), Style::default().fg(DIM)),
            Span::styled(
                name.to_string(),
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(right.to_string(), Style::default().fg(DIM)),
        ])
    };
    let map = vec![
        eng(" ┌─────── ", "Shader Engine 0", " ───────┐"),
        sh_row(0, "SH0"),
        sh_row(1, "SH1"),
        eng(" ├─────── ", "Shader Engine 1", " ───────┤"),
        sh_row(2, "SH0"),
        sh_row(3, "SH1"),
        border(" └───────────────────────────────┘"),
    ];

    // ---- middle-right: bench sidebar (next to the engines) ----
    let bench = if let Some((routed, _eff, gflops, ok)) = app.cu_bench_result {
        let per = if routed > 0 {
            gflops / routed as f64
        } else {
            0.0
        };
        vec![
            Line::from(Span::styled(
                "bench",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            )),
            Line::from(bd("──────")),
            Line::from(Span::styled(
                format!("{gflops:.0}"),
                Style::default()
                    .fg(if ok { GOOD } else { BAD })
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(bd("GFLOPS")),
            Line::from(bd(&format!("{per:.1} /CU"))),
            Line::from(Span::styled(
                if ok {
                    format!("{routed} CU ok")
                } else {
                    format!("{routed} CU !")
                },
                Style::default().fg(if ok { DIM } else { BAD }),
            )),
        ]
    } else {
        vec![
            Line::from(Span::styled(
                "bench",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            )),
            Line::from(bd("──────")),
            Line::from(bd("press")),
            Line::from(Span::styled(
                "[b]",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            )),
            Line::from(bd("to")),
            Line::from(bd("measure")),
        ]
    };
    // Fixed-width map; the bench is centered in the leftover space to its right.
    let midcols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(35), Constraint::Min(0)])
        .split(rows[1]);
    f.render_widget(Paragraph::new(map), midcols[0]);
    let benchcol = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(13)])
        .flex(Flex::Center)
        .split(midcols[1]);
    f.render_widget(Paragraph::new(bench), benchcol[0]);

    // ---- bottom: legend, routed CU, warnings, boot enumeration, health ----
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled(
            " ██",
            Style::default().fg(GOOD).add_modifier(Modifier::BOLD),
        ),
        bd(" on  "),
        Span::styled("··", dim),
        bd(" off  "),
        Span::styled(
            "██",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        bd(" pending  ·  each pair = 1 WGP (2 CU)"),
    ]));
    let mut summ = vec![Span::styled(
        format!(" {}/40 CU routed", sh.cu),
        Style::default()
            .fg(if pending { ACCENT } else { GOOD })
            .add_modifier(Modifier::BOLD),
    )];
    if sh.effective_cu < sh.cu {
        summ.push(Span::styled(
            format!(" · {} effective", sh.effective_cu),
            Style::default().fg(WARN).add_modifier(Modifier::BOLD),
        ));
    }
    if pending {
        summ.push(Span::styled(
            format!("  (live {} — [a] apply)", curoute::cu_count(&app.cu_live)),
            dim,
        ));
    }
    lines.push(Line::from(summ));

    // Empty array = a real SAFETY flag: compute with a whole shader array gated
    // off can hang gfx1013. Unequal lanes only cost a little efficiency.
    if !sh.empty_arrays.is_empty() {
        lines.push(Line::from(Span::styled(
            format!(
                " ! {} array(s) empty — can hang compute (unsafe)",
                sh.empty_arrays.len()
            ),
            Style::default().fg(BAD),
        )));
    } else if sh.unbalanced {
        lines.push(Line::from(Span::styled(
            " · unequal lanes cost a little efficiency — [b] to compare".to_string(),
            dim,
        )));
    }
    // Boot enumeration — how the APU came up (catches fuse-broken WGPs).
    if let Some(drv) = app.cu_driver {
        let boot_cu = curoute::cu_count(&drv);
        let broken: Vec<String> = curoute::ARRAYS
            .iter()
            .enumerate()
            .flat_map(|(ai, (se, sh))| {
                (0..5u32)
                    .filter(move |w| drv[ai] & (1 << w) == 0)
                    .map(move |w| format!("SE{se}.SH{sh}/WGP{w}"))
            })
            .collect();
        let healthy = broken.is_empty();
        lines.push(Line::from(vec![
            Span::styled(
                format!("at boot: {boot_cu}/40 CU came up"),
                Style::default()
                    .fg(if healthy { GOOD } else { BAD })
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                if healthy {
                    "   all WGP healthy".to_string()
                } else {
                    format!("   broken: {}", broken.join(", "))
                },
                Style::default().fg(if healthy { DIM } else { BAD }),
            ),
        ]));
    }
    if !app.cu_health.is_empty() {
        lines.push(Line::from(Span::styled(
            "health-test:",
            Style::default().fg(ACCENT),
        )));
        for (text, ok) in &app.cu_health {
            lines.push(Line::from(Span::styled(
                format!("  {text}"),
                Style::default().fg(if *ok { GOOD } else { BAD }),
            )));
        }
    }
    f.render_widget(Paragraph::new(lines), rows[2]);
}

fn bar(frac: f64, width: usize, color: Color) -> Span<'static> {
    let frac = frac.clamp(0.0, 1.0);
    let fill = (frac * width as f64).round() as usize;
    let s: String = "█".repeat(fill) + &"░".repeat(width.saturating_sub(fill));
    Span::styled(s, Style::default().fg(color))
}

fn draw_cpu(f: &mut Frame, area: Rect, app: &ApuScreen, focused: bool) {
    let block = panel("CPU", focused);
    let inner = block.inner(area);
    f.render_widget(block, area);
    // Three columns spread across the panel: controls | F/Vid curve | threads.
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(30),
            Constraint::Length(34),
            Constraint::Length(38),
        ])
        .flex(Flex::SpaceBetween)
        .split(inner);
    // A column title, centered over the column's content.
    let header = |t: &str| -> Line<'static> {
        Line::from(Span::styled(
            t.to_string(),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))
        .centered()
    };

    // ---- col 0: overclock controls + live Vid/temp ----
    let c = &app.cpu;
    let field = |i: usize, label: &str, val: String| -> Line {
        let sel = focused && app.cpu_sel == i;
        let mut vs = Style::default().fg(ACCENT).add_modifier(Modifier::BOLD);
        if sel {
            vs = vs.add_modifier(Modifier::REVERSED);
        }
        Line::from(vec![
            Span::styled(if sel { " ▸ " } else { "   " }, Style::default().fg(KEY)),
            Span::styled(format!("{label:<10}"), Style::default().fg(DIM)),
            Span::styled(val, vs),
        ])
    };
    // While a field is being edited, show the in-progress value (and preview the
    // predicted Vid from it) so the user sees their typing live.
    let edit_val = |i: usize| -> Option<u32> {
        if focused && app.cpu_sel == i {
            if let Edit::Value(v) = app.edit {
                return Some(v);
            }
        }
        None
    };
    let boost_shown = edit_val(0).unwrap_or(c.boost_mhz);
    let curve_scale_shown = match edit_val(1) {
        Some(mag) => -(mag as i32),
        None => c.curve_scale,
    };
    let cpu_temp_shown = edit_val(2).unwrap_or(c.cpu_temp_c);
    let mut preview = c.clone();
    preview.boost_mhz = boost_shown;
    preview.curve_scale = curve_scale_shown;
    let pred = preview.predicted_vid_mv();
    let pred_ok = pred <= ocq3::VID_CEILING_MV;
    let live_vid = app.cpu_live.as_ref().and_then(|l| l.cur_vid_mv);
    let mut left = vec![
        header("overclock"),
        field(0, "boost clk", format!("{boost_shown} MHz")),
        field(
            1,
            "curve",
            format!(
                "{} {}",
                curve_scale_shown,
                if curve_scale_shown == 0 {
                    "(stock)"
                } else {
                    "(undervolt)"
                }
            ),
        ),
        field(2, "temp cap", format!("{cpu_temp_shown} C")),
    ];
    // Live Vid (temp lives in the top system card now — not repeated here).
    left.push(Line::from(vec![
        Span::styled("   Vid now  ", Style::default().fg(DIM)),
        Span::styled(
            live_vid
                .map(|v| format!("{v} mV"))
                .unwrap_or_else(|| "—".into()),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
    ]));
    left.push(Line::from(vec![
        Span::styled("   pred Vid ", Style::default().fg(DIM)),
        Span::styled(
            format!("{pred} mV"),
            Style::default()
                .fg(if pred_ok { GOOD } else { BAD })
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            if pred_ok {
                format!(" / {} cap", ocq3::VID_CEILING_MV)
            } else {
                " — undervolt more".into()
            },
            Style::default().fg(if pred_ok { DIM } else { BAD }),
        ),
    ]));
    // ---- col 1 (centre): F/Vid curve (2800-4100), two stacks so every step
    // shows. ▸ marks the active boost clock; red = over the Vid ceiling. Uses
    // the in-edit values so the whole curve previews LIVE as you dial the
    // undervolt / boost — before you commit it. ----
    let samples = cpu::curve_samples(curve_scale_shown);
    let (vmin, vmax) = (700.0f64, 1400.0f64);
    // Boost is 50 MHz-granular but the curve is plotted every 100 MHz — mark the
    // nearest plotted step so a 3550-style setpoint still shows where it sits.
    let boost_mark = ((boost_shown + 50) / 100) * 100;
    let curve_cell = |clk: u32, vid: u32| -> Vec<Span<'static>> {
        let frac = ((vid as f64 - vmin) / (vmax - vmin)).clamp(0.0, 1.0);
        let at_boost = clk == boost_mark;
        let color = if vid > ocq3::VID_CEILING_MV {
            BAD
        } else if at_boost {
            ACCENT
        } else {
            GOOD
        };
        vec![
            Span::styled(
                format!("{}{clk:>4} ", if at_boost { "▸" } else { " " }),
                Style::default().fg(if at_boost { ACCENT } else { DIM }),
            ),
            bar(frac, 5, color),
            Span::styled(format!(" {vid:>4}"), Style::default().fg(color)),
        ]
    };
    let half = samples.len().div_ceil(2);
    let mut curve = vec![header("F/Vid curve  MHz->mV")];
    for r in 0..half {
        let (clk, vid) = samples[r];
        let mut spans = curve_cell(clk, vid);
        if let Some(&(clk2, vid2)) = samples.get(r + half) {
            spans.push(Span::raw("  "));
            spans.extend(curve_cell(clk2, vid2));
        }
        curve.push(Line::from(spans));
    }
    // ---- col 2 (right): per-thread activity (all 12) + per-core boost freq ----
    let cell = |i: usize| -> Vec<Span<'static>> {
        let u = app.cpu_util.get(i).copied().unwrap_or(0);
        let col = if u >= 85 {
            BAD
        } else if u >= 50 {
            WARN
        } else {
            GOOD
        };
        vec![
            Span::styled(format!("CPU{i:<2} "), Style::default().fg(DIM)),
            bar(u as f64 / 100.0, 5, col),
            Span::styled(format!(" {u:>3}"), Style::default().fg(col)),
        ]
    };
    // A grid row is 32 cols wide (two 15-col cells + a 2-col gap). Center the
    // header + core-freq line over THAT width (not the wider column) so both sit
    // over the grid.
    const GRID_W: usize = 32;
    let thead = "threads  util% + core MHz";
    let hlead = GRID_W.saturating_sub(thead.chars().count()) / 2;
    let mut right = vec![Line::from(Span::styled(
        format!("{}{thead}", " ".repeat(hlead)),
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
    ))];
    // CPU0-5 left column, CPU6-11 right column (SMT sibling pairs are adjacent).
    for row in 0..6usize {
        let mut spans = cell(row);
        spans.push(Span::raw("  "));
        spans.extend(cell(row + 6));
        right.push(Line::from(spans));
    }
    if let Some(l) = &app.cpu_live {
        if !l.cores.is_empty() {
            // Per-core boost freq, centered UNDER the thread grid (pad within its
            // width, not the whole column) so it lines up cleanly. Header labels it.
            let freqs = l
                .cores
                .iter()
                .map(|m| format!("{m}"))
                .collect::<Vec<_>>()
                .join(" ");
            let lead = GRID_W.saturating_sub(freqs.chars().count()) / 2;
            right.push(Line::from(""));
            right.push(Line::from(Span::styled(
                format!("{}{freqs}", " ".repeat(lead)),
                Style::default().fg(GOOD),
            )));
        }
    }
    // Render all three columns with the SAME top pad (based on the tallest) so
    // their headers line up while the block stays roughly vertically centered.
    let maxlen = left.len().max(curve.len()).max(right.len());
    let pad = (inner.height as usize).saturating_sub(maxlen) / 2;
    let padded = |v: Vec<Line<'static>>| -> Vec<Line<'static>> {
        let mut o = vec![Line::from(""); pad];
        o.extend(v);
        o
    };
    f.render_widget(Paragraph::new(padded(left)), cols[0]);
    f.render_widget(Paragraph::new(padded(curve)), cols[1]);
    f.render_widget(Paragraph::new(padded(right)), cols[2]);
}

fn draw_gpu(f: &mut Frame, area: Rect, app: &ApuScreen, focused: bool) {
    let s = &app.snap;
    // The four governor tiers (editable here) come from the snapshot (gathered
    // once per second from governor.json — no fs read per frame).
    let g = &s.gov;
    let setrow = |i: usize, label: &str, val: u32| -> Line {
        let sel = focused && app.gpu_sel == i;
        let shown = if let (true, Edit::Value(v)) = (sel, &app.edit) {
            *v
        } else {
            val
        };
        let editing = sel && matches!(app.edit, Edit::Value(_));
        let mut vs = Style::default().fg(ACCENT).add_modifier(Modifier::BOLD);
        if editing {
            vs = Style::default()
                .fg(KEY)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED);
        } else if sel {
            vs = vs.add_modifier(Modifier::REVERSED);
        }
        Line::from(vec![
            Span::styled(if sel { " ▸ " } else { "   " }, Style::default().fg(KEY)),
            Span::styled(format!("{label:<7}"), Style::default().fg(DIM)),
            Span::styled(format!("{shown} MHz"), vs),
        ])
    };
    // Live gauges merged from the old top "live" card.
    let gauge = |label: &str, val: String, frac: f64, color: Color| -> Line {
        Line::from(vec![
            Span::styled(format!("   {label:<6}"), Style::default().fg(DIM)),
            bar(frac.clamp(0.0, 1.0), 8, color),
            Span::styled(
                format!(" {val}"),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
        ])
    };
    // GfxClk: sysfs first, else the SMU telemetry node (QueryGfxclk).
    let gfxclk = s.gfxclk.or_else(|| {
        app.tele.iter().find_map(|l| {
            l.split_once("GfxClk:")
                .and_then(|(_, r)| r.split_whitespace().next())
                .and_then(|n| n.parse::<u32>().ok())
        })
    });
    // GPU temp cap (moved here from the CPU panel; applied via SMU queue-3).
    let temp_cap = {
        let sel = focused && app.gpu_sel == 5;
        let shown = if let (true, Edit::Value(v)) = (sel, &app.edit) {
            *v
        } else {
            app.cpu.gpu_temp_c
        };
        let editing = sel && matches!(app.edit, Edit::Value(_));
        let mut vs = Style::default().fg(ACCENT).add_modifier(Modifier::BOLD);
        if editing {
            vs = Style::default()
                .fg(KEY)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED);
        } else if sel {
            vs = vs.add_modifier(Modifier::REVERSED);
        }
        Line::from(vec![
            Span::styled(if sel { " ▸ " } else { "   " }, Style::default().fg(KEY)),
            Span::styled("temp cap ", Style::default().fg(DIM)),
            Span::styled(format!("{shown} C"), vs),
        ])
    };
    // Editable GPU voltage SETPOINT (sel 6) — the OD voltage for the high clock
    // (pp_od_clk_voltage). Sits with the other editable fields, under temp cap;
    // the live value is the `vdd` meter in the gauges below.
    let volt_row = {
        let sel = focused && app.gpu_sel == 6;
        let shown = if let (true, Edit::Value(v)) = (sel, &app.edit) {
            Some(*v)
        } else {
            s.od_vddc
        };
        let editing = sel && matches!(app.edit, Edit::Value(_));
        let mut vs = Style::default().fg(ACCENT).add_modifier(Modifier::BOLD);
        if editing {
            vs = Style::default()
                .fg(KEY)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED);
        } else if sel {
            vs = vs.add_modifier(Modifier::REVERSED);
        }
        Line::from(vec![
            Span::styled(if sel { " ▸ " } else { "   " }, Style::default().fg(KEY)),
            Span::styled("voltage  ", Style::default().fg(DIM)),
            Span::styled(
                shown
                    .map(|v| format!("{v} mV"))
                    .unwrap_or_else(|| "—".into()),
                vs,
            ),
            // The OD sclk this setpoint applies at (point 0 = the top state).
            Span::styled(
                s.od_sclk
                    .filter(|_| shown.is_some())
                    .map(|c| format!(" @ {c} MHz"))
                    .unwrap_or_default(),
                Style::default().fg(DIM),
            ),
        ])
    };
    // Which power mode is active — cross-checked against the GPU power unit so
    // the row can't LIE: power.json saying "governor" while the unit is dead
    // means nothing is governing (no thermal demotion, no heat protection).
    let unit_dead = !s.gpu_unit_active;
    let mode = match (s.force_mhz, s.governor_on) {
        (Some(m), _) if unit_dead => (format!("manual {m} MHz (unit NOT running!)"), BAD),
        (Some(m), _) => (format!("manual {m} MHz"), ACCENT),
        (None, true) if unit_dead => ("governor set but NOT running!".to_string(), BAD),
        (None, true) => ("auto (governor)".to_string(), GOOD),
        (None, false) if !s.gpu_unit_installed => {
            ("released (BAPM; no boot unit)".to_string(), DIM)
        }
        (None, false) => ("released (BAPM)".to_string(), DIM),
    };
    let mut lines = vec![
        Line::from(vec![
            Span::styled("   mode  ", Style::default().fg(DIM)),
            Span::styled(
                mode.0,
                Style::default().fg(mode.1).add_modifier(Modifier::BOLD),
            ),
        ]),
        setrow(0, "high", g.high_mhz),
        setrow(1, "mid", g.mid_mhz),
        setrow(2, "idle", g.idle_mhz),
        setrow(3, "deep", g.deep_mhz),
        setrow(4, "force", app.gpu_force),
        temp_cap,
        volt_row,
        Line::from(""),
    ];
    if let Some(m) = gfxclk {
        lines.push(gauge(
            "gfxclk",
            format!("{m} MHz"),
            (m as f64 - 350.0) / (2500.0 - 350.0),
            GOOD,
        ));
    }
    if let Some(w) = app.gpu_power_w {
        let col = if w >= 200 {
            BAD
        } else if w >= 150 {
            WARN
        } else {
            GOOD
        };
        lines.push(gauge("power", format!("{w} W"), w as f64 / 225.0, col));
    }
    // Live GPU voltage meter (vddgfx from amdgpu hwmon — SMU-safe sysfs read),
    // scaled across the OD VDDC range so the bar tracks the setpoint above.
    if let Some(mv) = app.gpu_vid {
        let (lo, hi) = s.od_range;
        let frac = mv.saturating_sub(lo) as f64 / (hi.saturating_sub(lo)).max(1) as f64;
        lines.push(gauge("vdd", format!("{mv} mV"), frac, ACCENT));
    }
    f.render_widget(Paragraph::new(lines).block(panel("GPU", focused)), area);
}
