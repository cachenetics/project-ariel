// SPDX-License-Identifier: GPL-2.0-only
//! Suite config. For M0 this only carries the default landing tab; later milestones
//! extend it. Read from `/var/lib/arieltune/config.toml` when present (best-effort;
//! a missing or malformed file falls back to defaults, never an error to the user).

use serde::Deserialize;
use std::path::PathBuf;

/// The four tabs, in fixed left-to-right order. This IS the tab order.
pub const TAB_ORDER: [&str; 4] = ["wiki", "bios", "apu", "mem"];

/// Resolve a tab name (case-insensitive) to its index in [`TAB_ORDER`].
pub fn tab_index(name: &str) -> Option<usize> {
    let n = name.trim().to_ascii_lowercase();
    TAB_ORDER.iter().position(|t| *t == n)
}

#[derive(Debug, Default, Deserialize)]
pub struct Config {
    /// Which tab the TUI opens to when nothing overrides it. Defaults to `wiki`.
    pub default_tab: Option<String>,
}

fn config_path() -> PathBuf {
    PathBuf::from("/var/lib/arieltune/config.toml")
}

impl Config {
    /// Best-effort load; defaults on any error.
    pub fn load() -> Config {
        match std::fs::read_to_string(config_path()) {
            Ok(s) => toml::from_str(&s).unwrap_or_default(),
            Err(_) => Config::default(),
        }
    }

    /// Resolve the launch tab index given an explicit override (from `--tab` or a bare
    /// `arieltune <app>` subcommand). Priority: explicit > config `default_tab` > wiki.
    pub fn resolve_launch_tab(&self, explicit: Option<&str>) -> usize {
        explicit
            .and_then(tab_index)
            .or_else(|| self.default_tab.as_deref().and_then(tab_index))
            .unwrap_or(0) // wiki
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_order_is_wiki_bios_apu_mem() {
        assert_eq!(TAB_ORDER, ["wiki", "bios", "apu", "mem"]);
    }

    #[test]
    fn tab_index_is_case_insensitive() {
        assert_eq!(tab_index("WIKI"), Some(0));
        assert_eq!(tab_index(" Apu "), Some(2));
        assert_eq!(tab_index("mem"), Some(3));
        assert_eq!(tab_index("nope"), None);
    }

    #[test]
    fn resolve_priority_explicit_over_config_over_wiki() {
        let cfg = Config {
            default_tab: Some("bios".into()),
        };
        // explicit wins
        assert_eq!(cfg.resolve_launch_tab(Some("apu")), 2);
        // config default when no explicit
        assert_eq!(cfg.resolve_launch_tab(None), 1);
        // wiki when neither
        assert_eq!(Config::default().resolve_launch_tab(None), 0);
        // an invalid explicit falls through to config
        assert_eq!(cfg.resolve_launch_tab(Some("bogus")), 1);
    }
}
