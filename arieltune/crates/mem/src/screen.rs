// SPDX-License-Identifier: GPL-2.0-only
//! Interactive MEM tab — a single Tune view:
//!   * a live measured bandwidth/random/latency readout (animated while benching),
//!   * an editable timings table (stock/live/draft/range columns), and
//!   * a history of each applied edit's result; `d` explains the selected field.
//!
//! Overlays (opened from the view): saved-timings picker (`p`), live UMC
//! registers (`u`), system memtest (`m`), and the write/clear confirms.
//!
//! Edits build a draft; nothing touches CMOS until a confirmed write.
//!
//! Ported from memtune's `ui.rs`: the raw-mode/alt-screen/Terminal/event-loop/
//! panic-hook plumbing moved to the suite shell; the state + draw + key handling
//! became this [`Screen`]. `draw` renders into the shell's content `area`. The
//! background bench/memtest keep their `thread::spawn` + join-handle, but the
//! POLL moved into [`Screen::tick`] so the UI thread never blocks — keys still
//! switch tabs while a bench runs. `Post::Quit` -> [`Outcome::Quit`];
//! `Post::Reboot` -> [`Outcome::Reboot`] (the write flow applies timings via CMOS
//! and reboots). The write-path safety is preserved verbatim (draft staging,
//! confirm-then-reboot, auto-backup).
//!
//! DEVIATION (non-blocking post-reboot eval): memtune ran the "did the last edit
//! train?" benchmark BLOCKING at TUI launch. Blocking here would freeze suite
//! startup and hold the shared compute lock, so instead the eval kicks off as a
//! background bench on first [`Screen::on_enter`] and records to history when it
//! completes (same result, off the UI thread).

use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::Result;
use arieltune_tui_kit::{Outcome, Screen};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::prelude::*;
use ratatui::widgets::{Block, BorderType, Borders, Cell, Clear, Paragraph, Row, Table, Wrap};

use crate::cmos;
use crate::config::{signature_name, Field, MemConf, CONFIG_SIZE, FIELDS, SIG_ABL, SIG_LINUX_TOOL};
use crate::fields::{self, Dir, Tier};
use crate::{bench, config, metrics, profiles, sysmem, tune, umc};

// Palette
const ACCENT: Color = Color::Cyan;
const GOOD: Color = Color::Green;
const WARN: Color = Color::Yellow;
const BAD: Color = Color::Red;
const DIM: Color = Color::DarkGray;
const INK: Color = Color::Black;
/// Footer key chips — purple so the [keys] stand out from their labels.
const KEY: Color = Color::Magenta;
/// 256-bit GDDR6 @ 14 Gbps — the theoretical ceiling the readout scales to.
const BW_CEILING: f64 = 448.0;
/// Interactive system-memtest defaults (a quick gate; the CLI `memtest --secs`
/// does the long soak). 2048 MiB / 30s mirrors the quick CLI invocation.
const MEMTEST_MB: usize = 2048;
const MEMTEST_SECS: u64 = 30;

fn editable() -> Vec<&'static Field> {
    FIELDS
        .iter()
        .filter(|f| !matches!(f.name, "Signature" | "Checksum"))
        .collect()
}

fn step_for(f: &Field, big: bool) -> i64 {
    let base = if f.name == "ClockSpeed" { 50 } else { 1 };
    if big {
        base * 10
    } else {
        base
    }
}

fn rounded(title: &str) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(DIM))
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))
}

enum Mode {
    Normal,
    Confirm(Confirm),
    /// Typing a number into the selected field's draft (buffer of digits).
    EditField(String),
    /// Live UMC register viewer overlay.
    Umc,
    /// System-RAM memtest overlay (live progress + result).
    Memtest,
    /// Saved-timings picker: choose a known-good config and reboot into it.
    Profiles,
}

#[derive(Clone, Copy)]
enum Confirm {
    Write,
    ClearHistory,
}

enum Bench {
    Idle,
    Running(JoinHandle<Option<bench::BenchResult>>),
    Done(bench::BenchResult),
    Failed,
}

enum Memtest {
    Idle,
    Running(JoinHandle<Option<sysmem::SysMemResult>>),
    Done(sysmem::SysMemResult),
    Failed,
}

/// The MEM tab: memtune's Tune view as an [`arieltune_tui_kit::Screen`].
pub struct MemScreen {
    fields: Vec<&'static Field>,
    current: MemConf,
    draft: MemConf,
    /// The original config saved on first run (for the "stock" column + revert).
    stock: Option<MemConf>,
    sel: usize,
    mode: Mode,
    status: String,
    bench: Bench,
    memtest: Memtest,
    /// When the current memtest started — drives the overlay's elapsed readout.
    memtest_started: Instant,
    started: Instant,
    /// Right pane shows the field doc (true) or the edit history (false).
    show_doc: bool,
    /// Arrows drive the history list (true) instead of the timings table (false).
    focus_history: bool,
    /// Selected history row (newest = 0) when the history is focused.
    hist_sel: usize,
    /// Live GPU/memory telemetry (refreshed on idle ticks, not during a bench).
    telem: Option<metrics::Telemetry>,
    /// Fixed UMC offsets to show in the live overlay — captured once when it
    /// opens so rows stay put (no jumping as a register dips through zero).
    umc_offsets: Vec<u32>,
    /// Saved-timings picker contents (rebuilt each time the overlay opens).
    profiles: Vec<profiles::Profile>,
    /// Selected row in the saved-timings picker.
    prof_sel: usize,
    /// A staged edit rebooted and is awaiting evaluation (kicked off on_enter).
    eval_pending: bool,
    /// The currently-running bench is the post-reboot eval — record it to history
    /// when it completes (vs. a plain interactive `b` bench, which only displays).
    eval_recording: bool,
}

impl MemScreen {
    /// Build the MEM screen from the live CMOS config (infallible: off a BC-250,
    /// or without root, CMOS is unreadable — fall back to a zero config + a
    /// view-only status, never a hard error at suite startup).
    pub fn new() -> Self {
        match cmos::read_config() {
            Ok(buf) => {
                let current = MemConf::from_bytes(buf);
                // Capture the original timings the first time memtune runs, so
                // they're never lost; reuse the saved snapshot on later runs.
                let _ = tune::ensure_stock(&current);
                let stock = tune::stock();
                Self::build(current, stock, "loaded from CMOS".into(), true)
            }
            Err(e) => {
                // No live config: never write a zero buffer as "stock".
                let current = MemConf::from_bytes([0u8; CONFIG_SIZE]);
                let stock = tune::stock();
                Self::build(
                    current,
                    stock,
                    format!("CMOS not readable ({e}) — run as root on a BC-250; view-only"),
                    false,
                )
            }
        }
    }

