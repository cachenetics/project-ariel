// SPDX-License-Identifier: GPL-2.0-only
//! Tune-page state: a persistent log of edits and their measured results.
//!
//! The TUI Tune tab lets you edit timings by hand and apply them with a reboot.
//! When you write+reboot, the diff is staged here; on the next launch (after the
//! reboot) it's evaluated — did it train? what bandwidth/latency? — and appended
//! to the history.

use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::cmos;
use crate::config::{MemConf, SIG_LINUX_TOOL};

const STATE_PATH: &str = "/var/lib/memtune/tune.json";

/// Orphaned state files from removed features (the old autotune/guided/search
/// code and its drive helper). Current memtune writes only `tune.json` and
/// `backups/`; sweep these on startup so upgraders don't carry dead files.
const STALE_FILES: &[&str] = &[
    "/var/lib/memtune/guided.json",
    "/var/lib/memtune/results.json",
    "/var/lib/memtune/state.json",
    "/var/lib/memtune/drive.log",
];

/// Best-effort removal of orphaned state files (see `STALE_FILES`). Ignores
/// every error — a missing file or a non-root run just leaves them be.
pub fn cleanup_stale() {
    for f in STALE_FILES {
        let _ = fs::remove_file(f);
    }
}

/// Crash-safe write: temp file -> fsync -> atomic rename -> fsync dir. A hard
/// power-loss mid-write can't corrupt or lose the target (you get either the old
/// or the new file, fully on disk) — the history/stock record must survive a
/// reboot or a CMOS clear.
fn write_durable(path: &str, data: &[u8]) -> Result<()> {
    let p = Path::new(path);
    let dir = p.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(dir)?;
    let tmp = p.with_extension("tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?; // fsync contents + metadata
    }
    fs::rename(&tmp, p)?; // atomic replace
    if let Ok(d) = fs::File::open(dir) {
        let _ = d.sync_all(); // fsync the dir so the rename itself is durable
    }
    Ok(())
}

