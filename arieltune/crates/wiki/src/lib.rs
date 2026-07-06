// SPDX-License-Identifier: GPL-2.0-only
//! WIKI tab -- the ASRock BC-250 OEM System Manual, browse + CLI.
//!
//! Ported from the standalone `wikitune`: `screen` is the interactive two-pane
//! browser (now an [`arieltune_tui_kit::Screen`] the suite shell drives); `cli` is
//! the structured projection of the same embedded manual (search/get/export/...).
//! Both read one source of truth through one parser, so the human view and the
//! machine view never drift.

pub mod cli;
pub mod screen;

pub use cli::Cmd;
pub use screen::WikiScreen;

/// Build the WIKI screen (loads the embedded manual).
pub fn screen() -> WikiScreen {
    WikiScreen::new(bc250_catalog::manual_book::load())
}

/// Run a WIKI CLI subcommand (the non-TUI projection).
pub fn run_cli(cmd: Cmd) -> anyhow::Result<()> {
    cli::run(cmd)
}
