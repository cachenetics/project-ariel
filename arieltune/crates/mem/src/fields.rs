// SPDX-License-Identifier: GPL-2.0-only
//! Field encyclopedia — a plain-language description of every `MemConf` timing,
//! plus which direction is faster, how much it's worth, and how risky it is.
//!
//! This is the teaching layer: the TUI editor shows the doc for whichever field
//! the cursor is on, and `memtune explain` prints them from the CLI. Content is
//! grounded in GDDR6/DRAM tuning fundamentals and the BC-250 community findings
//! (bottom-binned Micron ICs, tREF the most rewarding timing, ~1750 the ceiling).

/// Which way a field moves performance.
#[derive(Clone, Copy, PartialEq)]
pub enum Dir {
    /// Lower value = tighter = faster (most timings).
    LowerFaster,
    /// Higher value = faster (e.g. a longer refresh interval = fewer refreshes).
    HigherFaster,
    /// Not a performance knob (e.g. the VRAM carve-out).
    None,
}

impl Dir {
    pub fn hint(self) -> &'static str {
        match self {
            Dir::LowerFaster => "lower = tighter / faster",
            Dir::HigherFaster => "higher = faster (fewer refreshes)",
            Dir::None => "no performance effect",
        }
    }
}

/// A coarse magnitude used for both gain and risk.
#[derive(Clone, Copy, PartialEq)]
pub enum Tier {
    None,
    Low,
    Med,
    High,
}

impl Tier {
    pub fn label(self) -> &'static str {
        match self {
            Tier::None => "none",
            Tier::Low => "low",
            Tier::Med => "med",
            Tier::High => "high",
        }
    }
}

/// One field's documentation. `name` matches `config::Field::name`.
pub struct FieldDoc {
    pub name: &'static str,
    pub full: &'static str,
    pub group: &'static str,
    pub faster: Dir,
    pub gain: Tier,
    pub risk: Tier,
    /// One-line summary (shown as the headline).
    pub blurb: &'static str,
    /// Two-to-four sentence explanation.
    pub detail: &'static str,
    /// Board-specific tip; "" if none.
    pub note: &'static str,
}

/// Look up the doc for a field by name.
pub fn doc(name: &str) -> Option<&'static FieldDoc> {
    DOCS.iter().find(|d| d.name == name)
}

