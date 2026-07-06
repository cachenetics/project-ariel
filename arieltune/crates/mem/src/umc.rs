// SPDX-License-Identifier: GPL-2.0-only
//! Live UMC (memory controller) register readback via the `bc250_smu` kernel
//! module's SMN ioctl.
//!
//! The BC-250's UMC sits at SMN base `0x14000` (verified; non-FUSE, so
//! reads are safe — the FUSE region `0x17400+` is the wedge hazard). A read-only,
//! advanced view of the controller's live state; needs the community `bc250_smu`
//! module loaded.
//!
//! Decode notes (from the umc_6_7_0 cross-ref + on-board probing):
//!   * The readable low window (`+0x000..+0x09C`) holds 8 static config regs and
//!     5 free-running counters (`+0x028..+0x038` — refresh/activity, they climb).
//!   * The DRAM *timing* registers (tCL/tRCD/tREF) are NOT in this window — they
//!     sit higher, past an ENODEV gap, so they aren't exposed here.
//!   * ECC is DISABLED on this consumer GDDR6 (`EccCtrl = 0`), so the hardware
//!     `EccErrCnt` stability counter is unavailable — the software integrity
//!     check (`memtune bench`) is the stability signal instead.

use std::fs::OpenOptions;
use std::os::fd::AsRawFd;
use std::time::Duration;

use anyhow::{Context, Result};

const DEV: &str = "/dev/bc250-smu";
/// `_IOWR('B', 6, struct { u32 reg; u32 val; })`
const SMN_READ: libc::c_ulong = 0xC008_4206;
const UMC_BASE: u32 = 0x0001_4000;
/// umc_6_7_0 `regUMCCH0_0_EccCtrl` = dword 0x53 → byte 0x14C.
const ECC_CTRL: u32 = UMC_BASE + 0x53 * 4;

pub struct Reg {
    pub off: u32,
    pub val: u32,
    pub counter: bool,
}

fn open_dev() -> Result<std::fs::File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(DEV)
        .with_context(|| format!("open {DEV} — is the bc250_smu kernel module loaded?"))
}

fn smn_read(fd: i32, addr: u32) -> Option<u32> {
    let mut buf = [0u8; 8];
    buf[0..4].copy_from_slice(&addr.to_ne_bytes());
    // SAFETY: SMN_READ takes a *mut struct{u32 reg; u32 val} = our 8-byte buf.
    let rc = unsafe { libc::ioctl(fd, SMN_READ, buf.as_mut_ptr()) };
    (rc == 0).then(|| u32::from_ne_bytes([buf[4], buf[5], buf[6], buf[7]]))
}

/// Read the live UMC window, classifying each non-zero reg as a static config
/// reg or a free-running counter (two passes ~150ms apart — counters move).
pub fn read() -> Result<Vec<Reg>> {
    let f = open_dev()?;
    let fd = f.as_raw_fd();
    let scan = |fd: i32| {
        let mut m = std::collections::BTreeMap::new();
        for off in (0u32..0x100).step_by(4) {
            match smn_read(fd, UMC_BASE + off) {
                Some(v) if v != 0 && v != 0xFFFF_FFFF => {
                    m.insert(off, v);
                }
                Some(_) => {}
                None => break, // end of readable window
            }
        }
        m
    };
    let a = scan(fd);
    std::thread::sleep(Duration::from_millis(150));
    let b = scan(fd);
    Ok(a.iter()
        .map(|(&off, &val)| Reg {
            off,
            val,
            counter: b.get(&off) != Some(&val),
        })
        .collect())
}

/// The stable set of "interesting" offsets to display — the union of registers
/// non-zero in any of a few quick reads. Captured once when the live view opens
/// so rows don't appear/disappear as a register dips through zero.
pub fn live_offsets() -> Result<Vec<u32>> {
    let f = open_dev()?;
    let fd = f.as_raw_fd();
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..4 {
        for off in (0u32..0x100).step_by(4) {
            match smn_read(fd, UMC_BASE + off) {
                Some(v) if v != 0 && v != 0xFFFF_FFFF => {
                    seen.insert(off);
                }
                Some(_) => {}
                None => break,
            }
        }
    }
    Ok(seen.into_iter().collect())
}

/// Read the current values of a fixed offset list (single pass) — for the live
/// view, so the displayed rows stay put while values update.
pub fn read_values(offsets: &[u32]) -> Result<Vec<Reg>> {
    let f = open_dev()?;
    let fd = f.as_raw_fd();
    Ok(offsets
        .iter()
        .map(|&off| Reg {
            off,
            val: smn_read(fd, UMC_BASE + off).unwrap_or(0),
            counter: (0x028..=0x038).contains(&off),
        })
        .collect())
}

/// Is GDDR6 ECC enabled? (None if the control reg isn't readable.)
pub fn ecc_enabled() -> Option<bool> {
    let f = open_dev().ok()?;
    smn_read(f.as_raw_fd(), ECC_CTRL).map(|v| v != 0)
}

/// CLI: print the live UMC registers + ECC status.
pub fn dump() -> Result<()> {
    let regs = read()?;
    println!("UMC (memory controller) live registers @ SMN 0x{UMC_BASE:05X}:");
    if regs.is_empty() {
        println!("  (no live registers — UMC not enumerated here?)");
    }
    for r in &regs {
        let tag = if r.counter {
            "  live counter (refresh/activity)"
        } else {
            "  config"
        };
        println!("  +0x{:03X} = 0x{:08X}{tag}", r.off, r.val);
    }
    match ecc_enabled() {
        Some(true) => println!("\nECC: ENABLED — EccErrCnt would track errors."),
        Some(false) => println!(
            "\nECC: disabled (EccCtrl=0) — no hardware error counter on this GDDR6;\n\
             use `arieltune mem bench` (integrity check) for stability."
        ),
        None => {}
    }
    println!(
        "Note: DRAM timing registers (tCL/tRCD/tREF) aren't in the readable window.\n\
         Non-FUSE region, read-only."
    );
    Ok(())
}
