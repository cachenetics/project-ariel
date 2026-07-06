// SPDX-License-Identifier: GPL-2.0-only
//! Interactive WIKI browser -- a two-pane view of the authored ASRock BC-250 OEM
//! System Manual. Left pane: the manual tree (chapter -> section). Right pane: the
//! rendered section. Ported from wikitune's `ui.rs`: the terminal/event-loop
//! plumbing moved to the suite shell; the state + draw + key handling became this
//! [`Screen`]. Palette follows the house rule: color HEADERS only, plain body.

use arieltune_tui_kit::{Outcome, Screen};
use bc250_catalog::manual_book::Chapter;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{prelude::*, widgets::*};

const ACCENT: Color = Color::Cyan;
const GOOD: Color = Color::Green;
const DIM: Color = Color::DarkGray;
const KEY: Color = Color::Magenta;
const BAD: Color = Color::Red;

enum Row {
    Chapter(usize),        // chapter index -- shows the chapter lead-in
    Section(usize, usize), // (chapter, section) -- shows the section
}

#[derive(PartialEq)]
enum Focus {
    Nav,
    Content,
}

/// The WIKI tab: the manual browser as an [`arieltune_tui_kit::Screen`].
pub struct WikiScreen {
    chapters: Vec<Chapter>,
    rows: Vec<Row>,
    view: Vec<usize>,
    sel: usize,
    nav_off: usize,
    focus: Focus,
    scroll: u16,
    searching: bool,
    query: String,
}

impl WikiScreen {
    pub fn new(chapters: Vec<Chapter>) -> Self {
        let mut rows = Vec::new();
        for (ci, ch) in chapters.iter().enumerate() {
            rows.push(Row::Chapter(ci));
            for si in 0..ch.sections.len() {
                rows.push(Row::Section(ci, si));
            }
        }
        let mut app = WikiScreen {
            chapters,
            rows,
            view: Vec::new(),
            sel: 0,
            nav_off: 0,
            focus: Focus::Nav,
            scroll: 0,
            searching: false,
            query: String::new(),
        };
        app.rebuild_view();
        app
    }

    fn row_title(&self, r: &Row) -> String {
        match *r {
            Row::Chapter(ci) => self.chapters[ci].title.clone(),
            Row::Section(ci, si) => self.chapters[ci].sections[si].title.clone(),
        }
    }

    fn row_matches(&self, r: &Row, q: &str) -> bool {
        match *r {
            Row::Chapter(ci) => {
                let ch = &self.chapters[ci];
                ch.title.to_lowercase().contains(q) || ch.preamble.to_lowercase().contains(q)
            }
            Row::Section(ci, si) => {
                let s = &self.chapters[ci].sections[si];
                s.title.to_lowercase().contains(q) || s.body.to_lowercase().contains(q)
            }
        }
    }

    fn rebuild_view(&mut self) {
        if self.searching && !self.query.is_empty() {
            let q = self.query.to_lowercase();
            self.view = (0..self.rows.len())
                .filter(|i| self.row_matches(&self.rows[*i], &q))
                .collect();
        } else {
            self.view = (0..self.rows.len()).collect();
        }
        if self.sel >= self.view.len() {
            self.sel = self.view.len().saturating_sub(1);
        }
    }

    fn move_sel(&mut self, dir: isize) {
        if self.view.is_empty() {
            return;
        }
        let ni = self.sel as isize + dir;
        if ni >= 0 && (ni as usize) < self.view.len() {
            self.sel = ni as usize;
            self.scroll = 0;
        }
    }

    /// Jump to the next/previous chapter head.
    fn jump_chapter(&mut self, dir: isize) {
        let n = self.view.len();
        if n == 0 {
            return;
        }
        let mut i = self.sel as isize;
        loop {
            i += dir;
            if i < 0 || i as usize >= n {
                return;
            }
            if matches!(self.rows[self.view[i as usize]], Row::Chapter(_)) {
                self.sel = i as usize;
                self.scroll = 0;
                return;
            }
        }
    }

    fn cur(&self) -> Option<(&str, &str)> {
        let r = self.rows.get(*self.view.get(self.sel)?)?;
        Some(match *r {
            Row::Chapter(ci) => (
                self.chapters[ci].title.as_str(),
                self.chapters[ci].preamble.as_str(),
            ),
            Row::Section(ci, si) => {
                let s = &self.chapters[ci].sections[si];
                (s.title.as_str(), s.body.as_str())
            }
        })
    }

