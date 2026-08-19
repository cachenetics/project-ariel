// SPDX-License-Identifier: GPL-2.0-only
//! arieltune -- unified BC-250 tuning suite.
//!
//! Bare `arieltune` launches the tabbed TUI at the default tab (WIKI, or the
//! configured `default_tab`). `arieltune <app>` with no further subcommand launches
//! the TUI focused on that app's tab; `arieltune <app> <subcommand>` runs that app's
//! CLI. `--tab <name>` overrides the launch tab.

mod config;
mod migrate;
mod shell;
mod tabs;
mod uninstall;

use anyhow::Result;
use clap::{Parser, Subcommand};

use config::Config;
use shell::Shell;

#[derive(Parser)]
#[command(
    name = "arieltune",
    version,
    about = "BC-250 tuning suite: WIKI (manual), BIOS (CBS/OEM Setup), APU (liberation+clocks), MEM (GDDR6 timings).",
    long_about = "arieltune is the unified tuner for the ASRock BC-250 (Cyan Skillfish, gfx1013, PCI 1002:13FE).\n\
        \n\
        Tabs (each is also a CLI namespace): apu, mem, bios, wiki.\n\
        Bare `arieltune` or `arieltune <tab>` opens the TUI; `arieltune <tab> <subcommand>` runs a CLI action.\n\
        \n\
        Safety model, common across tabs: most hardware-writing commands PREVIEW by default and only act\n\
        with an explicit flag (--run for apu build/liberate, --write for mem/bios, --apply for oem-set,\n\
        --arm for oem-stage). Writes to SMU clocks/CMOS/EFI/SPI flash need root and often a REBOOT.\n\
        SMU rule: arieltune must be the ONLY actuator on the single MP1 mailbox; a second driver racing it\n\
        cripples clocks or wedges the GPU. Agents: prefer --json where offered.\n\
        \n\
        Migrate/uninstall are suite-level and dry-run by default (need --apply)."
)]
struct Cli {
    /// Open the TUI at this tab: wiki, bios, apu, or mem. Overrides the config default_tab.
    #[arg(long, global = true)]
    tab: Option<String>,

    #[command(subcommand)]
    cmd: Option<Top>,
}

#[derive(Subcommand)]
enum Top {
    /// Launch the tabbed TUI (same as bare `arieltune`).
    Tui,
    /// APU tab: liberation, CU routing, GPU/CPU clocks and voltage. Bare `apu` opens the TUI tab.
    ///
    /// `arieltune apu <subcommand>` runs the CLI. Hardware writes need root; kernel/CU changes need a
    /// REBOOT; gpu/cpu clock and voltage writes act live via the SMU. build/liberate PREVIEW by default
    /// (--run to execute). SMU RULE: arieltune must be the ONLY actuator on the single MP1 mailbox; a
    /// second SMU driver racing it cripples clocks or wedges the GPU.
    Apu {
        #[command(subcommand)]
        cmd: Option<apu::Cmd>,
    },
    /// MEM tab: GDDR6 memory-timing tuner. Bare `mem` opens the TUI tab.
    ///
    /// `arieltune mem <subcommand>` runs the CLI. Timing writes STAGE into CMOS and are trained by ABL
    /// on the NEXT boot (not live). Previews unless --write (auto-backs-up first); needs root + /dev/port.
    /// A bad config can fail to train, so keep a known-good backup and re-check the signature after reboot.
    Mem {
        #[command(subcommand)]
        cmd: Option<mem::Cmd>,
    },
    /// BIOS tab: AMD CBS + OEM Setup surface. Bare `bios` opens the TUI tab.
    ///
    /// `arieltune bios <subcommand>` runs the CLI. Reads are safe; writes touch EFI vars / SPI flash /
    /// NVRAM, apply on reboot, and are DANGEROUS (a wrong setting can fail to POST, needing a CMOS/NVRAM
    /// clear or reflash). Previews by default (--write/--apply/--arm); needs root; gated to known firmware.
    Bios {
        #[command(flatten)]
        global: bios::Global,
        #[command(subcommand)]
        cmd: Option<bios::Cmd>,
    },
    /// WIKI tab: the embedded BC-250 manual. Bare `wiki` opens the TUI tab.
    ///
    /// `arieltune wiki <subcommand>` reads the manual. Fully read-only; use --json or `export` for
    /// machine-readable records (RAG ingestion / tooling).
    Wiki {
        #[command(subcommand)]
        cmd: Option<wiki::Cmd>,
    },
    /// Migrate a box from the four standalone tools onto the suite. Dry-run unless --apply.
    ///
    /// Safe: disables the legacy aputune-*/memtune/biostune units and keeps each app's config.
    /// Preview only until --apply (which stops/disables units and creates the runtime dir; needs root).
    Migrate {
        /// Actually stop/disable legacy units + create the runtime dir (needs root). Default: preview.
        #[arg(long)]
        apply: bool,
    },
    /// Remove the suite (units, smiflash DKMS module, binary, symlinks). Dry-run unless --apply.
    ///
    /// Preview only until --apply (needs root). --revert-hw restores stock hardware (releases GPU
    /// clock/voltage, restores CPU) BEFORE removal; --purge also deletes saved state.
    Uninstall {
        /// Actually perform the removal. Default: preview only.
        #[arg(long)]
        apply: bool,
        /// Also delete per-app state (/var/lib/{arieltune,aputune,memtune,biostune}). DESTRUCTIVE.
        #[arg(long)]
        purge: bool,
        /// Restore stock hardware (release GPU clock/voltage, restore CPU) before removal.
        #[arg(long = "revert-hw")]
        revert_hw: bool,
    },
}

