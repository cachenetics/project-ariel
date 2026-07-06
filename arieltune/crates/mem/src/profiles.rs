// SPDX-License-Identifier: GPL-2.0-only
//! Saved timing profiles for the TUI picker.
//!
//! The `p` overlay lets you pick a known-good config and reboot into it in two
//! keystrokes, instead of hand-dialing the timings back. A profile is just a
//! decoded `MemConf`; three sources feed the list:
//!   * named `*.hex` saves in `/var/lib/memtune/` (e.g. `known_good_1928.hex`) —
//!     the deliberate "production" saves you `backup <name>.hex`'d;
//!   * the built-in `recommended` config (the 1750 known-good baseline);
//!   * the `stock` snapshot captured the first time memtune ran.
//!
//! The rotating auto-backups under `backups/` are intentionally NOT listed —
//! they're the safety net for a bad write, not curated profiles.

use crate::config::{MemConf, CONFIG_SIZE};
use crate::tune;

const STATE_DIR: &str = "/var/lib/memtune";

/// One selectable config in the picker.
pub struct Profile {
    /// What the row shows (a save's file stem, or a built-in name).
    pub label: String,
    /// Short origin hint: "saved" / "built-in" / "original".
    pub source: &'static str,
    pub conf: MemConf,
}

/// Build the picker list: named saves first (the point of the feature), then the
/// built-in recommended baseline, then the original stock snapshot if captured.
/// Always returns at least the built-in `recommended`, so the list is never empty.
pub fn discover() -> Vec<Profile> {
    let mut out = Vec::new();

    // Named `*.hex` saves sitting directly in the state dir (read_dir does not
    // recurse, so the `backups/` auto-dumps are excluded). Sorted by name so the
    // ordering is stable across launches.
    if let Ok(rd) = std::fs::read_dir(STATE_DIR) {
        let mut named: Vec<Profile> = rd
            .flatten()
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("hex"))
            .filter_map(|e| {
                let stem = e.path().file_stem()?.to_string_lossy().into_owned();
                let hex = std::fs::read_to_string(e.path()).ok()?;
                let conf = MemConf::from_hex(&hex)?;
                Some(Profile {
                    label: stem,
                    source: "saved",
                    conf,
                })
            })
            .collect();
        named.sort_by(|a, b| a.label.cmp(&b.label));
        out.append(&mut named);
    }

    // Built-in recommended (the 1750 known-good baseline).
    let mut rec = MemConf::from_bytes([0u8; CONFIG_SIZE]);
    rec.apply_recommended();
    out.push(Profile {
        label: "recommended (1750)".into(),
        source: "built-in",
        conf: rec,
    });

    // Original timings captured on first run, if we have them.
    if let Some(s) = tune::stock() {
        out.push(Profile {
            label: "stock (original)".into(),
            source: "original",
            conf: s,
        });
    }

    out
}
