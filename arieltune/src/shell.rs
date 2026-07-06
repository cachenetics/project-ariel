// SPDX-License-Identifier: GPL-2.0-only
//! The tab shell: owns the terminal, the panic-restore hook, the single event loop,
//! the tab bar, and the global status line. It delegates draw + key handling to the
//! active [`Screen`] and enforces the collision-safe global key set.
//!
//! Global keys (only when the active screen is not modal, and only as an `Ignored`
//! fallback after the pane sees the key first):
//!   1..4            -> jump to WIKI/BIOS/APU/MEM (F1-F4 and Alt-1..4 also work)
//!   Ctrl-Tab        -> next tab       Ctrl-Shift-Tab -> previous tab
//!   Ctrl-Q          -> quit the whole suite
//! Bare `Tab`/`Shift-Tab`/`q` are left to the panes, which bind them internally.
//! A pane that itself needs a digit (a value edit, a search box) reports
//! `modal()`, which suppresses the global 1-4 so the digit reaches the pane.

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::{execute, terminal};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::{Frame, Terminal};
use std::io::{self, Stdout};

use arieltune_tui_kit::{theme, widgets, Outcome, Screen};

type Term = Terminal<CrosstermBackend<Stdout>>;

enum Flow {
    Continue,
    Quit,
    Reboot(String),
}

pub struct Shell {
    screens: Vec<Box<dyn Screen>>,
    active: usize,
}

impl Shell {
    /// Build the shell over the ordered screens, focused on `default`.
    pub fn new(screens: Vec<Box<dyn Screen>>, default: usize) -> Self {
        let active = default.min(screens.len().saturating_sub(1));
        let mut shell = Shell { screens, active };
        if let Some(s) = shell.screens.get_mut(active) {
            s.on_enter();
        }
        shell
    }

    /// Set up the terminal, run the loop, tear down, and act on a reboot request.
    pub fn run(mut self) -> Result<()> {
        install_panic_hook();
        let mut term = setup_terminal()?;
        let outcome = self.event_loop(&mut term);
        restore_terminal()?;
        if let Some(msg) = outcome? {
            println!("{msg}");
            reboot();
        }
        Ok(())
    }

    fn event_loop(&mut self, term: &mut Term) -> Result<Option<String>> {
        loop {
            term.draw(|f| self.draw(f))?;
            let timeout = self.screens[self.active].tick_hint();
            if event::poll(timeout)? {
                if let Event::Key(k) = event::read()? {
                    if k.kind != KeyEventKind::Press {
                        continue;
                    }
                    match self.route_key(k) {
                        Flow::Quit => return Ok(None),
                        Flow::Reboot(m) => return Ok(Some(m)),
                        Flow::Continue => {}
                    }
                }
            }
            self.screens[self.active].tick();
        }
    }

    /// Pane-first routing: the active screen sees the key first. If it returns
    /// `Ignored` and is not modal, the shell tries its global bindings.
    fn route_key(&mut self, k: KeyEvent) -> Flow {
        let modal = self.screens[self.active].modal();
        match self.screens[self.active].on_key(k) {
            Outcome::Consumed => Flow::Continue,
            Outcome::Quit => Flow::Quit,
            Outcome::Reboot(m) => Flow::Reboot(m),
            Outcome::Ignored => {
                if modal {
                    Flow::Continue
                } else {
                    self.global_key(k).unwrap_or(Flow::Continue)
                }
            }
        }
    }

    fn global_key(&mut self, k: KeyEvent) -> Option<Flow> {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        match k.code {
            // Bare 1-4 jump directly to a tab. The shell only sees these when the
            // active pane returned Ignored (i.e. it is not modal and does not want
            // the digit), so a pane's own number use is never stolen. The `Char`
            // arms also match Alt-1..4, and F1-F4 stay as aliases.
            KeyCode::Char('1') => self.switch_to(0),
            KeyCode::Char('2') => self.switch_to(1),
            KeyCode::Char('3') => self.switch_to(2),
            KeyCode::Char('4') => self.switch_to(3),
            KeyCode::F(1) => self.switch_to(0),
            KeyCode::F(2) => self.switch_to(1),
            KeyCode::F(3) => self.switch_to(2),
            KeyCode::F(4) => self.switch_to(3),
            KeyCode::Char('q') if ctrl => return Some(Flow::Quit),
            KeyCode::BackTab if ctrl => self.cycle(-1),
            KeyCode::Tab if ctrl && shift => self.cycle(-1),
            KeyCode::Tab if ctrl => self.cycle(1),
            _ => return None,
        }
        Some(Flow::Continue)
    }

    fn switch_to(&mut self, idx: usize) {
        if idx >= self.screens.len() || idx == self.active {
            return;
        }
        self.screens[self.active].on_exit();
        self.active = idx;
        self.screens[self.active].on_enter();
    }

    fn cycle(&mut self, dir: i32) {
        let n = self.screens.len() as i32;
        if n == 0 {
            return;
        }
        let next = (((self.active as i32) + dir) % n + n) % n;
        self.switch_to(next as usize);
    }

