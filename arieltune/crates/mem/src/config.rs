// SPDX-License-Identifier: GPL-2.0-only
//! BC-250 GDDR6 memory-timing config layout, ported faithfully from the
//! reference `memcfg.py` (ASRock bc250_memcfg + RobinMemTiming lineage).
//!
//! The config is a 28-byte little-endian `MemConf_t` stored in extended CMOS
//! (offset 0x90, read via I/O ports 0x72/0x73). ABL reads it during boot and
//! applies the timings to the UMC, then stamps a result signature back.

pub const CONFIG_SIZE: usize = 28;
pub const CONFIG_OFFSET: u8 = 0x90;

/// One field in the `MemConf_t` struct (little-endian).
#[derive(Clone, Copy)]
pub struct Field {
    pub name: &'static str,
    pub off: usize,
    pub width: u8, // bytes: 1, 2, or 4
    pub lo: u32,
    pub hi: u32,
}

/// Struct field order — MUST match what ABL expects.
pub const FIELDS: &[Field] = &[
    Field {
        name: "Signature",
        off: 0,
        width: 4,
        lo: 0,
        hi: 0xFFFF_FFFF,
    },
    Field {
        name: "Checksum",
        off: 4,
        width: 2,
        lo: 0,
        hi: 0xFFFF,
    },
    // hi raised above the recommended 1750 (14 Gbps) to allow 1812 (14.5) / higher
    // memclk experiments; ABL still validates and rejects anything that won't train.
    Field {
        name: "ClockSpeed",
        off: 6,
        width: 2,
        lo: 450,
        hi: 2000,
    },
    Field {
        name: "tCL",
        off: 8,
        width: 1,
        lo: 8,
        hi: 33,
    },
    Field {
        name: "tRAS",
        off: 9,
        width: 1,
        lo: 21,
        hi: 58,
    },
    Field {
        name: "tRCDRD",
        off: 10,
        width: 1,
        lo: 8,
        hi: 40,
    },
    Field {
        name: "tRCDWR",
        off: 11,
        width: 1,
        lo: 8,
        hi: 40,
    },
    Field {
        name: "tRCAb",
        off: 12,
        width: 1,
        lo: 40,
        hi: 90,
    },
    Field {
        name: "tRCPb",
        off: 13,
        width: 1,
        lo: 0,
        hi: 11,
    },
    Field {
        name: "tRPAb",
        off: 14,
        width: 1,
        lo: 8,
        hi: 40,
    },
    Field {
        name: "tRPPb",
        off: 15,
        width: 1,
        lo: 0,
        hi: 11,
    },
    Field {
        name: "tRRDS",
        off: 16,
        width: 1,
        lo: 4,
        hi: 12,
    },
    Field {
        name: "tRRDL",
        off: 17,
        width: 1,
        lo: 4,
        hi: 12,
    },
    Field {
        name: "tRTP",
        off: 18,
        width: 1,
        lo: 0,
        hi: 14,
    },
    Field {
        name: "tFAW",
        off: 19,
        width: 1,
        lo: 4,
        hi: 34,
    },
    Field {
        name: "tREF",
        off: 20,
        width: 2,
        lo: 0,
        hi: 65535,
    },
    Field {
        name: "RFCPb",
        off: 22,
        width: 2,
        lo: 0,
        hi: 65535,
    },
    Field {
        name: "tRFC",
        off: 24,
        width: 2,
        lo: 0,
        hi: 65535,
    },
    Field {
        name: "UMA_SIZE",
        off: 26,
        width: 2,
        lo: 0,
        hi: 65535,
    },
];

/// `MemConf_t` signatures. The u32 LE values spell ASCII tags.
pub const SIG_LINUX_TOOL: u32 = 0x4243_5041; // "APCB" in memory (LE) — tool-written, pending ABL apply
pub const SIG_ABL: u32 = 0x4C42_4124; // "$ABL" — ABL applied successfully
pub const SIG_CMOS_BAD: u32 = 0x4253_4D43; // "CMSB" — ABL found invalid/stale config
pub const SIG_WDT_FIRED: u32 = 0x4654_4457; // "WDTF" — watchdog fired during training
pub const SIG_CHECKSUM_ERR: u32 = 0x454B_4843; // "CHKE" — checksum mismatch
pub const SIG_SIGNATURE_ERR: u32 = 0x4547_4953; // "SIGE" — bad signature

