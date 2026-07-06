// SPDX-License-Identifier: GPL-2.0-only
//! Interactive browser + editor for the BIOS settings catalogue — the BIOS tab.
//!
//! Layout: a left nav tree (compartments -> categories) and, on the right, a
//! full-width settings list above a detail panel. `/` searches across every
//! setting.
//!
//! Editing: settings stored in `AmdSetup` are editable (left/right to change, or
//! `e` to type a value); changes build a draft and are written together with
//! `w` (which backs up the variable, writes it, and reboots to apply). OEM
//! `Setup` settings (1-byte) are also editable when the `smiflash` SMM driver is
//! loaded — `w` writes them through the SMM NVAR-append path (no flash rig,
//! bypassing the boot-service variable lock); IOMMU is kept to the CLI. Note:
//! many CBS settings are applied by AGESA from the APCB copy, so an AmdSetup
//! write can be cosmetic — the detail panel says so per setting.
//!
//! Ported from biostune's `ui.rs`: the terminal/event-loop plumbing moved to the
//! suite shell; the state + draw + key handling became this [`Screen`]. The
//! write path is preserved verbatim (draft staging, dirty-store protection,
//! risk-gated inline edit, confirm-then-reboot).

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result};
use arieltune_tui_kit::{Outcome, Screen};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::{Block, BorderType, Borders, Cell, Clear, Paragraph, Row, Table, Wrap};

use crate::catalog::{self, Setting};
use crate::efivar::{self, EfiVars};
use crate::{dirty, nvram, oem, smm};

const ACCENT: Color = Color::Cyan;
const GOOD: Color = Color::Green;
const WARN: Color = Color::Yellow;
const BAD: Color = Color::Red;
const DIM: Color = Color::DarkGray;
const INK: Color = Color::Black;
const KEY: Color = Color::Magenta;
const HEAD: Color = Color::Magenta;

#[derive(PartialEq)]
enum Focus {
    Nav,
    Settings,
}

enum Mode {
    Normal,
    Search(String),
    /// Typing a numeric value into the selected setting's draft.
    EditNum(String),
    /// Confirm writing the staged drafts + reboot.
    Confirm,
}

/// A row in the left nav tree.
enum NavRow {
    Compartment(String),
    /// (display name, setting count, index into `cats`)
    Category(String, usize, usize),
}

/// The BIOS tab: the categorized settings browser + editor as an
/// [`arieltune_tui_kit::Screen`].
pub struct BiosScreen {
    settings: Vec<Setting>,
    cats: Vec<(String, Vec<usize>)>,
    nav: Vec<NavRow>,
    nav_cat_rows: Vec<usize>,
    efi: EfiVars,
    /// Pending edits: setting index -> new raw value (AmdSetup only).
    draft: HashMap<usize, u32>,
    focus: Focus,
    nav_sel: usize,
    set_sel: usize,
    matches: Vec<usize>,
    searching: bool,
    mode: Mode,
    status: String,
    /// smiflash SMM driver present → OEM `Setup` settings are editable.
    smm_ok: bool,
}

/// Whether an OEM `Setup` setting is editable in the TUI: 1-byte only (the SMM
/// NVAR-append path edits a single byte), the smiflash driver must be loaded, and
/// only Safe-classified settings edit inline — anything caution/brick/one-way is
/// kept to the CLI, where `--force` and the printed recovery force acknowledgement.
fn oem_editable(s: &Setting, smm_ok: bool) -> bool {
    s.varstore == "Setup"
        && s.width() == 1
        && smm_ok
        && crate::risk::assess(s).risk == crate::risk::Risk::Safe
}

