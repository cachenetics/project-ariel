// SPDX-License-Identifier: GPL-2.0-only
//! MEM tab -- the BC-250 GDDR6 memory-timing tuner, browse + edit + bench + CLI.
//!
//! Ported from the standalone `memtune`: `screen` is the interactive Tune view
//! (live measured bandwidth/random/latency readout, editable timings table, edit
//! history, saved-timings picker, live UMC / system-memtest overlays) -- now an
//! [`arieltune_tui_kit::Screen`] the suite shell drives; `cli` is the non-TUI
//! surface (dump/set/recommended/bench/backup/restore/doctor/umc/memtest/...).
//!
//! All CMOS/timing safety is preserved verbatim: writes stage into CMOS and take
//! effect on the NEXT boot (ABL trains them), a confirmed write reboots for the
//! operator, and nothing touches CMOS until that confirm.
//!
//! The GPU bench runs on the ONE shared Vulkan device owned by [`ariel_compute`]
//! (its first real consumer): the bench holds the process-wide compute lock for
//! its whole session, so a MEM bench and an APU KAT can never drive the GPU at
//! once.

mod bench;
mod cmos;
mod config;
mod fields;
mod metrics;
mod profiles;
mod sysmem;
mod tune;
mod umc;

pub mod cli;
pub mod screen;

pub use cli::Cmd;
pub use screen::MemScreen;

/// Build the MEM screen (loads the live CMOS config + the stock/history state).
pub fn screen() -> MemScreen {
    // Sweep orphaned state files from removed features (best-effort), as memtune
    // did on every launch.
    tune::cleanup_stale();
    MemScreen::new()
}

/// Run a MEM CLI subcommand (the non-TUI projection + CMOS write paths).
pub fn run_cli(cmd: Cmd) -> anyhow::Result<()> {
    cli::run(cmd)
}
