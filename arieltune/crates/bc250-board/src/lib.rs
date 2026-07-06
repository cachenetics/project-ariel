// SPDX-License-Identifier: GPL-2.0-only
//! bc250-board — the one place BC-250 board detection lives.
//!
//! This crate merges the three old per-tool board checks into a single
//! APU-family-first seam:
//!   * aputune's PCI 1002:13fe scan (now `ariel_hal::ariel_apu_present`),
//!   * memtune's `is_bc250`,
//!   * biostune's DMI product/board classification (lifted below).
//!
//! Detection is two-stage and deliberately ordered:
//!   1. the APU must be present (`ariel_hal::ariel_apu_present`) — the silicon;
//!   2. the DMI identity must confirm the BC-250 board — the board gate.
//!
//! The DMI `Compat` verdict (from biostune) is retained: it is what the
//! brick-class BIOS write paths gate on. Board detection here says "this is a
//! BC-250"; `Compat` says "how far we trust the catalogue offsets against THIS
//! unit's firmware".

use std::fs;

// ---------------------------------------------------------------------------
// APU-family-first detection seam (design s.8).
// ---------------------------------------------------------------------------

/// The APU silicon family under the board.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ApuFamily {
    /// AMD Cyan Skillfish (Ariel), PCI 1002:13fe — the BC-250's APU.
    CyanSkillfish,
}

/// The carrier board the APU is soldered to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BoardProfile {
    /// ASRock BC-250 blade.
    Bc250,
    // FUTURE: `Ps5` — the same Cyan Skillfish APU appears on the PS5 APU carrier.
    // A distinct BoardProfile so board-specific offsets/actuation can diverge.
    // Not implemented in M2.
}

/// A detected APU + board pair.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ApuBoard {
    pub family: ApuFamily,
    pub profile: BoardProfile,
}

/// Detect the APU + board.
///
/// `Some(Bc250)` iff the Ariel APU is present AND the board is a BC-250:
///   1. require `ariel_hal::ariel_apu_present()` — no APU, no board;
///   2. confirm the board via DMI (product/board name). The DMI `Compat` verdict
///      is the board gate: any BC-250 verdict (Verified / ProbableAsrock /
///      UnknownBios) confirms the board; `NotBc250` on READABLE DMI means a
///      genuinely different board -> `None`.
///
/// FALLBACK: if DMI is unreadable (no `/sys/class/dmi/id` — e.g. a minimal or
/// virtualised environment), the APU id is authoritative and we return
/// `Some(Bc250)`. The 1002:13fe APU only ships on the BC-250 today, so an
/// unreadable DMI must not veto a positive APU probe.
pub fn detect() -> Option<ApuBoard> {
    if !ariel_hal::ariel_apu_present() {
        return None;
    }
    let confirmed = match classify_running() {
        // DMI readable and says NOT a BC-250 board -> a different carrier.
        Compat::NotBc250 if dmi_readable() => false,
        // Either a BC-250 DMI verdict, or DMI unreadable (fall back to the APU).
        _ => true,
    };
    if confirmed {
        Some(ApuBoard {
            family: ApuFamily::CyanSkillfish,
            profile: BoardProfile::Bc250,
        })
    } else {
        None
    }
}

/// Convenience: is this host a BC-250?
pub fn is_bc250() -> bool {
    detect().is_some()
}

/// Is the Ariel APU (PCI 1002:13fe) present, regardless of the DMI board verdict?
///
/// This is the silicon-only half of [`detect`]: it does NOT require the DMI
/// board name to match. The 1002:13fe APU only ships on the BC-250 today, so a
/// positive result is a sufficient gate for actuators whose only risk on a
/// non-BC-250 host is touching that host's hardware — it deliberately does NOT
/// veto a genuine BC-250 whose DMI identity has been rebranded/modded.
pub fn apu_present() -> bool {
    ariel_hal::ariel_apu_present()
}

// ---------------------------------------------------------------------------
// DMI board identity + BIOS write-safety verdict (lifted from biostune).
// ---------------------------------------------------------------------------