impl BiosScreen {
    pub fn load(efi: EfiVars) -> Self {
        let settings = catalog::load();
        let cats = catalog::by_category(&settings);
        let comps = catalog::by_compartment(&settings, &cats);
        let mut nav = Vec::new();
        let mut nav_cat_rows = Vec::new();
        for (comp, cat_idxs) in &comps {
            nav.push(NavRow::Compartment(comp.to_string()));
            for &ci in cat_idxs {
                let (name, idx) = &cats[ci];
                nav_cat_rows.push(nav.len());
                nav.push(NavRow::Category(name.clone(), idx.len(), ci));
            }
        }
        let smm_ok = smm::Smm::available();
        let status = if efi.has("AmdSetup") {
            format!(
                "{} settings — left/right to change, w to write{}",
                settings.len(),
                if smm_ok {
                    " (OEM editing: ON)"
                } else {
                    " (OEM editing off — load smiflash.ko)"
                }
            )
        } else {
            "AmdSetup not readable — run as root on a BC-250 to edit".into()
        };
        BiosScreen {
            settings,
            cats,
            nav,
            nav_cat_rows,
            efi,
            draft: HashMap::new(),
            focus: Focus::Nav,
            nav_sel: 0,
            set_sel: 0,
            matches: Vec::new(),
            searching: false,
            mode: Mode::Normal,
            status,
            smm_ok,
        }
    }

    fn sel_cat(&self) -> Option<usize> {
        let row = *self.nav_cat_rows.get(self.nav_sel)?;
        match &self.nav[row] {
            NavRow::Category(_, _, ci) => Some(*ci),
            _ => None,
        }
    }

    fn current_list(&self) -> Vec<usize> {
        if self.searching {
            self.matches.clone()
        } else {
            self.sel_cat()
                .map(|ci| self.cats[ci].1.clone())
                .unwrap_or_default()
        }
    }

    /// Index (into `settings`) of the highlighted setting.
    fn sel_idx(&self) -> Option<usize> {
        self.current_list().get(self.set_sel).copied()
    }

    fn editable(&self, si: usize) -> bool {
        let s = &self.settings[si];
        if s.varstore == "Setup" {
            oem_editable(s, self.smm_ok)
        } else {
            efivar::is_writable(&s.varstore)
                && self.efi.has("AmdSetup")
                && crate::risk::assess(s).risk == crate::risk::Risk::Safe
        }
    }

