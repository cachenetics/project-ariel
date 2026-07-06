// SPDX-License-Identifier: GPL-2.0-only
//! Extended-CMOS access via the legacy I/O ports.
//!
//! The BC-250 stores its `MemConf_t` at extended-CMOS offset 0x90, reached
//! through index/data ports 0x72/0x73 (the second 128-byte CMOS bank). We poke
//! those ports through `/dev/port`, where seeking to an address and doing a
//! 1-byte read/write performs `inb`/`outb` on that port. Requires root and a
//! kernel built with `CONFIG_DEVPORT`.

use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};

use anyhow::{anyhow, Context, Result};

use crate::config::{CONFIG_OFFSET, CONFIG_SIZE};

const CMOS_INDEX: u64 = 0x72;
const CMOS_DATA: u64 = 0x73;

fn open_port() -> Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/port")
        .map_err(|e| match e.kind() {
            // The overwhelmingly common first-run mistake — say it plainly.
            ErrorKind::PermissionDenied => {
                anyhow!("cannot access /dev/port: permission denied — run arieltune mem with sudo")
            }
            ErrorKind::NotFound => anyhow!(
                "/dev/port not found — the kernel needs CONFIG_DEVPORT \
                 (or this isn't a BC-250). Try `arieltune mem doctor`."
            ),
            _ => anyhow!("open /dev/port: {e}"),
        })
}

fn read_byte(port: &mut File, off: u8) -> Result<u8> {
    port.seek(SeekFrom::Start(CMOS_INDEX))?;
    port.write_all(&[off])?;
    port.seek(SeekFrom::Start(CMOS_DATA))?;
    let mut b = [0u8; 1];
    port.read_exact(&mut b)?;
    Ok(b[0])
}

fn write_byte(port: &mut File, off: u8, val: u8) -> Result<()> {
    port.seek(SeekFrom::Start(CMOS_INDEX))?;
    port.write_all(&[off])?;
    port.seek(SeekFrom::Start(CMOS_DATA))?;
    port.write_all(&[val])?;
    Ok(())
}

/// Snapshot the current CMOS config to a timestamped backup file and return its
/// path. Called automatically before any write so a known-good config is always
/// recoverable (`memtune restore <file>`).
pub fn auto_backup() -> Result<std::path::PathBuf> {
    let dir = std::path::Path::new("/var/lib/memtune/backups");
    std::fs::create_dir_all(dir).context("create backup dir")?;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let hex: String = read_config()?.iter().map(|b| format!("{b:02x}")).collect();
    let path = dir.join(format!("cmos-{ts}.hex"));
    std::fs::write(&path, hex).context("write backup file")?;
    Ok(path)
}

/// Read the 28-byte config block from extended CMOS.
pub fn read_config() -> Result<[u8; CONFIG_SIZE]> {
    let mut port = open_port()?;
    let mut buf = [0u8; CONFIG_SIZE];
    for (i, b) in buf.iter_mut().enumerate() {
        *b = read_byte(&mut port, CONFIG_OFFSET.wrapping_add(i as u8))?;
    }
    Ok(buf)
}

/// Write the 28-byte config block to extended CMOS.
///
/// SAFETY: this only takes effect on the NEXT boot, when ABL re-reads CMOS and
/// either trains the timings (stamps `$ABL`) or rejects them (stamps `CMOS_BAD`
/// / `WDT_FIRED`) and falls back to defaults. An over-aggressive set can cause a
/// no-POST recoverable only by a CMOS battery pull (or the `nvcmos` kernel
/// param). Callers must have validated ranges first.
pub fn write_config(buf: &[u8; CONFIG_SIZE]) -> Result<()> {
    // H2: identity-gate the actuator. The extended-CMOS index/data ports 0x72/0x73
    // are generic legacy CMOS ports — on a NON-BC-250 host that happens to expose
    // /dev/port, blindly `outb`-ing our MemConf_t here would corrupt that machine's
    // CMOS. Every other actuator (SMU q0/q3, SPI flash) is BC-250-gated; this one
    // was gated only on /dev/port accessibility.
    //
    // Gate on the APU-present probe (PCI 1002:13fe) rather than the full DMI
    // board verdict: the 1002:13fe APU only ships on the BC-250, so its presence
    // is a sufficient gate, and it does not block a genuine BC-250 whose DMI
    // identity has been rebranded/modded (which the full is_bc250() verdict would
    // refuse) from a legitimate CMOS restore.
    // Reads (read_config/auto_backup) stay ungated — reading CMOS is harmless.
    if !bc250_board::apu_present() {
        return Err(anyhow!(
            "refusing CMOS write: no Ariel APU on this host (PCI 1002:13fe not found). \
             The extended-CMOS ports 0x72/0x73 are generic — writing here could corrupt \
             a non-BC-250 machine's CMOS."
        ));
    }
    let mut port = open_port()?;
    for (i, &val) in buf.iter().enumerate() {
        write_byte(&mut port, CONFIG_OFFSET.wrapping_add(i as u8), val)?;
    }
    Ok(())
}
