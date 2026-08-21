// SPDX-License-Identifier: GPL-2.0-only
//! cores — the BC-250 8-core CPU unlock + live per-core offline control.
//!
//! Two deliberately separated layers (see arieltune-core.md):
//!
//! * **Firmware layer** — the SMU-space core-presence mask (SMN 0x115A870).
//!   Stock `0x77` = 6C/12T (cores 3 and 7 masked), `0xFF` = 8C/16T. Written
//!   all-or-nothing via the q3 msg 0x98 backdoor ([`ariel_smu::ocq3::OcQ3`]).
//!   Applies at the next warm reboot; a cold boot reverts it. This module
//!   REFUSES any mask other than the known stock/full values — the community
//!   DXE drivers do the same, to avoid lockout on abnormal boards.
//!
//! * **OS layer** — per-thread online/offline via
//!   `/sys/devices/system/cpu/cpuN/online`. Arbitrary granularity (2 cores for
//!   a test, skip a defective core), instant, reversible, no firmware risk.
//!   This is the ONLY granular control arieltune ships; arbitrary firmware
//!   masks require the q2 0x23 SMU exploit and are parked as research.
//!
//! SAFETY RULES (must survive any edit):
//!   * No automatic reboots, ever. GabriWar's earlier unit rebooted itself and
//!     bootlooped a real board (the reset did not preserve the mask, and
//!     systemctl cannot reboot before D-Bus). `boot` applies and stops.
//!   * `boot` is the systemd-unit path: idempotent, exit 0 on refusal.
//!   * `verify` is ADVISORY. Its report is never read back for decisions:
//!     fleet images get cloned across blades, so persisted verdicts are
//!     untrustworthy lineage. Only live reads gate anything.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};

use ariel_smu::ocq3::{CoreUnlock, OcQ3, CORE_MASK_FULL, CORE_MASK_STOCK};

pub const UNIT_NAME: &str = "aputune-cores.service";
pub const UNIT_PATH: &str = "/etc/systemd/system/aputune-cores.service";
pub const REBOOT_MODE_PATH: &str = "/sys/kernel/reboot/mode";
pub const ACPI_OVERRIDE_DIR: &str = "/etc/initcpio/acpi_override";
pub const ACPI_BACKUP_DIR: &str = "/etc/initcpio/acpi_override-backups";
pub const MKINITCPIO_CONF: &str = "/etc/mkinitcpio.conf";
pub const REPORT_DIR: &str = "/var/lib/aputune";
pub const ACPI_TABLES: &[(&str, &str, &str)] = &[
    (
        "SSDT-CST.aml",
        "https://github.com/mendesrr/bc250-acpi-fix-updated-8c/raw/refs/heads/main/SSDT-CST.aml",
        "4ed0dfbae974426bdcc1d7d55da1d9669e5b7cc7c54b6e0683e8f2a6c76fbb81",
    ),
    (
        "SSDT-PST.aml",
        "https://github.com/mendesrr/bc250-acpi-fix-updated-8c/raw/refs/heads/main/SSDT-PST.aml",
        "1fb4a2d01a32eee3b9226bcbb19f63878c346d434a360b1bb28f837d4a1a3eb0",
    ),
];

/// Firmware/OS core state, computed from live reads only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoreState {
    /// Mask 0x77: stock 6C/12T.
    Locked,
    /// Mask 0xFF but fewer than 8 cores visible: warm reboot needed.
    PendingReboot,
    /// Mask 0xFF and 8 physical cores visible.
    Unlocked,
    /// Any other mask — all mutating verbs refuse.
    Abnormal(u8),
}

impl CoreState {
    pub fn label(self) -> &'static str {
        match self {
            CoreState::Locked => "LOCKED",
            CoreState::PendingReboot => "PENDING-REBOOT",
            CoreState::Unlocked => "UNLOCKED",
            CoreState::Abnormal(_) => "ABNORMAL",
        }
    }
}

/// Pure state derivation (unit-tested): mask + visible physical cores.
pub fn state_for(mask: u8, visible_cores: u32) -> CoreState {
    if mask == CORE_MASK_STOCK {
        CoreState::Locked
    } else if mask == CORE_MASK_FULL {
        if visible_cores >= 8 {
            CoreState::Unlocked
        } else {
            CoreState::PendingReboot
        }
    } else {
        CoreState::Abnormal(mask)
    }
}