/// Support the legacy binary names via argv[0]: when invoked through a compat
/// symlink (`aputune`/`memtune`/`biostune`/`wikitune` -> arieltune), inject the
/// matching subcommand namespace so old commands AND systemd units that call the
/// old names keep working unchanged. `at` and `arieltune` pass through.
fn compat_args() -> Vec<String> {
    inject_compat_namespace(std::env::args().collect())
}

/// Pure core of [`compat_args`]: given the full argv, if argv[0]'s basename is a
/// legacy tool name, insert its subcommand namespace at position 1.
fn inject_compat_namespace(mut args: Vec<String>) -> Vec<String> {
    let base = args
        .first()
        .and_then(|p| std::path::Path::new(p).file_name())
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string();
    let ns = match base.as_str() {
        "aputune" => Some("apu"),
        "memtune" => Some("mem"),
        "biostune" => Some("bios"),
        "wikitune" => Some("wiki"),
        _ => None,
    };
    if let Some(ns) = ns {
        args.insert(1, ns.to_string());
    }
    args
}

fn main() {
    // One-line error framing matching the tune tools (`error: ...`), rather than
    // anyhow's multi-line `Error:` / `Caused by:` Termination -- keeps every tab's
    // CLI output consistent with the standalone binaries it replaces.
    if let Err(e) = real_main() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn real_main() -> Result<()> {
    // Match the four apps: let SIGPIPE terminate normally so piping into `head` etc.
    // does not surface a broken-pipe panic.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

    let cli = Cli::parse_from(compat_args());

    // A namespaced subcommand WITH args is a CLI invocation; without args, it is a
    // launch-to-tab request.
    let launch_tab: Option<&str> = match cli.cmd {
        Some(Top::Migrate { apply }) => return migrate::run(apply),
        Some(Top::Uninstall {
            apply,
            purge,
            revert_hw,
        }) => return uninstall::run(apply, purge, revert_hw),
        // APU CLI is wired: a subcommand runs it; bare `arieltune apu` opens the tab.
        Some(Top::Apu { cmd: Some(c) }) => return apu::run_cli(c),
        Some(Top::Apu { cmd: None }) => Some("apu"),
        // MEM CLI is wired: a subcommand runs it; bare `arieltune mem` opens the tab.
        Some(Top::Mem { cmd: Some(c) }) => return mem::run_cli(c),
        Some(Top::Mem { cmd: None }) => Some("mem"),
        // BIOS CLI is wired: a subcommand runs it; bare `arieltune bios` opens the tab.
        Some(Top::Bios {
            global,
            cmd: Some(c),
        }) => return bios::run_cli(global, c),
        Some(Top::Bios { cmd: None, .. }) => Some("bios"),
        // WIKI CLI is wired: a subcommand runs it; bare `arieltune wiki` opens the tab.
        Some(Top::Wiki { cmd: Some(c) }) => return wiki::run_cli(c),
        Some(Top::Wiki { cmd: None }) => Some("wiki"),
        Some(Top::Tui) | None => None,
    };

    // `--tab` wins over the bare subcommand's implied tab.
    let explicit = cli.tab.as_deref().or(launch_tab);

    let cfg = Config::load();
    let default = cfg.resolve_launch_tab(explicit);

    Shell::new(tabs::screens(), default).run()
}

#[cfg(test)]
mod tests {
    use super::inject_compat_namespace;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn legacy_names_inject_namespace() {
        assert_eq!(
            inject_compat_namespace(v(&["/usr/local/bin/aputune", "gpu", "apply-boot"])),
            v(&["/usr/local/bin/aputune", "apu", "gpu", "apply-boot"])
        );
        assert_eq!(
            inject_compat_namespace(v(&["biostune", "--image", "x.bin", "apcb", "status"])),
            v(&["biostune", "bios", "--image", "x.bin", "apcb", "status"])
        );
        assert_eq!(
            inject_compat_namespace(v(&["memtune"])),
            v(&["memtune", "mem"])
        );
    }

    #[test]
    fn native_names_pass_through() {
        assert_eq!(
            inject_compat_namespace(v(&["/usr/local/bin/arieltune", "apu", "doctor"])),
            v(&["/usr/local/bin/arieltune", "apu", "doctor"])
        );
        assert_eq!(
            inject_compat_namespace(v(&["at", "mem"])),
            v(&["at", "mem"])
        );
    }
}