/// One completed edit and how it turned out.
#[derive(Clone, Serialize, Deserialize)]
pub struct Entry {
    pub desc: String,
    pub trained: bool,
    pub bw: f64,
    /// Random-access read throughput (GB/s) — the timing-sensitive number.
    #[serde(default)]
    pub random: f64,
    pub lat: f64,
    /// Integrity-check mismatches (>0 = trained but unstable).
    #[serde(default)]
    pub errors: u64,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct Pending {
    changes: Vec<(String, u32, u32)>,
}

#[derive(Default, Serialize, Deserialize)]
struct State {
    pending: Option<Pending>,
    history: Vec<Entry>,
    /// The original timings, captured (as hex) the first time memtune ran, so the
    /// user can always revert to where they started — never overwritten.
    #[serde(default)]
    stock: Option<String>,
}

fn load() -> State {
    std::fs::read_to_string(STATE_PATH)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save(s: &State) -> Result<()> {
    write_durable(STATE_PATH, serde_json::to_string_pretty(s)?.as_bytes())
}

fn describe(changes: &[(String, u32, u32)]) -> String {
    match changes {
        [] => "(no change)".into(),
        [(n, a, b)] => format!("{n} {a}->{b}"),
        [(n, a, b), rest @ ..] => format!("{n} {a}->{b} +{} more", rest.len()),
    }
}

/// Stage the diff just written to CMOS, to be evaluated after the reboot.
pub fn stage(changes: Vec<(String, u32, u32)>) -> Result<()> {
    let mut st = load();
    st.pending = Some(Pending { changes });
    save(&st)
}

/// Is there a staged edit that has already been applied (the box rebooted, so the
/// firmware flipped the signature away from the pending tag)?
///
/// M6: returns `Option<bool>` so a `/dev/port` read failure is NOT silently
/// treated as "applied". The old code substituted `[0u8; 28]` on a read error,
/// whose signature != SIG_LINUX_TOOL, which misreported a not-yet-applied (or
/// unreadable) state as applied and could trigger a bogus post-reboot eval.
///   * `None`        — nothing staged, OR the CMOS read failed (state unknown)
///   * `Some(true)`  — staged AND the firmware flipped the signature (applied)
///   * `Some(false)` — staged but the tool signature is still present (not applied)
pub fn pending_applied() -> Option<bool> {
    let has_pending = load().pending.is_some();
    // A read failure is "unknown", never "applied" — do not substitute zeros.
    classify_applied(has_pending, cmos::read_config().ok())
}

/// Pure decision for [`pending_applied`], split out so the read-failure path is
/// unit-testable without `/dev/port`. `raw` is `None` when the CMOS read failed.
fn classify_applied(has_pending: bool, raw: Option<[u8; 28]>) -> Option<bool> {
    if !has_pending {
        return None; // nothing staged
    }
    let raw = raw?; // read failed -> unknown, NOT "applied"
    Some(MemConf::from_bytes(raw).signature() != SIG_LINUX_TOOL)
}

/// Record the result of the staged edit and append it to the history (newest
/// first). No-op if nothing is staged.
pub fn record(trained: bool, bw: f64, random: f64, lat: f64, errors: u64) -> Result<()> {
    let mut st = load();
    let Some(p) = st.pending.take() else {
        return Ok(());
    };
    st.history.insert(
        0,
        Entry {
            desc: describe(&p.changes),
            trained,
            bw,
            random,
            lat,
            errors,
        },
    );
    save(&st)
}

pub fn history() -> Vec<Entry> {
    load().history
}

/// Delete one history entry by index (newest = 0). No-op if out of range.
pub fn delete(index: usize) -> Result<()> {
    let mut st = load();
    if index < st.history.len() {
        st.history.remove(index);
    }
    save(&st)
}

/// Clear the whole history.
pub fn clear() -> Result<()> {
    let mut st = load();
    st.history.clear();
    save(&st)
}

/// Record the original config the first time the app sees it, so it's never lost.
/// No-op once a stock snapshot exists.
pub fn ensure_stock(conf: &MemConf) -> Result<()> {
    let mut st = load();
    if st.stock.is_none() {
        st.stock = Some(conf.to_hex());
        save(&st)?;
    }
    Ok(())
}

/// The saved original config, if one has been captured.
pub fn stock() -> Option<MemConf> {
    MemConf::from_hex(&load().stock?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SIG_ABL;

    #[test]
    fn describe_one_and_many() {
        assert_eq!(
            describe(&[("tREF".into(), 9975, 10473)]),
            "tREF 9975->10473"
        );
        assert_eq!(
            describe(&[("tCL".into(), 24, 22), ("tRFC".into(), 280, 266)]),
            "tCL 24->22 +1 more"
        );
        assert_eq!(describe(&[]), "(no change)");
    }

    #[test]
    fn m6_read_error_is_unknown_not_applied() {
        // A CMOS read failure (raw = None) must NOT be reported as "applied" — the
        // old code substituted [0u8;28] whose signature != SIG_LINUX_TOOL and thus
        // misread as applied. With a pending edit staged, a read error -> None.
        assert_eq!(classify_applied(true, None), None);
        // No pending edit -> None regardless of the read.
        assert_eq!(classify_applied(false, None), None);
        assert_eq!(classify_applied(false, Some([0u8; 28])), None);
    }

    #[test]
    fn m6_signature_drives_applied_state() {
        // Signature field is off 0, width 4, little-endian.
        let mut staged = [0u8; 28];
        staged[0..4].copy_from_slice(&SIG_LINUX_TOOL.to_le_bytes());
        // Tool signature still present -> staged, NOT applied.
        assert_eq!(classify_applied(true, Some(staged)), Some(false));
        // Firmware flipped the signature away from the tool tag -> applied.
        let mut applied = [0u8; 28];
        applied[0..4].copy_from_slice(&SIG_ABL.to_le_bytes());
        assert_eq!(classify_applied(true, Some(applied)), Some(true));
    }
}
