// SPDX-License-Identifier: GPL-2.0-only
//! OEM `Setup` editing via SMM — the no-rig, no-variable-lock write path.
//!
//! The OEM `Setup` variable is boot-service-only (the OS can't `SetVariable` it)
//! and many of its knobs are suppressed in the menu. We change them by doing
//! exactly what the firmware does on an update: append ONE new data-only entry to
//! the variable's NVAR chain (a full copy of the live body with every changed
//! byte applied) and re-point the old tail's `next`. Both are programs into erased
//! (0xFF) flash = bit-clears only, so the AND-only SMM write (no erase) suffices.
//!
//! Crucially, an arbitrary number of fields are changed in a SINGLE append — so a
//! whole "save & exit" worth of OEM edits is one entry, the way a real BIOS would
//! commit them. (One append per field would needlessly grow the chain.) Verified
//! read-back at each step; on any mismatch we abort before activating, leaving the
//! variable unchanged.
//!
//! Proven live on the BC-250 (P3.00). The
//! firmware keeps no on-flash free pointer (it caches the free offset in DRAM from
//! a boot scan), so after an append the store is "SMM-dirty" until a reboot — see
//! `dirty.rs` for the guard that keeps a later firmware write from colliding.

use anyhow::{bail, Context, Result};

use crate::nvram::{self, DATA_HDR};
use crate::smm::Smm;

const A_VALID: u8 = 0x80;
const A_DATAONLY: u8 = 0x08;
const A_RUNTIME: u8 = 0x01;

/// Length of the first firmware volume (the NVRAM store) from its header.
fn fv_len(smm: &Smm) -> Result<usize> {
    let hdr = smm.read(0, 0x30).context("read FV header")?;
    if &hdr[0x28..0x2c] != b"_FVH" {
        bail!("first FV header not found (no _FVH at +0x28) — not an AMI flash?");
    }
    let len = u64::from_le_bytes(hdr[0x20..0x28].try_into().unwrap()) as usize;
    // sanity-cap: the NVRAM FV is small (128 KiB on the BC-250)
    Ok(len.clamp(0x1000, 0x80000))
}

/// Read the current value of one byte of an NVAR variable (live, via the update
/// chain). Returns (value, body_len). Used for dry-runs and verification.
pub fn oem_read(smm: &Smm, var: &str, want_size: usize, offset: usize) -> Result<(u8, usize)> {
    let n = fv_len(smm)?;
    let img = smm.read(0, n).context("read NVRAM FV")?;
    let chain = nvram::resolve_chain(&img, var, Some(want_size))
        .with_context(|| format!("variable {var:?} not found in NVAR store"))?;
    let blen = chain.body.len();
    if offset >= blen {
        bail!("offset 0x{offset:x} >= {var} body length 0x{blen:x}");
    }
    Ok((chain.body[offset], blen))
}

/// A planned NVAR-append (pure: computed from a flash image, no I/O).
enum AppendPlan {
    /// Every requested byte already holds its target value — nothing to do.
    AlreadySet,
    /// Write `entry` at `free`, then re-point the tail at `tail+6` by `delta`.
    Write {
        free: usize,
        entry: Vec<u8>,
        tail: usize,
        delta: usize,
    },
}

/// Plan the NVAR-append for applying `edits` (offset -> value) to `var` against
/// flash image `img`. Pure + deterministic so it can be unit-tested off-device.
fn plan_append(img: &[u8], var: &str, want: usize, edits: &[(usize, u8)]) -> Result<AppendPlan> {
    let chain = nvram::resolve_chain(img, var, Some(want))
        .with_context(|| format!("variable {var:?} not found in NVAR store"))?;
    let blen = chain.body.len();

    // Apply all edits to a copy of the live body.
    let mut body = chain.body.clone();
    for &(off, val) in edits {
        if off >= blen {
            bail!("offset 0x{off:x} >= {var} body length 0x{blen:x}");
        }
        body[off] = val;
    }
    if body == chain.body {
        return Ok(AppendPlan::AlreadySet);
    }

    // Build the new data-only update entry (full body, one entry for all edits).
    let new_attr = if chain.tail_attr & A_DATAONLY != 0 {
        chain.tail_attr
    } else {
        A_VALID | A_DATAONLY | (chain.tail_attr & A_RUNTIME)
    };
    let entry_size = DATA_HDR + blen;
    let mut entry = Vec::with_capacity(entry_size);
    entry.extend_from_slice(b"NVAR");
    entry.extend_from_slice(&(entry_size as u16).to_le_bytes());
    entry.extend_from_slice(&[0xff, 0xff, 0xff]); // next = end of chain
    entry.push(new_attr);
    entry.extend_from_slice(&body);
    debug_assert_eq!(entry.len(), entry_size);

    let free = nvram::free_pool(img).context("could not find NVAR free pool")?;
    let need = entry_size + 0x10;
    if free + need > img.len() || img[free..free + need].iter().any(|&b| b != 0xFF) {
        bail!(
            "NVAR free pool exhausted/not-erased at 0x{free:x} — the variable store is full of \
             update entries. A reboot (firmware compaction) or an NVRAM clear reclaims it."
        );
    }
    let delta = free
        .checked_sub(chain.tail)
        .filter(|d| *d > 0 && *d < 0x1000000)
        .with_context(|| format!("bad next delta from tail 0x{:x} to 0x{free:x}", chain.tail))?;

    Ok(AppendPlan::Write {
        free,
        entry,
        tail: chain.tail,
        delta,
    })
}

