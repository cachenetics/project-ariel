// SPDX-License-Identifier: GPL-2.0-only
//! Tuning profiles — a named bundle of CU / GPU / CPU knobs, draft-then-apply.
//!
//! A profile captures the operator-settable surface (40-CU arming, GPU clock
//! policy, CPU cclk/Vid) so a known-good setup can be saved, shared, and
//! re-applied. Built-in profiles ship in the binary; custom ones live under
//! `/var/lib/aputune/profiles/<name>.json`.
//!
//! Like memtune, applying is a draft until `--write`: without it, `apply` prints
//! exactly what it would do and changes nothing.

use std::fs;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::cpu::{self, CpuOc};
use crate::{gpuctl, persist};
use ariel_smu::ocq3::OcQ3;

pub const PROFILE_DIR: &str = "/var/lib/aputune/profiles";

/// GPU clock mode within a profile. A typed enum so a bad mode string fails at
/// PARSE time (loading the profile), not mid-apply after other actuation.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GpuMode {
    /// Release the manual pin; the persisted auto controller resumes.
    Auto,
    /// Pin `force_mhz` (the full manual-mode transition, persisted).
    Force,
}

/// GPU clock policy within a profile.
#[derive(Clone, Serialize, Deserialize)]
pub struct GpuTune {
    pub mode: GpuMode,
    #[serde(default)]
    pub force_mhz: Option<u32>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub description: String,
    /// Arm the 40-CU liberation (persisted via modprobe.d; needs reboot).
    #[serde(default)]
    pub cu_40: Option<bool>,
    #[serde(default)]
    pub gpu: Option<GpuTune>,
    #[serde(default)]
    pub cpu: Option<CpuOc>,
}

impl Profile {
    /// Human summary of what applying would do.
    pub fn plan_lines(&self) -> Vec<String> {
        let mut v = Vec::new();
        match &self.cu_40 {
            Some(true) => v.push("40-CU: arm (modprobe.d; reboot to apply)".into()),
            Some(false) => v.push("40-CU: disarm (revert to 24 CU next boot)".into()),
            None => {}
        }
        if let Some(g) = &self.gpu {
            match g.mode {
                GpuMode::Force => v.push(format!(
                    "GPU: force {} MHz (persisted manual pin)",
                    g.force_mhz.unwrap_or(0)
                )),
                GpuMode::Auto => v.push("GPU: auto (release pin; persisted auto mode)".into()),
            }
        }
        if let Some(c) = &self.cpu {
            v.push(format!(
                "CPU: boost {} MHz, curve scale {} (~{} mV @ boost), temp {}/{} C",
                c.boost_mhz,
                c.curve_scale,
                c.predicted_vid_mv(),
                c.cpu_temp_c,
                c.gpu_temp_c,
            ));
        }
        if v.is_empty() {
            v.push("(no-op: profile sets nothing)".into());
        }
        v
    }

    /// Validate the whole profile WITHOUT actuating anything: CPU point through
    /// the full brick-guard, GPU force needs a clock. Called before apply so a
    /// bad profile can never half-apply.
    pub fn validate(&self) -> Result<()> {
        if let Some(c) = &self.cpu {
            c.validate()?;
        }
        if let Some(g) = &self.gpu {
            if g.mode == GpuMode::Force && g.force_mhz.is_none() {
                bail!(
                    "profile '{}': gpu.force_mhz required for mode=force",
                    self.name
                );
            }
        }
        Ok(())
    }

    /// Apply to live hardware through the SAME state machine as the CLI/TUI —
    /// not raw SMU pokes:
    ///
    ///   * GPU force -> the full [`gpuctl::force`] path (unit stopped/restarted,
    ///     voltage floor raised BEFORE the clock, power.json persisted, mode
    ///     re-enacted) — a profile pin sticks across reboots like `gpu force`.
    ///   * GPU auto  -> the full [`gpuctl::unforce`] path (pin + voltage cleared,
    ///     preserved auto mode re-enacted).
    ///   * CPU       -> queue-3 apply + persist (cpu.json + boot service).
    ///
    /// The whole profile is validated FIRST; CU arming (persisted, reboot-scoped)
    /// is the caller's job and runs LAST.
    pub fn apply(&self, oc: Option<&OcQ3>) -> Result<()> {
        self.validate()?;
        if let Some(c) = &self.cpu {
            let oc = oc.context("CPU OC needs the queue-3 mailbox (root + BC-250)")?;
            c.apply(oc)?;
            c.save()?;
            if let Err(e) = persist::enable_cpu_oc() {
                eprintln!("profile: CPU OC applied live but not persisted ({e})");
            }
        }
        if let Some(g) = &self.gpu {
            match g.mode {
                GpuMode::Force => {
                    // validate() guaranteed force_mhz is present.
                    let mhz = g.force_mhz.context("gpu.force_mhz required")?;
                    gpuctl::force(mhz)?;
                }
                GpuMode::Auto => {
                    gpuctl::unforce()?;
                }
            }
        }
        Ok(())
    }
}

