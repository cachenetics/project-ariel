// SPDX-License-Identifier: GPL-2.0-only
//! The BIOS settings catalogue — the logical view's data model.
//!
//! Generated from the BC-250 IFR varmaps by `tools/gen_catalog.py` and embedded
//! at build time. Each entry is one BIOS setting: its category (the BIOS menu it
//! lives under), name, the varstore byte offset/width its value occupies, the
//! allowed options or numeric range, and its default. biostune presents these as
//! one unified, categorized tree (memtune owns memory; this owns the rest of the
//! BIOS surface).

use serde::Deserialize;

#[derive(Deserialize, Clone)]
pub struct Setting {
    pub category: String,
    pub name: String,
    pub offset: usize,
    pub bits: u8,
    pub default: Option<String>,
    /// (value, label) pairs for an enumerated setting.
    pub options: Vec<(u32, String)>,
    /// [lo, hi] for a numeric setting, else None.
    pub range: Option<[u32; 2]>,
    /// Which EFI variable holds this setting's value ("AmdSetup" or "Setup").
    pub varstore: String,
}

impl Setting {
    /// Value width in bytes (1/2/4).
    pub fn width(&self) -> usize {
        ((self.bits as usize) / 8).clamp(1, 4)
    }

    /// Human label for a raw value: the matching option label, the numeric value
    /// (with range hint), or a hex fallback.
    pub fn label_for(&self, v: u32) -> String {
        if let Some((_, l)) = self.options.iter().find(|(val, _)| *val == v) {
            return l.clone();
        }
        if self.range.is_some() {
            return v.to_string();
        }
        format!("0x{v:x}")
    }

    /// A compact description of the value space, for the detail pane.
    pub fn value_space(&self) -> String {
        if let Some([lo, hi]) = self.range {
            format!("range {lo}..={hi}")
        } else if !self.options.is_empty() {
            self.options
                .iter()
                .map(|(v, l)| format!("{v}={l}"))
                .collect::<Vec<_>>()
                .join(", ")
        } else {
            "(opaque value)".into()
        }
    }
}

/// Top-level compartments, in display order — the coarse grouping above the
/// (finer) categories, so the menu reads like a real BIOS rather than 75 flat
/// sections.
pub const COMPARTMENTS: &[&str] = &[
    "Processor",
    "Memory",
    "Data Fabric",
    "NBIO / PCIe",
    "Chipset / I/O",
    "Graphics / Display",
    "Power / Voltage",
    "Security / RAS",
    "Boot / Platform",
    "Other",
];

/// Map a category to its compartment by *function* (not by which varstore it
/// lives in) — OEM Setup and AMD CBS settings blend into one logical tree.
/// Keyword rules in priority order; a subsystem wins over the generic "debug"
/// label so e.g. "CPU Debug Control" lands under Processor.
pub fn compartment_of(category: &str, _varstore: &str) -> &'static str {
    let c = category.to_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| c.contains(n));
    if has(&["df ", "data fabric"]) {
        "Data Fabric"
    } else if has(&[
        "umc", "mrx", "dram", "memory", "ddr", "bank", "g6 mem", "mem ss",
    ]) {
        "Memory"
    } else if has(&[
        "smu",
        "pmm",
        "avfs",
        "power",
        "voltage",
        "telemetry",
        "dpm",
        "s3",
        "s5",
    ]) {
        "Power / Voltage"
    } else if has(&[
        "usb",
        "sata",
        "combophy",
        "fch",
        "sio ",
        " sio",
        "cdr",
        "lpc",
        "espi",
        "azalia",
        "smbus",
        "ir configuration",
        "sd configuration",
    ]) {
        "Chipset / I/O"
    } else if has(&[
        "pcie",
        "nbio",
        "pci ",
        "rxeq",
        "txeq",
        "swing",
        "los",
        "spc(",
        "aer(",
        "correctable",
        "gnb",
        "gpp",
        "gra group",
        "port",
        "dxio",
        "nb configuration",
        "nbio common",
    ]) {
        "NBIO / PCIe"
    } else if has(&["gfx", "display", "graphics", "azalia hd", "hd audio"]) {
        "Graphics / Display"
    } else if has(&["ras", "security", "accept", "tpm", "sev", "psb", "smee"]) {
        "Security / RAS"
    } else if has(&[
        "cpu",
        "core",
        "thread",
        "processor",
        "downcore",
        "prefetch",
        "c-state",
        "cstate",
        "smt",
        "vh common",
    ]) {
        "Processor"
    } else if has(&[
        "boot",
        "demo board",
        "hardware monitor",
        "fan",
        "numlock",
        "platform",
    ]) {
        "Boot / Platform"
    } else {
        "Other"
    }
}

