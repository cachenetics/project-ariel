// SPDX-License-Identifier: GPL-2.0-only
//! Write preflight — the single choke point every write goes through before it
//! touches the board. Composes two independent checks:
//!
//!   1. board/BIOS-version compatibility ([`board`]) — are the catalogue offsets
//!      valid against the firmware actually running, for this write mechanism.
//!   2. per-setting risk ([`risk`]) — is this specific setting brick/one-way/caution.
//!
//! Either check can downgrade to a warning, demand `--force`, or refuse outright.
//! Off-board (`--image FILE`) writes pass `compat = None` to skip the board gate.

use anyhow::{bail, Result};

use crate::catalog::Setting;
use crate::risk;
use bc250_board::{Compat, Gate, WriteClass};

/// Run both gates for a batch of settings about to be written via `class`.
/// Prints warnings to stderr; bails with guidance if a gate is unsatisfied.
/// `compat = None` skips the board gate (off-board file edits).
pub fn preflight(
    compat: Option<Compat>,
    class: WriteClass,
    settings: &[&Setting],
    force: bool,
) -> Result<()> {
    if let Some(compat) = compat {
        match compat.gate(class) {
            Gate::Allow => {}
            Gate::Warn(m) => eprintln!("warning: {m}"),
            Gate::RequireForce(m) => {
                if !force {
                    bail!("{m}");
                }
                eprintln!("warning (forced): {m}");
            }
            Gate::Refuse(m) => bail!("{m}"),
        }
    }
    for s in settings {
        let a = risk::assess(s);
        if a.risk.requires_force() {
            if !force {
                bail!(
                    "{}: {} [{}]\n  recovery: {}\n  re-run with --force if you understand the risk.",
                    s.name,
                    a.reason,
                    a.risk.tag(),
                    a.recovery
                );
            }
            eprintln!(
                "warning (forced) {} [{}]: {} — {}",
                s.name,
                a.risk.tag(),
                a.reason,
                a.recovery
            );
        }
    }
    Ok(())
}

/// Gate a bare flash-class write that has no per-setting context (e.g. an APCB
/// token flip). Board gate only.
pub fn preflight_flash(compat: Option<Compat>, force: bool) -> Result<()> {
    preflight(compat, WriteClass::FlashClass, &[], force)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bc250_board::Compat;

    fn setting(name: &str, cat: &str) -> Setting {
        Setting {
            category: cat.into(),
            name: name.into(),
            offset: 0,
            bits: 8,
            default: None,
            options: vec![(0, "Disabled".into()), (1, "Enabled".into())],
            range: None,
            varstore: "AmdSetup".into(),
        }
    }

    #[test]
    fn safe_setting_on_verified_board_passes() {
        let s = setting("Above 4G Decoding", "Advanced");
        assert!(preflight(Some(Compat::Verified), WriteClass::EfiVar, &[&s], false).is_ok());
    }

    #[test]
    fn brick_setting_needs_force_even_on_verified() {
        let s = setting("Tcl Ctrl", "Umc Common");
        assert!(preflight(Some(Compat::Verified), WriteClass::EfiVar, &[&s], false).is_err());
        assert!(preflight(Some(Compat::Verified), WriteClass::EfiVar, &[&s], true).is_ok());
    }

    #[test]
    fn flash_write_on_unknown_bios_needs_force() {
        let s = setting("Above 4G Decoding", "Advanced");
        assert!(preflight(
            Some(Compat::UnknownBios),
            WriteClass::FlashClass,
            &[&s],
            false
        )
        .is_err());
        assert!(preflight(
            Some(Compat::UnknownBios),
            WriteClass::FlashClass,
            &[&s],
            true
        )
        .is_ok());
    }

    #[test]
    fn non_bc250_refuses_even_with_force() {
        let s = setting("Above 4G Decoding", "Advanced");
        assert!(preflight(Some(Compat::NotBc250), WriteClass::EfiVar, &[&s], true).is_err());
    }

    #[test]
    fn off_board_skips_board_gate() {
        // compat = None: even an unknown board is fine (editing a file, not a board).
        assert!(preflight_flash(None, false).is_ok());
    }
}