    /// Why a setting can't be edited here (for the status line).
    fn why_locked(&self, si: usize) -> &'static str {
        let s = &self.settings[si];
        // A dangerous setting is CLI-only regardless of varstore.
        let r = crate::risk::assess(s).risk;
        if r != crate::risk::Risk::Safe {
            return match r {
                crate::risk::Risk::Brick => {
                    "BRICK-RISK — CLI only: `set/oem-set NAME=VAL --force` (can prevent POST)"
                }
                crate::risk::Risk::OneWay => {
                    "ONE-WAY — CLI only: `set NAME=VAL --force` (reverts only via power-cycle)"
                }
                _ => "CAUTION — edit via the CLI `set/oem-set NAME=VAL --force`",
            };
        }
        if s.varstore == "Setup" {
            if s.width() != 1 {
                "only 1-byte OEM settings are editable here"
            } else if !self.smm_ok {
                "OEM editing needs smiflash.ko (insmod smiflash.ko smi_port=0xB0)"
            } else {
                "not editable"
            }
        } else if !self.efi.has("AmdSetup") {
            "AmdSetup not readable — run as root on a BC-250"
        } else {
            "not editable"
        }
    }

    /// The value to show: drafted value if edited, else the live value.
    fn shown_value(&self, si: usize) -> Option<u32> {
        self.draft
            .get(&si)
            .copied()
            .or_else(|| self.efi.value(&self.settings[si]))
    }

    fn is_drafted(&self, si: usize) -> bool {
        self.draft.contains_key(&si)
    }

    /// Stage a value; drop it from the draft if it matches the live value.
    fn stage(&mut self, si: usize, v: u32) {
        let name = self.settings[si].name.clone();
        let live = self.efi.value(&self.settings[si]);
        if Some(v) == live {
            self.draft.remove(&si);
        } else {
            self.draft.insert(si, v);
        }
        self.status = format!("staged {name} = {}", self.settings[si].label_for(v));
    }

    /// Change the selected setting by `dir` (enum cycle / numeric step).
    fn nudge(&mut self, dir: i64) {
        let Some(si) = self.sel_idx() else { return };
        if !self.editable(si) {
            self.status = self.why_locked(si).into();
            return;
        }
        let cur = self.shown_value(si).unwrap_or(0);
        let s = &self.settings[si];
        let next = if let Some([lo, hi]) = s.range {
            (cur as i64 + dir).clamp(lo as i64, hi as i64) as u32
        } else if s.options.is_empty() {
            return;
        } else {
            let i = s.options.iter().position(|(v, _)| *v == cur).unwrap_or(0) as i64;
            let n = s.options.len() as i64;
            s.options[((i + dir).rem_euclid(n)) as usize].0
        };
        self.stage(si, next);
    }

    fn dirty(&self) -> bool {
        !self.draft.is_empty()
    }

    fn write(&mut self) -> Result<()> {
        // Drafts can mix AmdSetup (efivar write) and OEM Setup (SMM NVAR-append).
        let mut amd: Vec<(usize, u32)> = Vec::new();
        let mut oem: Vec<(usize, u32)> = Vec::new();
        for (&si, &v) in &self.draft {
            if self.settings[si].varstore == "Setup" {
                oem.push((si, v));
            } else {
                amd.push((si, v));
            }
        }

        // AmdSetup writes go through the firmware's SetVariable; that's unsafe while
        // an SMM append from a prior (un-rebooted) session is pending. (OEM appends
        // themselves are safe to stack — they don't collide with each other.)
        if !amd.is_empty() && dirty::is_dirty() {
            anyhow::bail!("{}", dirty::why());
        }
        let mut parts: Vec<String> = Vec::new();
        if !amd.is_empty() {
            let edits: Vec<efivar::Edit> = amd
                .iter()
                .map(|&(si, v)| {
                    let s = &self.settings[si];
                    efivar::Edit {
                        offset: s.offset,
                        width: s.width(),
                        value: v,
                    }
                })
                .collect();
            let backup = efivar::write_amdsetup(&edits)?;
            parts.push(format!(
                "AmdSetup {} (backup {})",
                edits.len(),
                backup.display()
            ));
        }
        if !oem.is_empty() {
            // All OEM field changes go into ONE NVAR append (a full Setup-body copy
            // with every changed byte), the way a BIOS commits a save-&-exit.
            let smm = smm::Smm::open()?;
            let edits: Vec<(usize, u8)> = oem
                .iter()
                .map(|&(si, v)| (self.settings[si].offset, v as u8))
                .collect();
            oem::oem_set(&smm, "Setup", nvram::SETUP_SIZE, &edits).context("OEM batch set")?;
            parts.push(format!("OEM {} via SMM (one entry)", oem.len()));
            dirty::mark(); // store is SMM-dirty until the reboot we're about to do
        }
        self.status = format!("wrote — {}", parts.join("; "));
        Ok(())
    }

    fn apply_search(&mut self, q: &str) {
        let ql = q.to_lowercase();
        self.matches = self
            .settings
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                s.name.to_lowercase().contains(&ql) || s.category.to_lowercase().contains(&ql)
            })
            .map(|(i, _)| i)
            .collect();
        self.searching = !q.is_empty();
        self.set_sel = 0;
        self.focus = Focus::Settings;
        self.status = format!("search '{q}' — {} match(es)", self.matches.len());
    }
}

