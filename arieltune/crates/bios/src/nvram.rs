// SPDX-License-Identifier: GPL-2.0-only
//! Recover current BIOS-setting values from the SPI flash's AMI NVAR store.
//!
//! The OEM `Setup` UEFI variable is boot-service-only, so Linux can't read it
//! from `/sys/firmware/efi/efivars/` — which is why those settings otherwise
//! show only their catalogue default. But the variable's *actual* current value
//! is stored in the flash (the AMI NVAR variable store), and biostune already
//! reads the flash for the APCB. This module parses the NVAR store and returns
//! the current `Setup` and `AmdSetup` variable bodies.
//!
//! Validated: the `AmdSetup` body recovered here is byte-identical to the live
//! `AmdSetup` efivar on the board, which confirms the parser.

/// NVAR entry attribute bits (AMI NVAR format, per UEFITool).
const ATTR_VALID: u8 = 0x80;
const ATTR_DATA_ONLY: u8 = 0x08;
const ATTR_GUID_INLINE: u8 = 0x04;
const ATTR_ASCII_NAME: u8 = 0x02;

/// Expected body sizes (the OEM Setup / AMD CBS varstores) — used to skip
/// same-named decoy variables.
pub const SETUP_SIZE: usize = 0x1d3; // 467
pub const AMDSETUP_SIZE: usize = 0x8b5; // 2229

#[derive(Default)]
pub struct NvramVars {
    pub setup: Option<Vec<u8>>,
    pub amdsetup: Option<Vec<u8>>,
}

fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

/// 10-byte NVAR entry header: "NVAR"(4) + size(2) + next(3) + attr(1).
pub const DATA_HDR: usize = 10;

/// A resolved NVAR variable update chain: the named head, the live tail, and the
/// tail's body (the value the firmware actually uses).
pub struct Chain {
    /// flash offset of the tail entry (the live one) — where `next` gets re-pointed
    pub tail: usize,
    /// the tail entry's attribute byte
    pub tail_attr: u8,
    /// the live value (the tail's body)
    pub body: Vec<u8>,
}

/// Find the named head entry for `name` (optionally requiring an expected body
/// size to skip same-named decoys).
fn find_head(image: &[u8], name: &str, want: Option<usize>) -> Option<usize> {
    let n = image.len();
    let mut o = 0;
    while o + DATA_HDR <= n {
        if &image[o..o + 4] != b"NVAR" {
            o += 1;
            continue;
        }
        let size = u16le(image, o + 4) as usize;
        if size < 0x0a || o + size > n {
            o += 1;
            continue;
        }
        let attr = image[o + 9];
        if attr & ATTR_VALID != 0 && attr & ATTR_DATA_ONLY == 0 {
            // named entry: parse its name
            let mut p = o + DATA_HDR + if attr & ATTR_GUID_INLINE != 0 { 16 } else { 1 };
            // M1: a named entry whose size barely covers the header leaves no room
            // for a name (p can exceed o+size) — the name slice would underflow and
            // panic. Reachable from a corrupt `--image` dump / `flashrom -r`. Skip it.
            let name_end = (o + size).min(n);
            if p > name_end {
                o += size;
                continue;
            }
            let nm = if attr & ATTR_ASCII_NAME != 0 {
                let end = image[p..name_end]
                    .iter()
                    .position(|&b| b == 0)
                    .map(|i| p + i)
                    .unwrap_or(name_end);
                let s = String::from_utf8_lossy(&image[p..end]).into_owned();
                p = end + 1;
                s
            } else {
                let mut end = p;
                while end + 1 < name_end && !(image[end] == 0 && image[end + 1] == 0) {
                    end += 2;
                }
                image[p..end]
                    .chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .filter_map(|u| char::from_u32(u as u32))
                    .collect()
            };
            // M1: p may sit one past the entry end (name ran to the edge with no
            // terminator) — use checked_sub so the body-size compare can't underflow.
            let body_len = (o + size).checked_sub(p);
            if nm == name && want.map(|w| body_len == Some(w)).unwrap_or(true) {
                return Some(o);
            }
        }
        o += size;
    }
    None
}