/// How far we trust the catalogue offsets against the running firmware.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Compat {
    /// BC-250 running a BIOS version whose offsets we proved (P2/P3/P5, incl. the
    /// clv / chipsetmenu mods that share those version strings).
    Verified,
    /// BC-250 running an ASRock-style `P#.##` version we have not explicitly
    /// characterised. Same board + same versioning scheme -> offsets almost
    /// certainly match, but unproven.
    ProbableAsrock,
    /// A BC-250 board but an unrecognised BIOS (non-ASRock scheme / heavily
    /// modded). Offsets may not match — brick-class writes are unsafe.
    UnknownBios,
    /// Not a BC-250 at all. The catalogue is meaningless here; never write.
    NotBc250,
}

/// Which write mechanism, for gating purposes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WriteClass {
    /// `set` — a firmware-native `SetVariable` into AmdSetup. The firmware
    /// validates it and an NVRAM clear recovers it: lower blast radius.
    EfiVar,
    /// `oem-set` (SMM NVAR append) / `apcb` (flashrom) — raw writes at a catalogue
    /// offset straight into SPI flash. A wrong offset here is brick-class.
    FlashClass,
}

/// The outcome of a gate check.
pub enum Gate {
    /// Proceed silently.
    Allow,
    /// Proceed, but print this warning first.
    Warn(String),
    /// Refuse unless the operator passed `--force`; this is the message.
    RequireForce(String),
    /// Refuse outright — `--force` does not override (wrong board entirely).
    Refuse(String),
}

/// The detected DMI identity.
pub struct Board {
    pub product: String,
    pub bios_vendor: String,
    pub bios_version: String,
    pub compat: Compat,
}

const VERIFIED: &[&str] = &["P2.00", "P3.00", "P5.00"];

fn dmi(field: &str) -> String {
    fs::read_to_string(format!("/sys/class/dmi/id/{field}"))
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// True if the DMI identity is readable at all (the id node exists). Used to
/// distinguish "DMI says a different board" from "DMI is simply unavailable".
fn dmi_readable() -> bool {
    fs::metadata("/sys/class/dmi/id/product_name").is_ok()
        || fs::metadata("/sys/class/dmi/id/board_name").is_ok()
}

fn is_bc250_dmi(product: &str, board: &str) -> bool {
    let hit = |s: &str| {
        let u = s.to_uppercase();
        u.contains("BC-250") || u.contains("BC250") || u.contains("4U12G")
    };
    hit(product) || hit(board)
}

/// An ASRock-style version string like `P5.00` / `P10.02`: 'P', digits, '.', digits.
fn is_asrock_version(v: &str) -> bool {
    let v = v.trim();
    let mut c = v.chars();
    if !matches!(c.next(), Some('P') | Some('p')) {
        return false;
    }
    let rest: String = c.collect();
    let Some((maj, min)) = rest.split_once('.') else {
        return false;
    };
    !maj.is_empty()
        && !min.is_empty()
        && maj.chars().all(|d| d.is_ascii_digit())
        && min.chars().take(2).all(|d| d.is_ascii_digit())
}

pub fn classify(product: &str, board: &str, version: &str) -> Compat {
    if !is_bc250_dmi(product, board) {
        return Compat::NotBc250;
    }
    let vu = version.to_uppercase();
    if VERIFIED.iter().any(|v| vu.contains(v)) {
        Compat::Verified
    } else if is_asrock_version(version) {
        Compat::ProbableAsrock
    } else {
        Compat::UnknownBios
    }
}

/// Read DMI and classify.
pub fn detect_bios() -> Board {
    let product = dmi("product_name");
    let board = dmi("board_name");
    let bios_version = dmi("bios_version");
    let bios_vendor = dmi("bios_vendor");
    let compat = classify(&product, &board, &bios_version);
    Board {
        product,
        bios_vendor,
        bios_version,
        compat,
    }
}

/// Classify the running host's DMI directly (the board gate for [`detect`]).
pub fn classify_running() -> Compat {
    detect_bios().compat
}

impl Compat {
    pub fn label(self) -> &'static str {
        match self {
            Compat::Verified => "verified (offsets proven on this BIOS)",
            Compat::ProbableAsrock => "probable (ASRock BC-250, version not explicitly tested)",
            Compat::UnknownBios => "UNKNOWN BIOS (offsets may not match this firmware)",
            Compat::NotBc250 => "NOT a BC-250 board",
        }
    }

    /// Gate a write of the given class against this compatibility verdict.
    pub fn gate(self, class: WriteClass) -> Gate {
        use Compat::*;
        use WriteClass::*;
        match (self, class) {
            (Verified, _) => Gate::Allow,

            (ProbableAsrock, EfiVar) => Gate::Allow,
            (ProbableAsrock, FlashClass) => Gate::Warn(
                "BIOS version not one of the explicitly-tested P2/P3/P5 — offsets are \
                 near-certain to match (same ASRock BC-250 IFR family) but unverified. \
                 Have a flash backup before applying."
                    .into(),
            ),

            (UnknownBios, EfiVar) => Gate::RequireForce(
                "unrecognised BIOS: catalogue offsets are unverified against this firmware. \
                 An AmdSetup write is NVRAM-clear recoverable, but re-run with --force to \
                 accept the risk."
                    .into(),
            ),
            (UnknownBios, FlashClass) => Gate::RequireForce(
                "unrecognised BIOS: a flash-class write (SMM/flashrom) at an unverified \
                 offset is BRICK-CLASS on this firmware. Only proceed with --force if you \
                 have an external SPI programmer to recover."
                    .into(),
            ),

            (NotBc250, _) => Gate::Refuse(
                "this is not a BC-250 board — the BC-250 catalogue offsets are meaningless \
                 here and a write could brick it. Refusing."
                    .into(),
            ),
        }
    }
}