    /// Render the whole tab into the shell-provided `area` (header + panes + footer).
    fn render(&mut self, f: &mut Frame, area: Rect) {
        let root = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(0),
                Constraint::Length(2),
            ])
            .split(area);

        // Sub-header: the branded tab title + manual context, or the live search
        // query. Brand prefix matches the other tabs (` arieltune <tab> `).
        let header = if self.searching {
            Line::from(vec![
                Span::styled("search: ", Style::default().fg(DIM)),
                Span::styled(
                    format!("{}_", self.query),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
            ])
        } else {
            Line::from(vec![
                Span::styled(
                    " arieltune wiki ",
                    Style::default().fg(KEY).add_modifier(Modifier::BOLD),
                ),
                Span::styled("  BC-250 OEM manual", Style::default().fg(DIM)),
                Span::styled(
                    format!("   ·   {} chapters", self.chapters.len()),
                    Style::default().fg(DIM),
                ),
            ])
        };
        f.render_widget(Paragraph::new(header), root[0]);

        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(32), Constraint::Min(0)])
            .split(root[1]);

        draw_nav(f, self, body[0]);
        draw_content(f, self, body[1]);
        draw_footer(f, self, root[2]);
    }
}

impl Screen for WikiScreen {
    fn title(&self) -> &'static str {
        "WIKI"
    }

    fn draw(&mut self, f: &mut Frame, area: Rect) {
        self.render(f, area);
    }

    fn on_key(&mut self, k: KeyEvent) -> Outcome {
        // Search sub-mode: modal, so the shell does not steal keys. Every key here
        // is consumed (typed into the query or ends the search).
        if self.searching {
            match k.code {
                KeyCode::Esc => {
                    self.searching = false;
                    self.query.clear();
                    self.rebuild_view();
                }
                KeyCode::Enter => {
                    self.searching = false;
                    self.focus = Focus::Content;
                }
                KeyCode::Backspace => {
                    self.query.pop();
                    self.rebuild_view();
                }
                KeyCode::Char(c) => {
                    self.query.push(c);
                    self.rebuild_view();
                }
                _ => {}
            }
            return Outcome::Consumed;
        }

        match k.code {
            // Bare q / Esc / Ctrl-C quit the suite (no popup to close in WIKI). The
            // pane owns these; the shell's Ctrl-Q is the always-available alternative.
            KeyCode::Char('q') => Outcome::Quit,
            KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => Outcome::Quit,
            KeyCode::Esc => Outcome::Quit,
            KeyCode::Tab => {
                self.focus = if self.focus == Focus::Nav {
                    Focus::Content
                } else {
                    Focus::Nav
                };
                Outcome::Consumed
            }
            KeyCode::Char('/') => {
                self.searching = true;
                self.query.clear();
                Outcome::Consumed
            }
            KeyCode::Up | KeyCode::Char('k') => {
                match self.focus {
                    Focus::Nav => self.move_sel(-1),
                    Focus::Content => self.scroll = self.scroll.saturating_sub(1),
                }
                Outcome::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                match self.focus {
                    Focus::Nav => self.move_sel(1),
                    Focus::Content => self.scroll = self.scroll.saturating_add(1),
                }
                Outcome::Consumed
            }
            KeyCode::Left => {
                self.jump_chapter(-1);
                Outcome::Consumed
            }
            KeyCode::Right => {
                self.jump_chapter(1);
                Outcome::Consumed
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(15);
                Outcome::Consumed
            }
            KeyCode::PageDown | KeyCode::Char(' ') => {
                self.scroll = self.scroll.saturating_add(15);
                Outcome::Consumed
            }
            KeyCode::Home => {
                self.scroll = 0;
                Outcome::Consumed
            }
            KeyCode::Enter => {
                self.focus = Focus::Content;
                Outcome::Consumed
            }
            // Anything else falls through so the shell can switch tabs / quit.
            _ => Outcome::Ignored,
        }
    }

    fn modal(&self) -> bool {
        self.searching
    }
}

