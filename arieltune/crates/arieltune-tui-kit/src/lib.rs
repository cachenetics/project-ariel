// SPDX-License-Identifier: GPL-2.0-only
//! Shared TUI chrome for the arieltune suite.
//!
//! Provides the [`Screen`] trait every tab implements, the [`Outcome`] a key handler
//! returns, the house [`theme`], and a few common [`widgets`]. The suite binary owns
//! the terminal, the event loop, and the tab shell; the four merged apps (wiki/bios/
//! apu/mem) each become a `Screen`.

pub mod screen;
pub mod theme;
pub mod widgets;

pub use screen::{Outcome, Screen};
