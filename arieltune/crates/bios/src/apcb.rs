// SPDX-License-Identifier: GPL-2.0-only
//! APCB (AGESA PSP Customization Block) parse + checksum-preserving edit.
//!
//! On the BC-250 the `$BHD` entry type 0x60 at SPI flash `0xAB1000` is an APCB
//! (V3): the unsigned, compile-time hardware-init config the PSP's ABL3 stage
//! reads before the x86 cores leave reset. It programs DRAM training, fabric
//! topology, NBIO/PCIe, and FCH. Editing a token here and reflashing is the one
//! path that actually changes those settings on this board (the parallel
//! `Setup`/`AmdSetup` EFI variables are decorative — AGESA acts on this copy
//! first). Ground-truthed against a real board dump.
//!
//! Layout (V3):
//!   +0x00  magic "APCB"
//!   +0x04  header_size (u16, = 0x20)
//!   +0x06  version     (u16, = 0x20)
//!   +0x08  total_size  (u32, e.g. 2920 — excludes the 0xFF pad to 8192)
//!   +0x10  checksum    (u8 — chosen so sum(bytes[..total_size]) % 256 == 0)
//!   +0x20  first group header (ASCII magic: PSPG/DFG /MEMG/FCHG/CBSG)
//!
//! The sum-to-zero scheme means any byte edit is balanced by adjusting the
//! checksum byte at +0x10 — `rebalance_checksum` does that, and every mutator
//! calls it.

use anyhow::{anyhow, bail, Context, Result};

/// `$BHD` body (= APCB) flash offset on the BC-250.
pub const BHD_BODY_ADDR: usize = 0x00ab_1000;
/// Allocated APCB slot size (the rest is 0xFF padding).
pub const APCB_SLOT: usize = 8192;

const MAGIC: &[u8; 4] = b"APCB";
const CKSUM_OFF: usize = 0x10;
const HEADER_SIZE: usize = 0x20;

/// Known APCB group magics (a stripped group means its tokens are cosmetic).
pub const KNOWN_GROUPS: &[(&[u8; 4], &str)] = &[
    (b"PSPG", "PSP — boot/debug/secure policy"),
    (b"CCXG", "CPU complex — downcore, SMT, prefetch"),
    (b"DFG ", "Data Fabric — Infinity Fabric tuning"),
    (b"MEMG", "Memory — DRAM training, MEMCLK, UMA"),
    (b"GNBG", "GPU/NBIO — CU mask, VCN, harvest"),
    (b"FCHG", "FCH chipset — USB, SATA, SMBus"),
    (b"CBSG", "CBS — Common BIOS Settings tokens"),
];

#[derive(Clone, Copy, Debug)]
pub struct Header {
    pub header_size: u16,
    pub version: u16,
    pub total_size: u32,
    pub checksum: u8,
}

#[derive(Clone, Debug)]
pub struct Group {
    pub magic: [u8; 4],
    pub label: String,
    pub offset: usize,
    pub size: usize,
    pub body_offset: usize,
    pub body_size: usize,
}

impl Group {
    pub fn name(&self) -> String {
        String::from_utf8_lossy(&self.magic).trim().to_string()
    }
}

/// A CBS token record inside the CBSG group: a 4-byte `[Pri][Ord][Flag][Value]`
/// quad the ABL3 token parser walks (layout verified against a real BC-250 token
/// trace). The token id is `Ord<<8 | Pri`; the
/// **Value byte is the enable bit** (0x00 = the token is gated off, so AGESA
/// does not apply the matching AmdSetup value).
#[derive(Clone, Debug)]
pub struct TokenRec {
    pub offset: usize,
    pub pri: u8,
    pub ord: u8,
    pub flag: u8,
    pub value: u8,
}

impl TokenRec {
    /// Token id as reported in the trace (e.g. Ord 0x15, Pri 0x01 -> 0x1501).
    pub fn id(&self) -> u16 {
        ((self.ord as u16) << 8) | self.pri as u16
    }
    /// Whether AGESA will act on this token (enable byte is non-zero).
    pub fn enabled(&self) -> bool {
        self.value != 0
    }
}

/// An owned APCB image — exactly the 8192-byte slot.
#[derive(Clone)]
pub struct Apcb {
    pub buf: Vec<u8>,
}

fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

impl Apcb {
    /// Parse + validate an APCB slot. Rejects bad magic or a broken checksum
    /// (we refuse to edit an APCB we can't trust).
    pub fn parse(buf: Vec<u8>) -> Result<Self> {
        if buf.len() < HEADER_SIZE {
            bail!("APCB too small: {} bytes", buf.len());
        }
        if &buf[..4] != MAGIC {
            bail!("bad APCB magic: {:02x?} (not an APCB)", &buf[..4]);
        }
        let a = Apcb { buf };
        let h = a.header();
        if h.total_size as usize > a.buf.len() {
            bail!(
                "APCB total_size 0x{:x} exceeds slot 0x{:x}",
                h.total_size,
                a.buf.len()
            );
        }
        if !a.verify_checksum() {
            bail!("APCB checksum invalid (sum mod 256 != 0) — refusing to use");
        }
        Ok(a)
    }