/// Live snapshot: firmware mask, visible cores/threads, offlined threads, and
/// per-core thread online states (all gathered once, so the TUI draw is
/// fs-read-free).
#[derive(Clone, Debug)]
pub struct CoreSnapshot {
    pub state: CoreState,
    pub mask: u8,
    pub cores: u32,
    pub threads: u32,
    pub offline: u32,
    /// (core id, [(cpu, online), ..]) sorted by core id.
    pub per_core: Vec<(u32, Vec<(u32, bool)>)>,
}

/// List every cpuN directory index (0..max). The cpuN dirs exist even when the
/// CPU is offline — offlined CPUs lose their `topology/` subtree but keep the
/// dir and the `online` file, so this is the recovery-safe enumeration.
pub fn cpu_dirs() -> Vec<u32> {
    let mut cpus: Vec<u32> = Vec::new();
    for e in fs::read_dir("/sys/devices/system/cpu")
        .into_iter()
        .flatten()
        .flatten()
    {
        let name = e.file_name().to_string_lossy().into_owned();
        if let Some(n) = name.strip_prefix("cpu") {
            if let Ok(n) = n.parse::<u32>() {
                cpus.push(n);
            }
        }
    }
    cpus.sort_unstable();
    cpus
}

/// Physical core (SMN mask bit) a logical CPU belongs to. Offlined CPUs lose
/// their `topology/` subtree, so this falls back through: own core_id -> the
/// online SMT sibling's core_id -> the x86 adjacent-SMT-pair index (cpu/2).
/// The last resort keeps offlined cores grouped and recoverable.
pub fn core_of(cpu: u32) -> Option<u32> {
    let read = |c: u32| -> Option<u32> {
        fs::read_to_string(format!("/sys/devices/system/cpu/cpu{c}/topology/core_id"))
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
    };
    if let Some(c) = read(cpu) {
        return Some(c);
    }
    if let Some(c) = read(cpu ^ 1) {
        return Some(c);
    }
    Some(cpu / 2)
}

/// Physical core id -> first CPU number map, sorted by core id.
pub fn core_map() -> Vec<(u32, u32)> {
    let mut map: Vec<(u32, u32)> = Vec::new();
    for cpu in cpu_dirs() {
        let Some(core) = core_of(cpu) else { continue };
        if !map.iter().any(|(c, _)| *c == core) {
            map.push((core, cpu));
        }
    }
    map.sort_by_key(|(c, _)| *c);
    map
}

fn cpu_is_online(cpu: u32) -> bool {
    let p = format!("/sys/devices/system/cpu/cpu{cpu}/online");
    if !Path::new(&p).exists() {
        return true; // cpu0 and non-hotpluggable CPUs have no online file
    }
    fs::read_to_string(&p)
        .map(|s| s.trim() != "0")
        .unwrap_or(true)
}

/// Total logical threads and offlined-thread count (sysfs, live).
fn thread_counts() -> (u32, u32) {
    let mut total = 0u32;
    let mut off = 0u32;
    for e in fs::read_dir("/sys/devices/system/cpu")
        .into_iter()
        .flatten()
        .flatten()
    {
        let name = e.file_name().to_string_lossy().into_owned();
        if let Some(n) = name.strip_prefix("cpu") {
            if n.parse::<u32>().is_ok() {
                total += 1;
                let cpu = n.parse::<u32>().unwrap();
                if !cpu_is_online(cpu) {
                    off += 1;
                }
            }
        }
    }
    (total, off)
}

/// Live snapshot (requires root + BC-250 — `OcQ3::open` refuses other silicon).
pub fn snapshot() -> Result<CoreSnapshot> {
    let q = OcQ3::open()?;
    let mask = q.core_mask()?;
    let per_core = core_threads();
    let cores = per_core.len() as u32;
    let (threads, offline) = thread_counts();
    Ok(CoreSnapshot {
        state: state_for(mask, cores),
        mask,
        cores,
        threads,
        offline,
        per_core,
    })
}