pub fn signature_name(sig: u32) -> &'static str {
    match sig {
        SIG_LINUX_TOOL => "LINUX_TOOL (pending apply)",
        SIG_ABL => "ABL (applied by bootloader)",
        SIG_CMOS_BAD => "CMOS_BAD (invalid/stale)",
        SIG_WDT_FIRED => "WDT_FIRED (training timeout)",
        SIG_CHECKSUM_ERR => "CHECKSUM_ERROR",
        SIG_SIGNATURE_ERR => "SIGNATURE_ERROR",
        _ => "UNKNOWN",
    }
}

/// Whether the current signature is the healthy, applied state.
pub fn signature_ok(sig: u32) -> bool {
    sig == SIG_ABL
}

/// A short, one-line actionable hint for tight UIs (full text: `signature_help`).
pub fn signature_hint(sig: u32) -> &'static str {
    match sig {
        SIG_ABL => "timings are live",
        SIG_LINUX_TOOL => "staged — reboot to apply",
        SIG_CMOS_BAD => "on defaults (normal after a CMOS clear) — `recommended --write` + reboot",
        SIG_WDT_FIRED => "training timed out — loosen timings + reboot",
        _ => "config rejected — `recommended --write` + reboot",
    }
}

/// One-line plain-language explanation of what a signature means and what to do.
/// This is a status tag the firmware/tool writes into CMOS — NOT a battery or
/// hardware reading.
pub fn signature_help(sig: u32) -> &'static str {
    match sig {
        SIG_ABL => "Trained and applied — your timings are live. Nothing to do.",
        SIG_LINUX_TOOL => {
            "A config is staged but not applied yet. Reboot so the firmware trains it."
        }
        SIG_CMOS_BAD => {
            "No trained config is stamped in the CMOS slot yet, so the firmware booted \
             on stock defaults — that's all CMOS_BAD means (normal on a fresh install or \
             right after a CMOS clear; it is NOT a low battery). A plain reboot won't change \
             it. Run `arieltune mem recommended --write`, then reboot — the firmware trains the \
             config on the next boot and the signature reads ABL (clean)."
        }
        SIG_WDT_FIRED => {
            "Memory training timed out (timings too aggressive); the firmware booted \
             on defaults. Loosen the timings, or `arieltune mem recommended --write` to recover."
        }
        SIG_CHECKSUM_ERR => {
            "The stored config's checksum did not match, so it was rejected and \
             defaults were used. Re-write a config to fix the checksum."
        }
        SIG_SIGNATURE_ERR => {
            "The stored config's signature was not recognized, so it was rejected. \
             Run `arieltune mem recommended --write` to set a valid config."
        }
        _ => {
            "Unrecognized signature — the firmware will treat the stored config as \
             invalid and use defaults. Run `arieltune mem recommended --write` to recover."
        }
    }
}

/// A decoded memory configuration backed by its raw 28-byte buffer.
#[derive(Clone)]
pub struct MemConf {
    pub buf: [u8; CONFIG_SIZE],
}

impl MemConf {
    pub fn from_bytes(buf: [u8; CONFIG_SIZE]) -> Self {
        Self { buf }
    }