/// Apply a batch of byte edits to an NVAR variable in ONE append, via SMM.
/// Returns `Some(free_offset)` if an entry was written, `None` if every edit was
/// already at its target. `want_size` is the expected variable body size.
pub fn oem_set(
    smm: &Smm,
    var: &str,
    want_size: usize,
    edits: &[(usize, u8)],
) -> Result<Option<usize>> {
    let n = fv_len(smm)?;
    let img = smm.read(0, n).context("read NVRAM FV")?;

    let (free, entry, tail, delta) = match plan_append(&img, var, want_size, edits)? {
        AppendPlan::AlreadySet => return Ok(None),
        AppendPlan::Write {
            free,
            entry,
            tail,
            delta,
        } => (free, entry, tail, delta),
    };

    // 1) write the entry, signature LAST (a partial write is never a valid entry)
    smm.write((free + 4) as u32, &entry[4..])
        .context("write new entry body")?;
    smm.write(free as u32, b"NVAR")
        .context("write entry signature")?;
    if smm.read(free as u32, entry.len())? != entry {
        bail!("new-entry read-back mismatch — NOT activated; {var} unchanged");
    }
    // 2) activate: shrink old tail's `next` (0xFFFFFF -> delta); all bit-clears
    let nb = (delta as u32).to_le_bytes();
    smm.write((tail + 6) as u32, &nb[..3])
        .context("patch tail.next")?;
    if smm.read((tail + 6) as u32, 3)? != nb[..3] {
        bail!("tail.next read-back mismatch");
    }
    // 3) re-resolve live and confirm every edit landed
    let img2 = smm.read(0, n)?;
    let c2 = nvram::resolve_chain(&img2, var, Some(want_size)).context("re-resolve after write")?;
    for &(off, val) in edits {
        let got = c2.body.get(off).copied().unwrap_or(0xFF);
        if got != val {
            bail!("post-write {var}+0x{off:x} is 0x{got:02x}, expected 0x{val:02x}");
        }
    }
    Ok(Some(free))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Build a minimal NVAR store: a named "Setup" head linked to one data-only
    // update entry, then a 0xFF free pool. Returns (img, tail, free).
    fn store_with_chain(head_b4g: u8, tail_b4g: u8) -> (Vec<u8>, usize, usize) {
        const SZ: usize = 0x1d3;
        let mut img = vec![0u8; 0x80];
        let head = img.len();
        let mut hbody = vec![0u8; SZ];
        hbody[0xda] = head_b4g;
        let hname = b"Setup\0";
        let hsize = DATA_HDR + 1 + hname.len() + SZ;
        let tail = head + hsize;
        img.extend_from_slice(b"NVAR");
        img.extend_from_slice(&(hsize as u16).to_le_bytes());
        img.extend_from_slice(&(tail - head).to_le_bytes()[..3]);
        img.push(A_VALID | 0x02);
        img.push(0x05);
        img.extend_from_slice(hname);
        img.extend_from_slice(&hbody);
        let mut tbody = vec![0u8; SZ];
        tbody[0xda] = tail_b4g;
        let tsize = DATA_HDR + SZ;
        img.extend_from_slice(b"NVAR");
        img.extend_from_slice(&(tsize as u16).to_le_bytes());
        img.extend_from_slice(&[0xff, 0xff, 0xff]);
        img.push(A_VALID | A_DATAONLY);
        img.extend_from_slice(&tbody);
        let free = img.len();
        img.resize(free + 0x4000, 0xFF);
        (img, tail, free)
    }

    #[test]
    fn resolve_chain_returns_tail_not_head() {
        let (img, tail, _free) = store_with_chain(0, 1);
        let c = nvram::resolve_chain(&img, "Setup", Some(0x1d3)).unwrap();
        assert_eq!(c.tail, tail);
        assert_eq!(
            c.body[0xda], 1,
            "must read the chain tail, not the stale head"
        );
    }

    #[test]
    fn plan_append_one_entry_for_many_fields() {
        // change THREE fields at once -> exactly ONE new entry carrying all three.
        let (img, tail, free) = store_with_chain(0, 0);
        let edits = [(0xda_usize, 1u8), (0x05, 1), (0xb0, 1)];
        match plan_append(&img, "Setup", 0x1d3, &edits).unwrap() {
            AppendPlan::Write {
                free: f,
                entry,
                tail: t,
                delta,
            } => {
                assert_eq!((f, t, delta), (free, tail, free - tail));
                assert_eq!(&entry[0..4], b"NVAR");
                assert_eq!(entry[9] & A_DATAONLY, A_DATAONLY);
                assert_eq!(entry.len(), DATA_HDR + 0x1d3);
                // all three edits present in the one entry's body
                assert_eq!(entry[DATA_HDR + 0xda], 1);
                assert_eq!(entry[DATA_HDR + 0x05], 1);
                assert_eq!(entry[DATA_HDR + 0xb0], 1);
                assert_eq!(&entry[6..9], &[0xff, 0xff, 0xff]);
            }
            _ => panic!("expected a Write plan"),
        }
    }

    #[test]
    fn plan_append_all_already_set_is_noop() {
        let (img, _t, _f) = store_with_chain(0, 1);
        assert!(matches!(
            plan_append(&img, "Setup", 0x1d3, &[(0xda, 1)]).unwrap(),
            AppendPlan::AlreadySet
        ));
    }

    #[test]
    fn plan_append_rejects_full_pool() {
        let (mut img, _t, free) = store_with_chain(0, 0);
        for b in img[free..].iter_mut() {
            *b = 0x5a;
        }
        assert!(plan_append(&img, "Setup", 0x1d3, &[(0xda, 1)]).is_err());
    }
}