impl Screen for BiosScreen {
    fn title(&self) -> &'static str {
        "BIOS"
    }

    fn draw(&mut self, f: &mut Frame, area: Rect) {
        draw(f, area, self);
    }

    fn on_key(&mut self, k: KeyEvent) -> Outcome {
        match &mut self.mode {
            Mode::Search(buf) => {
                match k.code {
                    KeyCode::Char(c) => {
                        buf.push(c);
                        let q = buf.clone();
                        self.apply_search(&q);
                    }
                    KeyCode::Backspace => {
                        buf.pop();
                        let q = buf.clone();
                        self.apply_search(&q);
                    }
                    KeyCode::Enter => self.mode = Mode::Normal,
                    KeyCode::Esc => {
                        self.mode = Mode::Normal;
                        self.searching = false;
                        self.set_sel = 0;
                        self.status = "search cleared".into();
                    }
                    _ => {}
                }
                return Outcome::Consumed;
            }
            Mode::EditNum(buf) => {
                match k.code {
                    KeyCode::Char(c) if c.is_ascii_digit() => buf.push(c),
                    KeyCode::Backspace => {
                        buf.pop();
                    }
                    KeyCode::Enter => {
                        let parsed = buf.parse::<u32>();
                        self.mode = Mode::Normal;
                        if let (Ok(v), Some(si)) = (parsed, self.sel_idx()) {
                            let v = match &self.settings[si].range {
                                Some([lo, hi]) => v.clamp(*lo, *hi),
                                None => v,
                            };
                            self.stage(si, v);
                        } else {
                            self.status = "edit cancelled".into();
                        }
                    }
                    KeyCode::Esc => {
                        self.mode = Mode::Normal;
                        self.status = "edit cancelled".into();
                    }
                    _ => {}
                }
                return Outcome::Consumed;
            }
            Mode::Confirm => {
                match k.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => {
                        self.mode = Mode::Normal;
                        match self.write() {
                            Ok(()) => {
                                return Outcome::Reboot(
                                    "wrote changes — rebooting to apply...".into(),
                                )
                            }
                            Err(e) => self.status = format!("write failed: {e}"),
                        }
                    }
                    _ => {
                        self.mode = Mode::Normal;
                        self.status = "cancelled".into();
                    }
                }
                return Outcome::Consumed;
            }
            Mode::Normal => {}
        }

        match k.code {
            KeyCode::Char('q') => return Outcome::Quit,
            KeyCode::Esc => {
                if self.searching {
                    self.searching = false;
                    self.set_sel = 0;
                    self.focus = Focus::Nav;
                    self.status = "search cleared".into();
                } else {
                    return Outcome::Quit;
                }
            }
            KeyCode::Char('/') => {
                self.mode = Mode::Search(String::new());
                self.status = "search: type to filter, Enter to keep, Esc to clear".into();
            }
            KeyCode::Tab | KeyCode::BackTab => {
                self.focus = if self.focus == Focus::Nav {
                    Focus::Settings
                } else {
                    Focus::Nav
                };
            }
            KeyCode::Up => move_sel(self, -1),
            KeyCode::Down => move_sel(self, 1),
            KeyCode::Right if self.focus == Focus::Settings => self.nudge(1),
            KeyCode::Left if self.focus == Focus::Settings => self.nudge(-1),
            KeyCode::Enter | KeyCode::Right if self.focus == Focus::Nav => {
                self.focus = Focus::Settings
            }
            KeyCode::Left if self.focus == Focus::Nav => {}
            KeyCode::Char('e') if self.focus == Focus::Settings => {
                if let Some(si) = self.sel_idx() {
                    if self.editable(si) && self.settings[si].range.is_some() {
                        self.mode = Mode::EditNum(String::new());
                        self.status = format!(
                            "editing {} — type a value, Enter to set",
                            self.settings[si].name
                        );
                    } else if !self.editable(si) {
                        self.status = self.why_locked(si).into();
                    } else {
                        self.status = "use left/right to change this setting".into();
                    }
                }
            }
            KeyCode::Char('0') => {
                if let Some(si) = self.sel_idx() {
                    if self.draft.remove(&si).is_some() {
                        self.status = "reverted this setting".into();
                    }
                }
            }
            KeyCode::Char('r') => {
                self.draft.clear();
                self.status = "all staged changes cleared".into();
            }
            KeyCode::Char('w') => {
                if !self.dirty() {
                    self.status = "no changes to write".into();
                } else if !self.efi.has("AmdSetup") {
                    self.status = "AmdSetup not writable here".into();
                } else {
                    self.mode = Mode::Confirm;
                }
            }
            // Anything else falls through so the shell can switch tabs / quit.
            _ => return Outcome::Ignored,
        }
        Outcome::Consumed
    }

    /// Search / field-edit / confirm are modal sub-states: while in one the shell
    /// must not steal global switch/quit keys.
    fn modal(&self) -> bool {
        matches!(
            self.mode,
            Mode::Search(_) | Mode::EditNum(_) | Mode::Confirm
        )
    }

    /// No live telemetry — an idle 1s poll is plenty.
    fn tick_hint(&self) -> Duration {
        Duration::from_millis(1000)
    }
}