/// Built-in profiles, always available.
pub fn builtins() -> Vec<Profile> {
    vec![
        Profile {
            name: "safe".into(),
            description: "Roll back all experiments: release GPU clock, open the \
                          CPU window wide, hand voltage to BAPM."
                .into(),
            cu_40: None,
            gpu: Some(GpuTune {
                mode: GpuMode::Auto,
                force_mhz: None,
            }),
            cpu: Some(CpuOc {
                boost_mhz: 3500,
                curve_scale: 0,
                cpu_temp_c: 100,
                gpu_temp_c: 100,
            }),
        },
        Profile {
            name: "balanced".into(),
            description: "40-CU on, GPU released to BAPM (use `gpu autosleep` for \
                          app-driven power), CPU 3.8 GHz with a curve undervolt. \
                          The everyday setup."
                .into(),
            cu_40: Some(true),
            gpu: Some(GpuTune {
                mode: GpuMode::Auto,
                force_mhz: None,
            }),
            cpu: Some(CpuOc {
                boost_mhz: 3800,
                curve_scale: cpu::safe_scale_for(3800, 1250),
                cpu_temp_c: 90,
                gpu_temp_c: 90,
            }),
        },
        Profile {
            name: "performance".into(),
            description: "40-CU on, GPU pinned to the 2230 MHz run state (BAPM \
                          owns Vid), CPU 4.0 GHz with a curve undervolt. Stress-test \
                          before trusting; watch thermals."
                .into(),
            cu_40: Some(true),
            gpu: Some(GpuTune {
                mode: GpuMode::Force,
                force_mhz: Some(2230),
            }),
            cpu: Some(CpuOc {
                boost_mhz: 4000,
                curve_scale: cpu::safe_scale_for(4000, 1275),
                cpu_temp_c: 95,
                gpu_temp_c: 95,
            }),
        },
    ]
}

/// Reject profile names that could escape PROFILE_DIR or make odd filenames.
/// Only `[A-Za-z0-9._-]`, non-empty, not starting with a dot.
pub fn valid_name(name: &str) -> Result<()> {
    anyhow::ensure!(!name.is_empty(), "profile name is empty");
    anyhow::ensure!(
        !name.starts_with('.')
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')),
        "invalid profile name '{name}' (use letters, digits, . _ -)"
    );
    Ok(())
}

fn custom_path(name: &str) -> PathBuf {
    PathBuf::from(PROFILE_DIR).join(format!("{name}.json"))
}

/// All profiles: built-ins plus any custom ones on disk (custom shadows
/// a built-in of the same name).
pub fn all() -> Vec<Profile> {
    let mut map: Vec<Profile> = builtins();
    if let Ok(entries) = fs::read_dir(PROFILE_DIR) {
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) == Some("json") {
                if let Ok(txt) = fs::read_to_string(&p) {
                    if let Ok(prof) = serde_json::from_str::<Profile>(&txt) {
                        map.retain(|x| x.name != prof.name);
                        map.push(prof);
                    }
                }
            }
        }
    }
    map
}

pub fn find(name: &str) -> Option<Profile> {
    all().into_iter().find(|p| p.name == name)
}

/// Persist a custom profile.
pub fn save(prof: &Profile) -> Result<PathBuf> {
    valid_name(&prof.name)?;
    fs::create_dir_all(PROFILE_DIR).with_context(|| format!("mkdir {PROFILE_DIR} (need root)"))?;
    let path = custom_path(&prof.name);
    let json = serde_json::to_string_pretty(prof)?;
    fs::write(&path, json).with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

/// Delete a custom profile (built-ins cannot be deleted).
pub fn delete(name: &str) -> Result<()> {
    valid_name(name)?;
    let path = custom_path(name);
    if !path.exists() {
        bail!("no custom profile '{name}' at {}", path.display());
    }
    fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_plain_names() {
        for n in ["fast", "my-oc", "oc_4ghz", "v1.2"] {
            assert!(valid_name(n).is_ok(), "{n}");
        }
    }

    #[test]
    fn rejects_traversal_and_odd_names() {
        for n in [
            "",
            ".",
            "..",
            "../x",
            "a/b",
            ".hidden",
            "a b",
            "x\n",
            "eviL/../y",
        ] {
            assert!(valid_name(n).is_err(), "should reject {n:?}");
        }
    }

    #[test]
    fn builtins_are_valid_and_safe() {
        for p in builtins() {
            assert!(valid_name(&p.name).is_ok(), "builtin {}", p.name);
            if let Some(c) = &p.cpu {
                assert!(c.validate().is_ok(), "builtin {} cpu unsafe", p.name);
            }
        }
    }
}
