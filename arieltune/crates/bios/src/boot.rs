// SPDX-License-Identifier: GPL-2.0-only
//! Boot-time OEM-Setup writes — the no-flash path.
//!
//! The OEM `Setup` variable is boot-service-only, so it can't be written from a
//! running Linux. But at *boot-services* time it's writable, so biostune stages a
//! one-shot boot that runs a tiny UEFI helper which sets the variable, then
//! resets straight back into Linux. No SPI flash is touched — so a bad value is
//! recoverable with an NVRAM/CMOS clear, not an external programmer.
//!
//! Flow:
//!   1. copy a UEFI Shell + setup_var.efi to <ESP>/EFI/biostune/
//!   2. write <ESP>/startup.nsh: the setup_var writes, then `reset` (so it can
//!      NEVER sit in the shell — it always returns to the OS)
//!   3. create a boot entry for the shell and set BootNext to it (ONE-SHOT:
//!      firmware consumes BootNext, so any failed boot falls back to the normal
//!      boot order on the next power cycle)
//!   4. the operator reboots; the helper applies the change and resets to Linux
//!   5. `oem-clear` removes the entry + staged files afterwards
//!
//! Recovery if the one-shot misbehaves: power-cycle the board — BootNext is
//! already consumed, so it boots normally. No flash, no rig.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};

/// Where biostune keeps the UEFI helper binaries (placed by install.sh).
pub const EFI_LIB_DIR: &str = "/usr/lib/biostune/efi";
const SHELL_EFI: &str = "Shell.efi";
const SETUP_VAR_EFI: &str = "setup_var.efi";
/// Staging dir on the EFI System Partition.
const ESP_SUBDIR: &str = "EFI/biostune";
const BOOT_LABEL: &str = "biostune-oneshot";

/// One pending OEM-Setup write (within the boot-service-only `Setup` var).
pub struct SetupEdit {
    pub name: String,
    pub offset: usize,
    pub value: u8,
}

/// Locate the mounted EFI System Partition (where the bootloader lives).
pub fn find_esp() -> Result<PathBuf> {
    for cand in ["/boot/efi", "/efi", "/boot"] {
        let p = Path::new(cand);
        if p.join("EFI").is_dir() {
            return Ok(p.to_path_buf());
        }
    }
    bail!("could not find the EFI System Partition (looked in /boot/efi, /efi, /boot)")
}

/// (disk, partition-number) backing the ESP, for efibootmgr.
fn esp_disk_part(esp: &Path) -> Result<(String, u32)> {
    // findmnt -no SOURCE <esp>  ->  /dev/nvme0n1p1
    let out = Command::new("findmnt")
        .args(["-no", "SOURCE", &esp.to_string_lossy()])
        .output()
        .context("run findmnt")?;
    let src = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if src.is_empty() {
        bail!(
            "could not resolve the ESP block device for {}",
            esp.display()
        );
    }
    // split a partition device into (disk, partnum): nvme0n1p1 -> (nvme0n1, 1); sda1 -> (sda, 1)
    let dev = src.trim_start_matches("/dev/");
    let (disk, num) =
        split_part(dev).ok_or_else(|| anyhow!("could not parse partition from {src}"))?;
    Ok((format!("/dev/{disk}"), num))
}

fn split_part(dev: &str) -> Option<(String, u32)> {
    // trailing digits are the partition number; for nvme/mmc the disk ends in 'p'
    let idx = dev.rfind(|c: char| !c.is_ascii_digit())?;
    let (head, digits) = dev.split_at(idx + 1);
    let num: u32 = digits.parse().ok()?;
    let disk = head.strip_suffix('p').unwrap_or(head);
    Some((disk.to_string(), num))
}

fn helpers_present() -> Result<()> {
    for f in [SHELL_EFI, SETUP_VAR_EFI] {
        let p = Path::new(EFI_LIB_DIR).join(f);
        if !p.exists() {
            bail!(
                "missing UEFI helper {} — install it to {EFI_LIB_DIR} \
                 (install.sh fetches the UEFI Shell + setup_var.efi)",
                p.display()
            );
        }
    }
    Ok(())
}

/// Render the startup.nsh that applies the edits and always resets back to the OS.
pub fn startup_nsh(edits: &[SetupEdit]) -> String {
    let mut s = String::from("@echo -off\n");
    s.push_str("echo arieltune: applying OEM Setup change(s)...\n");
    for e in edits {
        // setup_var.efi writes the "Setup" variable at <offset> = <value>
        s.push_str(&format!("echo   {} = 0x{:02x}\n", e.name, e.value));
        s.push_str(&format!(
            "fs0:\\EFI\\biostune\\setup_var.efi 0x{:x} 0x{:02x}\n",
            e.offset, e.value
        ));
    }
    // ALWAYS reset, so the one-shot can never sit idle in the shell.
    s.push_str("echo arieltune: done, rebooting back to the OS...\n");
    s.push_str("reset -c\n");
    s
}