fn move_sel(app: &mut BiosScreen, dir: i64) {
    match app.focus {
        Focus::Nav => {
            if app.nav_cat_rows.is_empty() {
                return;
            }
            let n = app.nav_cat_rows.len() as i64;
            app.nav_sel = ((app.nav_sel as i64 + dir).rem_euclid(n)) as usize;
            app.set_sel = 0;
            app.searching = false;
        }
        Focus::Settings => {
            let len = app.current_list().len();
            if len == 0 {
                return;
            }
            app.set_sel = ((app.set_sel as i64 + dir).rem_euclid(len as i64)) as usize;
        }
    }
}

// ---- drawing ---------------------------------------------------------------

fn block(title: &str, focused: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(if focused { ACCENT } else { DIM }))
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))
}

fn draw(f: &mut Frame, area: Rect, app: &BiosScreen) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(2),
        ])
        .split(area);

    let title = match &app.mode {
        Mode::Search(buf) => Line::from(vec![
            Span::styled(
                " arieltune bios ",
                Style::default().fg(KEY).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  search: ", Style::default().fg(DIM)),
            Span::styled(
                format!("{buf}_"),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        _ => {
            let dirty = if app.dirty() {
                Span::styled(
                    format!("  {} unsaved", app.draft.len()),
                    Style::default().fg(WARN).add_modifier(Modifier::BOLD),
                )
            } else {
                Span::raw("")
            };
            Line::from(vec![
                Span::styled(
                    " arieltune bios ",
                    Style::default().fg(KEY).add_modifier(Modifier::BOLD),
                ),
                Span::styled("  BIOS settings", Style::default().fg(DIM)),
                dirty,
            ])
        }
    };
    f.render_widget(Paragraph::new(title), rows[0]);

    let main = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(42), Constraint::Min(36)])
        .split(rows[1]);
    draw_nav(f, main[0], app);

    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(6), Constraint::Length(15)])
        .split(main[1]);
    draw_settings(f, right[0], app);
    draw_detail(f, right[1], app);

    draw_footer(f, rows[2], app);

    if let Mode::Confirm = app.mode {
        draw_confirm(f, area, app);
    }
}

fn draw_nav(f: &mut Frame, area: Rect, app: &BiosScreen) {
    let focused = app.focus == Focus::Nav && !app.searching;
    let sel_row = app.nav_cat_rows.get(app.nav_sel).copied().unwrap_or(0);
    let h = area.height.saturating_sub(2) as usize;
    let start = sel_row
        .saturating_sub(h / 2)
        .min(app.nav.len().saturating_sub(h.max(1)));
    let mut lines: Vec<Line> = Vec::new();
    for (i, row) in app.nav.iter().enumerate().skip(start).take(h.max(1)) {
        match row {
            NavRow::Compartment(name) => lines.push(Line::from(Span::styled(
                name.clone(),
                Style::default().fg(HEAD).add_modifier(Modifier::BOLD),
            ))),
            NavRow::Category(name, count, _) => {
                let sel = i == sel_row && !app.searching;
                let style = if sel {
                    Style::default()
                        .fg(INK)
                        .bg(ACCENT)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("  {name} "), style),
                    Span::styled(format!("({count})"), Style::default().fg(DIM)),
                ]));
            }
        }
    }
    f.render_widget(
        Paragraph::new(lines).block(block("compartments", focused)),
        area,
    );
}

