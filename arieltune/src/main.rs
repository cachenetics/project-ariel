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
    about = "BC-250 tuning suite (WIKI | BIOS | APU | MEM)"
)]
struct Cli {
    /// Open the TUI at this tab (wiki | bios | apu | mem). Overrides the config default.
    #[arg(long, global = true)]
    tab: Option<String>,

    #[command(subcommand)]
    cmd: Option<Top>,
}

#[derive(Subcommand)]
enum Top {
    /// Launch the tabbed TUI (same as bare `arieltune`).
    Tui,
    /// APU liberation + tuner (was `aputune`). No subcommand -> TUI at the APU tab.
    Apu {
        #[command(subcommand)]
        cmd: Option<apu::Cmd>,
    },
    /// GDDR6 memory-timing tuner (was `memtune`). No subcommand -> TUI at the MEM tab.
    Mem {
        #[command(subcommand)]
        cmd: Option<mem::Cmd>,
    },
    /// BIOS / CBS surface (was `biostune`). No subcommand -> TUI at the BIOS tab.
    Bios {
        #[command(flatten)]
        global: bios::Global,
        #[command(subcommand)]
        cmd: Option<bios::Cmd>,
    },
    /// Knowledge manual (was `wikitune`). No subcommand -> TUI at the WIKI tab.
    Wiki {
        #[command(subcommand)]
        cmd: Option<wiki::Cmd>,
    },
    /// Migrate a box from the four standalone tools onto the suite (safe;
    /// disables legacy aputune-* units, keeps per-app config). Dry-run by default.
    Migrate {
        /// Actually stop/disable legacy units + create the runtime dir (needs root).
        #[arg(long)]
        apply: bool,
    },
    /// Remove the suite: stop/disable all units, dkms-remove smiflash, remove the
    /// binary + symlinks. Dry-run by default. `--revert-hw` restores stock hardware
    /// first; `--purge` also deletes saved state. Needs root to actuate.
    Uninstall {
        /// Actually perform the removal (otherwise dry-run).
        #[arg(long)]
        apply: bool,
        /// Also delete per-app state (/var/lib/{arieltune,aputune,memtune,biostune}).
        #[arg(long)]
        purge: bool,
        /// Restore stock hardware (release GPU clock/voltage, restore CPU) first.
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
