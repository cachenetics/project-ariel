// SPDX-License-Identifier: GPL-2.0-only
//! Runtime patch-state detection.
//!
//! For each member of [`patches::SERIES`] we probe the booted kernel for that
//! patch's [`Tell`] and classify it. The result is the ground truth the rest of
//! the tool keys off: whether to offer a live action, fall back, or offer to
//! *build the patch into the system* (see `kbuild`).

use std::fs;
use std::path::{Path, PathBuf};

use crate::cu;
use crate::patches::{self, Tell};

/// Per-patch detection result.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    /// The patch's unique runtime tell is present.
    Present,
    /// The tell is definitively absent.
    Absent,
    /// No unique tell of its own; inferred Present because the detectable
    /// members of the series are Present.
    Inferred,
    /// Could not determine (e.g. debugfs not mounted, not root).
    Unknown,
}

impl State {
    pub fn glyph(self) -> &'static str {
        match self {
            State::Present => "[ok]",
            State::Absent => "[--]",
            State::Inferred => "[in]",
            State::Unknown => "[??]",
        }
    }
}

pub struct PatchStatus {
    pub id: &'static str,
    pub title: &'static str,
    pub tell: Tell,
    pub state: State,
}

pub struct Report {
    pub rows: Vec<PatchStatus>,
    /// amdgpu debugfs dir found at probe time (None if unavailable).
    pub dbg_dir: Option<PathBuf>,
    /// Is this host actually a BC-250?
    pub is_bc250: bool,
}

impl Report {
    /// All series members live (Present or Inferred)?
    pub fn fully_patched(&self) -> bool {
        self.rows
            .iter()
            .all(|r| matches!(r.state, State::Present | State::Inferred))
    }

    /// Members whose unique tell is definitively missing.
    pub fn missing(&self) -> Vec<&PatchStatus> {
        self.rows
            .iter()
            .filter(|r| r.state == State::Absent)
            .collect()
    }
}

/// Machine-readable doctor summary. Serialized by `doctor --json` and parsed
/// back by kbuild's post-install remote verification — one struct both ways so
/// the two sides cannot drift.
#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct DoctorJson {
    pub is_bc250: bool,
    /// Running kernel release (`uname -r`).
    pub kernel: String,
    /// Series members live (Present or Inferred).
    pub present: usize,
    /// Series size (patches::count()).
    pub total: usize,
    pub fully: bool,
}

impl DoctorJson {
    pub fn from_report(rep: &Report) -> Self {
        DoctorJson {
            is_bc250: rep.is_bc250,
            kernel: ariel_hal::running_kernel(),
            present: rep
                .rows
                .iter()
                .filter(|r| matches!(r.state, State::Present | State::Inferred))
                .count(),
            total: patches::count(),
            fully: rep.fully_patched(),
        }
    }
}

// The APU-presence probe (`ariel_apu_present`, was `is_bc250`), the amdgpu
// debugfs discovery (`amdgpu_dbg_dir`), and the running-kernel read
// (`running_kernel`) were lifted into `ariel-hal` in M2 (silicon-generic, no
// longer aputune-local). This module keeps only the aputune-specific patch-state
// probing and calls `ariel_hal::*` for those three.

/// First GPU sysfs device dir exposing `pp_dpm_sclk`.
fn pp_dpm_sclk_path() -> Option<PathBuf> {
    let entries = fs::read_dir("/sys/class/drm").ok()?;
    for e in entries.flatten() {
        let cand = e.path().join("device/pp_dpm_sclk");
        if cand.exists() {
            return Some(cand);
        }
    }
    None
}

/// Max advertised SCLK state, in MHz, from `pp_dpm_sclk`.
fn sclk_max_mhz() -> Option<u32> {
    let txt = fs::read_to_string(pp_dpm_sclk_path()?).ok()?;
    txt.lines()
        .filter_map(|l| {
            // lines look like "1: 2500Mhz *"
            let mhz = l.split(':').nth(1)?;
            let digits: String = mhz
                .chars()
                .take_while(|c| c.is_ascii_digit() || c.is_whitespace())
                .collect();
            digits.trim().parse::<u32>().ok()
        })
        .max()
}

fn modparam_present(name: &str) -> bool {
    Path::new("/sys/module/amdgpu/parameters")
        .join(name)
        .exists()
}

fn probe(tell: Tell, dbg: Option<&Path>) -> State {
    match tell {
        Tell::Bundled => State::Inferred, // refined by the caller pass
        Tell::ModParam(name) => {
            if modparam_present(name) {
                State::Present
            } else {
                State::Absent
            }
        }
        Tell::Debugfs(name) => match dbg {
            Some(d) => {
                if d.join(name).exists() {
                    State::Present
                } else {
                    State::Absent
                }
            }
            None => State::Unknown,
        },
        Tell::SclkMax(min) => match sclk_max_mhz() {
            Some(m) if m >= min => State::Present,
            Some(_) => State::Absent,
            None => State::Unknown,
        },
        Tell::CuCount(n) => match cu::active_cu_count() {
            Some(c) if c >= n => State::Present,
            Some(_) => State::Absent,
            None => State::Unknown,
        },
    }
}

/// Probe the full series against the running kernel.
pub fn report() -> Report {
    let dbg = ariel_hal::amdgpu_dbg_dir();
    let mut rows: Vec<PatchStatus> = patches::SERIES
        .iter()
        .map(|p| PatchStatus {
            id: p.id,
            title: p.title,
            tell: p.tell,
            state: probe(p.tell, dbg.as_deref()),
        })
        .collect();

    // Refine Bundled rows from the uniquely-detectable members:
    //   * ALL detectable Present  -> the whole series is live: Inferred.
    //   * ALL detectable Absent   -> the series is absent: Absent.
    //   * mixed / partial         -> Unknown. A partial series (e.g. a kernel
    //     built with only half the patches) must NOT report its undetectable
    //     members as present — "any one present" proved nothing about the rest.
    let detectable: Vec<State> = rows
        .iter()
        .filter(|r| !matches!(r.tell, Tell::Bundled))
        .map(|r| r.state)
        .collect();
    let all_present = !detectable.is_empty() && detectable.iter().all(|s| *s == State::Present);
    let all_absent = !detectable.is_empty() && detectable.iter().all(|s| *s == State::Absent);
    for r in rows.iter_mut() {
        if matches!(r.tell, Tell::Bundled) {
            r.state = if all_present {
                State::Inferred
            } else if all_absent {
                State::Absent
            } else {
                State::Unknown
            };
        }
    }

    Report {
        rows,
        dbg_dir: dbg,
        is_bc250: ariel_hal::ariel_apu_present(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doctor_json_shape() {
        let d = DoctorJson {
            is_bc250: true,
            kernel: "6.12.4-aputune".into(),
            present: 12,
            total: 12,
            fully: true,
        };
        let s = serde_json::to_string(&d).unwrap();
        // Field names + JSON types are the contract the remote verification
        // (and any script) parses — assert them on the raw value.
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["is_bc250"], serde_json::Value::Bool(true));
        assert!(v["kernel"].is_string());
        assert_eq!(v["kernel"], "6.12.4-aputune");
        assert!(v["present"].is_u64());
        assert_eq!(v["present"], 12);
        assert!(v["total"].is_u64());
        assert_eq!(v["total"], 12);
        assert_eq!(v["fully"], serde_json::Value::Bool(true));
        // And it round-trips.
        let back: DoctorJson = serde_json::from_str(&s).unwrap();
        assert_eq!(back, d);
    }
}