fn draw_settings(f: &mut Frame, area: Rect, app: &BiosScreen) {
    let focused = app.focus == Focus::Settings || app.searching;
    let list = app.current_list();
    let title = if app.searching {
        format!("results ({})", list.len())
    } else {
        app.sel_cat()
            .map(|ci| app.cats[ci].0.clone())
            .unwrap_or_default()
    };
    if list.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled("(no settings)", Style::default().fg(DIM)))
                .block(block(&title, focused)),
            area,
        );
        return;
    }
    let h = area.height.saturating_sub(3) as usize;
    let start = app.set_sel.saturating_sub(h.saturating_sub(1));
    let rows: Vec<Row> = list
        .iter()
        .enumerate()
        .skip(start)
        .take(h.max(1))
        .map(|(i, &si)| {
            let s = &app.settings[si];
            let sel = i == app.set_sel;
            let drafted = app.is_drafted(si);
            let val = app
                .shown_value(si)
                .map(|v| s.label_for(v))
                .unwrap_or_else(|| "—".into());
            let vc = if drafted {
                WARN
            } else if app.shown_value(si).is_some() {
                GOOD
            } else {
                DIM
            };
            let lock = match crate::risk::assess(s).risk {
                crate::risk::Risk::Brick => " [BRICK]",
                crate::risk::Risk::OneWay => " [1-WAY]",
                crate::risk::Risk::Caution => " [caution]",
                crate::risk::Risk::Safe if !app.editable(si) => " (ro)",
                crate::risk::Risk::Safe => "",
            };
            let name_style = if sel {
                Style::default()
                    .fg(INK)
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            let name = if app.searching {
                format!(
                    "{} {}{}  ·  {}",
                    if sel { ">" } else { " " },
                    s.name,
                    lock,
                    s.category
                )
            } else {
                format!("{} {}{}", if sel { ">" } else { " " }, s.name, lock)
            };
            let mark = if drafted { "*" } else { " " };
            Row::new(vec![
                Cell::from(name).style(name_style),
                Cell::from(format!("{mark}{val}")).style(Style::default().fg(vc).add_modifier(
                    if drafted {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    },
                )),
            ])
        })
        .collect();
    let t = Table::new(rows, [Constraint::Min(40), Constraint::Length(24)])
        .header(
            Row::new(vec!["  setting", "value"])
                .style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
        )
        .block(block(&title, focused));
    f.render_widget(t, area);
}

