// SPDX-License-Identifier: GPL-2.0-only
//! Read current BIOS-setting values from the live EFI variables.
//!
//! The values biostune displays come from the firmware's runtime varstores under
//! `/sys/firmware/efi/efivars/`. The AMD CBS settings live in `AmdSetup`; the OEM
//! settings live in `Setup`. Each efivar file is a 4-byte attribute mask followed
//! by the data — we strip the mask and index by the catalogue's offset/width.
//!
//! Honesty note: on the BC-250 `AmdSetup` is readable but AGESA applies the APCB
//! copy first, so these values are the *stored* config (what the BIOS menu
//! shows), not necessarily what the silicon runs. `Setup` is boot-service-only on
//! the BC-250 and usually absent here — those settings then show no live value.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};

const EFIVARS_DIR: &str = "/sys/firmware/efi/efivars";
const AMDSETUP: &str = "AmdSetup-3a997502-647a-4c82-998e-52ef9486a247";
const SETUP: &str = "Setup-ec87d643-eba4-4bb5-a1e5-3f3e36b20da9";
const BACKUP_DIR: &str = "/var/lib/biostune/backups";

/// Only AmdSetup is runtime-writable on the BC-250 (Setup is boot-service-only).
pub fn is_writable(varstore: &str) -> bool {
    varstore == "AmdSetup"
}

/// One pending edit: write `value` (LE, `width` bytes) at `offset` in AmdSetup.
#[derive(Clone, Copy)]
pub struct Edit {
    pub offset: usize,
    pub width: usize,
    pub value: u32,
}

/// Apply edits to the live AmdSetup EFI variable.
///
/// Backs up the current variable first, clears the efivarfs immutable bit, then
/// writes the whole `[attrs][data]` buffer back in one go. Returns the backup
/// path. Changes take effect on the next boot (AGESA re-reads NVRAM) — and note
/// many CBS settings are actually applied from the APCB copy, so an AmdSetup
/// write can be cosmetic. Needs root.
pub fn write_amdsetup(edits: &[Edit]) -> Result<PathBuf> {
    let path = Path::new(EFIVARS_DIR).join(AMDSETUP);
    let mut raw = std::fs::read(&path)
        .with_context(|| format!("read {} (run as root on a BC-250)", path.display()))?;
    if raw.len() < 4 {
        bail!("AmdSetup variable too small");
    }
    // backup the untouched variable
    std::fs::create_dir_all(BACKUP_DIR).context("create backup dir")?;
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let backup = Path::new(BACKUP_DIR).join(format!("amdsetup-{ts}.bin"));
    std::fs::write(&backup, &raw).context("write backup")?;

    for e in edits {
        let start = 4 + e.offset; // skip the 4-byte attribute mask
        let end = start + e.width;
        if end > raw.len() {
            bail!("edit at +0x{:x} is outside AmdSetup", e.offset);
        }
        let bytes = e.value.to_le_bytes();
        raw[start..end].copy_from_slice(&bytes[..e.width]);
    }

    // efivarfs sets the immutable bit after the first write of a session; clear
    // it (best-effort) or the write fails silently with EPERM.
    let _ = Command::new("chattr").arg("-i").arg(&path).status();

    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .with_context(|| format!("open {} for write (need root)", path.display()))?;
    f.write_all(&raw)
        .with_context(|| format!("write {} (immutable? not root?)", path.display()))?;
    Ok(backup)
}

/// Snapshot of the readable varstores (data only, attribute mask stripped).
pub struct EfiVars {
    amdsetup: Option<Vec<u8>>,
    setup: Option<Vec<u8>>,
}

fn read_var(file: &str) -> Option<Vec<u8>> {
    let p = Path::new(EFIVARS_DIR).join(file);
    let raw = std::fs::read(p).ok()?;
    if raw.len() < 4 {
        return None;
    }
    Some(raw[4..].to_vec()) // drop the 4-byte attribute mask
}

impl EfiVars {
    pub fn read() -> Self {
        EfiVars {
            amdsetup: read_var(AMDSETUP),
            setup: read_var(SETUP),
        }
    }

    /// Build the value source from a full SPI flash image (the AMI NVAR store).
    /// Recovers BOTH varstores — including the OEM `Setup` that efivarfs hides —
    /// so OEM settings show their real current value instead of the default.
    pub fn from_nvram(image: &[u8]) -> Self {
        let v = crate::nvram::read_varstores(image);
        EfiVars {
            amdsetup: v.amdsetup,
            setup: v.setup,
        }
    }

    fn store(&self, varstore: &str) -> Option<&[u8]> {
        match varstore {
            "AmdSetup" => self.amdsetup.as_deref(),
            "Setup" => self.setup.as_deref(),
            _ => None,
        }
    }

    pub fn has(&self, varstore: &str) -> bool {
        self.store(varstore).is_some()
    }

    /// Decode a setting's current value (LE, width bytes) from its varstore, or
    /// None if the varstore isn't present or the offset is out of range.
    pub fn value(&self, s: &crate::catalog::Setting) -> Option<u32> {
        let data = self.store(&s.varstore)?;
        let w = s.width();
        let end = s.offset + w;
        if end > data.len() {
            return None;
        }
        let mut v = 0u32;
        for (i, &b) in data[s.offset..end].iter().enumerate() {
            v |= (b as u32) << (8 * i);
        }
        Some(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Setting;

    fn setting(offset: usize, bits: u8, store: &str) -> Setting {
        Setting {
            category: "X".into(),
            name: "T".into(),
            offset,
            bits,
            default: None,
            options: vec![],
            range: None,
            varstore: store.into(),
        }
    }

    #[test]
    fn decodes_le_value_from_synthetic_store() {
        // build a vars snapshot by hand
        let mut data = vec![0u8; 16];
        data[4] = 0xcd;
        data[5] = 0xab; // u16 @ off4 = 0xabcd
        let vars = EfiVars {
            amdsetup: Some(data),
            setup: None,
        };
        assert_eq!(vars.value(&setting(4, 16, "AmdSetup")), Some(0xabcd));
        assert_eq!(vars.value(&setting(4, 8, "AmdSetup")), Some(0xcd));
        assert_eq!(vars.value(&setting(99, 8, "AmdSetup")), None); // out of range
        assert_eq!(vars.value(&setting(0, 8, "Setup")), None); // store absent
    }
}