    pub fn header(&self) -> Header {
        Header {
            header_size: u16le(&self.buf, 4),
            version: u16le(&self.buf, 6),
            total_size: u32le(&self.buf, 8),
            checksum: self.buf[CKSUM_OFF],
        }
    }

    pub fn total_size(&self) -> usize {
        self.header().total_size as usize
    }

    /// True if sum(bytes[..total_size]) % 256 == 0.
    pub fn verify_checksum(&self) -> bool {
        let n = self.total_size().min(self.buf.len());
        self.buf[..n].iter().fold(0u8, |a, &b| a.wrapping_add(b)) == 0
    }

    /// Set the +0x10 byte so the sum-to-zero invariant holds again.
    pub fn rebalance_checksum(&mut self) {
        let n = self.total_size().min(self.buf.len());
        self.buf[CKSUM_OFF] = 0;
        let s = self.buf[..n].iter().fold(0u8, |a, &b| a.wrapping_add(b));
        self.buf[CKSUM_OFF] = s.wrapping_neg();
        debug_assert!(self.verify_checksum());
    }

    /// Locate the known group magics (searched after the header).
    pub fn groups(&self) -> Vec<Group> {
        let end = self.total_size();
        let mut raw: Vec<(usize, [u8; 4], String)> = Vec::new();
        for (magic, label) in KNOWN_GROUPS {
            if let Some(off) = find(&self.buf[..end], **magic, HEADER_SIZE) {
                let mut m = [0u8; 4];
                m.copy_from_slice(*magic);
                raw.push((off, m, label.to_string()));
            }
        }
        raw.sort_by_key(|r| r.0);
        let starts: Vec<usize> = raw
            .iter()
            .map(|r| r.0)
            .chain(std::iter::once(end))
            .collect();
        raw.iter()
            .enumerate()
            .map(|(i, (off, magic, label))| {
                let size = starts[i + 1] - off;
                Group {
                    magic: *magic,
                    label: label.clone(),
                    offset: *off,
                    size,
                    body_offset: off + GROUP_HEADER_SIZE,
                    body_size: size.saturating_sub(GROUP_HEADER_SIZE),
                }
            })
            .collect()
    }

    pub fn group(&self, magic: &[u8; 4]) -> Option<Group> {
        self.groups().into_iter().find(|g| &g.magic == magic)
    }

    /// Standard AMD groups absent from this APCB (their tokens are cosmetic).
    pub fn stripped_groups(&self) -> Vec<String> {
        let present: Vec<[u8; 4]> = self.groups().iter().map(|g| g.magic).collect();
        KNOWN_GROUPS
            .iter()
            .filter(|(m, _)| (**m == *b"CCXG" || **m == *b"GNBG") && !present.contains(*m))
            .map(|(m, _)| String::from_utf8_lossy(*m).trim().to_string())
            .collect()
    }

    /// Walk the CBSG group's CBS token records (`[Pri][Ord][Flag][Value]`,
    /// 4-byte quads). Collects records whose Pri byte is 0x01 — the token
    /// marker. The Value byte is each token's enable bit.
    pub fn cbsg_tokens(&self) -> Vec<TokenRec> {
        let Some(cbsg) = self.group(b"CBSG") else {
            return Vec::new();
        };
        let end = (cbsg.offset + cbsg.size).min(self.total_size());
        let mut out = Vec::new();
        let mut p = cbsg.body_offset;
        while p + 4 <= end {
            if self.buf[p] == 0x01 {
                out.push(TokenRec {
                    offset: p,
                    pri: self.buf[p],
                    ord: self.buf[p + 1],
                    flag: self.buf[p + 2],
                    value: self.buf[p + 3],
                });
            }
            p += 4;
        }
        out
    }

    /// Find a CBS token by its id (`Ord<<8 | Pri`).
    pub fn cbs_token(&self, id: u16) -> Option<TokenRec> {
        self.cbsg_tokens().into_iter().find(|t| t.id() == id)
    }

    /// Set a CBS token's enable byte (the Value byte, at token offset + 3) and
    /// rebalance the checksum. `enable` true -> 0x01, false -> 0x00.
    pub fn set_token_enable(&mut self, id: u16, enable: bool) -> Result<()> {
        let tok = self
            .cbs_token(id)
            .ok_or_else(|| anyhow!("CBS token 0x{id:04x} not present in this APCB"))?;
        self.set_u8(tok.offset + 3, if enable { 0x01 } else { 0x00 })
    }

    // ---- field access (bounds- + checksum-safe) ----------------------------

    fn check_off(&self, off: usize, width: usize) -> Result<()> {
        // checked_add so a caller-supplied off/width near usize::MAX can't wrap the
        // bounds check into a false pass.
        let end = off
            .checked_add(width)
            .ok_or_else(|| anyhow::anyhow!("offset +0x{off:x} (+{width}) overflows"))?;
        if end > self.total_size() {
            bail!(
                "offset +0x{off:x} (+{width}) is outside the APCB body (total 0x{:x})",
                self.total_size()
            );
        }
        Ok(())
    }