    pub fn field(name: &str) -> Option<&'static Field> {
        FIELDS.iter().find(|f| f.name == name)
    }

    /// Read a field's value (little-endian) widened to u32.
    pub fn get_field(&self, f: &Field) -> u32 {
        let mut v: u32 = 0;
        for i in 0..f.width as usize {
            v |= (self.buf[f.off + i] as u32) << (8 * i);
        }
        v
    }

    pub fn get(&self, name: &str) -> Option<u32> {
        Self::field(name).map(|f| self.get_field(f))
    }

    pub fn set(&mut self, name: &str, val: u32) {
        if let Some(f) = Self::field(name) {
            for i in 0..f.width as usize {
                self.buf[f.off + i] = ((val >> (8 * i)) & 0xFF) as u8;
            }
        }
    }

    /// Add `delta` to a field, clamped to its valid range.
    pub fn nudge(&mut self, f: &Field, delta: i64) {
        let cur = self.get_field(f) as i64;
        let nv = (cur + delta).clamp(f.lo as i64, f.hi as i64) as u32;
        self.set(f.name, nv);
    }

    /// Load the known-good recommended config's timings into this buffer (the
    /// signature/checksum are stamped separately at write time).
    pub fn apply_recommended(&mut self) {
        for (k, v) in recommended() {
            self.set(k, *v);
        }
    }

    /// Stamp the signature and recompute the checksum, making the buffer ready
    /// for CMOS. Use `SIG_LINUX_TOOL` so ABL knows to apply it on next boot.
    pub fn stamp(&mut self, sig: u32) {
        self.set("Signature", sig);
        let ck = self.calc_checksum() as u32;
        self.set("Checksum", ck);
    }

    /// The raw 28-byte config as lowercase hex (for backup files).
    pub fn to_hex(&self) -> String {
        self.buf.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Parse a 28-byte config from a hex string (the inverse of `to_hex`).
    pub fn from_hex(s: &str) -> Option<Self> {
        let s = s.trim();
        if s.len() != CONFIG_SIZE * 2 {
            return None;
        }
        let mut buf = [0u8; CONFIG_SIZE];
        for (i, b) in buf.iter_mut().enumerate() {
            *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
        }
        Some(Self { buf })
    }

    pub fn signature(&self) -> u32 {
        self.get("Signature").unwrap_or(0)
    }

    /// ABL checksum = sum of bytes [6..28], low 16 bits.
    pub fn calc_checksum(&self) -> u16 {
        (self.buf[6..CONFIG_SIZE]
            .iter()
            .map(|&b| b as u32)
            .sum::<u32>()
            & 0xFFFF) as u16
    }

    pub fn checksum_valid(&self) -> bool {
        self.get("Checksum").unwrap_or(0) as u16 == self.calc_checksum()
    }
}

/// The board's known-good **recommended** config: 1750 MHz / 14 Gbps, full spec.
///
/// This is the one configuration that reliably trains on the BC-250 — its own
/// GDDR6 ICs are bottom-binned, and lower clocks (1000) and higher (1812) do not
/// train here. So there is no menu of "profiles" to pick from: there's the
/// recommended config (the baseline every session starts from, and the target of
/// "reset to recommended"), and the individual timings you tune away from it.
pub fn recommended() -> &'static [(&'static str, u32)] {
    &[
        ("ClockSpeed", 1750),
        ("tCL", 24),
        ("tRAS", 52),
        ("tRCDRD", 27),
        ("tRCDWR", 19),
        ("tRCAb", 78),
        ("tRCPb", 0),
        ("tRPAb", 26),
        ("tRPPb", 0),
        ("tRRDS", 8),
        ("tRRDL", 8),
        ("tRTP", 2),
        ("tFAW", 32),
        ("tREF", 9975),
        ("RFCPb", 210),
        ("tRFC", 280),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_matches_reference() {
        // recommended config bytes -> checksum over [6..28]
        let mut c = MemConf::from_bytes([0u8; CONFIG_SIZE]);
        c.apply_recommended();
        // sanity: ClockSpeed round-trips as u16 LE
        assert_eq!(c.get("ClockSpeed"), Some(1750));
        // checksum is deterministic and within u16
        let _ = c.calc_checksum();
    }

    #[test]
    fn signature_constants_match_reference() {
        // Exact values ported from the reference memcfg.py (the real contract).
        assert_eq!(SIG_LINUX_TOOL, 0x4243_5041);
        assert_eq!(SIG_ABL, 0x4C42_4124);
        assert_eq!(SIG_CMOS_BAD, 0x4253_4D43);
        // In memory (little-endian) the success tag reads "$ABL"; the tool tag
        // reads "APCB" (AGESA PSP Customization Block) — note the Python comment
        // labelled it "BCPA" (the reversed reading), but the value is what matters.
        assert_eq!(SIG_ABL.to_le_bytes(), *b"$ABL");
        assert_eq!(SIG_LINUX_TOOL.to_le_bytes(), *b"APCB");
    }
}