    fn draw(&mut self, f: &mut Frame) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // tab bar
                Constraint::Min(0),    // active screen
                Constraint::Length(1), // status line
            ])
            .split(f.area());
        self.draw_tabbar(f, chunks[0]);
        let active = self.active;
        self.screens[active].draw(f, chunks[1]);
        self.draw_status(f, chunks[2]);
    }

    fn draw_tabbar(&self, f: &mut Frame, area: ratatui::layout::Rect) {
        let mut spans: Vec<Span> = Vec::new();
        for (i, s) in self.screens.iter().enumerate() {
            let label = format!(" {} ", s.title());
            let style = if i == self.active {
                theme::header()
            } else {
                theme::dim()
            };
            spans.push(Span::styled(label, style));
            spans.push(Span::raw(" "));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn draw_status(&self, f: &mut Frame, area: ratatui::layout::Rect) {
        let mut spans: Vec<Span> = Vec::new();
        spans.extend(widgets::key_hint("1-4", "tabs"));
        spans.push(Span::raw("   "));
        spans.extend(widgets::key_hint("^Q", "quit"));
        if let Some(hint) = self.screens[self.active].status_hint() {
            spans.push(Span::raw("   "));
            spans.push(Span::styled(hint, theme::dim()));
        }
        f.render_widget(
            Paragraph::new(Line::from(spans)).alignment(Alignment::Left),
            area,
        );
    }
}

fn setup_terminal() -> Result<Term> {
    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, terminal::EnterAlternateScreen)?;
    Ok(Terminal::new(CrosstermBackend::new(stdout))?)
}

fn restore_terminal() -> Result<()> {
    terminal::disable_raw_mode()?;
    execute!(io::stdout(), terminal::LeaveAlternateScreen)?;
    Ok(())
}

/// Restore the terminal on panic before the default hook prints -- otherwise a panic
/// in a remote session leaves the operator with a wedged tty. (aputune's pattern.)
fn install_panic_hook() {
    let original = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = restore_terminal();
        original(info);
    }));
}

fn reboot() {
    let _ = std::process::Command::new("systemctl")
        .arg("reboot")
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tabs::StubScreen;
    use crossterm::event::KeyModifiers;

    // Hermetic doubles: every stub returns Ignored, so these tests exercise the
    // shell's global routing regardless of the real tab set.
    fn shell() -> Shell {
        let screens: Vec<Box<dyn Screen>> = vec![
            Box::new(StubScreen::new("WIKI", "", "")),
            Box::new(StubScreen::new("BIOS", "", "")),
            Box::new(StubScreen::new("APU", "", "")),
            Box::new(StubScreen::new("MEM", "", "")),
        ];
        Shell::new(screens, 0)
    }

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn cycle_wraps_both_ways() {
        let mut s = shell();
        assert_eq!(s.active, 0);
        s.cycle(1);
        assert_eq!(s.active, 1);
        s.cycle(-1);
        assert_eq!(s.active, 0);
        s.cycle(-1); // wrap to last
        assert_eq!(s.active, 3);
        s.cycle(1); // wrap to first
        assert_eq!(s.active, 0);
    }

    #[test]
    fn function_keys_jump_directly() {
        let mut s = shell();
        // Stubs return Ignored, so the shell's global bindings apply.
        matches!(
            s.route_key(key(KeyCode::F(3), KeyModifiers::NONE)),
            Flow::Continue
        );
        assert_eq!(s.active, 2); // F3 -> APU (index 2)
        matches!(
            s.route_key(key(KeyCode::F(1), KeyModifiers::NONE)),
            Flow::Continue
        );
        assert_eq!(s.active, 0); // F1 -> WIKI
    }

    #[test]
    fn alt_numbers_jump() {
        let mut s = shell();
        s.route_key(key(KeyCode::Char('4'), KeyModifiers::ALT));
        assert_eq!(s.active, 3); // Alt-4 -> MEM
    }

    #[test]
    fn bare_numbers_jump() {
        let mut s = shell();
        // Stubs return Ignored, so bare 1-4 fall through to the global tab switch.
        s.route_key(key(KeyCode::Char('3'), KeyModifiers::NONE));
        assert_eq!(s.active, 2); // 3 -> APU
        s.route_key(key(KeyCode::Char('1'), KeyModifiers::NONE));
        assert_eq!(s.active, 0); // 1 -> WIKI
        s.route_key(key(KeyCode::Char('4'), KeyModifiers::NONE));
        assert_eq!(s.active, 3); // 4 -> MEM
    }

    #[test]
    fn ctrl_q_quits() {
        let mut s = shell();
        assert!(matches!(
            s.route_key(key(KeyCode::Char('q'), KeyModifiers::CONTROL)),
            Flow::Quit
        ));
    }

    #[test]
    fn ctrl_tab_cycles() {
        let mut s = shell();
        s.route_key(key(KeyCode::Tab, KeyModifiers::CONTROL));
        assert_eq!(s.active, 1);
        s.route_key(key(
            KeyCode::Tab,
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        ));
        assert_eq!(s.active, 0);
    }

    #[test]
    fn bare_q_is_not_global_quit() {
        // A bare `q` reaches global_key only via the Ignored fallback; global_key
        // does not bind bare `q`, so it is a no-op (panes own it).
        let mut s = shell();
        assert!(matches!(
            s.route_key(key(KeyCode::Char('q'), KeyModifiers::NONE)),
            Flow::Continue
        ));
        assert_eq!(s.active, 0);
    }
}
