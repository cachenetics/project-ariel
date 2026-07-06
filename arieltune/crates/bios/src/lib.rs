// SPDX-License-Identifier: GPL-2.0-only
//! BIOS tab -- the BC-250 AMD CBS + OEM Setup surface, browse + edit + CLI.
//!
//! Ported from the standalone `biostune`: `screen` is the interactive
//! categorized browser/editor (now an [`arieltune_tui_kit::Screen`] the suite
//! shell drives); `cli` is the structured projection + the write paths
//! (categories/dump/get/set/apcb/oem-set/effect/driver/doctor). Both read the
//! same embedded catalogue through one parser, so the human view and the machine
//! view never drift. All BIOS-write safety (board/BIOS-version gates, per-setting
//! risk, --force, the SMM-dirty store guard) is preserved verbatim.
//!
//! Board detection is delegated to [`bc250_board`] (the lifted
//! Compat/WriteClass/Gate); there is no local copy of the DMI detect.

mod apcb;
mod boot;
mod catalog;
mod descriptions;
mod dirty;
mod effect;
mod efivar;
mod flash;
mod gate;
mod nvram;
mod oem;
mod risk;
mod smm;

pub mod cli;
pub mod screen;

pub use cli::{Cmd, Global};
pub use screen::BiosScreen;

/// Build the BIOS screen (loads the embedded catalogue + live EFI values).
///
/// On a live board with the smiflash driver loaded, read OEM `Setup` values
/// straight from flash via SMM so the tab shows their REAL current values
/// (efivarfs hides the boot-service-only Setup var).
pub fn screen() -> BiosScreen {
    let _ = smm::load(); // best-effort: enable OEM editing if the module is installed
    let mut e = efivar::EfiVars::read();
    if smm::Smm::available() {
        if let Ok(img) = smm::Smm::open().and_then(|s| s.read(0, 0x20000)) {
            e = efivar::EfiVars::from_nvram(&img);
        }
    }
    BiosScreen::load(e)
}

/// Run a BIOS CLI subcommand (the non-TUI projection + write paths).
pub fn run_cli(global: Global, cmd: Cmd) -> anyhow::Result<()> {
    cli::run(global, cmd)
}