// hwmon carrier fan/temp snapshot: TODO -- lift the nct6687/nct6686 read from
// aputune telemetry.rs once the APU tab lands. Left out of M2 to keep this crate
// dependency-light (it would drag in the full telemetry surface).

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verified_versions() {
        assert_eq!(classify("ASRock BC-250", "", "P5.00"), Compat::Verified);
        assert_eq!(classify("BC-250", "", "P3.00"), Compat::Verified);
        // clv / chipsetmenu mods report the same base version string
        assert_eq!(classify("BC-250", "", "P5.00_clv"), Compat::Verified);
    }

    #[test]
    fn probable_vs_unknown() {
        assert_eq!(classify("BC-250", "", "P7.02"), Compat::ProbableAsrock);
        assert_eq!(classify("BC-250", "", "P10.00"), Compat::ProbableAsrock);
        assert_eq!(classify("BC-250", "", "N33_L1.05"), Compat::UnknownBios);
        assert_eq!(classify("BC-250", "", ""), Compat::UnknownBios);
    }

    #[test]
    fn non_bc250_is_refused() {
        assert_eq!(
            classify("MZ73-LM0-000", "MZ73-LM0-000", "R07_F33"),
            Compat::NotBc250
        );
        // even with a P#.## version, a non-BC-250 product is NotBc250
        assert_eq!(classify("X570 Taichi", "", "P5.00"), Compat::NotBc250);
    }

    #[test]
    fn board_name_fallback() {
        // some units report the model only in board_name
        assert_eq!(
            classify("To be filled by O.E.M.", "BC-250", "P5.00"),
            Compat::Verified
        );
    }

    #[test]
    fn asrock_version_parser() {
        assert!(is_asrock_version("P5.00"));
        assert!(is_asrock_version("P10.2"));
        assert!(!is_asrock_version("R07_F33"));
        assert!(!is_asrock_version("5.00"));
        assert!(!is_asrock_version("P.00"));
    }

    #[test]
    fn gate_matrix() {
        assert!(matches!(
            Compat::Verified.gate(WriteClass::FlashClass),
            Gate::Allow
        ));
        assert!(matches!(
            Compat::ProbableAsrock.gate(WriteClass::EfiVar),
            Gate::Allow
        ));
        assert!(matches!(
            Compat::ProbableAsrock.gate(WriteClass::FlashClass),
            Gate::Warn(_)
        ));
        assert!(matches!(
            Compat::UnknownBios.gate(WriteClass::FlashClass),
            Gate::RequireForce(_)
        ));
        assert!(matches!(
            Compat::NotBc250.gate(WriteClass::EfiVar),
            Gate::Refuse(_)
        ));
    }
}