static RAW: &str = include_str!("catalog.json");

/// Parse the embedded catalogue. Panics only if the build embedded bad JSON.
pub fn load() -> Vec<Setting> {
    serde_json::from_str(RAW).expect("embedded catalog.json must be valid")
}

/// Distinct categories in first-seen order, each with the indices of its
/// settings (into `settings`).
pub fn by_category(settings: &[Setting]) -> Vec<(String, Vec<usize>)> {
    let mut order: Vec<String> = Vec::new();
    let mut groups: std::collections::HashMap<String, Vec<usize>> =
        std::collections::HashMap::new();
    for (i, s) in settings.iter().enumerate() {
        if !groups.contains_key(&s.category) {
            order.push(s.category.clone());
        }
        groups.entry(s.category.clone()).or_default().push(i);
    }
    order
        .into_iter()
        .map(|c| {
            let idx = groups.remove(&c).unwrap_or_default();
            (c, idx)
        })
        .collect()
}

/// Group categories into compartments, in COMPARTMENTS order, dropping empty
/// ones. Each entry is (compartment, indices into `cats`).
pub fn by_compartment(
    settings: &[Setting],
    cats: &[(String, Vec<usize>)],
) -> Vec<(&'static str, Vec<usize>)> {
    let comp_of = |ci: usize| -> &'static str {
        let (name, idx) = &cats[ci];
        let varstore = idx
            .first()
            .map(|&i| settings[i].varstore.as_str())
            .unwrap_or("");
        compartment_of(name, varstore)
    };
    COMPARTMENTS
        .iter()
        .filter_map(|&comp| {
            let members: Vec<usize> = (0..cats.len()).filter(|&ci| comp_of(ci) == comp).collect();
            (!members.is_empty()).then_some((comp, members))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compartments_cover_all_categories() {
        let s = load();
        let cats = by_category(&s);
        let comps = by_compartment(&s, &cats);
        let total: usize = comps.iter().map(|(_, v)| v.len()).sum();
        assert_eq!(
            total,
            cats.len(),
            "every category lands in exactly one compartment"
        );
    }

    #[test]
    fn embedded_catalog_loads() {
        let s = load();
        assert!(
            s.len() > 1000,
            "catalogue should have the full BIOS surface"
        );
        // every setting has a category and name
        assert!(s
            .iter()
            .all(|x| !x.category.is_empty() && !x.name.is_empty()));
    }

    #[test]
    fn categories_partition_settings() {
        let s = load();
        let cats = by_category(&s);
        let total: usize = cats.iter().map(|(_, v)| v.len()).sum();
        assert_eq!(total, s.len());
    }

    #[test]
    fn label_and_width() {
        let s = Setting {
            category: "X".into(),
            name: "T".into(),
            offset: 0,
            bits: 8,
            default: None,
            options: vec![(0, "Disabled".into()), (1, "Enabled".into())],
            range: None,
            varstore: "AmdSetup".into(),
        };
        assert_eq!(s.width(), 1);
        assert_eq!(s.label_for(1), "Enabled");
        assert_eq!(s.label_for(9), "0x9");
    }
}
