// SPDX-License-Identifier: GPL-2.0-only
//! Tab set. Each real screen is live now: WIKI (M1), BIOS (M3), APU (M5), MEM
//! (M4). `StubScreen` is retained only as a hermetic Screen double for the shell
//! routing tests (`crate::shell` tests construct it), so it is `dead_code` in a
//! production (non-test) build -- allowed below rather than removed.

use arieltune_tui_kit::{theme, widgets, Outcome, Screen};
use crossterm::event::KeyEvent;
use ratatui::layout::{Alignment, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

/// A no-op Screen (returns `Ignored` for every key) used only by the shell tests
/// to exercise the global switch/quit routing independent of the real tab set.
#[allow(dead_code)]
pub struct StubScreen {
    title: &'static str,
    blurb: &'static str,
    milestone: &'static str,
}

#[allow(dead_code)]
impl StubScreen {
    pub fn new(title: &'static str, blurb: &'static str, milestone: &'static str) -> Self {
        StubScreen {
            title,
            blurb,
            milestone,
        }
    }
}

impl Screen for StubScreen {
    fn title(&self) -> &'static str {
        self.title
    }

    fn draw(&mut self, f: &mut Frame, area: Rect) {
        let lines = vec![
            Line::from(""),
            Line::from(Span::styled(self.title, theme::header())),
            Line::from(""),
            Line::from(Span::raw(self.blurb)),
            Line::from(""),
            Line::from(Span::styled(
                format!("stub tab -- real screen lands in {}", self.milestone),
                theme::dim(),
            )),
        ];
        let p = Paragraph::new(lines)
            .block(widgets::panel(self.title, true))
            .alignment(Alignment::Center);
        f.render_widget(p, area);
    }

    fn on_key(&mut self, _key: KeyEvent) -> Outcome {
        Outcome::Ignored
    }

    fn status_hint(&self) -> Option<String> {
        Some(format!("{} (stub)", self.title))
    }
}

/// The four tabs in fixed order WIKI | BIOS | APU | MEM.
pub fn screens() -> Vec<Box<dyn Screen>> {
    vec![
        Box::new(wiki::screen()),
        Box::new(bios::screen()),
        Box::new(apu::screen()),
        Box::new(mem::screen()),
    ]
}