fn describe_mask(mask: u8) -> String {
    let on: Vec<u8> = (0..8).filter(|i| mask & (1 << *i) != 0).collect();
    let off: Vec<u8> = (0..8).filter(|i| mask & (1 << *i) == 0).collect();
    format!(
        "0x{mask:02X}  enabled=[{}] disabled=[{}]",
        on.iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(" "),
        off.iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(" ")
    )
}

fn mce_count() -> u32 {
    Command::new("dmesg")
        .output()
        .ok()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter(|l| {
                    l.contains("Machine Check Exception")
                        || (l.contains("mce:") && (l.contains("error") || l.contains("corrected")))
                })
                .count() as u32
        })
        .unwrap_or(0)
}

/// Per-core CPU list with per-thread online flags, sorted by core id.
/// `(core_id, [(cpu, online), ..])` — used by the TUI core map. Groups EVERY
/// cpu dir (via [`core_of`]), so fully-offlined cores stay visible/recoverable.
pub fn core_threads() -> Vec<(u32, Vec<(u32, bool)>)> {
    let mut cores: Vec<(u32, Vec<(u32, bool)>)> = Vec::new();
    for cpu in cpu_dirs() {
        let Some(core) = core_of(cpu) else { continue };
        match cores.iter_mut().find(|(c, _)| *c == core) {
            Some((_, v)) => v.push((cpu, cpu_is_online(cpu))),
            None => cores.push((core, vec![(cpu, cpu_is_online(cpu))])),
        }
    }
    for (_, v) in cores.iter_mut() {
        v.sort_by_key(|(c, _)| *c);
    }
    cores.sort_by_key(|(c, _)| *c);
    cores
}

/// `aputune cores status`.
pub fn status() -> Result<()> {
    let s = snapshot()?;
    println!("SMN 0x115A870 = {}", describe_mask(s.mask));
    println!("state: {}", s.state.label());
    match s.state {
        CoreState::Locked => {
            println!("stock 6 cores / 12 threads — run 'aputune cores apply' to unlock")
        }
        CoreState::Unlocked => println!("all 8 cores / 16 threads active"),
        CoreState::PendingReboot => {
            println!("mask is set but the firmware has not re-enumerated yet — warm reboot needed")
        }
        CoreState::Abnormal(m) => println!("unknown mask 0x{m:02X} — all writes are refused"),
    }
    println!(
        "cores visible to this kernel: {} ({} threads, {} offline)",
        s.cores, s.threads, s.offline
    );
    println!("MCE entries in dmesg: {}", mce_count());
    println!(
        "boot unit: {}",
        if Path::new(UNIT_PATH).exists() {
            "installed"
        } else {
            "not installed"
        }
    );
    println!("state={}", s.state.label());
    Ok(())
}

fn set_warm_reboot() {
    if let Err(e) = fs::write(REBOOT_MODE_PATH, "warm") {
        eprintln!("warning: could not set warm reboot mode ({e})");
    }
}

/// `aputune cores apply`. Never reboots unless `warm_reboot`.
/// `force_abnormal` bypasses the 0x77 gate (experimental escape hatch).
pub fn apply(warm_reboot: bool, force_abnormal: bool) -> Result<()> {
    let before = snapshot()?;
    println!("before: mask {}", describe_mask(before.mask));

    let q = OcQ3::open()?;
    let outcome = if force_abnormal {
        println!(
            "WARNING: --force-abnormal bypasses the 0x77 gate (starting mask 0x{:02X}) — EXPERIMENTAL",
            before.mask
        );
        q.unlock_cores_any()?
    } else {
        q.unlock_cores()?
    };
    match outcome {
        CoreUnlock::AlreadyUnlocked => {
            println!("already unlocked, nothing to do");
        }
        CoreUnlock::Unlocked => {
            println!(
                "mask written and verified 0xFF — all 8 cores will appear after a WARM reboot"
            );
            println!(
                "a cold boot (power removed) reverts to 6 cores; the boot unit re-applies the mask"
            );
            println!("WARNING: the SoC power/thermal envelope changes with 2 extra cores — re-validate any CPU OC/undervolt and GPU VDDC settings");
            if warm_reboot {
                set_warm_reboot();
                println!("rebooting (warm)...");
                let st = Command::new("systemctl").arg("reboot").status()?;
                if !st.success() {
                    bail!("systemctl reboot failed ({st})");
                }
            } else {
                println!("run 'aputune cores status' after reboot; state should be UNLOCKED");
            }
        }
    }
    Ok(())
}