pub const DOCS: &[FieldDoc] = &[
    FieldDoc {
        name: "ClockSpeed",
        full: "Memory Clock",
        group: "Clock",
        faster: Dir::HigherFaster,
        gain: Tier::High,
        risk: Tier::High,
        blurb: "GDDR6 command clock in MHz; data rate per pin = clock x 8.",
        detail: "The master memory frequency. GDDR6 moves 8 bits per pin per clock, \
so the per-pin data rate is ClockSpeed x 8 (1750 MHz -> 14.0 Gbps). Across the \
256-bit bus that is ~448 GB/s at 1750. Raising the clock is the single biggest \
bandwidth lever, but the GDDR6 has to TRAIN at the new speed during boot or the \
board won't POST.",
        note: "This board's Micron ICs are bottom-binned. Measured here: 1750 trains \
(~440 GB/s, ~97% of theoretical); 1812 / 14.5 Gbps does NOT train; and oddly \
sub-1750 also fails to train. 1750 is effectively the ceiling on this hardware.",
    },
    FieldDoc {
        name: "tCL",
        full: "CAS Latency",
        group: "Access",
        faster: Dir::LowerFaster,
        gain: Tier::Low,
        risk: Tier::Med,
        blurb: "Clocks from a READ command until data starts returning.",
        detail: "Column Address Strobe latency: after the column address is sent, the \
chip waits tCL clocks before the first data word appears. Lower = data sooner = \
lower latency, but set it too low and reads come back corrupted. A primary timing.",
        note: "",
    },
    FieldDoc {
        name: "tRAS",
        full: "Row Active Time",
        group: "Activate",
        faster: Dir::LowerFaster,
        gain: Tier::Low,
        risk: Tier::Med,
        blurb: "Minimum time a row stays open (ACT -> PRE).",
        detail: "Once a row is activated it must stay open at least tRAS clocks before \
it can be precharged (closed). Too low and the row's charge isn't fully restored \
before close, which corrupts data. Usually about tRCD + tRTP at minimum.",
        note: "",
    },
    FieldDoc {
        name: "tRCDRD",
        full: "RAS-to-CAS Delay, Read",
        group: "Activate",
        faster: Dir::LowerFaster,
        gain: Tier::Med,
        risk: Tier::Med,
        blurb: "ACT -> READ: delay from opening a row to reading it.",
        detail: "Time from activating (opening) a row until a READ to a column in that \
row is allowed; the row has to be sensed before its columns can be read. Lower \
speeds up the first access to a freshly opened row.",
        note: "",
    },
    FieldDoc {
        name: "tRCDWR",
        full: "RAS-to-CAS Delay, Write",
        group: "Activate",
        faster: Dir::LowerFaster,
        gain: Tier::Low,
        risk: Tier::Med,
        blurb: "ACT -> WRITE: delay from opening a row to writing it.",
        detail: "Like tRCDRD but for writes, which can usually begin sooner than reads \
after a row opens.",
        note: "",
    },
    FieldDoc {
        name: "tRCAb",
        full: "Row Cycle, all-bank",
        group: "Activate",
        faster: Dir::LowerFaster,
        gain: Tier::Low,
        risk: Tier::Med,
        blurb: "Full ACT -> ACT cycle to the same bank.",
        detail: "The minimum complete row cycle measured all-bank: activate, use, \
precharge, then re-activate. Roughly tRAS + tRP. It floors how fast a bank can be \
reopened.",
        note: "",
    },
    FieldDoc {
        name: "tRCPb",
        full: "Row Cycle, per-bank",
        group: "Activate",
        faster: Dir::LowerFaster,
        gain: Tier::Low,
        risk: Tier::Low,
        blurb: "Per-bank component of the row cycle.",
        detail: "The per-bank part of the row cycle, used alongside GDDR6 per-bank \
refresh. A small tuning value with little measurable effect on these chips.",
        note: "",
    },
    FieldDoc {
        name: "tRPAb",
        full: "Row Precharge, all-bank",
        group: "Precharge",
        faster: Dir::LowerFaster,
        gain: Tier::Low,
        risk: Tier::Med,
        blurb: "PRE -> ACT: idle needed to close a row before opening another.",
        detail: "After precharging (closing) a row, the bank must idle tRP clocks \
before a new row can be activated. Lower lets you switch rows faster; too low \
corrupts.",
        note: "",
    },
    FieldDoc {
        name: "tRPPb",
        full: "Row Precharge, per-bank",
        group: "Precharge",
        faster: Dir::LowerFaster,
        gain: Tier::Low,
        risk: Tier::Low,
        blurb: "Per-bank component of row precharge.",
        detail: "The per-bank part of the precharge timing, paired with per-bank \
refresh. Small effect.",
        note: "",
    },
    FieldDoc {
        name: "tRRDS",
        full: "Row-to-Row Delay, Short",
        group: "Activate",
        faster: Dir::LowerFaster,
        gain: Tier::Low,
        risk: Tier::Low,
        blurb: "ACT -> ACT between different bank groups.",
        detail: "Minimum spacing between activating rows in two DIFFERENT bank groups. \
Limits how quickly activations can fan out. 'Short' because crossing bank groups is \
cheaper than staying within one.",
        note: "",
    },
    FieldDoc {
        name: "tRRDL",
        full: "Row-to-Row Delay, Long",
        group: "Activate",
        faster: Dir::LowerFaster,
        gain: Tier::Low,
        risk: Tier::Low,
        blurb: "ACT -> ACT within the same bank group.",
        detail: "Like tRRDS but for two activations inside the SAME bank group, which \
need more spacing ('long').",
        note: "",
    },
    FieldDoc {
        name: "tRTP",
        full: "Read-to-Precharge",
        group: "Access",
        faster: Dir::LowerFaster,
        gain: Tier::Low,
        risk: Tier::Low,
        blurb: "READ -> PRE: wait before closing a row after a read.",
        detail: "After a read, the bank must wait tRTP before it can be precharged, so \
in-flight data isn't cut off. Interacts with tRAS to bound the row cycle.",
        note: "",
    },
    FieldDoc {
        name: "tFAW",
        full: "Four Activate Window",
        group: "Activate",
        faster: Dir::LowerFaster,
        gain: Tier::Low,
        risk: Tier::Med,
        blurb: "Rolling window allowing at most four activations.",
        detail: "A sliding window during which no more than four row activations may \
occur, capping peak activation current. Lower allows denser activations (faster) \
but stresses power delivery.",
        note: "",
    },
    FieldDoc {
        name: "tREF",
        full: "Refresh Interval (tREFI)",
        group: "Refresh",
        faster: Dir::HigherFaster,
        gain: Tier::High,
        risk: Tier::Med,
        blurb: "How often a refresh is issued; higher = fewer refreshes.",
        detail: "The average interval between refresh commands. Refreshing steals \
cycles from real traffic, so a LONGER interval (higher value) means fewer \
interruptions and more usable bandwidth. But cells leak charge: stretch it too far \
-- especially when the chips are hot -- and data corrupts.",
        note: "Community consensus: tREF is the most rewarding GDDR6 timing to tune on \
these chips. Raise it gradually and re-test while warm.",
    },
    FieldDoc {
        name: "RFCPb",
        full: "Refresh Cycle, per-bank",
        group: "Refresh",
        faster: Dir::LowerFaster,
        gain: Tier::Med,
        risk: Tier::Med,
        blurb: "Duration of a single per-bank refresh (REFpb).",
        detail: "How long one per-bank refresh keeps that bank busy. Lower returns the \
bank to service sooner. Per-bank refresh lets the other banks keep working, so it's \
gentler on bandwidth than an all-bank refresh.",
        note: "",
    },
    FieldDoc {
        name: "tRFC",
        full: "Refresh Cycle Time (all-bank)",
        group: "Refresh",
        faster: Dir::LowerFaster,
        gain: Tier::High,
        risk: Tier::Med,
        blurb: "Duration of an all-bank refresh; stalls the whole rank.",
        detail: "How long an all-bank refresh takes, during which the entire rank is \
busy. Together with tREF it sets total refresh overhead, making it a big bandwidth \
lever. Lower = shorter stalls, but too low and the refresh can't complete -> \
corruption.",
        note: "Pairs with tREF -- tune the two together when chasing refresh overhead.",
    },
    FieldDoc {
        name: "UMA_SIZE",
        full: "VRAM Carve-out",
        group: "Misc",
        faster: Dir::None,
        gain: Tier::None,
        risk: Tier::Low,
        blurb: "GDDR6 reserved as dedicated VRAM (MB, 16 MB aligned).",
        detail: "How much of the 16 GB is set aside as dedicated VRAM for the GPU. Not \
a timing -- it doesn't change speed, only the split between system and video memory. \
Must be >= 256 and aligned to 16 MB.",
        note: "",
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::FIELDS;

    /// Every editable timing field must have a doc entry (so the editor pane is
    /// never blank as the cursor moves).
    #[test]
    fn every_editable_field_documented() {
        for f in FIELDS {
            if matches!(f.name, "Signature" | "Checksum") {
                continue;
            }
            assert!(doc(f.name).is_some(), "missing FieldDoc for {}", f.name);
        }
    }
}