    /// Write a byte and rebalance the checksum.
    pub fn set_u8(&mut self, off: usize, val: u8) -> Result<()> {
        self.check_off(off, 1)?;
        self.buf[off] = val;
        self.rebalance_checksum();
        Ok(())
    }
}

/// coreboot APCB inter-group header size.
const GROUP_HEADER_SIZE: usize = 16;

fn find(hay: &[u8], needle: [u8; 4], from: usize) -> Option<usize> {
    if from >= hay.len() {
        return None;
    }
    hay[from..]
        .windows(4)
        .position(|w| w == needle)
        .map(|p| p + from)
}

// ---- whole-image helpers ---------------------------------------------------

/// Find the APCB inside a full SPI image: try the canonical `$BHD` offset, else
/// scan for the magic.
pub fn locate(image: &[u8]) -> Result<usize> {
    if image.len() >= BHD_BODY_ADDR + 4 && &image[BHD_BODY_ADDR..BHD_BODY_ADDR + 4] == MAGIC {
        return Ok(BHD_BODY_ADDR);
    }
    find(image, *MAGIC, 0).ok_or_else(|| anyhow!("no APCB magic found in image"))
}

/// Extract the 8192-byte APCB slot from a full SPI image.
pub fn extract(image: &[u8], addr: usize) -> Result<Apcb> {
    let end = addr + APCB_SLOT;
    if image.len() < end {
        bail!("image too small for APCB slot at 0x{addr:x}");
    }
    Apcb::parse(image[addr..end].to_vec())
}

/// Splice an edited APCB slot back into a full SPI image (in place).
pub fn splice(image: &mut [u8], addr: usize, apcb: &Apcb) -> Result<()> {
    let end = addr + apcb.buf.len();
    image
        .get_mut(addr..end)
        .context("image too small to splice APCB")?
        .copy_from_slice(&apcb.buf);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // A minimal synthetic APCB: header + a CBSG group with two CBS token records
    // `[Pri][Ord][Flag][Value]`, checksummed. Exercises parse / token walk /
    // enable-bit edit / checksum without a real dump (the real-dump parity check
    // is the `--image` integration test).
    fn synth() -> Apcb {
        let total = 0x60usize;
        let mut b = vec![0u8; APCB_SLOT];
        b[..4].copy_from_slice(MAGIC);
        b[4..6].copy_from_slice(&0x20u16.to_le_bytes()); // header_size
        b[6..8].copy_from_slice(&0x20u16.to_le_bytes()); // version
        b[8..12].copy_from_slice(&(total as u32).to_le_bytes());
        // CBSG group header at 0x20, body at 0x30
        b[0x20..0x24].copy_from_slice(b"CBSG");
        // two tokens: 0x1501 disabled, 0x0601 enabled
        b[0x30..0x34].copy_from_slice(&[0x01, 0x15, 0x20, 0x00]); // id 0x1501, value 0 (off)
        b[0x34..0x38].copy_from_slice(&[0x01, 0x06, 0x61, 0x01]); // id 0x0601, value 1 (on)
        let s = b[..total].iter().fold(0u8, |a, &x| a.wrapping_add(x));
        b[CKSUM_OFF] = s.wrapping_neg();
        Apcb::parse(b).unwrap()
    }

    #[test]
    fn parse_and_checksum() {
        let a = synth();
        assert!(a.verify_checksum());
        assert_eq!(a.header().version, 0x20);
    }

    #[test]
    fn tokens_parse_with_enable_state() {
        let a = synth();
        let toks = a.cbsg_tokens();
        assert_eq!(toks.len(), 2);
        let t1501 = a.cbs_token(0x1501).unwrap();
        assert!(!t1501.enabled());
        let t0601 = a.cbs_token(0x0601).unwrap();
        assert!(t0601.enabled());
    }

    #[test]
    fn set_token_enable_rebalances() {
        let mut a = synth();
        a.set_token_enable(0x1501, true).unwrap();
        assert!(a.verify_checksum(), "checksum must stay valid after edit");
        assert!(a.cbs_token(0x1501).unwrap().enabled());
        // disabling back also stays valid
        a.set_token_enable(0x1501, false).unwrap();
        assert!(a.verify_checksum());
        assert!(!a.cbs_token(0x1501).unwrap().enabled());
    }

    #[test]
    fn unknown_token_errors() {
        let mut a = synth();
        assert!(a.set_token_enable(0xdead, true).is_err());
    }

    #[test]
    fn bad_magic_rejected() {
        let mut b = vec![0u8; APCB_SLOT];
        b[..4].copy_from_slice(b"XXXX");
        assert!(Apcb::parse(b).is_err());
    }

    #[test]
    fn extract_splice_roundtrip() {
        let a = synth();
        let mut image = vec![0xffu8; BHD_BODY_ADDR + APCB_SLOT];
        splice(&mut image, BHD_BODY_ADDR, &a).unwrap();
        assert_eq!(locate(&image).unwrap(), BHD_BODY_ADDR);
        let b = extract(&image, BHD_BODY_ADDR).unwrap();
        assert_eq!(b.buf, a.buf);
    }
}