/// Boot-time path for `aputune-cores.service`. Idempotent; NEVER reboots;
/// exit 0 even on refusal (no restart storm, per design).
pub fn boot() -> Result<()> {
    let mask = match OcQ3::open().and_then(|q| q.core_mask()) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("cores boot: {e}");
            return Ok(());
        }
    };
    if mask == CORE_MASK_FULL {
        println!("already unlocked — nothing to do");
        return Ok(());
    }
    if mask != CORE_MASK_STOCK {
        eprintln!("unexpected mask 0x{mask:02X}, not touching anything");
        return Ok(());
    }
    let q = OcQ3::open()?;
    match q.unlock_cores() {
        Ok(CoreUnlock::Unlocked) => {
            println!("mask set to 0xFF — all 8 cores will appear on your next reboot");
        }
        Ok(CoreUnlock::AlreadyUnlocked) => {}
        Err(e) => {
            eprintln!("unlock failed ({e}) — staying at 6 cores");
        }
    }
    Ok(())
}

/// `aputune cores install` — write + enable the boot unit.
pub fn install() -> Result<()> {
    let unit = format!(
        "[Unit]\n\
         Description=BC-250 8-core unlock (SMU msg 0x98)\n\
         After=multi-user.target\n\
         ConditionPathExists=/sys/bus/pci/devices/0000:00:00.0/config\n\
         \n\
         [Service]\n\
         Type=oneshot\n\
         RemainAfterExit=yes\n\
         ExecStart=/usr/local/bin/arieltune apu cores boot\n\
         \n\
         [Install]\n\
         WantedBy=multi-user.target\n"
    );
    fs::write(UNIT_PATH, unit).context("write unit file")?;
    for args in [&["daemon-reload"][..], &["enable", UNIT_NAME][..]] {
        let st = Command::new("systemctl").args(args).status()?;
        if !st.success() {
            bail!("systemctl {} failed ({st})", args.join(" "));
        }
    }
    println!("installed: {UNIT_PATH} (enabled)");
    println!("the unit NEVER reboots; after a cold boot it sets the mask, and the cores appear on your next reboot");
    Ok(())
}

/// `aputune cores uninstall`.
pub fn uninstall() -> Result<()> {
    let _ = Command::new("systemctl")
        .args(["disable", "--now", UNIT_NAME])
        .status();
    let _ = fs::remove_file(UNIT_PATH);
    let _ = Command::new("systemctl").arg("daemon-reload").status();
    println!("removed. cores stay unlocked until the next cold boot.");
    Ok(())
}

// ---------------------------------------------------------------------------
// OS layer — live per-core offline/online
// ---------------------------------------------------------------------------

fn set_core_online(core: u32, online: bool) -> Result<u32> {
    // BOTH threads of the core, from the robust grouping (offline-safe).
    let cpus: Vec<u32> = core_threads()
        .iter()
        .find(|(c, _)| *c == core)
        .map(|(_, v)| v.iter().map(|(cpu, _)| *cpu).collect())
        .unwrap_or_else(|| {
            // Fully-offline core with no readable topology: the adjacent SMT
            // pair (x86 enumeration order) is the best available guess.
            vec![2 * core, 2 * core + 1]
        });
    if cpus.is_empty() {
        bail!("core {core} has no known logical CPUs");
    }
    if !online && cpus.contains(&0) {
        bail!("refusing to offline cpu0 (core 0) — the kernel cannot offline cpu0");
    }
    let mut touched = 0;
    for cpu in &cpus {
        let p = format!("/sys/devices/system/cpu/cpu{cpu}/online");
        if Path::new(&p).exists() {
            fs::write(&p, if online { "1" } else { "0" }).with_context(|| format!("write {p}"))?;
            touched += 1;
        }
    }
    let (threads, offline) = thread_counts();
    println!(
        "core {core} {} — now {threads} threads visible, {offline} offline",
        if online { "online" } else { "offline" }
    );
    Ok(touched)
}

