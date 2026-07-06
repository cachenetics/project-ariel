// SPDX-License-Identifier: GPL-2.0-only
//! `arieltune uninstall [--purge] [--revert-hw]` -- one command that replaces the
//! four standalone teardowns. Dry-run by default; `--apply` actuates.
//!
//! Plain uninstall (`--apply`):
//!   * stop + disable every suite unit (arieltune-*) AND legacy unit (aputune-*)
//!   * dkms-remove the BIOS `smiflash` module + drop its staged driver dir
//!   * remove the binary + the `at` alias + the four compat symlinks
//!   * REPORT the persistent hardware state that outlives the app (GPU boot-pin,
//!     GDDR6 CMOS timings, EFI/APCB writes) with the one-liner to revert each
//!
//! `--revert-hw`: best-effort restore of stock hardware state FIRST (APU GPU clock
//! and voltage released, APU CPU OC restored to firmware defaults). Memory timings
//! in CMOS (reboot-applied) and BIOS/EFI writes can't be auto-stocked without a
//! backup/NVRAM-clear -- those are reported, not forced.
//!
//! `--purge`: also delete the per-app state dirs (/var/lib/{arieltune,aputune,
//! memtune,biostune}) + /run/arieltune. Guarded: refuses while a GPU/boot unit is
//! still active (stop it first, so nothing re-pins mid-purge).

use anyhow::{bail, Result};
use std::process::{Command, Stdio};

const PREFIX: &str = "/usr/local";

const SUITE_UNITS: &[&str] = &[
    "arieltune-gpu.service",
    "arieltune-route.service",
    "arieltune-cpu-oc.service",
    "arieltune-gpu-clock.service",
    "arieltune-gpu-governor.service",
    "arieltune-autosleep.service",
    "arieltune-poke@.service",
];
const LEGACY_UNITS: &[&str] = &[
    "aputune-gpu.service",
    "aputune-route.service",
    "aputune-cpu-oc.service",
    "aputune-gpu-clock.service",
    "aputune-gpu-governor.service",
    "aputune-autosleep.service",
    "aputune-poke@.service",
];
/// GPU/boot units whose activity blocks a `--purge` (they re-pin the clock).
const ACTIVE_BLOCKERS: &[&str] = &["arieltune-gpu.service", "aputune-gpu.service"];

const BINARIES: &[&str] = &[
    "arieltune",
    "at",
    "aputune",
    "memtune",
    "biostune",
    "wikitune",
];
const STATE_DIRS: &[&str] = &[
    "/var/lib/arieltune",
    "/var/lib/aputune",
    "/var/lib/memtune",
    "/var/lib/biostune",
    "/run/arieltune",
];
const DRIVER_DIRS: &[&str] = &["/usr/share/arieltune/driver", "/usr/share/biostune/driver"];