fn draw_nav(f: &mut Frame, app: &mut WikiScreen, area: Rect) {
    let focused = app.focus == Focus::Nav;
    let border = if focused { ACCENT } else { DIM };
    let title = if focused {
        " ▸ Contents "
    } else {
        " Contents "
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border))
        .title(Span::styled(
            title,
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let h = inner.height as usize;
    if app.sel < app.nav_off {
        app.nav_off = app.sel;
    } else if h > 0 && app.sel >= app.nav_off + h {
        app.nav_off = app.sel + 1 - h;
    }
    let mut lines: Vec<Line> = Vec::new();
    for vi in app.nav_off..(app.nav_off + h).min(app.view.len()) {
        let ri = app.view[vi];
        let sel = vi == app.sel;
        let (text, base) = match &app.rows[ri] {
            Row::Chapter(ci) => (
                app.chapters[*ci].title.clone(),
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Row::Section(_, _) => (
                format!("   {}", app.row_title(&app.rows[ri])),
                Style::default().fg(Color::Gray),
            ),
        };
        let style = if sel {
            Style::default()
                .fg(Color::Black)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            base
        };
        lines.push(Line::from(Span::styled(text, style)));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_content(f: &mut Frame, app: &mut WikiScreen, area: Rect) {
    let focused = app.focus == Focus::Content;
    let border = if focused { ACCENT } else { DIM };
    let (title, bodytext) = app.cur().unwrap_or(("—", ""));
    let mark = if focused { "▸ " } else { "" };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border))
        .title(Span::styled(
            format!(" {mark}{title} "),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let width = inner.width.saturating_sub(1) as usize;
    // Designed pages carry their own fixed-width layout + header rule --
    // render them verbatim. Un-designed (markdown) sections still reflow.
    let designed = bodytext.contains('┌') || bodytext.contains("── ");
    let lines: Vec<Line> = if designed {
        verbatim_lines(bodytext)
    } else {
        let mut v = vec![
            Line::from(Span::styled(
                title.to_string(),
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            )),
            Line::default(),
        ];
        v.extend(md_lines(bodytext, width.max(20)));
        v
    };
    let max = lines.len() as u16;
    if app.scroll >= max {
        app.scroll = max.saturating_sub(1);
    }
    f.render_widget(Paragraph::new(lines).scroll((app.scroll, 0)), inner);
}

fn draw_footer(f: &mut Frame, app: &WikiScreen, area: Rect) {
    // Status line: the current section, or the search match count.
    let status = if app.searching {
        format!("{} match(es)", app.view.len())
    } else if let Some((t, _)) = app.cur() {
        t.to_string()
    } else {
        "no selection".to_string()
    };

    let keys: &[(&str, &str)] = if app.searching {
        &[
            ("[type]", "filter"),
            ("[Enter]", "read"),
            ("[Esc]", "cancel"),
        ]
    } else {
        &[
            ("[Tab]", "pane"),
            ("[↑↓]", "move"),
            ("[←→]", "chapter"),
            ("[Space]", "page"),
            ("[/]", "search"),
            ("[q]", "quit"),
        ]
    };

    let p = Paragraph::new(vec![
        Line::from(Span::styled(
            status,
            Style::default().fg(GOOD).add_modifier(Modifier::BOLD),
        )),
        key_line(keys),
    ]);
    f.render_widget(p, area);
}

/// A key-hint line in the tune-tool theme: each bracketed key in bold magenta with
/// a dim label, buttons two spaces apart.
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

/// Box-drawing / frame characters (plus space) -- rendered dim so frames recede and
/// the header text stands out.
fn is_frame(c: char) -> bool {
    "─│┌┐└┘├┤┬┴┼╪═╔╗╚╝╠╣╦╩ ".contains(c)
}

/// Render a designed page verbatim (fixed-width, no reflow). Only the headers
/// carry color, to break up the wall of text: the `── TITLE ──` section rule and
/// each `┌─ LABEL ─┐` box label render bold magenta (CAUTION boxes red), with frames
/// dim; everything else -- prose, values, tables, the hardware tree -- stays plain.
/// One `Line` per source line, so scroll offsets are unchanged.
fn verbatim_lines(body: &str) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for l in body.lines() {
        let t = l.trim();
        if t.starts_with("```") || t.starts_with("~~~") {
            continue;
        }
        // Section header rule -- a magenta header across the top of the section.
        if t.starts_with("── ") {
            out.push(Line::from(header_spans(l, KEY)));
            continue;
        }
        // Box label (`┌─ LABEL ─┐`) -- a magenta header for each block, red for a
        // CAUTION so safety still stands out.
        if t.starts_with('┌') && t.ends_with('┐') {
            let color = if l.contains("CAUTION") { BAD } else { KEY };
            out.push(Line::from(header_spans(l, color)));
            continue;
        }
        // Body: frame-only lines dim, everything else plain.
        let style = if !t.is_empty() && l.chars().all(is_frame) {
            Style::default().fg(DIM)
        } else {
            Style::default().fg(Color::Gray)
        };
        out.push(Line::from(Span::styled(l.to_string(), style)));
    }
    out
}

/// Split a header line into frame runs (dim) and text runs (`color`, bold) -- so a
/// `┌─ LABEL ─┐` or `── TITLE ──` line reads as a colored header without touching
/// the body around it.
fn header_spans(line: &str, color: Color) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut buf = String::new();
    let mut cur: Option<bool> = None;
    let flush = |buf: &mut String, is_frame: bool, spans: &mut Vec<Span<'static>>| {
        if buf.is_empty() {
            return;
        }
        let style = if is_frame {
            Style::default().fg(DIM)
        } else {
            Style::default().fg(color).add_modifier(Modifier::BOLD)
        };
        spans.push(Span::styled(std::mem::take(buf), style));
    };
    for c in line.chars() {
        let f = is_frame(c);
        if cur != Some(f) {
            if let Some(prev) = cur {
                flush(&mut buf, prev, &mut spans);
            }
            cur = Some(f);
        }
        buf.push(c);
    }
    if let Some(prev) = cur {
        flush(&mut buf, prev, &mut spans);
    }
    if spans.is_empty() {
        spans.push(Span::raw(String::new()));
    }
    spans
}

// ---------------- markdown -> ratatui ----------------

fn md_lines(body: &str, width: usize) -> Vec<Line<'static>> {
    let src: Vec<&str> = body.lines().collect();
    let mut out: Vec<Line> = Vec::new();
    let mut i = 0;
    let mut in_code = false;
    while i < src.len() {
        let line = src[i];
        let trimmed = line.trim_start();

        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_code = !in_code;
            out.push(Line::from(Span::styled("  ┄┄┄", Style::default().fg(DIM))));
            i += 1;
            continue;
        }
        if in_code {
            out.push(Line::from(Span::styled(
                format!("  {line}"),
                Style::default().fg(Color::Gray),
            )));
            i += 1;
            continue;
        }

        if trimmed.starts_with('|') {
            let mut block = Vec::new();
            while i < src.len() && src[i].trim_start().starts_with('|') {
                block.push(src[i].trim());
                i += 1;
            }
            out.extend(render_table(&block));
            continue;
        }

        let hashes = line.chars().take_while(|c| *c == '#').count();
        if hashes >= 1 && line[hashes..].starts_with(' ') {
            let text = line[hashes + 1..].trim();
            out.push(Line::from(Span::styled(
                text.to_string(),
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            )));
            i += 1;
            continue;
        }

        if let Some(rest) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            for (j, w) in wrap(rest, width.saturating_sub(2)).iter().enumerate() {
                let prefix = if j == 0 { " • " } else { "   " };
                let mut spans = vec![Span::styled(
                    prefix.to_string(),
                    Style::default().fg(ACCENT),
                )];
                spans.extend(inline_spans(w, Style::default()));
                out.push(Line::from(spans));
            }
            i += 1;
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("> ") {
            for w in wrap(rest, width.saturating_sub(2)) {
                out.push(Line::from(Span::styled(
                    format!("▏ {w}"),
                    Style::default().fg(DIM).add_modifier(Modifier::ITALIC),
                )));
            }
            i += 1;
            continue;
        }

        if trimmed.is_empty() {
            out.push(Line::default());
            i += 1;
            continue;
        }

        for w in wrap(line, width) {
            out.push(Line::from(inline_spans(&w, Style::default())));
        }
        i += 1;
    }
    out
}

