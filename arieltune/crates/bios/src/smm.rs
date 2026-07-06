// SPDX-License-Identifier: GPL-2.0-only
//! SMM SPI-flash medium — the no-rig OEM write path.
//!
//! Drives the `smiflash` kernel module (`/proc/smiflash`), which fires AMI's
//! SmiFlash SW-SMI handler to read/write the SPI flash from SMM context. This is
//! how biostune changes OEM `Setup` settings with no external programmer and
//! without going through the firmware's locked SetVariable path.
//!
//! The module must be loaded with the platform's real SW-SMI command port (the
//! FADT SMI_CMD field — 0xB0 on the BC-250, NOT the conventional 0xB2):
//!     sudo insmod smiflash.ko smi_port=0xB0
//! biostune can't insmod for you; `available()` reports if it's loaded.
//!
//! Addressing: `field = chip offset directly` (the handler biases it into the
//! 0xFF000000 MMIO window). Writes are AND-only (program bits 1->0; no erase) —
//! the OEM-edit path appends into erased (0xFF) free space, so that's fine.

use std::fs::OpenOptions;
use std::io;
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::process::Command;

const PROC: &str = "/proc/smiflash";
const MAXDATA: usize = 256;

/// Where a loose (non-DKMS) `smiflash.ko` is looked for, in order. The DKMS
/// install (the recommended path) makes the module available via `modprobe`
/// instead — `load()` tries that first.
const KO_PATHS: &[&str] = &["/usr/lib/biostune/smiflash.ko", "/tmp/smiflash.ko"];

/// Where `biostune install.sh` stages the DKMS driver sources, so
/// `biostune driver build` can (re)build them after install.
const DRIVER_SRC_DIRS: &[&str] = &["/usr/share/biostune/driver", "driver"];

/// Is the module registered with DKMS / installed under /lib/modules (i.e.
/// loadable via `modprobe`)?
fn modprobe_known() -> bool {
    Command::new("modinfo")
        .arg("smiflash")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// The real SW-SMI command port from the ACPI FADT (`SMI_CMD` @ offset 48).
/// On the BC-250 this is 0xB0 — NOT the conventional 0xB2 (writing 0xB2 raises no
/// SMI at all). Never hardcode it; ask the firmware.
pub fn smi_cmd_port() -> io::Result<u16> {
    let d = std::fs::read("/sys/firmware/acpi/tables/FACP")?;
    if d.len() < 52 || &d[0..4] != b"FACP" {
        return Err(io::Error::other("FADT not present or malformed"));
    }
    let smi_cmd = u32::from_le_bytes([d[48], d[49], d[50], d[51]]);
    if smi_cmd == 0 || smi_cmd > 0xFFFF {
        return Err(io::Error::other(format!(
            "FADT SMI_CMD = 0x{smi_cmd:x} (no usable SW-SMI command port)"
        )));
    }
    Ok(smi_cmd as u16)
}

/// Find the installed loose smiflash.ko, if any.
pub fn ko_path() -> Option<&'static str> {
    KO_PATHS.iter().copied().find(|p| Path::new(p).exists())
}

/// Human-readable install state of the module for `driver status`.
pub fn install_state() -> String {
    if modprobe_known() {
        "installed via DKMS (modprobe smiflash)".into()
    } else if let Some(p) = ko_path() {
        format!("loose module at {p}")
    } else {
        "not installed — run `arieltune bios driver build` (DKMS)".into()
    }
}

/// Load the smiflash driver (idempotent) with the FADT-derived SW-SMI port.
/// No-op if already loaded. Needs root and an installed smiflash.ko.
pub fn load() -> io::Result<()> {
    if Smm::available() {
        return Ok(());
    }
    let port = smi_cmd_port()?;
    // Prefer the DKMS-installed module via modprobe (the recommended install);
    // fall back to a loose smiflash.ko for the manual/dev path.
    if modprobe_known() {
        let status = Command::new("modprobe")
            .arg("smiflash")
            .arg(format!("smi_port=0x{port:x}"))
            .status()?;
        if status.success() && Smm::available() {
            return Ok(());
        }
    }
    let ko = ko_path().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "smiflash module not installed. Install it with `arieltune bios driver build` \
             (DKMS — builds it for your kernel on the board), or manually per \
             driver/README.md."
                .to_string(),
        )
    })?;
    let status = Command::new("insmod")
        .arg(ko)
        .arg(format!("smi_port=0x{port:x}"))
        .status()?;
    if !status.success() {
        return Err(io::Error::other(format!(
            "insmod {ko} smi_port=0x{port:x} failed (root? kernel-version match?)"
        )));
    }
    if !Smm::available() {
        return Err(io::Error::other(
            "insmod reported ok but /proc/smiflash absent",
        ));
    }
    Ok(())
}

