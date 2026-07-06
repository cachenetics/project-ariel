// SPDX-License-Identifier: GPL-2.0-only
//! The SPI-flash medium (analog of memtune's `cmos.rs`).
//!
//! biostune reads/writes the 16 MiB BIOS image. On the board that's the live
//! W25Q128 via `flashrom -p internal` (verified working in-system on the
//! BC-250). With `--image <file>` it operates on a dump instead — safe for
//! inspection on any machine, and the way the test suite runs off-board.
//!
//! Writing the live chip is the one dangerous act here: a corrupt APCB header
//! bricks the board (recovery needs the external SPI rig). So `write_image`
//! refuses unless the only bytes that changed sit inside the APCB slot.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};

use crate::apcb::{self, APCB_SLOT};

pub const BACKUP_DIR: &str = "/var/lib/biostune/backups";

/// Where biostune reads/writes the image from.
pub enum Medium {
    /// The live SPI chip via `flashrom -p internal`.
    Live,
    /// A dump file (`--image`). Never touches hardware.
    File(PathBuf),
}

impl Medium {
    pub fn label(&self) -> String {
        match self {
            Medium::Live => "live SPI (flashrom -p internal)".into(),
            Medium::File(p) => format!("image file {}", p.display()),
        }
    }

    pub fn is_live(&self) -> bool {
        matches!(self, Medium::Live)
    }

    /// Read the full BIOS image.
    pub fn read_image(&self) -> Result<Vec<u8>> {
        match self {
            Medium::File(p) => {
                std::fs::read(p).with_context(|| format!("read image {}", p.display()))
            }
            Medium::Live => {
                let tmp = tmp_path("biostune-read");
                run_flashrom(&["-r", path_str(&tmp)]).context(
                    "flashrom read failed (run as root on a BC-250; see `arieltune bios doctor`)",
                )?;
                let data = std::fs::read(&tmp).context("read flashrom output")?;
                let _ = std::fs::remove_file(&tmp);
                Ok(data)
            }
        }
    }

    /// Write the full image back, verifying first that nothing outside the APCB
    /// slot changed vs `original`. `flashrom` re-verifies the chip after write.
    pub fn write_image(&self, original: &[u8], new: &[u8]) -> Result<()> {
        guard_apcb_only(original, new)?;
        match self {
            Medium::File(p) => {
                std::fs::write(p, new).with_context(|| format!("write image {}", p.display()))
            }
            Medium::Live => {
                let tmp = tmp_path("biostune-write");
                std::fs::write(&tmp, new).context("stage image for flashrom")?;
                let r = run_flashrom(&["-w", path_str(&tmp), "--verify"]);
                let _ = std::fs::remove_file(&tmp);
                r.context("flashrom write failed — the chip may be unchanged; verify with `arieltune bios dump`")
            }
        }
    }
}

/// Copy `image` to a timestamped backup under BACKUP_DIR; returns its path.
pub fn auto_backup(image: &[u8]) -> Result<PathBuf> {
    std::fs::create_dir_all(BACKUP_DIR).context("create backup dir")?;
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = Path::new(BACKUP_DIR).join(format!("flash-{ts}.bin"));
    std::fs::write(&path, image).context("write backup")?;
    Ok(path)
}