    fn build(current: MemConf, stock: Option<MemConf>, status: String, live: bool) -> Self {
        MemScreen {
            fields: editable(),
            draft: current.clone(),
            current,
            stock,
            sel: 0,
            mode: Mode::Normal,
            status,
            bench: Bench::Idle,
            memtest: Memtest::Idle,
            memtest_started: Instant::now(),
            started: Instant::now(),
            show_doc: false,
            focus_history: false,
            hist_sel: 0,
            telem: metrics::read(),
            umc_offsets: Vec::new(),
            profiles: Vec::new(),
            prof_sel: 0,
            // Only evaluate a staged edit when we actually read a live config AND
            // the CMOS read confirms the firmware flipped the signature (applied).
            // A read failure returns None -> not treated as applied (M6).
            eval_pending: live && tune::pending_applied() == Some(true),
            eval_recording: false,
        }
    }

    fn reload(&mut self) -> Result<()> {
        self.current = MemConf::from_bytes(cmos::read_config()?);
        self.draft = self.current.clone();
        self.status = "reloaded from CMOS — draft reset".into();
        Ok(())
    }

    fn dirty(&self) -> bool {
        self.fields
            .iter()
            .any(|f| self.current.get_field(f) != self.draft.get_field(f))
    }

    /// The changed fields (name, live, draft) — the diff being written.
    fn diff(&self) -> Vec<(String, u32, u32)> {
        self.fields
            .iter()
            .filter_map(|f| {
                let (a, b) = (self.current.get_field(f), self.draft.get_field(f));
                (a != b).then(|| (f.name.to_string(), a, b))
            })
            .collect()
    }

    fn write(&mut self) -> Result<()> {
        let _ = cmos::auto_backup();
        let mut out = self.draft.clone();
        out.stamp(SIG_LINUX_TOOL);
        cmos::write_config(&out.buf)
    }

    fn start_bench(&mut self) {
        if matches!(self.bench, Bench::Running(_)) {
            return;
        }
        self.bench = Bench::Running(std::thread::spawn(|| bench::run().ok()));
        self.status = "running bench (bandwidth + random + latency + integrity)...".into();
    }

    fn start_memtest(&mut self) {
        if matches!(self.memtest, Memtest::Running(_)) {
            return;
        }
        self.memtest_started = Instant::now();
        self.memtest = Memtest::Running(std::thread::spawn(|| {
            sysmem::test(MEMTEST_MB, MEMTEST_SECS).ok()
        }));
        self.status = format!("running system memtest ({MEMTEST_MB} MiB, {MEMTEST_SECS}s)...");
    }

    fn poll_memtest(&mut self) {
        if let Memtest::Running(h) = &self.memtest {
            if h.is_finished() {
                let done = std::mem::replace(&mut self.memtest, Memtest::Idle);
                if let Memtest::Running(h) = done {
                    self.memtest = match h.join() {
                        Ok(Some(r)) => {
                            self.status = if r.errors == 0 {
                                format!(
                                    "memtest OK — 0 errors over {} pass(es) of {MEMTEST_MB} MiB in {:.0}s",
                                    r.passes, r.secs
                                )
                            } else {
                                format!(
                                    "memtest FAILED — {} errors: this config CORRUPTS system RAM",
                                    r.errors
                                )
                            };
                            Memtest::Done(r)
                        }
                        _ => {
                            self.status = "memtest failed (allocation?)".into();
                            Memtest::Failed
                        }
                    };
                }
            }
        }
    }

    fn poll_bench(&mut self) {
        if let Bench::Running(h) = &self.bench {
            if h.is_finished() {
                let done = std::mem::replace(&mut self.bench, Bench::Idle);
                if let Bench::Running(h) = done {
                    self.bench = match h.join() {
                        Ok(Some(r)) => {
                            if self.eval_recording {
                                // Post-reboot eval: log the trained edit's result.
                                self.eval_recording = false;
                                let _ = tune::record(
                                    true,
                                    r.bandwidth_gbps,
                                    r.random_gbps,
                                    r.latency_ns,
                                    r.stability_errors,
                                );
                                self.status = if r.stability_errors == 0 {
                                    format!(
                                        "recorded: {:.1} GB/s, {:.0} ns, stable",
                                        r.bandwidth_gbps, r.latency_ns
                                    )
                                } else {
                                    format!(
                                        "recorded: {:.1} GB/s but {} integrity errors — UNSTABLE",
                                        r.bandwidth_gbps, r.stability_errors
                                    )
                                };
                            } else {
                                // The numbers live in the measured-bandwidth box;
                                // keep the status line from echoing them.
                                self.status = "bench complete".into();
                            }
                            Bench::Done(r)
                        }
                        _ => {
                            if self.eval_recording {
                                self.eval_recording = false;
                                let _ = tune::record(true, 0.0, 0.0, 0.0, 0);
                                self.status = "edit trained, but the benchmark failed".into();
                            } else {
                                self.status = "benchmark failed (Vulkan?)".into();
                            }
                            Bench::Failed
                        }
                    };
                }
            }
        }
    }