fn render_table(block: &[&str]) -> Vec<Line<'static>> {
    let cells: Vec<Vec<String>> = block
        .iter()
        .map(|r| {
            let r = r.trim().trim_start_matches('|').trim_end_matches('|');
            r.split('|').map(|c| c.trim().to_string()).collect()
        })
        .collect();
    let is_sep = |row: &[String]| {
        !row.is_empty()
            && row
                .iter()
                .all(|c| !c.is_empty() && c.chars().all(|ch| ch == '-' || ch == ':' || ch == ' '))
    };
    let ncols = cells.iter().map(|r| r.len()).max().unwrap_or(0);
    if ncols == 0 {
        return Vec::new();
    }
    let mut w = vec![0usize; ncols];
    for row in &cells {
        if is_sep(row) {
            continue;
        }
        for (c, cell) in row.iter().enumerate() {
            w[c] = w[c].max(cell.chars().count());
        }
    }
    let mut out = Vec::new();
    for (ri, row) in cells.iter().enumerate() {
        if is_sep(row) {
            let rule: Vec<String> = w.iter().map(|width| "─".repeat(*width)).collect();
            out.push(Line::from(Span::styled(
                format!(" {}", rule.join("─┼─")),
                Style::default().fg(DIM),
            )));
            continue;
        }
        let header = ri == 0;
        let mut parts = Vec::new();
        for (c, cw) in w.iter().enumerate() {
            let cell = row.get(c).cloned().unwrap_or_default();
            let pad = cw.saturating_sub(cell.chars().count());
            parts.push(format!("{cell}{}", " ".repeat(pad)));
        }
        let text = format!(" {}", parts.join(" │ "));
        let style = if header {
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        out.push(Line::from(Span::styled(text, style)));
    }
    out
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(8);
    let mut out = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        if cur.is_empty() {
            cur = word.to_string();
        } else if cur.chars().count() + 1 + word.chars().count() <= width {
            cur.push(' ');
            cur.push_str(word);
        } else {
            out.push(std::mem::take(&mut cur));
            cur = word.to_string();
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

fn inline_spans(text: &str, base: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut buf = String::new();
    let mut chars = text.chars().peekable();
    let flush = |buf: &mut String, spans: &mut Vec<Span<'static>>| {
        if !buf.is_empty() {
            spans.push(Span::styled(std::mem::take(buf), base));
        }
    };
    while let Some(c) = chars.next() {
        match c {
            '`' => {
                flush(&mut buf, &mut spans);
                let mut code = String::new();
                for c2 in chars.by_ref() {
                    if c2 == '`' {
                        break;
                    }
                    code.push(c2);
                }
                spans.push(Span::styled(code, Style::default().fg(GOOD)));
            }
            '*' if chars.peek() == Some(&'*') => {
                chars.next();
                flush(&mut buf, &mut spans);
                let mut bold = String::new();
                while let Some(c2) = chars.next() {
                    if c2 == '*' && chars.peek() == Some(&'*') {
                        chars.next();
                        break;
                    }
                    bold.push(c2);
                }
                spans.push(Span::styled(bold, base.add_modifier(Modifier::BOLD)));
            }
            _ => buf.push(c),
        }
    }
    flush(&mut buf, &mut spans);
    if spans.is_empty() {
        spans.push(Span::styled(String::new(), base));
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    use bc250_catalog::manual_book;
    use ratatui::backend::TestBackend;

    #[test]
    fn renders_manual_frame() {
        let mut app = WikiScreen::new(manual_book::load());
        let mut term = Terminal::new(TestBackend::new(160, 45)).unwrap();
        term.draw(|f| {
            let a = f.area();
            app.render(f, a);
        })
        .unwrap();
        let buf = term.backend().buffer().clone();
        let mut all = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                all.push_str(buf.cell((x, y)).unwrap().symbol());
            }
            all.push('\n');
        }
        assert!(all.contains("arieltune wiki"));
        assert!(all.contains("BC-250 OEM manual"));
        assert!(all.contains("Overview"));
        assert!(app.chapters.len() >= 7, "overview + 6 chapters");
        assert!(app
            .chapters
            .iter()
            .any(|c| c.title.contains("Carrier Board")));
    }

    #[test]
    fn only_headers_are_colored() {
        // A box label is a magenta header; the frame around it is dim.
        let spans = header_spans("┌─ CHIPSET ─────┐", KEY);
        assert_eq!(spans[0].style.fg, Some(DIM), "frame run is dim");
        let label = spans
            .iter()
            .find(|s| s.content.contains("CHIPSET"))
            .unwrap();
        assert_eq!(label.style.fg, Some(KEY));
        assert!(label.style.add_modifier.contains(Modifier::BOLD));

        // A CAUTION box label is red.
        let caution = header_spans("┌─ CAUTION — GPIO ─────┐", BAD);
        assert!(caution
            .iter()
            .any(|s| s.content.contains("CAUTION") && s.style.fg == Some(BAD)));

        // Body lines carry no header/token color -- a box interior row is plain.
        let lines = verbatim_lines("│  Part      AMD A68H   PCI 0x13FE  │\n");
        assert!(lines[0].spans.iter().all(|s| s.style.fg != Some(KEY)
            && s.style.fg != Some(GOOD)
            && s.style.fg != Some(BAD)));
    }
}