/// Back up the current image before a write. On the live chip a backup is
/// mandatory (we never flash without a recovery copy); in file mode it's
/// best-effort — the source file is itself the backup — so editing a dump as a
/// normal user doesn't require `/var/lib/biostune`.
pub fn backup_before(medium: &Medium, image: &[u8]) -> Result<Option<PathBuf>> {
    let _ = ensure_stock(image);
    match auto_backup(image) {
        Ok(p) => Ok(Some(p)),
        Err(e) if !medium.is_live() => {
            eprintln!("note: skipping backup in file mode ({e})");
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

/// Save the first-seen image as the immutable stock reference (once).
pub fn ensure_stock(image: &[u8]) -> Result<PathBuf> {
    std::fs::create_dir_all(BACKUP_DIR).context("create backup dir")?;
    let path = Path::new(BACKUP_DIR).join("stock.bin");
    if !path.exists() {
        std::fs::write(&path, image).context("write stock backup")?;
    }
    Ok(path)
}

/// Refuse a write whose changes reach outside the APCB slot — the core brick
/// guard. Catches a wrong base image, a corrupted buffer, or any bug that would
/// touch the `$PSP`/key-store/`$PS1` regions.
fn guard_apcb_only(original: &[u8], new: &[u8]) -> Result<()> {
    if original.len() != new.len() {
        bail!(
            "image size changed ({} -> {}) — refusing to write",
            original.len(),
            new.len()
        );
    }
    let addr = apcb::locate(original)?;
    let slot = addr..addr + APCB_SLOT;
    let outside: Vec<usize> = (0..original.len())
        .filter(|i| original[*i] != new[*i] && !slot.contains(i))
        .collect();
    if !outside.is_empty() {
        bail!(
            "{} byte(s) outside the APCB slot would change (first at 0x{:x}) — \
             refusing to write (this protects the PSP/key-store/signed regions)",
            outside.len(),
            outside[0]
        );
    }
    Ok(())
}

fn tmp_path(stem: &str) -> PathBuf {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("{stem}-{ts}.bin"))
}

fn path_str(p: &Path) -> &str {
    p.to_str().unwrap_or("/dev/null")
}

fn flashrom_base() -> Vec<&'static str> {
    vec!["-p", "internal"]
}

fn run_flashrom(extra: &[&str]) -> Result<()> {
    let mut args = flashrom_base();
    args.extend_from_slice(extra);
    let status = Command::new("flashrom")
        .args(&args)
        .status()
        .map_err(|e| anyhow!("could not run flashrom: {e} (is it installed?)"))?;
    if !status.success() {
        bail!("flashrom {:?} exited with {status}", extra);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apcb::BHD_BODY_ADDR;

    fn image_with_apcb() -> Vec<u8> {
        // minimal valid APCB spliced into a full-size image
        let total = 0x400usize;
        let mut a = vec![0u8; APCB_SLOT];
        a[..4].copy_from_slice(b"APCB");
        a[4..6].copy_from_slice(&0x20u16.to_le_bytes());
        a[6..8].copy_from_slice(&0x20u16.to_le_bytes());
        a[8..12].copy_from_slice(&(total as u32).to_le_bytes());
        let s = a[..total].iter().fold(0u8, |x, &b| x.wrapping_add(b));
        a[0x10] = s.wrapping_neg();
        let mut img = vec![0xffu8; BHD_BODY_ADDR + APCB_SLOT];
        img[BHD_BODY_ADDR..BHD_BODY_ADDR + APCB_SLOT].copy_from_slice(&a);
        img
    }

    #[test]
    fn guard_allows_apcb_slot_change() {
        let orig = image_with_apcb();
        let mut new = orig.clone();
        new[BHD_BODY_ADDR + 0x200] ^= 0xff; // inside the slot
        assert!(guard_apcb_only(&orig, &new).is_ok());
    }

    #[test]
    fn guard_blocks_outside_change() {
        let orig = image_with_apcb();
        let mut new = orig.clone();
        new[0x1000] ^= 0xff; // far outside the APCB slot
        assert!(guard_apcb_only(&orig, &new).is_err());
    }

    #[test]
    fn guard_blocks_size_change() {
        let orig = image_with_apcb();
        let mut new = orig.clone();
        new.push(0);
        assert!(guard_apcb_only(&orig, &new).is_err());
    }

    #[test]
    fn file_medium_roundtrips() {
        let dir = std::env::temp_dir().join(format!("biostune-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("img.bin");
        let orig = image_with_apcb();
        std::fs::write(&p, &orig).unwrap();
        let m = Medium::File(p.clone());
        let read = m.read_image().unwrap();
        assert_eq!(read.len(), orig.len());
        let mut new = orig.clone();
        new[BHD_BODY_ADDR + 0x100] ^= 0xff;
        m.write_image(&orig, &new).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), new);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