/// Resolve a variable's NVAR update chain to its live value. AMI stores updates
/// as data-only entries linked from the head by a 3-byte forward-delta `next`
/// (0xFFFFFF = end); the live value is the TAIL's body — NOT the head's (which is
/// stale once a variable has been written). `want` is the expected body size.
pub fn resolve_chain(image: &[u8], name: &str, want: Option<usize>) -> Option<Chain> {
    let n = image.len();
    let head = find_head(image, name, want)?;
    let mut o = head;
    loop {
        let nxt =
            (image[o + 6] as usize) | (image[o + 7] as usize) << 8 | (image[o + 8] as usize) << 16;
        if nxt == 0xFFFFFF || nxt == 0 {
            break; // end of chain (0xFFFFFF) or no-update (0)
        }
        let cand = o + nxt;
        if cand <= o || cand + DATA_HDR > n || &image[cand..cand + 4] != b"NVAR" {
            break; // broken/non-forward chain — stop at last good
        }
        o = cand;
    }
    let tail = o;
    let size = u16le(image, tail + 4) as usize;
    // M2: the chain-tail was validated for "NVAR" magic + tail+DATA_HDR<=n only,
    // NOT that its size covers the header. A data-only tail with size < DATA_HDR
    // underflows `size - DATA_HDR` (debug panic; release wraps to a huge body_len
    // that then wraps the bounds check). Reachable from a corrupt flash image.
    if size < DATA_HDR || tail + size > n {
        return None;
    }
    let tail_attr = image[tail + 9];
    let tail_end = tail + size;
    let (body_off, body_len) = if tail_attr & ATTR_DATA_ONLY != 0 {
        // size >= DATA_HDR verified above, so this subtraction cannot underflow.
        (tail + DATA_HDR, size - DATA_HDR)
    } else {
        // no updates: body is after the head's header+guid+name
        let mut p = tail
            + DATA_HDR
            + if tail_attr & ATTR_GUID_INLINE != 0 {
                16
            } else {
                1
            };
        // M2: a short non-data-only tail can push p past the entry end — the name
        // slice would underflow/panic and `tail_end - p` would wrap. Bail cleanly.
        if p > tail_end {
            return None;
        }
        if tail_attr & ATTR_ASCII_NAME != 0 {
            let end = image[p..tail_end]
                .iter()
                .position(|&b| b == 0)
                .map(|i| p + i)
                .unwrap_or(tail_end);
            p = end + 1;
        } else {
            while p + 1 < tail_end && !(image[p] == 0 && image[p + 1] == 0) {
                p += 2;
            }
            p += 2;
        }
        // p may now sit past tail_end (name ran to the edge) — checked_sub -> None.
        match tail_end.checked_sub(p) {
            Some(len) => (p, len),
            None => return None,
        }
    };
    if body_off + body_len > n {
        return None;
    }
    let _ = head; // head located for clarity; only the tail is needed downstream
    Some(Chain {
        tail,
        tail_attr,
        body: image[body_off..body_off + body_len].to_vec(),
    })
}

/// First-free offset = end of the contiguous NVAR entry stream, where the trailing
/// 0xFF free pool begins (mid-store deleted-entry holes are not reused). This is
/// where the firmware appends new variable updates.
pub fn free_pool(image: &[u8]) -> Option<usize> {
    let n = image.len();
    let store = image.windows(4).position(|w| w == b"NVAR")?;
    if store == 0 || store >= 0x200 {
        return None;
    }
    let mut o = store;
    let mut last_end = store;
    while o + DATA_HDR <= n {
        if &image[o..o + 4] == b"NVAR" {
            let size = u16le(image, o + 4) as usize;
            if (0x0a..=n - o).contains(&size) {
                last_end = o + size;
                o += size;
                continue;
            }
        }
        if image[o] == 0xFF {
            let mut j = o;
            while j < n && image[j] == 0xFF {
                j += 1;
            }
            if j - o >= 0x1000 {
                break;
            }
            o = j;
            continue;
        }
        o += 1;
    }
    Some(last_end)
}

