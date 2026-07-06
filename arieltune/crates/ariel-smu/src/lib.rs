// SPDX-License-Identifier: GPL-2.0-only
//! ariel-smu — the Ariel APU's SMU mailboxes.
//!
//! Two independent mailbox surfaces, deliberately kept apart:
//!
//! - [`smu`] — queue-0, the GPU PMFW mailbox amdgpu itself polls. Every
//!   actuation routes through the kernel-serialized `amdgpu_smu_send_raw`
//!   debugfs node so it can never race the driver.
//! - [`ocq3`] — queue-3, the CPU-OC mailbox (boost / curve-undervolt / temp
//!   caps), driven over the raw SMN aperture ([`ariel_hal::SmnAperture`]). This
//!   is a SEPARATE mailbox amdgpu never touches.
//!
//! Both carry their safety envelopes (Vid ceilings, clamps, dead-code danger
//! guards) verbatim from the aputune originals.

pub mod ocq3;
pub mod smu;