/// Build + install the smiflash module for the running kernel via DKMS. Runs the
/// staged `driver/install-dkms.sh` (idempotent). Needs root, `dkms`, and kernel
/// headers; on a BC-250 it builds on the board (the prepare hook handles the
/// stripped CachyOS headers). Returns the script's combined output on failure.
pub fn build() -> io::Result<()> {
    let script = DRIVER_SRC_DIRS
        .iter()
        .map(|d| format!("{d}/install-dkms.sh"))
        .find(|p| Path::new(p).exists())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "driver sources not found in {DRIVER_SRC_DIRS:?} — reinstall arieltune \
                     (install.sh stages them), or run driver/install-dkms.sh from a checkout."
                ),
            )
        })?;
    let status = Command::new("sh").arg(&script).status()?;
    if !status.success() {
        return Err(io::Error::other(format!(
            "{script} failed — see its output above (need root, dkms, and kernel headers)."
        )));
    }
    Ok(())
}

/// Unload the smiflash driver (best-effort).
pub fn unload() -> io::Result<()> {
    let status = Command::new("rmmod").arg("smiflash").status()?;
    if !status.success() {
        return Err(io::Error::other("rmmod smiflash failed"));
    }
    Ok(())
}

// SW-SMI command bytes
const BEGIN: u8 = 0x20;
const READ: u8 = 0x21;
const WRITE: u8 = 0x23;
const END: u8 = 0x24;

// _IOWR('F', 1, struct smiflash_op) — the op struct is __packed = 270 bytes:
//   B(cmd) I(offset) I(size) B(status) I(dlen) 256s(data)
// dir=3<<30 | size(270)<<16 | 'F'(0x46)<<8 | nr(1)
const SMIFLASH_DO: libc::c_ulong = (3 << 30) | (270 << 16) | ((b'F' as libc::c_ulong) << 8) | 1;

#[repr(C, packed)]
struct Op {
    cmd: u8,
    offset: u32,
    size: u32,
    status: u8,
    dlen: u32,
    data: [u8; MAXDATA],
}

impl Op {
    fn new(cmd: u8, offset: u32, size: u32, payload: &[u8]) -> Self {
        let mut data = [0u8; MAXDATA];
        let n = payload.len().min(MAXDATA);
        data[..n].copy_from_slice(&payload[..n]);
        Op {
            cmd,
            offset,
            size,
            status: 0xEE,
            dlen: n as u32,
            data,
        }
    }
}

/// An open handle to the SMM flash driver.
pub struct Smm {
    file: std::fs::File,
}

impl Smm {
    /// Is the smiflash driver present? (module loaded)
    pub fn available() -> bool {
        std::path::Path::new(PROC).exists()
    }

    pub fn open() -> io::Result<Self> {
        if !Self::available() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "{PROC} absent — smiflash driver not loaded. Run `arieltune bios driver load` \
                     (loads it with the FADT SMI_CMD port). If it isn't installed yet, run \
                     `arieltune bios driver build` (DKMS — builds it for your kernel on the board)."
                ),
            ));
        }
        let file = OpenOptions::new().read(true).write(true).open(PROC)?;
        Ok(Smm { file })
    }

    /// Fire one SMI. Returns (status, returned 256-byte data buffer).
    fn ioctl(&self, mut op: Op) -> io::Result<(u8, [u8; MAXDATA])> {
        let rc = unsafe { libc::ioctl(self.file.as_raw_fd(), SMIFLASH_DO, &mut op as *mut Op) };
        if rc < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((op.status, op.data))
    }

    /// Read `n` bytes of flash at chip `off` (any length; chunked, transaction-wrapped).
    pub fn read(&self, off: u32, n: usize) -> io::Result<Vec<u8>> {
        let mut out = Vec::with_capacity(n);
        self.ioctl(Op::new(BEGIN, 0, 0, &[]))?;
        let res = (|| {
            while out.len() < n {
                let c = (n - out.len()).min(MAXDATA);
                let (st, data) =
                    self.ioctl(Op::new(READ, off + out.len() as u32, c as u32, &[]))?;
                if st != 0 {
                    return Err(io::Error::other(format!(
                        "smm read @0x{:x} status=0x{:02x}",
                        off as usize + out.len(),
                        st
                    )));
                }
                out.extend_from_slice(&data[..c]);
            }
            Ok(())
        })();
        let _ = self.ioctl(Op::new(END, 0, 0, &[]));
        res?;
        Ok(out)
    }

    /// Write `data` at chip `off` (any length; chunked). AND-only: clears bits
    /// (1->0) only — caller must ensure the target is erased (0xFF) where bits
    /// need setting. No erase is performed.
    pub fn write(&self, off: u32, data: &[u8]) -> io::Result<()> {
        self.ioctl(Op::new(BEGIN, 0, 0, &[]))?;
        let res = (|| {
            let mut p = 0;
            while p < data.len() {
                let chunk = &data[p..(p + MAXDATA).min(data.len())];
                let (st, _) =
                    self.ioctl(Op::new(WRITE, off + p as u32, chunk.len() as u32, chunk))?;
                if st != 0 {
                    return Err(io::Error::other(format!(
                        "smm write @0x{:x} status=0x{:02x}",
                        off as usize + p,
                        st
                    )));
                }
                p += chunk.len();
            }
            Ok(())
        })();
        let _ = self.ioctl(Op::new(END, 0, 0, &[]));
        res
    }
}