/// Parse the flash image and return the current Setup / AmdSetup bodies, following
/// each variable's update chain to its live value.
pub fn read_varstores(image: &[u8]) -> NvramVars {
    NvramVars {
        setup: resolve_chain(image, "Setup", Some(SETUP_SIZE)).map(|c| c.body),
        amdsetup: resolve_chain(image, "AmdSetup", Some(AMDSETUP_SIZE)).map(|c| c.body),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Build a tiny flash with one valid ASCII-named, guid-indexed NVAR "Setup".
    fn img_with_setup() -> Vec<u8> {
        let mut body = vec![0u8; SETUP_SIZE];
        body[0xda] = 1; // Above 4G "on"
                        // header(10) + guididx(1) + "Setup\0"(6) + body
        let payload = 1 + 6 + body.len();
        let size = 10 + payload;
        let mut e = Vec::new();
        e.extend_from_slice(b"NVAR");
        e.extend_from_slice(&(size as u16).to_le_bytes());
        e.extend_from_slice(&[0, 0, 0]); // next
        e.push(ATTR_VALID | ATTR_ASCII_NAME); // attr: valid, ascii name, guid-by-index
        e.push(0x10); // guid index
        e.extend_from_slice(b"Setup\0");
        e.extend_from_slice(&body);
        // pad some leading bytes so offset != 0
        let mut img = vec![0xffu8; 32];
        img.extend_from_slice(&e);
        img
    }

    #[test]
    fn recovers_setup_body() {
        let img = img_with_setup();
        let vars = read_varstores(&img);
        let s = vars.setup.expect("should find Setup");
        assert_eq!(s.len(), SETUP_SIZE);
        assert_eq!(s[0xda], 1);
        assert!(vars.amdsetup.is_none());
    }

    #[test]
    fn ignores_short_decoy() {
        // a "Setup"-named entry of the wrong size must be skipped
        let mut img = img_with_setup();
        // append a 6-byte "Setup" decoy
        let body = vec![0u8; 6];
        let size = 10 + 1 + 6 + body.len();
        let mut e = Vec::new();
        e.extend_from_slice(b"NVAR");
        e.extend_from_slice(&(size as u16).to_le_bytes());
        e.extend_from_slice(&[0, 0, 0]);
        e.push(ATTR_VALID | ATTR_ASCII_NAME);
        e.push(0x10);
        e.extend_from_slice(b"Setup\0");
        e.extend_from_slice(&body);
        img.extend_from_slice(&e);
        let vars = read_varstores(&img);
        assert_eq!(vars.setup.unwrap().len(), SETUP_SIZE); // not the 6-byte one
    }

    #[test]
    fn m1_named_entry_size_equals_header_does_not_panic() {
        // M1: a named (valid, not data-only) NVAR entry whose size == DATA_HDR
        // leaves no room for the name — the old code computed p = o+11 and sliced
        // image[o+11..o+10], panicking. Must skip gracefully.
        let mut img = vec![0xffu8; 16];
        img.extend_from_slice(b"NVAR");
        img.extend_from_slice(&(DATA_HDR as u16).to_le_bytes()); // size == header
        img.extend_from_slice(&[0, 0, 0]); // next
        img.push(ATTR_VALID | ATTR_ASCII_NAME); // named -> triggers name parse
                                                // no name/body bytes at all
                                                // Must not panic; nothing resolvable.
        let vars = read_varstores(&img);
        assert!(vars.setup.is_none());
        assert!(vars.amdsetup.is_none());
        // find_head over the malformed entry must return None, not panic.
        assert!(find_head(&img, "Setup", None).is_none());
    }

    #[test]
    fn m2_short_data_only_tail_returns_none_no_panic() {
        // M2: a valid named head whose update `next` points at a data-only tail
        // with size < DATA_HDR must resolve to None (not underflow `size-DATA_HDR`).
        let mut body = vec![0u8; SETUP_SIZE];
        body[0] = 0xaa;
        let payload = 1 + 6 + body.len(); // guididx + "Setup\0" + body
        let head_size = DATA_HDR + payload;
        let mut img = vec![0xffu8; 8];
        let head_off = img.len();
        img.extend_from_slice(b"NVAR");
        img.extend_from_slice(&(head_size as u16).to_le_bytes());
        // next: forward-delta to the malformed tail placed right after the head
        let next_delta = head_size;
        img.extend_from_slice(&[
            (next_delta & 0xff) as u8,
            ((next_delta >> 8) & 0xff) as u8,
            0,
        ]);
        img.push(ATTR_VALID | ATTR_ASCII_NAME);
        img.push(0x10); // guid index
        img.extend_from_slice(b"Setup\0");
        img.extend_from_slice(&body);
        // malformed data-only tail: size = 4 (< DATA_HDR), plus padding so the
        // cand+DATA_HDR<=n check passes and we actually reach the size guard.
        assert_eq!(img.len(), head_off + head_size);
        img.extend_from_slice(b"NVAR");
        img.extend_from_slice(&4u16.to_le_bytes()); // size < DATA_HDR
        img.extend_from_slice(&[0xff, 0xff, 0xff]); // next = end
        img.push(ATTR_VALID | ATTR_DATA_ONLY);
        img.extend_from_slice(&[0u8; 16]); // padding for bounds
                                           // Must not panic; the short tail is rejected -> None.
        assert!(resolve_chain(&img, "Setup", None).is_none());
    }
}