    /// Normal-mode key handling (was memtune's `tune_key`). Returns true when the
    /// key was handled (Consumed), false when it should fall through to the shell.
    fn tune_key(&mut self, code: KeyCode, big: bool) -> bool {
        match code {
            // focus: Tab swaps arrow control between the timings table and history
            KeyCode::Tab | KeyCode::BackTab => {
                self.focus_history = !self.focus_history;
                if self.focus_history {
                    self.show_doc = false; // show the list we're selecting in
                    self.hist_sel = 0;
                }
            }
            KeyCode::Up => {
                if self.focus_history {
                    self.hist_sel = self.hist_sel.saturating_sub(1);
                } else {
                    self.sel = (self.sel + self.fields.len() - 1) % self.fields.len();
                }
            }
            KeyCode::Down => {
                if self.focus_history {
                    let len = tune::history().len();
                    if len > 0 {
                        self.hist_sel = (self.hist_sel + 1).min(len - 1);
                    }
                } else {
                    self.sel = (self.sel + 1) % self.fields.len();
                }
            }
            KeyCode::Left if !self.focus_history => {
                let f = self.fields[self.sel];
                self.draft.nudge(f, -step_for(f, big));
            }
            KeyCode::Right if !self.focus_history => {
                let f = self.fields[self.sel];
                self.draft.nudge(f, step_for(f, big));
            }
            KeyCode::Char('x') => {
                if self.focus_history {
                    let len = tune::history().len();
                    if len > 0 {
                        let _ = tune::delete(self.hist_sel);
                        self.hist_sel = self.hist_sel.min(len.saturating_sub(2));
                        self.status = "deleted history entry".into();
                    } else {
                        self.status = "history is empty".into();
                    }
                } else {
                    self.status = "press Tab to focus the history, then x to delete a row".into();
                }
            }
            KeyCode::Char('c') => {
                if tune::history().is_empty() {
                    self.status = "history is already empty".into();
                } else {
                    self.mode = Mode::Confirm(Confirm::ClearHistory);
                }
            }
            KeyCode::Char('u') => {
                self.umc_offsets = umc::live_offsets().unwrap_or_default();
                self.mode = Mode::Umc;
            }
            KeyCode::Char('p') => {
                self.profiles = profiles::discover();
                self.prof_sel = 0;
                self.mode = Mode::Profiles;
            }
            KeyCode::Char('e') if !self.focus_history => {
                let f = self.fields[self.sel];
                self.mode = Mode::EditField(String::new());
                self.status = format!(
                    "editing {} — type a value ({}..{}), Enter to set, Esc to cancel",
                    f.name, f.lo, f.hi
                );
            }
            KeyCode::Char('d') => {
                self.show_doc = !self.show_doc;
                if self.show_doc {
                    self.focus_history = false;
                }
            }
            KeyCode::Char('0') => match &self.stock {
                Some(s) => {
                    self.draft = s.clone();
                    self.focus_history = false;
                    self.status =
                        "loaded the saved stock config into the draft (press w to apply)".into();
                }
                None => self.status = "no stock snapshot saved yet".into(),
            },
            KeyCode::Char('r') => {
                if let Err(e) = self.reload() {
                    self.status = format!("reload failed: {e}");
                }
            }
            KeyCode::Char('w') => {
                if self.dirty() {
                    self.mode = Mode::Confirm(Confirm::Write);
                } else {
                    self.status = "no changes to write".into();
                }
            }
            // Unhandled: let the shell apply a global binding.
            _ => return false,
        }
        true
    }
}

impl Default for MemScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl Screen for MemScreen {
    fn title(&self) -> &'static str {
        "MEM"
    }

    fn draw(&mut self, f: &mut Frame, area: Rect) {
        draw(f, area, self);
    }

    fn on_key(&mut self, k: KeyEvent) -> Outcome {
        let big = k.modifiers.contains(KeyModifiers::SHIFT);

        if let Mode::Confirm(kind) = self.mode {
            match k.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.mode = Mode::Normal;
                    match kind {
                        Confirm::Write => {
                            // stage the diff for post-reboot history, write, reboot
                            let diff = self.diff();
                            let _ = tune::stage(diff);
                            match self.write() {
                                Ok(()) => {
                                    return Outcome::Reboot(
                                        "applying edit — rebooting to test it...".into(),
                                    )
                                }
                                Err(e) => self.status = format!("write failed: {e}"),
                            }
                        }
                        Confirm::ClearHistory => {
                            self.status = match tune::clear() {
                                Ok(()) => "history cleared".into(),
                                Err(e) => format!("clear failed: {e}"),
                            };
                            self.hist_sel = 0;
                        }
                    }
                }
                _ => {
                    self.mode = Mode::Normal;
                    self.status = "cancelled".into();
                }
            }
            return Outcome::Consumed;
        }

        // Typing a numeric value into the selected field's draft.
        if matches!(self.mode, Mode::EditField(_)) {
            let Mode::EditField(mut buf) = std::mem::replace(&mut self.mode, Mode::Normal) else {
                unreachable!()
            };
            match k.code {
                KeyCode::Char(c) if c.is_ascii_digit() => {
                    buf.push(c);
                    self.mode = Mode::EditField(buf);
                }
                KeyCode::Backspace => {
                    buf.pop();
                    self.mode = Mode::EditField(buf);
                }
                KeyCode::Enter => match buf.parse::<u32>() {
                    Ok(v) => {
                        let f = self.fields[self.sel];
                        let clamped = v.clamp(f.lo, f.hi);
                        self.draft.set(f.name, clamped);
                        self.status = if clamped == v {
                            format!("set {} = {clamped}", f.name)
                        } else {
                            format!("set {} = {clamped} (clamped to {}..{})", f.name, f.lo, f.hi)
                        };
                    }
                    Err(_) => self.status = "edit cancelled (no number entered)".into(),
                },
                KeyCode::Esc => self.status = "edit cancelled".into(),
                // ignore other keys, stay in edit mode
                _ => self.mode = Mode::EditField(buf),
            }
            return Outcome::Consumed;
        }

        // Saved-timings picker: navigable, so it gets its own handler (unlike the
        // UMC/memtest overlays below, which close on any key). Enter loads the
        // selected config into the draft and drops straight into the write+reboot
        // confirm — "pick a known-good config and reboot into it" in two keys.
        if matches!(self.mode, Mode::Profiles) {
            match k.code {
                KeyCode::Up => self.prof_sel = self.prof_sel.saturating_sub(1),
                KeyCode::Down if !self.profiles.is_empty() => {
                    self.prof_sel = (self.prof_sel + 1).min(self.profiles.len() - 1);
                }
                KeyCode::Enter => {
                    if let Some(p) = self.profiles.get(self.prof_sel) {
                        self.draft = p.conf.clone();
                        let label = p.label.clone();
                        self.focus_history = false;
                        if self.dirty() {
                            self.status = format!("loaded '{label}' — confirm to write + reboot");
                            self.mode = Mode::Confirm(Confirm::Write);
                        } else {
                            self.mode = Mode::Normal;
                            self.status = format!("'{label}' already matches the live config");
                        }
                    } else {
                        self.mode = Mode::Normal;
                    }
                }
                KeyCode::Esc | KeyCode::Char('q') => {
                    self.mode = Mode::Normal;
                    self.status = "cancelled".into();
                }
                _ => {}
            }
            return Outcome::Consumed;
        }

        // The live UMC / memtest overlays: any key closes them. The memtest thread
        // keeps running in the background (its result lands in the status line via
        // poll_memtest), so closing the overlay never cancels the test.
        if matches!(self.mode, Mode::Umc | Mode::Memtest) {
            self.mode = Mode::Normal;
            return Outcome::Consumed;
        }

        match k.code {
            KeyCode::Char('q') | KeyCode::Esc => return Outcome::Quit,
            KeyCode::Char('b') => {
                self.start_bench();
                return Outcome::Consumed;
            }
            KeyCode::Char('m') => {
                self.start_memtest();
                self.mode = Mode::Memtest;
                return Outcome::Consumed;
            }
            _ => {}
        }
        if self.tune_key(k.code, big) {
            Outcome::Consumed
        } else {
            Outcome::Ignored
        }
    }

    /// Poll the background bench/memtest threads and refresh telemetry — moved
    /// out of the old blocking event loop so keys switch tabs while a bench runs.
    fn tick(&mut self) {
        self.poll_bench();
        self.poll_memtest();
        // Refresh live telemetry on idle ticks only — reading gpu_metrics during a
        // compute dispatch can race. Right after a bench, the clock is still near
        // its run value (slow descend), so this shows what the bench ran at.
        if !matches!(self.bench, Bench::Running(_)) {
            if let Some(t) = metrics::read() {
                self.telem = Some(t);
            }
        }
    }

    /// Animate faster while a benchmark/memtest runs (bus pulse) or the live UMC
    /// overlay is up (so its counters visibly move); idle otherwise.
    fn tick_hint(&self) -> Duration {
        if matches!(self.bench, Bench::Running(_))
            || matches!(self.memtest, Memtest::Running(_))
            || matches!(self.mode, Mode::Umc | Mode::Memtest)
        {
            Duration::from_millis(90)
        } else {
            Duration::from_millis(300)
        }
    }

    /// First focus: seed telemetry, and kick off the post-reboot eval (if a staged
    /// edit rebooted) as a background bench — see the module DEVIATION note.
    fn on_enter(&mut self) {
        if let Some(t) = metrics::read() {
            self.telem = Some(t);
        }
        if self.eval_pending {
            self.eval_pending = false;
            let sig = self.current.signature();
            let trained = config::signature_ok(sig) && self.current.checksum_valid();
            if trained {
                self.eval_recording = true;
                self.start_bench();
                self.status = "evaluating the last edit (benchmarking)...".into();
            } else {
                let _ = tune::record(false, 0.0, 0.0, 0.0, 0);
                self.status = format!("last edit was rejected ({}) — logged", signature_name(sig));
            }
        }
    }

    /// Confirm / field-edit / picker / overlays are modal sub-states: while in one
    /// the shell must not steal global switch/quit keys.
    fn modal(&self) -> bool {
        !matches!(self.mode, Mode::Normal)
    }
}