fn draw_detail(f: &mut Frame, area: Rect, app: &BiosScreen) {
    let Some(si) = app.sel_idx() else {
        f.render_widget(
            Paragraph::new(Span::styled("(select a setting)", Style::default().fg(DIM)))
                .block(block("detail", false)),
            area,
        );
        return;
    };
    let s = &app.settings[si];
    let live = app.efi.value(s);
    let (desc, _) = crate::descriptions::describe(s);
    let editable = app.editable(si);

    let live_str = live.map(|v| s.label_for(v)).unwrap_or_else(|| "—".into());
    let mut value_line = vec![
        Span::styled("current ", Style::default().fg(DIM)),
        Span::styled(
            live_str,
            Style::default().fg(GOOD).add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(&dv) = app.draft.get(&si) {
        value_line.push(Span::styled("  ->  ", Style::default().fg(DIM)));
        value_line.push(Span::styled(
            s.label_for(dv),
            Style::default().fg(WARN).add_modifier(Modifier::BOLD),
        ));
    }

    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                &s.name,
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("   [{}]", s.category), Style::default().fg(DIM)),
            if editable {
                Span::raw("")
            } else {
                Span::styled("   read-only", Style::default().fg(WARN))
            },
        ]),
        Line::from(Span::styled(desc, Style::default().fg(Color::White))),
        Line::from(""),
        Line::from(value_line),
        Line::from(vec![
            Span::styled("values  ", Style::default().fg(DIM)),
            Span::styled(s.value_space(), Style::default().fg(Color::White)),
        ]),
    ];
    let note: String = if editable {
        if s.varstore == "Setup" {
            "editable via SMM (no flash rig) — left/right to change; written to the NVAR \
             store, applies on reboot."
                .into()
        } else {
            "editable — left/right to change; takes effect on reboot (may be cosmetic if \
             AGESA uses the APCB copy)."
                .into()
        }
    } else {
        let r = crate::risk::assess(s);
        if r.risk != crate::risk::Risk::Safe {
            let cmd = if s.varstore == "Setup" {
                "oem-set"
            } else {
                "set"
            };
            format!(
                "{}: {} — edit via CLI `{} {}=VAL --force`. Recovery: {}.",
                r.risk.tag(),
                r.reason,
                cmd,
                s.name,
                r.recovery
            )
        } else if s.varstore == "Setup" && !app.smm_ok {
            "OEM setting — load smiflash.ko (smi_port=0xB0) to edit it here, or use the \
             CLI `oem-set`. Launch with --from-flash to see its real value."
                .into()
        } else if s.varstore == "Setup" {
            "OEM setting not editable here (only 1-byte settings are). Use the CLI for others."
                .into()
        } else {
            "AmdSetup not readable — run as root on a BC-250 to edit.".into()
        }
    };
    // A prominent risk banner for anything non-Safe.
    let rk = crate::risk::assess(s);
    if rk.risk != crate::risk::Risk::Safe {
        lines.push(Line::from(Span::styled(
            format!("  {}: {}", rk.risk.tag(), rk.reason),
            Style::default().fg(WARN).add_modifier(Modifier::BOLD),
        )));
    }
    lines.push(Line::from(Span::styled(note, Style::default().fg(DIM))));
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .block(block("detail", false)),
        area,
    );
}

fn draw_footer(f: &mut Frame, area: Rect, app: &BiosScreen) {
    let keys: &[(&str, &str)] = &[
        ("[Tab]/←→", "focus"),
        ("[↑↓]", "move"),
        ("[←→]", "change"),
        ("[e]", "type"),
        ("[0]", "revert"),
        ("[w]", "write+reboot"),
        ("[/]", "search"),
        ("[q]", "quit"),
    ];
    let mut spans = Vec::new();
    for (i, (k, l)) in keys.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(
            (*k).to_string(),
            Style::default().fg(KEY).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(format!(" {l}"), Style::default().fg(DIM)));
    }
    f.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                &app.status,
                Style::default().fg(GOOD).add_modifier(Modifier::BOLD),
            )),
            Line::from(spans),
        ]),
        area,
    );
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

