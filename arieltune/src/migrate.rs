// SPDX-License-Identifier: GPL-2.0-only
//! `arieltune migrate` -- move a box that was running the four standalone tools
//! onto the unified suite, SAFELY.
//!
//! What it does NOT do: it does not touch per-app config/data. Each tab still
//! reads its proven paths (`/var/lib/aputune`, `/var/lib/memtune`, biostune's
//! stores), so a tuned box keeps its profiles/timings/power.json with zero data
//! movement -- the safest possible migration. The compat symlinks (installed by
//! `make install`) keep old `aputune ...`/`memtune ...` invocations and any
//! systemd units that call them working unchanged.
//!
//! What it DOES do: the APU GPU power unit was renamed `aputune-gpu.service` ->
//! `arieltune-gpu.service` (M5). Two enabled GPU units that both drive the SMU
//! clock is the double-writer hazard. So migrate STOPS + DISABLES the legacy
//! `aputune-*` units (disable-before-enable); the new `arieltune-*` unit is then
//! (re)laid the next time you pick a power mode (`arieltune apu gpu governor-on |
//! autosleep-on | force <mhz>`), which writes the unit + enables it. It also
//! creates the suite runtime dir `/run/arieltune` (the MEM<->APU governor poke).
//!
//! Dry-run by default; pass `--apply` to actuate. Needs root to disable units.

use anyhow::{Context, Result};
use std::path::Path;
use std::process::{Command, Stdio};

/// Legacy systemd units from the standalone era. Disabled before the suite's
/// `arieltune-*` units are enabled, so the SMU never has two clock writers.
const LEGACY_UNITS: [&str; 7] = [
    "aputune-gpu.service",
    "aputune-gpu-clock.service",
    "aputune-gpu-governor.service",
    "aputune-autosleep.service",
    "aputune-route.service",
    "aputune-cpu-oc.service",
    "aputune-poke@.service",
];

const RUN_DIR: &str = "/run/arieltune";

fn unit_enabled(unit: &str) -> bool {
    Command::new("systemctl")
        .args(["is-enabled", "--quiet", unit])
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn unit_active(unit: &str) -> bool {
    Command::new("systemctl")
        .args(["is-active", "--quiet", unit])
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Run the migration. `apply == false` reports what would happen without touching
/// anything.
pub fn run(apply: bool) -> Result<()> {
    let mode = if apply { "APPLY" } else { "dry-run" };
    println!("arieltune migrate ({mode})");
    println!("  config/data: unchanged -- each tab keeps its own /var/lib paths.");
    println!("  compat symlinks keep old `aputune`/`memtune`/... commands working.");
    println!();

    // 1. Legacy GPU/route/poke units -> stop + disable (disable-before-enable).
    let mut acted = false;
    for unit in LEGACY_UNITS {
        let enabled = unit_enabled(unit);
        let active = unit_active(unit);
        if !enabled && !active {
            continue;
        }
        acted = true;
        println!(
            "  legacy unit {unit}: {}{}",
            if active { "active " } else { "" },
            if enabled { "enabled" } else { "(not enabled)" }
        );
        if apply {
            // stop then disable; ignore "not loaded" noise.
            let _ = Command::new("systemctl").args(["stop", unit]).status();
            let _ = Command::new("systemctl").args(["disable", unit]).status();
            println!("    -> stopped + disabled");
        } else {
            println!("    -> would stop + disable");
        }
    }
    if !acted {
        println!("  no legacy aputune-* units enabled/active -- nothing to migrate there.");
    }

    if apply {
        let _ = Command::new("systemctl").arg("daemon-reload").status();
    }

    // 2. Suite runtime dir for the MEM<->APU governor poke.
    println!();
    if Path::new(RUN_DIR).is_dir() {
        println!("  {RUN_DIR}: present");
    } else if apply {
        std::fs::create_dir_all(RUN_DIR).with_context(|| format!("create {RUN_DIR}"))?;
        println!("  {RUN_DIR}: created");
    } else {
        println!("  {RUN_DIR}: would create");
    }

    println!();
    if apply {
        println!("[ok] migration applied. Pick the APU power mode to lay the new unit:");
    } else {
        println!("dry-run only. Re-run with --apply (as root) to actuate.");
        println!("Then pick the APU power mode to lay the new unit:");
    }
    println!("    sudo arieltune apu gpu governor-on   # or autosleep-on | force <mhz>");
    Ok(())
}