fn draw(f: &mut Frame, area: Rect, app: &MemScreen) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(2),
        ])
        .split(area);

    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            " arieltune mem ",
            Style::default().fg(KEY).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  GDDR6 memory timings", Style::default().fg(DIM)),
    ]));
    f.render_widget(title, chunks[0]);

    draw_tune(f, chunks[1], app);
    draw_footer(f, chunks[2], app);

    match &app.mode {
        Mode::Confirm(kind) => draw_confirm(f, area, app, *kind),
        Mode::EditField(buf) => draw_edit(f, area, app, buf),
        Mode::Umc => draw_umc(f, area, &app.umc_offsets),
        Mode::Memtest => draw_memtest(f, area, app),
        Mode::Profiles => draw_profiles(f, area, app),
        Mode::Normal => {}
    }
}

fn bw_color(ratio: f64) -> Color {
    if ratio >= 0.85 {
        GOOD
    } else if ratio >= 0.6 {
        WARN
    } else {
        BAD
    }
}

// ---- Tune view -------------------------------------------------------------

fn draw_tune(f: &mut Frame, area: Rect, app: &MemScreen) {
    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(7), Constraint::Min(0)])
        .split(area);
    let top = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(26), Constraint::Length(46)])
        .split(v[0]);

    // status panel (signature + what it means + checksum/dirty + bus spec)
    let sig = app.current.signature();
    let gbps_pin = app.current.get("ClockSpeed").unwrap_or(0) as f64 * 8.0 / 1000.0;
    let busline =
        format!("256-bit bus  .  8 chips x 2 sub-ch (16-bit, 1 GB)  .  ~{gbps_pin:.1} Gbps/pin");
    let ck = if app.current.checksum_valid() {
        Span::styled("checksum ok", Style::default().fg(GOOD))
    } else {
        Span::styled("checksum BAD", Style::default().fg(BAD))
    };
    let dirty = if app.dirty() {
        Span::styled(
            "  * draft modified",
            Style::default().fg(WARN).add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled("  clean", Style::default().fg(DIM))
    };
    let info = Paragraph::new(vec![
        Line::from(vec![
            Span::styled("signature  ", Style::default().fg(DIM)),
            Span::styled(
                signature_name(sig),
                sig_style(sig).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("  -> ", Style::default().fg(DIM)),
            Span::styled(config::signature_hint(sig), sig_style(sig)),
        ]),
        Line::from(vec![
            Span::styled("checksum   ", Style::default().fg(DIM)),
            ck,
            dirty,
        ]),
        Line::from(Span::styled(busline, Style::default().fg(DIM))),
    ])
    .wrap(Wrap { trim: true })
    .block(rounded("BC-250 GDDR6"));
    f.render_widget(info, top[0]);

    draw_measured_box(f, top[1], app);

    // table on the left; history (default) or the field doc (`d`) on the right
    let b = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(56), Constraint::Min(38)])
        .split(v[1]);
    draw_table(f, b[0], app);
    if app.show_doc {
        draw_field_doc(f, b[1], app.fields[app.sel].name);
    } else {
        draw_history(f, b[1], app.hist_sel, app.focus_history);
    }
}

