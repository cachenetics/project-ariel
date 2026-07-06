// SPDX-License-Identifier: GPL-2.0-only
//! Small shared widgets. Deliberately thin -- the panes own their own layouts; these
//! just enforce the header-color-only rule at the chrome boundary.

use ratatui::text::Span;
use ratatui::widgets::{Block, Borders};

use crate::theme;

/// A bordered panel whose TITLE carries the accent (or dim when unfocused). The body
/// is drawn by the caller and stays plain.
pub fn panel(title: &str, focused: bool) -> Block<'static> {
    let style = if focused {
        theme::header()
    } else {
        theme::dim()
    };
    Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(title.to_string(), style))
}

/// A `key label` hint: magenta bold key, plain label. Push these into a status Line.
pub fn key_hint(key: &str, label: &str) -> Vec<Span<'static>> {
    vec![
        Span::styled(key.to_string(), theme::key()),
        Span::raw(" "),
        Span::raw(label.to_string()),
    ]
}