fn draw_confirm(f: &mut Frame, area: Rect, app: &BiosScreen) {
    let area = centered(66, 11, area);
    f.render_widget(Clear, area);
    let oem_n = app
        .draft
        .keys()
        .filter(|&&si| app.settings[si].varstore == "Setup")
        .count();
    let amd_n = app.draft.len() - oem_n;
    let hdr = match (amd_n, oem_n) {
        (a, 0) => format!("Write {a} AmdSetup change(s) and REBOOT?"),
        (0, o) => format!("Write {o} OEM Setup change(s) via SMM and REBOOT?"),
        (a, o) => format!("Write {a} AmdSetup + {o} OEM change(s) and REBOOT?"),
    };
    let mut body = vec![Line::from(hdr), Line::from("")];
    for (&si, &v) in app.draft.iter().take(4) {
        let s = &app.settings[si];
        let tag = if s.varstore == "Setup" { "OEM" } else { "CBS" };
        body.push(Line::from(vec![
            Span::styled(format!("  [{tag}] "), Style::default().fg(DIM)),
            Span::styled(format!("{} = ", s.name), Style::default().fg(Color::White)),
            Span::styled(
                s.label_for(v),
                Style::default().fg(WARN).add_modifier(Modifier::BOLD),
            ),
        ]));
    }
    if app.draft.len() > 4 {
        body.push(Line::from(Span::styled(
            format!("  … and {} more", app.draft.len() - 4),
            Style::default().fg(DIM),
        )));
    }
    body.push(Line::from(""));
    if oem_n > 0 {
        body.push(Line::from(Span::styled(
            "OEM changes write the flash via SMM — recovery: NVRAM clear or reflash a backup.",
            Style::default().fg(WARN),
        )));
    } else {
        body.push(Line::from(Span::styled(
            "Backed up first. Applies on reboot (may be cosmetic for APCB-applied settings).",
            Style::default().fg(WARN),
        )));
    }
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
            .border_style(Style::default().fg(BAD))
            .title(Span::styled(
                " confirm write ",
                Style::default().fg(BAD).add_modifier(Modifier::BOLD),
            )),
    );
    f.render_widget(p, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn app() -> BiosScreen {
        BiosScreen::load(EfiVars::read())
    }

    #[test]
    fn renders_without_panic() {
        let mut term = Terminal::new(TestBackend::new(140, 40)).unwrap();
        let mut a = app();
        term.draw(|f| {
            let ar = f.area();
            draw(f, ar, &a)
        })
        .unwrap();
        a.focus = Focus::Settings;
        term.draw(|f| {
            let ar = f.area();
            draw(f, ar, &a)
        })
        .unwrap();
        a.apply_search("cpu");
        term.draw(|f| {
            let ar = f.area();
            draw(f, ar, &a)
        })
        .unwrap();
        a.mode = Mode::Confirm;
        // stage a fake draft so the confirm box has content
        if let Some(si) = a.settings.iter().position(|s| !s.options.is_empty()) {
            a.draft.insert(si, a.settings[si].options[0].0);
        }
        term.draw(|f| {
            let ar = f.area();
            draw(f, ar, &a)
        })
        .unwrap();
    }

    #[test]
    fn nav_has_compartments_and_categories() {
        let a = app();
        assert!(a.nav.iter().any(|r| matches!(r, NavRow::Compartment(_))));
        assert_eq!(a.nav_cat_rows.len(), a.cats.len());
        assert!(a.sel_cat().is_some());
    }

    #[test]
    fn stage_and_revert() {
        let mut a = app();
        // find an enum setting and stage a different value
        let si = a
            .settings
            .iter()
            .position(|s| s.options.len() >= 2)
            .unwrap();
        let other = a.settings[si].options[1].0;
        a.stage(si, other);
        // stage records unless it equals the live value (which is None off-board -> records)
        assert!(a.dirty() || a.efi.value(&a.settings[si]) == Some(other));
        a.draft.clear();
        assert!(!a.dirty());
    }

    #[test]
    fn small_terminal_ok() {
        for (w, h) in [(80u16, 24u16), (40, 12)] {
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            let a = app();
            term.draw(|f| {
                let ar = f.area();
                draw(f, ar, &a)
            })
            .unwrap();
        }
    }

    fn oem(name: &str, bits: u8) -> Setting {
        Setting {
            category: "PCI".into(),
            name: name.into(),
            offset: 0xDA,
            bits,
            default: None,
            options: vec![(0, "Disabled".into()), (1, "Enabled".into())],
            range: None,
            varstore: "Setup".into(),
        }
    }

    #[test]
    fn oem_1byte_editable_when_driver_loaded() {
        assert!(oem_editable(&oem("Above 4G Decoding", 8), true));
        assert!(!oem_editable(&oem("Above 4G Decoding", 8), false)); // no driver
    }

    #[test]
    fn oem_wide_and_iommu_are_not_editable() {
        assert!(!oem_editable(&oem("Some Word Setting", 16), true)); // 2-byte
        assert!(!oem_editable(&oem("IOMMU", 8), true)); // CLI-only
    }

    #[test]
    fn non_setup_varstore_is_not_oem_editable() {
        let mut s = oem("Combo CBS", 8);
        s.varstore = "AmdSetup".into();
        assert!(!oem_editable(&s, true)); // AmdSetup uses the efivar path, not this
    }
}
