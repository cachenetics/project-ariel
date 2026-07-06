// SPDX-License-Identifier: GPL-2.0-only
//! The house palette, encoded ONCE for the whole suite.
//!
//! House rule (inherited from wikitune, binding): color the section/box HEADERS
//! only -- plain body. No sporadic body tinting (tokens/keys/tree), no reversed
//! cyan chips. Headers use [`ACCENT`]; key hints use [`KEY`]; status verdicts use
//! [`GOOD`]/[`WARN`]/[`BAD`]; de-emphasized chrome uses [`DIM`]. Body text stays the
//! terminal default foreground.

use ratatui::style::{Color, Modifier, Style};

/// Focus / section headers / active tab.
pub const ACCENT: Color = Color::Cyan;
/// Key hints in the status line ("F1-F4", "^Q").
pub const KEY: Color = Color::Magenta;
/// A good/healthy verdict.
pub const GOOD: Color = Color::Green;
/// A caution verdict.
pub const WARN: Color = Color::Yellow;
/// A danger / caution verdict (destructive writes).
pub const BAD: Color = Color::Red;
/// De-emphasized chrome (inactive tabs, borders of unfocused panels).
pub const DIM: Color = Color::DarkGray;

/// A section/box header: accent, bold. The ONLY place color leads.
pub fn header() -> Style {
    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
}

/// A key hint: magenta, bold.
pub fn key() -> Style {
    Style::default().fg(KEY).add_modifier(Modifier::BOLD)
}

/// De-emphasized chrome.
pub fn dim() -> Style {
    Style::default().fg(DIM)
}

/// A caution header (destructive writes) -- red, bold. Still a header, not body.
pub fn caution() -> Style {
    Style::default().fg(BAD).add_modifier(Modifier::BOLD)
}
