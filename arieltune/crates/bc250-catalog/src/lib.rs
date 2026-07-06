// SPDX-License-Identifier: GPL-2.0-only
//! The authored ASRock BC-250 (Cyan Skillfish / Oberon APU) OEM System Manual.
//!
//! This crate owns one artifact: the human-authored, live-verified OEM manual
//! for the board, embedded at build time ([`manual_book::MANUAL_MD`]). It is the
//! single source of truth, projected two ways:
//!
//! - [`manual_book`] parses it into a chapter → section tree for the `wikitune`
//!   TUI to render as designed prose (the human view).
//! - [`manual_data`] projects that same parse into clean structured [`manual_data::Section`]
//!   records — prose, tagged boxes as cell grids, and cross-references — for
//!   agents to search and ingest (the machine view).
//!
//! Because both views go through the same parse of the same embedded file, the
//! human manual and the machine records can never drift.
//!
//! The manual is a *static* reference: it documents what the board is and how it
//! behaves, never a measured or dynamic value (bandwidth, latency, throughput,
//! temperature, power). Those belong to the companion tune apps (memtune,
//! aputune, biostune, arieltune), which measure and actuate the hardware live.

pub mod manual_book;
pub mod manual_data;
