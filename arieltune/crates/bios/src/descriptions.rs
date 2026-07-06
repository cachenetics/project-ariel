// SPDX-License-Identifier: GPL-2.0-only
//! Plain-English descriptions for BIOS settings, written for a general audience
//! (think: curious person with no firmware background) — almost a mini learning
//! moment per setting, including the deep debug knobs.
//!
//! The corpus (`descriptions.json`, one short line per setting) is embedded at
//! build time; it was authored to a consistent house style (one sentence, plain
//! words, gentle "leave default" steers for advanced/risky knobs). `describe`
//! looks a setting up by name and falls back to a name/options-derived line for
//! anything not in the corpus.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::catalog::Setting;

static RAW: &str = include_str!("descriptions.json");

fn corpus() -> &'static HashMap<String, String> {
    static MAP: OnceLock<HashMap<String, String>> = OnceLock::new();
    MAP.get_or_init(|| serde_json::from_str(RAW).expect("embedded descriptions.json must be valid"))
}

/// Words that read as a simple on/off-ish choice (for the fallback).
fn is_toggle(labels: &[&str]) -> bool {
    labels.iter().all(|l| {
        let l = l.to_lowercase();
        l.contains("enable") || l.contains("disable") || l == "on" || l == "off" || l == "auto"
    })
}

/// Build a plain-English sentence from a setting's options when it isn't in the
/// corpus (safety net; the corpus currently covers every catalogue setting).
fn fallback(s: &Setting) -> String {
    let name = &s.name;
    if let Some([lo, hi]) = s.range {
        let def = s.default.clone().unwrap_or_else(|| "its default".into());
        return format!(
            "A numeric value (from {lo} to {hi}) for \"{name}\". Best left at {def} unless you \
             know what it changes."
        );
    }
    let labels: Vec<&str> = s.options.iter().map(|(_, l)| l.as_str()).collect();
    let has_auto = labels.iter().any(|l| l.eq_ignore_ascii_case("auto"));
    if !labels.is_empty() && is_toggle(&labels) {
        let mut t = format!("Turns \"{name}\" on or off.");
        if has_auto {
            t.push_str(" \"Auto\" lets the firmware decide, which is usually the safe choice.");
        }
        return t;
    }
    format!(
        "An advanced firmware setting (\"{name}\"). Leave it at its default unless you have a \
         specific reason to change it."
    )
}

/// Returns (description, from_corpus). `from_corpus` is true for the authored
/// line, false for the generated fallback.
pub fn describe(s: &Setting) -> (String, bool) {
    match corpus().get(&s.name) {
        Some(d) => (d.clone(), true),
        None => (fallback(s), false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corpus_loads_and_is_large() {
        assert!(
            corpus().len() > 1000,
            "embedded corpus should cover the surface"
        );
    }

    #[test]
    fn every_catalogue_setting_has_a_description() {
        for st in crate::catalog::load() {
            let (d, _) = describe(&st);
            assert!(!d.is_empty(), "empty description for {}", st.name);
        }
    }

    #[test]
    fn known_setting_uses_corpus() {
        let s = crate::catalog::load()
            .into_iter()
            .find(|s| s.name == "SVM Mode");
        if let Some(s) = s {
            let (_d, from_corpus) = describe(&s);
            assert!(from_corpus);
        }
    }

    #[test]
    fn fallback_for_unknown() {
        let s = Setting {
            category: "X".into(),
            name: "ZZ_not_in_corpus".into(),
            offset: 0,
            bits: 8,
            default: None,
            options: vec![(0, "Disabled".into()), (1, "Enabled".into())],
            range: None,
            varstore: "AmdSetup".into(),
        };
        let (d, from_corpus) = describe(&s);
        assert!(!from_corpus);
        assert!(d.contains("on or off"));
    }
}