/// Offline all cores except 0, or one specific core. OS layer only.
pub fn offline(core: Option<u32>) -> Result<()> {
    match core {
        Some(c) => {
            set_core_online(c, false)?;
        }
        None => {
            // No topology needed: write 0 to every hotpluggable CPU but cpu0.
            // This path must keep working when cores are already offline.
            for cpu in cpu_dirs() {
                if cpu == 0 {
                    continue;
                }
                let p = format!("/sys/devices/system/cpu/cpu{cpu}/online");
                if Path::new(&p).exists() {
                    fs::write(&p, "0").with_context(|| format!("write {p}"))?;
                }
            }
            let (threads, offline) = thread_counts();
            println!(
                "all cores except 0 offline — now {threads} threads visible, {offline} offline"
            );
        }
    }
    Ok(())
}

/// Online one core or every present CPU. The `None` (all) path writes 1 to
/// every hotpluggable CPU dir with no topology dependency — the guaranteed
/// recovery route after anything got offlined.
pub fn online(core: Option<u32>) -> Result<()> {
    match core {
        Some(c) => {
            set_core_online(c, true)?;
        }
        None => {
            for cpu in cpu_dirs() {
                let p = format!("/sys/devices/system/cpu/cpu{cpu}/online");
                if Path::new(&p).exists() {
                    fs::write(&p, "1").with_context(|| format!("write {p}"))?;
                }
            }
            let (threads, offline) = thread_counts();
            println!("all cpus online — now {threads} threads visible, {offline} offline");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// verify — advisory per-core stress-ng sweep (never a gate)
// ---------------------------------------------------------------------------

fn stress_one(cpu: u32, seconds: u64) -> (Option<f64>, Option<u64>, Option<u64>) {
    let out = Command::new("taskset")
        .args(["-c", &cpu.to_string()])
        .args([
            "stress-ng",
            "--cpu",
            "1",
            "--cpu-method",
            "all",
            "--verify",
            "--metrics-brief",
        ])
        .args(["-t", &format!("{seconds}s")])
        .output();
    let Ok(out) = out else {
        return (None, None, None);
    };
    let text = String::from_utf8_lossy(&out.stdout);
    // bogo ops/s: the metric table's cpu row, 7th whitespace field.
    let mut rate = None;
    for line in text.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.first() == Some(&"cpu") && f.len() >= 7 {
            rate = f[6].parse::<f64>().ok();
            break;
        }
    }
    let find = |tag: &str| -> Option<u64> {
        text.lines()
            .find_map(|l| l.strip_prefix(&format!("{tag}:")))
            .and_then(|s| s.split_whitespace().next())
            .and_then(|v| v.parse::<u64>().ok())
    };
    let passed = find("passed");
    let failed = find("failed");
    (rate, passed, failed)
}

/// Run the advisory sweep and return report lines (also the TUI worker path).
pub fn sweep_lines(seconds: u64) -> Result<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    if Command::new("stress-ng").arg("--version").output().is_err() {
        bail!("stress-ng not installed (the sweep needs it; verify is advisory)");
    }
    let map = core_map();
    out.push(format!(
        "per-core sweep ({}s each, 1 thread pinned per physical core) on {} cores",
        seconds,
        map.len()
    ));
    out.push("core  cpu  bogo-ops/s     passed failed  note".into());
    let mut rates: Vec<(u32, f64)> = Vec::new();
    let mut problems: Vec<String> = Vec::new();
    for (core, cpu) in &map {
        let (rate, passed, failed) = stress_one(*cpu, seconds);
        let tag = if *core == 3 || *core == 7 { "NEW" } else { "" };
        let fail = failed.unwrap_or(0);
        if fail > 0 {
            problems.push(format!(
                "core {core}: {fail} verify failures (wrong results)"
            ));
        }
        if let Some(r) = rate {
            rates.push((*core, r));
        }
        out.push(format!(
            "{:<4} {:<4} {:>12} {:>8} {:>7}  {}",
            core,
            cpu,
            rate.map(|r| format!("{r:.0}"))
                .unwrap_or_else(|| "?".into()),
            passed.map(|p| p.to_string()).unwrap_or_else(|| "?".into()),
            failed.map(|f| f.to_string()).unwrap_or_else(|| "?".into()),
            tag
        ));
    }
    if !rates.is_empty() {
        rates.sort_by_key(|(_, r)| *r as i64);
        let med = rates[rates.len() / 2].1;
        out.push(format!("deviation from median ({med:.0} bogo-ops/s):"));
        for (core, r) in &rates {
            let pct = (r - med) / med * 100.0;
            let tag = if *core == 3 || *core == 7 {
                "   <-- NEW"
            } else {
                ""
            };
            out.push(format!("  core {core}: {r:>10.0}  ({pct:+.1}%){tag}"));
        }
    }
    out.push(format!("MCE entries in dmesg: {}", mce_count()));
    if problems.is_empty() {
        out.push(
            "verdict: no verify failures observed (advisory only — this never gates anything)"
                .into(),
        );
    } else {
        out.push(format!("verdict: PROBLEMS — {}", problems.join("; ")));
        out.push("  a core that fails --verify produces WRONG results. Do not use it.".into());
    }
    Ok(out)
}

/// `aputune cores verify [seconds]` — advisory sweep, prints + saves a report.
pub fn verify(seconds: u64) -> Result<()> {
    let lines = sweep_lines(seconds)?;
    for l in &lines {
        println!("{l}");
    }

    // Report for the record only — nothing reads it back for decisions
    // (fleet images are cloned across blades; persisted verdicts lie).
    fs::create_dir_all(REPORT_DIR).ok();
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let _ = fs::write(
        format!("{REPORT_DIR}/cores-verify-{ts}.txt"),
        lines.join("\n"),
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// ACPI — 8-core SSDT-CST/PST initcpio override
// ---------------------------------------------------------------------------

fn cpus_without_idle() -> u32 {
    let mut n = 0;
    for (_, cpu) in core_map() {
        let dir = format!("/sys/devices/system/cpu/cpu{cpu}/cpuidle");
        if !Path::new(&dir).exists()
            || fs::read_dir(&dir)
                .map(|mut d| d.next().is_none())
                .unwrap_or(true)
        {
            n += 1;
        }
    }
    n
}

fn cst_objects(path: &Path) -> u32 {
    let Ok(bytes) = fs::read(path) else { return 0 };
    // Count distinct C0xx processor objects in the AML blob.
    let mut found = std::collections::HashSet::new();
    for i in 0..bytes.len().saturating_sub(3) {
        if bytes[i] == b'C' && bytes[i + 1] == b'0' {
            if let Ok(s) = std::str::from_utf8(&bytes[i..i + 4]) {
                if s.chars().all(|c| c.is_ascii_hexdigit()) {
                    found.insert(s.to_string());
                }
            }
        }
    }
    found.len() as u32
}

/// `aputune cores acpi status`.
pub fn acpi_status() -> Result<()> {
    let s = snapshot()?;
    let no_idle = cpus_without_idle();
    let cst = Path::new(ACPI_OVERRIDE_DIR).join("SSDT-CST.aml");
    let objs = if cst.exists() { cst_objects(&cst) } else { 0 };
    println!("logical threads: {}", s.threads);
    println!("cpus without idle states: {no_idle}");
    println!(
        "installed CST processor objects: {objs} ({} installed)",
        if cst.exists() { "override" } else { "none" }
    );
    if no_idle > 0 && s.threads > 12 {
        println!("hint: threads 12-15 lack C-states — 'aputune cores acpi install' fixes this");
    }
    Ok(())
}

/// `aputune cores acpi install` — stage the 8-core tables + rebuild initramfs.
pub fn acpi_install() -> Result<()> {
    if !Path::new(MKINITCPIO_CONF).exists() {
        bail!(
            "{MKINITCPIO_CONF} not found — initcpio ACPI override needs it (Arch-family initramfs)"
        );
    }
    fs::create_dir_all(ACPI_OVERRIDE_DIR).context("mkdir acpi override")?;
    fs::create_dir_all(ACPI_BACKUP_DIR).context("mkdir acpi backup")?;
    for (name, url, sha) in ACPI_TABLES {
        let dst = Path::new(ACPI_OVERRIDE_DIR).join(name);
        if dst.exists() {
            fs::copy(&dst, Path::new(ACPI_BACKUP_DIR).join(name))
                .with_context(|| format!("backup {name}"))?;
        }
        let st = Command::new("curl")
            .args(["-fsSL", "-o"])
            .arg(&dst)
            .arg(*url)
            .status()?;
        if !st.success() {
            bail!("download of {name} failed (network needed)");
        }
        // Pin the artifact: refuse a changed AML instead of staging it.
        let out = Command::new("sha256sum").arg(&dst).output()?;
        let got = String::from_utf8_lossy(&out.stdout);
        let got = got.split_whitespace().next().unwrap_or("");
        if got != *sha {
            bail!("sha256 mismatch for {name} (expected {sha}, got {got}) — refusing staged AML");
        }
        println!("staged {name} (backup kept in {ACPI_BACKUP_DIR})");
    }
    // Ensure the acpi_override hook exists.
    let conf = fs::read_to_string(MKINITCPIO_CONF)?;
    let mut new_conf = conf.clone();
    if !conf.contains("acpi_override") {
        if let Some(hooks_line) = conf.lines().find(|l| l.starts_with("HOOKS=")) {
            let inner = &hooks_line[6..];
            let mut hooks: Vec<&str> = inner
                .trim_start_matches('(')
                .trim_end_matches(')')
                .split_whitespace()
                .collect();
            // Insert before the last hook (usually filesystems/encrypt...).
            let insert_at = hooks.len().saturating_sub(1).min(if hooks.is_empty() {
                0
            } else {
                hooks.len() - 1
            });
            hooks.insert(insert_at, "acpi_override");
            new_conf = new_conf.replace(hooks_line, &format!("HOOKS=({})", hooks.join(" ")));
        } else {
            bail!("no HOOKS= line in {MKINITCPIO_CONF}");
        }
    }
    if new_conf != conf {
        fs::write(MKINITCPIO_CONF, &new_conf).context("update mkinitcpio.conf")?;
        println!("added acpi_override hook to HOOKS");
    }
    println!("rebuilding initramfs (mkinitcpio -P)...");
    let st = Command::new("mkinitcpio").arg("-P").status()?;
    if !st.success() {
        bail!("mkinitcpio -P failed ({st})");
    }
    println!("done — reboot for the 8-core tables to take effect");
    Ok(())
}

/// `aputune cores acpi revert`.
pub fn acpi_revert() -> Result<()> {
    let mut reverted = false;
    for (name, _, _) in ACPI_TABLES {
        let dst = Path::new(ACPI_OVERRIDE_DIR).join(name);
        let bak = Path::new(ACPI_BACKUP_DIR).join(name);
        if bak.exists() {
            fs::copy(&bak, &dst).with_context(|| format!("restore {name}"))?;
            reverted = true;
        } else {
            let _ = fs::remove_file(&dst);
        }
    }
    if reverted {
        println!("restored previous tables");
    } else {
        println!("no backup found — override tables removed");
    }
    if Path::new(MKINITCPIO_CONF).exists() {
        let st = Command::new("mkinitcpio").arg("-P").status()?;
        if !st.success() {
            bail!("mkinitcpio -P failed ({st})");
        }
        println!("initramfs rebuilt");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_machine() {
        assert_eq!(state_for(CORE_MASK_STOCK, 6), CoreState::Locked);
        assert_eq!(state_for(CORE_MASK_STOCK, 8), CoreState::Locked);
        assert_eq!(state_for(CORE_MASK_FULL, 6), CoreState::PendingReboot);
        assert_eq!(state_for(CORE_MASK_FULL, 8), CoreState::Unlocked);
        assert_eq!(state_for(0x7F, 7), CoreState::Abnormal(0x7F));
        assert_eq!(state_for(0x03, 2), CoreState::Abnormal(0x03));
        assert_eq!(state_for(0x00, 0), CoreState::Abnormal(0x00));
    }

    #[test]
    fn mask_descriptions() {
        assert!(describe_mask(0x77).contains("enabled=[0 1 2 4 5 6]"));
        assert!(describe_mask(0x77).contains("disabled=[3 7]"));
        assert!(describe_mask(0xFF).contains("enabled=[0 1 2 3 4 5 6 7]"));
    }
}