fn unit_active(unit: &str) -> bool {
    Command::new("systemctl")
        .args(["is-active", "--quiet", unit])
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Best-effort self-invocation of a wired CLI path (for --revert-hw). Uses the
/// still-installed binary; tolerates failure (no board / not root).
fn self_cli(args: &[&str], apply: bool) {
    if !apply {
        println!("    would run: arieltune {}", args.join(" "));
        return;
    }
    let exe = std::env::current_exe().unwrap_or_else(|_| "arieltune".into());
    let ok = Command::new(exe)
        .args(args)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    println!(
        "    arieltune {} -> {}",
        args.join(" "),
        if ok { "[ok]" } else { "[skipped/failed]" }
    );
}

pub fn run(apply: bool, purge: bool, revert_hw: bool) -> Result<()> {
    let mode = if apply { "APPLY" } else { "dry-run" };
    println!(
        "arieltune uninstall ({mode}){}{}",
        if purge { " --purge" } else { "" },
        if revert_hw { " --revert-hw" } else { "" }
    );
    println!();

    // 0. Purge guard: a live GPU/boot unit would re-pin the clock mid-purge.
    if purge {
        for u in ACTIVE_BLOCKERS {
            if unit_active(u) {
                bail!("{u} is still active -- stop it before --purge (it would re-pin the clock). Re-run without --purge, or `systemctl stop {u}` first.");
            }
        }
    }

    // 1. --revert-hw: restore stock hardware FIRST (best-effort), while the binary
    //    + units still exist.
    if revert_hw {
        println!("revert-hw: restoring stock hardware state (best-effort)...");
        self_cli(&["apu", "gpu", "unforce"], apply); // release GFX clock lock
        self_cli(&["apu", "gpu", "unforce-vid"], apply); // stock SMU voltage curve
        self_cli(&["apu", "cpu", "restore"], apply); // CPU OC -> firmware defaults
        println!("  note: GDDR6 timings (CMOS, reboot-applied) + BIOS/EFI writes are");
        println!("        NOT auto-stocked -- see the persistent-state report below.");
        println!();
    }

    // 2. Stop + disable all suite + legacy units.
    println!("units: stop + disable (suite + legacy)...");
    for u in SUITE_UNITS.iter().chain(LEGACY_UNITS.iter()) {
        if apply {
            let _ = Command::new("systemctl")
                .args(["stop", u])
                .stderr(Stdio::null())
                .status();
            let _ = Command::new("systemctl")
                .args(["disable", u])
                .stderr(Stdio::null())
                .status();
        }
    }
    if apply {
        let _ = Command::new("systemctl").arg("daemon-reload").status();
        println!(
            "  stopped + disabled {} unit names",
            SUITE_UNITS.len() + LEGACY_UNITS.len()
        );
    } else {
        println!(
            "  would stop + disable {} unit names (arieltune-* + aputune-*)",
            SUITE_UNITS.len() + LEGACY_UNITS.len()
        );
    }

    // 3. BIOS smiflash DKMS module + staged driver dirs.
    println!();
    println!("driver: dkms-remove smiflash + drop staged driver dirs...");
    if apply {
        let _ = Command::new("dkms")
            .args(["remove", "smiflash", "--all"])
            .stderr(Stdio::null())
            .status();
        for d in DRIVER_DIRS {
            let _ = std::fs::remove_dir_all(d);
        }
        println!("  dkms remove smiflash --all; removed {DRIVER_DIRS:?}");
    } else {
        println!("  would: dkms remove smiflash --all; rm -rf {DRIVER_DIRS:?}");
    }

    // 4. Binary + alias + compat symlinks.
    println!();
    println!("binary: remove {PREFIX}/bin/{{{}}}", BINARIES.join(","));
    if apply {
        for b in BINARIES {
            let _ = std::fs::remove_file(format!("{PREFIX}/bin/{b}"));
        }
        println!("  removed the binary + `at` + the four compat symlinks");
    }

    // 5. --purge: per-app state dirs.
    if purge {
        println!();
        println!("purge: delete per-app state dirs...");
        for d in STATE_DIRS {
            if apply {
                let _ = std::fs::remove_dir_all(d);
                println!("  rm -rf {d}");
            } else {
                println!("  would rm -rf {d}");
            }
        }
    } else {
        println!();
        println!("state kept (no --purge): /var/lib/{{arieltune,aputune,memtune,biostune}} -- a reinstall picks up your saved profiles/timings.");
    }

    // 6. Persistent hardware-state report (what outlives the app).
    println!();
    println!("Persistent hardware state that outlives the app:");
    println!(
        "  * GPU boot-pin / clock  -> {}",
        if revert_hw {
            "released above (revert-hw)"
        } else {
            "still set; revert: sudo arieltune apu gpu unforce && sudo arieltune apu gpu unforce-vid"
        }
    );
    println!(
        "  * CPU OC / curve        -> {}",
        if revert_hw {
            "restored above (revert-hw)"
        } else {
            "still set; revert: sudo arieltune apu cpu restore"
        }
    );
    println!("  * GDDR6 memory timings  -> persist in CMOS until overwritten; revert: restore a stock backup (sudo arieltune mem restore <backup> --write) then reboot");
    println!("  * BIOS / EFI / APCB     -> persist in NVRAM/flash; revert: sudo arieltune bios oem-clear, or an NVRAM clear / reflash for deeper changes");
    if !apply {
        println!();
        println!("dry-run only. Re-run with --apply (as root) to actuate.");
    } else {
        println!();
        println!("[ok] uninstall complete.");
    }
    Ok(())
}
