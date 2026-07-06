// SPDX-License-Identifier: GPL-2.0-only
//! ariel-hal — silicon-generic low-level access for the Ariel APU.
//!
//! Ariel is the AMD Cyan Skillfish APU (PCI 1002:13fe) at the heart of the
//! BC-250. This crate holds the primitives every higher tuner builds on:
//!
//! - APU presence ([`ariel_apu_present`]) — the 1002:13fe PCI scan.
//! - amdgpu debugfs discovery ([`amdgpu_dbg_dir`]).
//! - the running kernel release ([`running_kernel`]).
//! - the raw SMN aperture ([`SmnAperture`]) — PCI-config index/data access at
//!   0xB8/0xBC on `0000:00:00.0`, the generic SMN read/write primitive that
//!   ariel-smu's queue-3 mailbox is built on.
//!
//! Nothing here is board-specific: board identity (DMI, BC-250 vs a future
//! carrier) lives in `bc250-board`.

use std::fs::{self, File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// The Ariel APU is PCI 1002:13fe (Cyan Skillfish). This confirms the APU
/// silicon is present; it does NOT confirm the board (that is `bc250-board`).
/// Renamed from the old `is_bc250()` because 1002:13fe is the APU id, not the
/// board id.
pub fn ariel_apu_present() -> bool {
    // /sys/bus/pci/devices/*/device == 0x13fe AND vendor == 0x1002
    let Ok(entries) = fs::read_dir("/sys/bus/pci/devices") else {
        return false;
    };
    for e in entries.flatten() {
        let dev = fs::read_to_string(e.path().join("device")).unwrap_or_default();
        let ven = fs::read_to_string(e.path().join("vendor")).unwrap_or_default();
        if dev.trim().eq_ignore_ascii_case("0x13fe") && ven.trim().eq_ignore_ascii_case("0x1002") {
            return true;
        }
    }
    false
}

/// The running kernel release (`uname -r`), read from procfs (no subprocess).
pub fn running_kernel() -> String {
    fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".into())
}

/// Locate the amdgpu debugfs directory (`/sys/kernel/debug/dri/<n>`). The
/// numbered dirs are not stable; pick the one that exposes amdgpu nodes.
pub fn amdgpu_dbg_dir() -> Option<PathBuf> {
    let root = Path::new("/sys/kernel/debug/dri");
    let entries = fs::read_dir(root).ok()?;
    for e in entries.flatten() {
        let p = e.path();
        if p.join("amdgpu_pm_info").exists()
            || p.join("amdgpu_gpu_recover").exists()
            || p.join("amdgpu_fence_info").exists()
        {
            return Some(p);
        }
    }
    None
}

/// Preflight: refuse unless running as root. The SMN aperture + amdgpu debugfs
/// nodes are 0600 root-owned; a non-root caller only gets confusing EACCES
/// deeper in. Optional convenience — callers may check euid themselves.
pub fn require_root() -> Result<()> {
    // SAFETY: geteuid is always safe (no args, no memory).
    let euid = unsafe { libc::geteuid() };
    if euid != 0 {
        anyhow::bail!("must run as root (euid 0); current euid {euid}");
    }
    Ok(())
}

/// The APU root function exposes the SMN aperture in PCI config space.
const CFG_BDF: &str = "0000:00:00.0";
const SMN_INDEX: u64 = 0xB8;
const SMN_DATA: u64 = 0xBC;

/// The generic SMN read/write primitive: PCI-config index (0xB8) / data (0xBC)
/// on `0000:00:00.0`. Higher mailbox protocols (ariel-smu's queue-3 OcQ3) build
/// on top of this.
///
/// SAFETY — why this SMN path is not a footgun:
///   * The SMN aperture used (PCI-config 0xB8 index / 0xBC data) is independent
///     of amdgpu's runtime MMIO SMN aperture, so the index latch can't be
///     stomped by the driver mid-transaction.
///   * Callers `flock(LOCK_EX)` the config fd ([`SmnAperture::lock`]) to
///     serialize their own accesses.
///   * Short/failed transfers are NEVER swallowed (see [`SmnAperture::wc`] /
///     [`SmnAperture::rc`]): a short SMN write would leave a STALE argument
///     register in place, and firing a command anyway could send a completely
///     different request (e.g. a massive overvolt from a leftover Vid arg).
pub struct SmnAperture {
    f: File,
}

impl SmnAperture {
    /// Open the SMN aperture on the APU root function (`0000:00:00.0`). Requires
    /// root; does not probe any mailbox.
    pub fn open() -> Result<Self> {
        let path = format!("/sys/bus/pci/devices/{CFG_BDF}/config");
        let f = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("open {path} (need root)"))?;
        Ok(SmnAperture { f })
    }

    /// Serialize our own SMN accesses. Hold this across a full index+data
    /// transaction (and across a whole mailbox send) so the index latch is not
    /// stomped between writing the register selector and the data.
    pub fn lock(&self) {
        unsafe { libc::flock(self.f.as_raw_fd(), libc::LOCK_EX) };
    }
    pub fn unlock(&self) {
        unsafe { libc::flock(self.f.as_raw_fd(), libc::LOCK_UN) };
    }

    // I/O errors here are NEVER swallowed: a failed/short SMN write would leave
    // a STALE argument register in place, and firing the command anyway could
    // send a completely different request (e.g. a massive overvolt from a
    // leftover Vid arg). Every transfer must be verified before proceeding.
    fn wc(&self, off: u64, v: u32) -> std::io::Result<()> {
        let n = self.f.write_at(&v.to_le_bytes(), off)?;
        if n != 4 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                format!("short PCI-config write at 0x{off:x} ({n}/4 bytes)"),
            ));
        }
        Ok(())
    }
    fn rc(&self, off: u64) -> std::io::Result<u32> {
        let mut b = [0u8; 4];
        let n = self.f.read_at(&mut b, off)?;
        if n != 4 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!("short PCI-config read at 0x{off:x} ({n}/4 bytes)"),
            ));
        }
        Ok(u32::from_le_bytes(b))
    }

    /// Write `v` to SMN register `reg` (index latch then data). Guarded against
    /// short transfers.
    pub fn wreg(&self, reg: u32, v: u32) -> std::io::Result<()> {
        self.wc(SMN_INDEX, reg)?;
        self.wc(SMN_DATA, v)
    }
    /// Read SMN register `reg` (index latch then data). Guarded against short
    /// transfers.
    pub fn rreg(&self, reg: u32) -> std::io::Result<u32> {
        self.wc(SMN_INDEX, reg)?;
        self.rc(SMN_DATA)
    }
}