/// The animated measured-bandwidth box (top-right of the Tune view). Pulses
/// while benching.
fn draw_measured_box(f: &mut Frame, area: Rect, app: &MemScreen) {
    let mut lines = match &app.bench {
        Bench::Done(r) => {
            let rr = (r.bandwidth_gbps / BW_CEILING).clamp(0.0, 1.0);
            let w = 26usize;
            let fill = (rr * w as f64).round() as usize;
            vec![
                Line::from(vec![
                    Span::styled(
                        format!("{:.1}", r.bandwidth_gbps),
                        Style::default()
                            .fg(bw_color(rr))
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" GB/s", Style::default().fg(DIM)),
                    Span::styled(
                        format!("   {:.0} ns", r.latency_ns),
                        Style::default().fg(ACCENT),
                    ),
                    Span::styled(format!("   {:.0}%", rr * 100.0), Style::default().fg(DIM)),
                ]),
                Line::from(vec![
                    Span::styled("\u{2588}".repeat(fill), Style::default().fg(bw_color(rr))),
                    Span::styled(
                        "\u{2591}".repeat(w.saturating_sub(fill)),
                        Style::default().fg(DIM),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("rnd ", Style::default().fg(DIM)),
                    Span::styled(
                        format!("{:.1}", r.random_gbps),
                        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" GB/s    ", Style::default().fg(DIM)),
                    if r.stability_bytes == 0 {
                        Span::raw("")
                    } else if r.stability_errors == 0 {
                        Span::styled(
                            "stable",
                            Style::default().fg(GOOD).add_modifier(Modifier::BOLD),
                        )
                    } else {
                        Span::styled(
                            format!("{} ERRORS", r.stability_errors),
                            Style::default().fg(BAD).add_modifier(Modifier::BOLD),
                        )
                    },
                ]),
            ]
        }
        Bench::Running(_) => vec![
            Line::from(Span::styled("measuring...", Style::default().fg(WARN))),
            pulse_line(26, app.started.elapsed().as_millis()),
        ],
        Bench::Failed => vec![Line::from(Span::styled(
            "bench failed",
            Style::default().fg(BAD),
        ))],
        Bench::Idle => vec![Line::from(Span::styled(
            "press b to measure",
            Style::default().fg(DIM),
        ))],
    };
    // Live telemetry line at the top: GFX clock (drives latency/random), the
    // locked memory clock, and GPU temp.
    let telem_line = match &app.telem {
        Some(t) => Line::from(vec![
            Span::styled("gfx ", Style::default().fg(DIM)),
            Span::styled(
                format!("{} MHz", t.gfxclk_mhz),
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled("   mem ", Style::default().fg(DIM)),
            Span::styled(format!("{}", t.uclk_mhz), Style::default().fg(Color::White)),
            Span::styled("   ", Style::default().fg(DIM)),
            Span::styled(
                format!("{:.0}\u{00B0}C", t.temp_c),
                Style::default()
                    .fg(temp_color(t.temp_c))
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        None => Line::from(Span::styled("clocks: n/a", Style::default().fg(DIM))),
    };
    lines.insert(0, Line::from(""));
    lines.insert(0, telem_line);
    let p = Paragraph::new(lines)
        .alignment(Alignment::Center)
        .block(rounded("measured"));
    f.render_widget(p, area);
}

/// GPU temp color — the BC-250's governor thermal-panics around 85 C.
fn temp_color(c: f64) -> Color {
    if c >= 80.0 {
        BAD
    } else if c >= 70.0 {
        WARN
    } else {
        GOOD
    }
}

fn draw_history(f: &mut Frame, area: Rect, sel: usize, focused: bool) {
    let title = if focused {
        "history (focused)"
    } else {
        "history"
    };
    let hist = tune::history();
    if hist.is_empty() {
        let p = Paragraph::new(Span::styled(
            "no edits yet — change a timing, press w",
            Style::default().fg(DIM),
        ))
        .wrap(Wrap { trim: true })
        .block(rounded(title));
        f.render_widget(p, area);
        return;
    }
    let rows: Vec<Row> = hist
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let selected = i == sel;
            let mark = if selected { ">" } else { " " };
            let name_style = if selected && focused {
                Style::default()
                    .fg(INK)
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else if selected {
                Style::default().fg(ACCENT)
            } else {
                Style::default().fg(Color::White)
            };
            // seq / rnd / lat columns (blank if it didn't train), then a status.
            // Bandwidth + random keep 0.1 GB/s precision (do NOT round to integer);
            // latency is whole ns.
            let (seq, rnd, lat) = if e.trained {
                (
                    format!("{:.1}", e.bw),
                    format!("{:.1}", e.random),
                    format!("{:.0}", e.lat),
                )
            } else {
                (String::new(), String::new(), String::new())
            };
            let (status, status_col) = if !e.trained {
                ("no-POST", BAD)
            } else if e.errors > 0 {
                ("UNSTABLE", BAD)
            } else {
                ("ok", GOOD)
            };
            Row::new(vec![
                Cell::from(format!("{mark} {}", e.desc)).style(name_style),
                Cell::from(seq).style(Style::default().fg(DIM)),
                Cell::from(rnd).style(Style::default().fg(ACCENT)),
                Cell::from(lat).style(Style::default().fg(DIM)),
                Cell::from(status).style(Style::default().fg(status_col)),
            ])
        })
        .collect();
    let table = Table::new(
        rows,
        [
            Constraint::Min(14),
            Constraint::Length(6),
            Constraint::Length(6),
            Constraint::Length(5),
            Constraint::Length(9),
        ],
    )
    .header(
        Row::new(vec!["  change", "seq", "rnd", "lat", "status"])
            .style(Style::default().fg(ACCENT)),
    )
    .block(rounded(title));
    f.render_widget(table, area);
}

/// Color for a gain tier (green = worth it, dim = barely).
fn gain_color(t: Tier) -> Color {
    match t {
        Tier::High => GOOD,
        Tier::Med => ACCENT,
        _ => DIM,
    }
}

/// Color for a risk tier (red = dangerous, green = safe).
fn risk_color(t: Tier) -> Color {
    match t {
        Tier::High => BAD,
        Tier::Med => WARN,
        Tier::Low => GOOD,
        Tier::None => DIM,
    }
}

/// A plain-language verdict on whether a field is worth tuning, from gain + risk.
fn recommendation(gain: Tier, risk: Tier) -> (&'static str, Color) {
    match (gain, risk) {
        (Tier::None, _) => ("not a performance knob", DIM),
        (Tier::High, Tier::High) => ("worth trying — but high risk", WARN),
        (Tier::High, _) => ("worth trying", GOOD),
        (Tier::Med, _) => ("minor gains possible", ACCENT),
        (Tier::Low, _) => ("rarely worth tuning", DIM),
    }
}

/// The teaching panel: explains whatever field the cursor is on (`d` toggles it).
fn draw_field_doc(f: &mut Frame, area: Rect, name: &str) {
    let Some(d) = fields::doc(name) else {
        let p = Paragraph::new(Span::styled("(no description)", Style::default().fg(DIM)))
            .block(rounded(name));
        f.render_widget(p, area);
        return;
    };

    let dir_color = match d.faster {
        Dir::LowerFaster => ACCENT,
        Dir::HigherFaster => GOOD,
        Dir::None => DIM,
    };

    let mut lines = vec![
        Line::from(Span::styled(
            d.full,
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled(format!("{}  ", d.group), Style::default().fg(DIM)),
            Span::styled("gain ", Style::default().fg(DIM)),
            Span::styled(
                d.gain.label(),
                Style::default()
                    .fg(gain_color(d.gain))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  risk ", Style::default().fg(DIM)),
            Span::styled(
                d.risk.label(),
                Style::default()
                    .fg(risk_color(d.risk))
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(Span::styled(
            d.faster.hint(),
            Style::default().fg(dir_color),
        )),
        {
            let (rec, col) = recommendation(d.gain, d.risk);
            Line::from(vec![
                Span::styled("recommendation: ", Style::default().fg(DIM)),
                Span::styled(rec, Style::default().fg(col).add_modifier(Modifier::BOLD)),
            ])
        },
        Line::from(""),
        Line::from(Span::styled(d.blurb, Style::default().fg(Color::White))),
        Line::from(""),
        Line::from(Span::styled(d.detail, Style::default().fg(Color::Gray))),
    ];
    if !d.note.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled(
                "note: ",
                Style::default().fg(WARN).add_modifier(Modifier::BOLD),
            ),
            Span::styled(d.note, Style::default().fg(WARN)),
        ]));
    }

    let title = format!("{name} — what it is");
    let p = Paragraph::new(lines)
        .wrap(Wrap { trim: true })
        .block(rounded(&title));
    f.render_widget(p, area);
}

fn draw_table(f: &mut Frame, area: Rect, app: &MemScreen) {
    let rows: Vec<Row> = app
        .fields
        .iter()
        .enumerate()
        .map(|(i, fl)| {
            let cur = app.current.get_field(fl);
            let dft = app.draft.get_field(fl);
            let changed = cur != dft;
            let selected = i == app.sel;
            let name = if selected {
                format!(" > {}", fl.name)
            } else {
                format!("   {}", fl.name)
            };
            let name_style = if selected {
                Style::default()
                    .fg(INK)
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            let draft_cell = if changed {
                Cell::from(format!("{dft}"))
                    .style(Style::default().fg(WARN).add_modifier(Modifier::BOLD))
            } else {
                Cell::from(format!("{dft}")).style(Style::default().fg(Color::White))
            };
            let stock_cell = match &app.stock {
                Some(s) => Cell::from(s.get_field(fl).to_string()).style(Style::default().fg(DIM)),
                None => Cell::from("-").style(Style::default().fg(DIM)),
            };
            Row::new(vec![
                Cell::from(name).style(name_style),
                stock_cell,
                Cell::from(cur.to_string()).style(Style::default().fg(DIM)),
                draft_cell,
                Cell::from(format!("{}..{}", fl.lo, fl.hi)).style(Style::default().fg(DIM)),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(17),
            Constraint::Length(7),
            Constraint::Length(7),
            Constraint::Length(7),
            Constraint::Length(11),
        ],
    )
    .header(
        Row::new(vec!["  field", "stock", "live", "draft", "range"])
            .style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
    )
    .block(rounded("timings"));
    f.render_widget(table, area);
}

/// A lit segment that sweeps back and forth across `width` (the benching pulse).
fn pulse_line(width: usize, ms: u128) -> Line<'static> {
    let win = 6usize.min(width);
    let travel = width.saturating_sub(win);
    let span = (2 * travel).max(1);
    let p = ((ms / 50) % span as u128) as usize;
    let pos = if p <= travel { p } else { span - p };
    Line::from(vec![
        Span::styled("\u{2591}".repeat(pos), Style::default().fg(DIM)),
        Span::styled(
            "\u{2588}".repeat(win),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "\u{2591}".repeat(width.saturating_sub(pos + win)),
            Style::default().fg(DIM),
        ),
    ])
}

// ---- chrome ----------------------------------------------------------------

/// Build the footer hint line: each `[key]` chip in purple, its label dimmed,
/// groups separated by a single gap so the whole strip stays compact.
fn key_line(items: &[(&str, &str)]) -> Line<'static> {
    let mut spans: Vec<Span> = Vec::new();
    for (i, (key, label)) in items.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(
            (*key).to_string(),
            Style::default().fg(KEY).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(format!(" {label}"), Style::default().fg(DIM)));
    }
    Line::from(spans)
}

fn draw_footer(f: &mut Frame, area: Rect, app: &MemScreen) {
    let keys: &[(&str, &str)] = if app.focus_history {
        &[
            ("[up/down]", "select"),
            ("[x]", "delete"),
            ("[c]", "clear all"),
            ("[Tab]", "back to timings"),
            ("[q]", "quit"),
        ]
    } else {
        &[
            ("[arrows]/[e]", "edit"),
            ("[p]", "saved"),
            ("[0]", "stock"),
            ("[d]", "explain"),
            ("[w]", "write+reboot"),
            ("[b]", "bench"),
            ("[m]", "memtest"),
            ("[u]", "umc"),
            ("[Tab]", "history"),
            ("[q]", "quit"),
        ]
    };
    let p = Paragraph::new(vec![
        Line::from(Span::styled(
            &app.status,
            Style::default().fg(GOOD).add_modifier(Modifier::BOLD),
        )),
        key_line(keys),
    ]);
    f.render_widget(p, area);
}

fn draw_confirm(f: &mut Frame, area: Rect, app: &MemScreen, kind: Confirm) {
    let area = centered(60, 9, area);
    f.render_widget(Clear, area);
    let (title, mut body) = match kind {
        Confirm::Write => {
            let n = app.diff().len();
            let clk = app.draft.get("ClockSpeed").unwrap_or(0);
            (
                " confirm write + reboot ",
                vec![
                    Line::from(format!(
                        "Write {n} changed field(s) @ {clk} MHz and REBOOT?"
                    )),
                    Line::from(""),
                    Line::from(Span::styled(
                        "Applies on reboot; a bad config can no-POST",
                        Style::default().fg(WARN),
                    )),
                    Line::from(Span::styled(
                        "(recover: clear CMOS / `recommended --write`).",
                        Style::default().fg(WARN),
                    )),
                ],
            )
        }
        Confirm::ClearHistory => {
            let n = tune::history().len();
            (
                " confirm clear history ",
                vec![
                    Line::from(format!(
                        "Delete all {n} history entr{}?",
                        if n == 1 { "y" } else { "ies" }
                    )),
                    Line::from(""),
                    Line::from(Span::styled(
                        "This only clears the log of past edits.",
                        Style::default().fg(WARN),
                    )),
                    Line::from(Span::styled(
                        "It does not change any timings.",
                        Style::default().fg(WARN),
                    )),
                ],
            )
        }
    };
    body.push(Line::from(""));
    body.push(Line::from(vec![
        Span::styled(
            "  [y] yes ",
            Style::default()
                .fg(INK)
                .bg(GOOD)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("   "),
        Span::styled(" [n] no ", Style::default().fg(INK).bg(DIM)),
    ]));
    let p = Paragraph::new(body).alignment(Alignment::Center).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title(Span::styled(
                title,
                Style::default().fg(BAD).add_modifier(Modifier::BOLD),
            ))
            .border_style(Style::default().fg(BAD)),
    );
    f.render_widget(p, area);
}

fn draw_edit(f: &mut Frame, area: Rect, app: &MemScreen, buf: &str) {
    let fl = app.fields[app.sel];
    let cur = app.draft.get_field(fl);
    let area = centered(54, 8, area);
    f.render_widget(Clear, area);
    let body = vec![
        Line::from(vec![
            Span::styled("field   ", Style::default().fg(DIM)),
            Span::styled(
                fl.name,
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("   (current {cur}, range {}..{})", fl.lo, fl.hi),
                Style::default().fg(DIM),
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("new value  ", Style::default().fg(DIM)),
            Span::styled(
                format!("{buf}_"),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "[Enter] set   [Esc] cancel   (out-of-range is clamped)",
            Style::default().fg(DIM),
        )),
    ];
    let p = Paragraph::new(body).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title(Span::styled(
                " edit value ",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ))
            .border_style(Style::default().fg(ACCENT)),
    );
    f.render_widget(p, area);
}

/// Live UMC register overlay (refreshes each tick — counters move).
fn draw_umc(f: &mut Frame, area: Rect, offsets: &[u32]) {
    let area = centered(66, 22, area);
    f.render_widget(Clear, area);
    let mut body: Vec<Line> = Vec::new();
    match umc::read_values(offsets) {
        Ok(regs) if !regs.is_empty() => {
            for r in &regs {
                let (tag, col) = if r.counter {
                    ("live counter", ACCENT)
                } else {
                    ("config", DIM)
                };
                // Pad the tag to a fixed width so every row is the same length —
                // under center alignment that keeps the offset/value columns
                // vertically aligned (longer "live counter" rows would otherwise
                // shift left relative to "config" rows).
                body.push(Line::from(vec![
                    Span::styled(format!("+0x{:03X}  ", r.off), Style::default().fg(DIM)),
                    Span::styled(
                        format!("0x{:08X}", r.val),
                        Style::default().fg(Color::White),
                    ),
                    Span::styled(format!("   {tag:<12}"), Style::default().fg(col)),
                ]));
            }
        }
        Ok(_) => body.push(Line::from(Span::styled(
            "(no live registers)",
            Style::default().fg(DIM),
        ))),
        Err(e) => body.push(Line::from(Span::styled(
            format!("{e}"),
            Style::default().fg(BAD),
        ))),
    }
    body.push(Line::from(""));
    match umc::ecc_enabled() {
        Some(true) => body.push(Line::from(Span::styled(
            "ECC: enabled",
            Style::default().fg(GOOD),
        ))),
        Some(false) => body.push(Line::from(Span::styled(
            "ECC: disabled — no hw error counter (use the integrity check)",
            Style::default().fg(DIM),
        ))),
        None => {}
    }
    body.push(Line::from(Span::styled(
        "any key to close",
        Style::default().fg(DIM),
    )));
    let p = Paragraph::new(body)
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(Span::styled(
                    " UMC live registers (SMN 0x14000) ",
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                ))
                .border_style(Style::default().fg(ACCENT)),
        );
    f.render_widget(p, area);
}

fn draw_memtest(f: &mut Frame, area: Rect, app: &MemScreen) {
    let area = centered(62, 13, area);
    f.render_widget(Clear, area);
    let mut body: Vec<Line> = Vec::new();
    match &app.memtest {
        Memtest::Running(_) => {
            let secs = app.memtest_started.elapsed().as_secs().min(MEMTEST_SECS);
            body.push(Line::from(Span::styled(
                format!("hammering system RAM — {secs}s / {MEMTEST_SECS}s"),
                Style::default().fg(WARN).add_modifier(Modifier::BOLD),
            )));
            body.push(Line::from(""));
            body.push(pulse_line(40, app.started.elapsed().as_millis()));
            body.push(Line::from(""));
            body.push(Line::from(Span::styled(
                format!("moving-inversions over {MEMTEST_MB} MiB (CPU-side)"),
                Style::default().fg(DIM),
            )));
        }
        Memtest::Done(r) => {
            let (msg, col) = if r.errors == 0 {
                ("PASS — 0 errors".to_string(), GOOD)
            } else {
                (format!("FAIL — {} errors", r.errors), BAD)
            };
            let gibps = (r.bytes as f64 * r.passes as f64 * 3.0) / r.secs / 1e9;
            body.push(Line::from(Span::styled(
                msg,
                Style::default().fg(col).add_modifier(Modifier::BOLD),
            )));
            body.push(Line::from(""));
            body.push(Line::from(Span::styled(
                format!(
                    "{} passes  .  {} MiB  .  {:.0}s  .  ~{gibps:.0} GB/s",
                    r.passes,
                    r.bytes / (1024 * 1024),
                    r.secs
                ),
                Style::default().fg(DIM),
            )));
            body.push(Line::from(""));
            if r.errors == 0 {
                body.push(Line::from(Span::styled(
                    "this config does not corrupt system RAM",
                    Style::default().fg(DIM),
                )));
            } else {
                body.push(Line::from(Span::styled(
                    "this config CORRUPTS memory — do not keep it",
                    Style::default().fg(BAD),
                )));
            }
        }
        Memtest::Failed => body.push(Line::from(Span::styled(
            "memtest failed (allocation?)",
            Style::default().fg(BAD),
        ))),
        Memtest::Idle => body.push(Line::from(Span::styled("idle", Style::default().fg(DIM)))),
    }
    body.push(Line::from(""));
    body.push(Line::from(Span::styled(
        "any key to close (the test keeps running)",
        Style::default().fg(DIM),
    )));
    let p = Paragraph::new(body)
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(Span::styled(
                    " system memtest (CPU-side) ",
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                ))
                .border_style(Style::default().fg(ACCENT)),
        );
    f.render_widget(p, area);
}

/// The saved-timings picker overlay: pick a known-good config and reboot into it.
fn draw_profiles(f: &mut Frame, area: Rect, app: &MemScreen) {
    let h = (app.profiles.len() as u16 + 5).clamp(9, 22);
    let area = centered(72, h, area);
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(Span::styled(
            " saved timings — select + reboot ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::default().fg(ACCENT));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    let rows: Vec<Row> = app
        .profiles
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let selected = i == app.prof_sel;
            let mark = if selected { ">" } else { " " };
            let clk = p.conf.get("ClockSpeed").unwrap_or(0);
            // How many editable timings differ from what's live right now.
            let delta = app
                .fields
                .iter()
                .filter(|fl| app.current.get_field(fl) != p.conf.get_field(fl))
                .count();
            let (vs, vs_col) = if delta == 0 {
                ("live".to_string(), GOOD)
            } else {
                (format!("{delta} diff"), DIM)
            };
            let name_style = if selected {
                Style::default()
                    .fg(INK)
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            Row::new(vec![
                Cell::from(format!("{mark} {}", p.label)).style(name_style),
                Cell::from(p.source).style(Style::default().fg(DIM)),
                Cell::from(format!("{clk} MHz")).style(Style::default().fg(ACCENT)),
                Cell::from(vs).style(Style::default().fg(vs_col)),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Min(20),
            Constraint::Length(9),
            Constraint::Length(9),
            Constraint::Length(8),
        ],
    )
    .header(
        Row::new(vec!["  profile", "from", "clock", "vs live"]).style(Style::default().fg(ACCENT)),
    );
    f.render_widget(table, split[0]);

    let hint = Paragraph::new(key_line(&[
        ("[up/down]", "select"),
        ("[Enter]", "load + reboot"),
        ("[Esc]", "cancel"),
    ]));
    f.render_widget(hint, split[1]);
}

fn centered(w: u16, h: u16, area: Rect) -> Rect {
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect {
        x,
        y,
        width: w.min(area.width),
        height: h.min(area.height),
    }
}

fn sig_style(sig: u32) -> Style {
    match sig {
        SIG_ABL => Style::default().fg(GOOD),
        SIG_LINUX_TOOL => Style::default().fg(WARN),
        _ => Style::default().fg(BAD),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn fake_app() -> MemScreen {
        let mut c = MemConf::from_bytes([0u8; CONFIG_SIZE]);
        c.apply_recommended();
        c.stamp(SIG_ABL);
        let mut draft = c.clone();
        draft.set("tCL", 22); // a change so the diff path renders
        MemScreen {
            fields: editable(),
            stock: Some(c.clone()),
            current: c,
            draft,
            sel: 2,
            mode: Mode::Normal,
            status: "test".into(),
            bench: Bench::Done(bench::BenchResult {
                bandwidth_gbps: 412.0,
                random_gbps: 78.5,
                latency_ns: 180.0,
                stability_errors: 0,
                stability_bytes: 256 * 1024 * 1024,
            }),
            memtest: Memtest::Idle,
            memtest_started: Instant::now(),
            started: Instant::now(),
            show_doc: false,
            focus_history: false,
            hist_sel: 0,
            telem: Some(metrics::Telemetry {
                gfxclk_mhz: 1500,
                uclk_mhz: 450,
                temp_c: 64.0,
            }),
            umc_offsets: Vec::new(),
            profiles: Vec::new(),
            prof_sel: 0,
            eval_pending: false,
            eval_recording: false,
        }
    }

    /// Renders the page (history + doc panes) + the confirm overlays.
    #[test]
    fn renders_without_panic() {
        let mut term = Terminal::new(TestBackend::new(110, 34)).unwrap();
        let mut app = fake_app();

        term.draw(|f| {
            let ar = f.area();
            draw(f, ar, &app)
        })
        .unwrap();

        app.show_doc = true;
        term.draw(|f| {
            let ar = f.area();
            draw(f, ar, &app)
        })
        .unwrap();

        app.show_doc = false;
        app.focus_history = true;
        term.draw(|f| {
            let ar = f.area();
            draw(f, ar, &app)
        })
        .unwrap();

        app.mode = Mode::Confirm(Confirm::Write);
        term.draw(|f| {
            let ar = f.area();
            draw(f, ar, &app)
        })
        .unwrap();

        app.mode = Mode::Confirm(Confirm::ClearHistory);
        term.draw(|f| {
            let ar = f.area();
            draw(f, ar, &app)
        })
        .unwrap();

        app.mode = Mode::EditField("10500".into());
        term.draw(|f| {
            let ar = f.area();
            draw(f, ar, &app)
        })
        .unwrap();

        app.mode = Mode::Umc; // umc read errors off-board -> renders the error line
        term.draw(|f| {
            let ar = f.area();
            draw(f, ar, &app)
        })
        .unwrap();

        // Saved-timings picker, with a couple of profiles to exercise the rows.
        let mut prof = MemConf::from_bytes([0u8; CONFIG_SIZE]);
        prof.apply_recommended();
        app.profiles = vec![
            profiles::Profile {
                label: "known_good_1928".into(),
                source: "saved",
                conf: prof.clone(),
            },
            profiles::Profile {
                label: "recommended (1750)".into(),
                source: "built-in",
                conf: prof,
            },
        ];
        app.prof_sel = 1;
        app.mode = Mode::Profiles;
        term.draw(|f| {
            let ar = f.area();
            draw(f, ar, &app)
        })
        .unwrap();
    }

    /// A cramped terminal (80x24 serial console, or smaller) must clip, not panic.
    #[test]
    fn renders_small_terminal_without_panic() {
        for (w, h) in [(80u16, 24u16), (40, 12), (20, 8)] {
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            let app = fake_app();
            term.draw(|f| {
                let ar = f.area();
                draw(f, ar, &app)
            })
            .unwrap();
        }
    }
}
