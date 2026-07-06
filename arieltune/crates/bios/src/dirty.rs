// SPDX-License-Identifier: GPL-2.0-only
//! "SMM-dirty" marker — prevents the NVRAM-store collision class.
//!
//! Why this exists (learned from a live test): biostune's OEM writes
//! append directly to the SPI NVAR store via SMM, bypassing the firmware. The
//! firmware does NOT keep an on-flash free-space pointer — it computes the free
//! offset by scanning the store ONCE at boot and caches it in DRAM, advancing it
//! per firmware `SetVariable`. So after one of our SMM appends, the firmware's
//! cached pointer is STALE: its next runtime `SetVariable` (any efivar write —
//! e.g. a CBS `set`) writes into a slot we already used, overwriting our entry
//! and corrupting the store.
//!
//! The invariant: a firmware `SetVariable` is unsafe while the store is
//! "SMM-dirty" = an SMM append has happened since the last boot. We track that
//! with a marker in `/run` (tmpfs) — it is created on every SMM append and is
//! automatically gone after a reboot, which is exactly when the firmware
//! re-scans and the store is clean again. (OEM Setup changes need a reboot to
//! apply anyway, so this adds no extra step.)

use std::path::Path;

const MARKER: &str = "/run/biostune-smm-dirty";

/// Mark the NVAR store SMM-dirty (call right after an SMM append). Best-effort.
pub fn mark() {
    let _ = std::fs::write(MARKER, b"an SMM NVAR append is pending a reboot\n");
}

/// True if an SMM append has happened since the last boot (reboot clears it).
pub fn is_dirty() -> bool {
    Path::new(MARKER).exists()
}

/// One-line explanation for refusal messages.
pub fn why() -> &'static str {
    "an OEM (SMM) edit is pending a reboot — writing another variable now would \
     corrupt the NVRAM store. Reboot first (it also applies the OEM change), then retry."
}