/// Stage the helper + startup.nsh on the ESP (no boot entry yet). Returns the
/// startup.nsh text actually written.
pub fn stage(esp: &Path, edits: &[SetupEdit]) -> Result<String> {
    helpers_present()?;
    let dir = esp.join(ESP_SUBDIR);
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    for f in [SHELL_EFI, SETUP_VAR_EFI] {
        std::fs::copy(Path::new(EFI_LIB_DIR).join(f), dir.join(f))
            .with_context(|| format!("copy {f} to ESP"))?;
    }
    let nsh = startup_nsh(edits);
    std::fs::write(esp.join("startup.nsh"), &nsh).context("write startup.nsh")?;
    Ok(nsh)
}

/// Create the one-shot boot entry for the staged shell and set BootNext to it.
/// Returns the new boot number (e.g. "0007").
pub fn arm(esp: &Path) -> Result<String> {
    let (disk, part) = esp_disk_part(esp)?;
    let loader = format!("\\{}\\{}", ESP_SUBDIR.replace('/', "\\"), SHELL_EFI);
    // Capture BootOrder first: `--create` prepends the new entry to it, which we
    // must undo — the shell entry must be reachable ONLY via the one-shot
    // BootNext, never the normal boot order, or the board would loop into the
    // shell instead of reaching the OS.
    let orig_order = read_boot_order();
    let out = Command::new("efibootmgr")
        .args([
            "--create",
            "--disk",
            &disk,
            "--part",
            &part.to_string(),
            "--label",
            BOOT_LABEL,
            "--loader",
            &loader,
        ])
        .output()
        .context("efibootmgr --create")?;
    if !out.status.success() {
        bail!(
            "efibootmgr --create failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let num = parse_boot_num(&String::from_utf8_lossy(&out.stdout))
        .ok_or_else(|| anyhow!("could not determine the new boot entry number"))?;
    // restore the original BootOrder (drop the just-prepended shell entry)
    if let Some(order) = orig_order {
        let _ = Command::new("efibootmgr")
            .args(["--bootorder", &order])
            .status();
    }
    // set it as the ONE-SHOT next boot
    let set = Command::new("efibootmgr")
        .args(["--bootnext", &num])
        .status()
        .context("efibootmgr --bootnext")?;
    if !set.success() {
        bail!("efibootmgr --bootnext {num} failed");
    }
    Ok(num)
}

fn read_boot_order() -> Option<String> {
    let out = Command::new("efibootmgr").output().ok()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("BootOrder:").map(|r| r.trim().to_string()))
}

fn parse_boot_num(efibootmgr_out: &str) -> Option<String> {
    // the created entry line looks like "Boot0007* biostune-oneshot ..."
    efibootmgr_out
        .lines()
        .find(|l| l.contains(BOOT_LABEL))
        .and_then(|l| l.strip_prefix("Boot"))
        .map(|r| r[..4].to_string())
}

/// Tear down any biostune one-shot: clear BootNext, delete the entry, remove
/// staged files. Safe to call even if nothing is staged.
pub fn clear(esp: &Path) -> Result<()> {
    // delete BootNext (ignore if absent)
    let _ = Command::new("efibootmgr").arg("--delete-bootnext").status();
    // delete any biostune-oneshot entries
    if let Ok(out) = Command::new("efibootmgr").output() {
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            if line.contains(BOOT_LABEL) {
                if let Some(num) = line.strip_prefix("Boot").map(|r| r[..4].to_string()) {
                    let _ = Command::new("efibootmgr")
                        .args(["--bootnum", &num, "--delete-bootnum"])
                        .status();
                }
            }
        }
    }
    let _ = std::fs::remove_file(esp.join("startup.nsh"));
    let _ = std::fs::remove_dir_all(esp.join(ESP_SUBDIR));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_part_handles_nvme_and_sata() {
        assert_eq!(split_part("nvme0n1p1"), Some(("nvme0n1".into(), 1)));
        assert_eq!(split_part("sda1"), Some(("sda".into(), 1)));
        assert_eq!(split_part("mmcblk0p2"), Some(("mmcblk0".into(), 2)));
    }

    #[test]
    fn startup_nsh_writes_edits_and_always_resets() {
        let edits = vec![
            SetupEdit {
                name: "Bootup NumLock State".into(),
                offset: 0x00,
                value: 0x00,
            },
            SetupEdit {
                name: "Fast Boot".into(),
                offset: 0x01,
                value: 0x01,
            },
        ];
        let n = startup_nsh(&edits);
        assert!(n.contains("setup_var.efi 0x0 0x00"));
        assert!(n.contains("setup_var.efi 0x1 0x01"));
        assert!(
            n.trim_end().ends_with("reset -c"),
            "must always reset back to the OS"
        );
    }

    #[test]
    fn parse_boot_num_from_create_output() {
        let out = "BootCurrent: 0001\nBoot0007* biostune-oneshot\tHD(1,GPT,...)\n";
        assert_eq!(parse_boot_num(out), Some("0007".into()));
    }
}
