// SPDX-License-Identifier: GPL-2.0-only
use crossterm::event::KeyEvent;
use ratatui::{layout::Rect, Frame};
use std::time::Duration;

/// What a screen returns after handling a key.
///
/// The shell routes pane-first: the active screen sees every key first. Only when
/// it returns [`Outcome::Ignored`] (and it is not in a modal sub-state) does the
/// shell apply its own global bindings (tab switch, quit). This keeps each pane in
/// full control of its own keyspace -- important because the four merged apps each
/// bind bare `Tab`/`Shift-Tab`/`Shift`/`q` internally.
pub enum Outcome {
    /// The screen handled the key; the shell does nothing further.
    Consumed,
    /// The screen did not handle it; the shell may apply a global binding.
    Ignored,
    /// Tear the whole app down (a pane explicitly asked to quit the suite).
    Quit,
    /// Tear down + reboot with this operator message (the memtune/biostune write flow).
    Reboot(String),
}

/// One tab in the suite. Each former standalone app (wiki/bios/apu/mem) implements
/// this; the shell owns the terminal, the event loop, the tab bar, and the status line.
pub trait Screen {
    /// Tab-bar label. Fixed order is enforced by the shell, not the screen.
    fn title(&self) -> &'static str;

    /// Render into the content area (the tab bar and global status line are NOT here).
    fn draw(&mut self, f: &mut Frame, area: Rect);

    /// Handle one key press.
    fn on_key(&mut self, key: KeyEvent) -> Outcome;

    /// Throttled background refresh (telemetry, polling a bench thread). Default no-op.
    fn tick(&mut self) {}

    /// How long the shell may block in `event::poll` before calling [`Screen::tick`].
    /// An idle screen (e.g. BIOS, no telemetry) returns ~1s; a live one ~90-300ms.
    fn tick_hint(&self) -> Duration {
        Duration::from_millis(250)
    }

    /// Called when this tab gains focus -- seed live data so the first frame is not blank.
    fn on_enter(&mut self) {}

    /// Called when leaving this tab -- pause heavy polling, drop hardware handles if desired.
    fn on_exit(&mut self) {}

    /// Right-aligned per-screen help/status hint, merged into the global status line.
    fn status_hint(&self) -> Option<String> {
        None
    }

    /// True while the screen is in a modal sub-state (search box, confirm, field edit,
    /// a popup). While true the shell MUST NOT steal global switch/quit keys.
    fn modal(&self) -> bool {
        false
    }
}
