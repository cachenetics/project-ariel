// SPDX-License-Identifier: GPL-2.0-only
//! APU tab -- the BC-250 APU liberation + CPU/GPU/CU tuner, browse + edit + KAT + CLI.
//!
//! Ported from the standalone `aputune`: `screen` is the interactive four-panel
//! control center (system card / CPU OC / GPU clock / CU routing, live SMU +
//! carrier telemetry, draft-then-apply writes) -- now an
//! [`arieltune_tui_kit::Screen`] the suite shell drives; `cli` is the non-TUI
//! surface (patches/cu/gpu/cpu/profile/build/liberate/doctor).
//!
//! SMU access is shared, not duplicated: the queue-0 GPU PMFW mailbox
//! ([`ariel_smu::smu::Smu`]) and the queue-3 CPU-OC mailbox
//! ([`ariel_smu::ocq3::OcQ3`]) were lifted into `ariel-smu` in M2 with every
//! Vid ceiling / brick guard / clamp verbatim; this crate USES them (it does not
//! carry its own copies). The silicon-generic probes (`ariel_apu_present`,
//! `amdgpu_dbg_dir`, `running_kernel`, `SmnAperture`) come from `ariel-hal`.
//!
//! The CU health-test KAT dispatches through [`ariel_compute::with_session`] on
//! the ONE shared Vulkan device while holding the process-wide compute lock, so a
//! MEM bandwidth bench and an APU KAT can never drive the GPU at once.
//!
//! SAFETY BOUNDARY (design R3/R5): the GPU governor, the poke-driven autosleep,
//! and `gpu apply-boot` run FOREVER and are CLI-ONLY -- the TUI never spawns them
//! in-process (that would hang the event loop and, worse, put a second writer on
//! the SMU). The TUI only edits config + (re)starts the systemd unit; the single
//! `arieltune-gpu.service` daemon is the ONLY SMU clock writer.

mod cpu;
mod cu;
mod curoute;
mod cutest;
mod detect;
mod dpm;
mod gpuctl;
mod kbuild;
mod patches;
mod persist;
mod profile;
mod stress;
mod telemetry;

pub mod cli;
pub mod screen;

pub use cli::Cmd;
pub use screen::ApuScreen;

/// The single-writer poke file: the MEM bench WRITES it to hold the top clock
/// during a bench; the APU governor (the ONE SMU clock writer) READS it. File
/// IPC is the single-writer discipline -- an in-process clock pin from the bench
/// would race the governor daemon (the double-writer hazard), so it is NOT done.
pub use dpm::POKE_PATH;
/// The governor's runtime directory. Its existence is the "an arieltune GPU
/// governor is installed" tell the MEM bench keys off before poking.
pub use dpm::RUN_DIR as POKE_DIR;

/// Build the APU screen (opens the queue-3 mailbox if reachable + seeds config).
pub fn screen() -> ApuScreen {
    ApuScreen::new()
}

/// Run an APU CLI subcommand (the non-TUI liberation + tuner surface).
pub fn run_cli(cmd: Cmd) -> anyhow::Result<()> {
    cli::run(cmd)
}
