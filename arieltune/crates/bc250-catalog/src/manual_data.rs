// SPDX-License-Identifier: GPL-2.0-only
//! Structured projection of the authored OEM manual for machine consumption.
//!
//! This is the **agent-facing** view of the exact same embedded manual the TUI
//! renders ([`crate::manual_book::MANUAL_MD`]). The TUI reads it as designed
//! prose; agents read it as clean structured records. Both go through
//! [`crate::manual_book::parse`], so the CLI and the TUI can never drift — there
//! is one source of truth (the manual), projected two ways.
//!
//! Each `##` section of the manual becomes a [`Section`]: its narrative prose,
//! the header tagline, the labeled `┌─ … ─┐` boxes parsed into cell grids, and
//! the `See:` cross-references. Decorative flow diagrams (side-by-side boxes)
//! are preserved verbatim in `prose` rather than force-fit into a grid.

use crate::manual_book::{self, Chapter};
use serde::Serialize;

/// One labeled box inside a section, parsed into a cell grid.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Block {
    /// The box label, e.g. `CHIPSET`, `PCI FUNCTIONS`, `CAUTION` (may be empty).
    pub label: String,
    /// `"grid"` when the box uses `│`-separated columns (`┬┼┴` borders),
    /// otherwise `"list"` (columns split on runs of 2+ spaces).
    pub kind: String,
    /// Interior rows as cell lists. Pure box-drawing rules are dropped.
    /// Tabular boxes read cleanly here; use [`Block::text`] for narrative boxes
    /// (CAUTION / FINDING) whose lines are prose, not columns.
    pub rows: Vec<Vec<String>>,
    /// The interior content verbatim (borders stripped), one paragraph per line.
    /// Authoritative for prose boxes; a flattened view of a table otherwise.
    pub text: String,
}

/// One `##` section of the manual, structured for machine consumption.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Section {
    /// Stable slug id, e.g. `ch1-a68h-fch`.
    pub id: String,
    /// The chapter this section belongs to, e.g. `1. Carrier Board`.
    pub chapter: String,
    /// Chapter number (1..=6), or `None` for the front-matter Overview.
    pub chapter_num: Option<u32>,
    /// Section title, e.g. `A68H FCH — Fusion Controller Hub`.
    pub title: String,
    /// The right-hand descriptor from the `── TITLE ── tagline ──` rule.
    pub tagline: String,
    /// Narrative paragraphs (and any verbatim diagrams), blank-run collapsed.
    pub prose: String,
    /// Labeled boxes parsed into cell grids.
    pub blocks: Vec<Block>,
    /// `See:` cross-reference targets.
    pub cross_refs: Vec<String>,
}

const BOX_CHARS: &str = "─│┌┐└┘├┤┬┴┼╪═╔╗╚╝╠╣╦╩ ";

/// Load the manual and project every section into a structured [`Section`].
pub fn load() -> Vec<Section> {
    let chapters = manual_book::load();
    sections(&chapters)
}

/// Project parsed chapters into structured sections. The chapter lead-in
/// (preamble) becomes a section titled after the chapter (id `chN`).
pub fn sections(chapters: &[Chapter]) -> Vec<Section> {
    let mut out = Vec::new();
    for ch in chapters {
        let num = chapter_num(&ch.title);
        let cslug = num
            .map(|n| format!("ch{n}"))
            .unwrap_or_else(|| "overview".into());
        if !ch.preamble.trim().is_empty() {
            out.push(build_section(
                &cslug,
                &ch.title,
                num,
                &ch.title,
                &ch.preamble,
            ));
        }
        for s in &ch.sections {
            let id = format!("{cslug}-{}", slug(&s.title));
            out.push(build_section(&id, &ch.title, num, &s.title, &s.body));
        }
    }
    out
}

fn build_section(id: &str, chapter: &str, num: Option<u32>, title: &str, body: &str) -> Section {
    let p = parse_body(body);
    Section {
        id: id.to_string(),
        chapter: chapter.to_string(),
        chapter_num: num,
        title: title.to_string(),
        tagline: p.tagline,
        prose: p.prose,
        blocks: p.blocks,
        cross_refs: p.cross_refs,
    }
}

struct Parsed {
    tagline: String,
    prose: String,
    blocks: Vec<Block>,
    cross_refs: Vec<String>,
}

/// Parse one section body (the fenced designed page) into its parts.
fn parse_body(body: &str) -> Parsed {
    let mut tagline = String::new();
    let mut prose: Vec<String> = Vec::new();
    let mut blocks: Vec<Block> = Vec::new();
    let mut cross_refs: Vec<String> = Vec::new();

    let lines: Vec<&str> = body.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let raw = lines[i];
        let t = raw.trim();

        // Fence markers — skip.
        if t.starts_with("```") || t.starts_with("~~~") {
            i += 1;
            continue;
        }
        // Section header rule: `── TITLE ── tagline ──`.
        if t.starts_with("── ") {
            if tagline.is_empty() {
                tagline = header_tagline(t);
            }
            i += 1;
            continue;
        }
        // Cross-references.
        if let Some(rest) = t.strip_prefix("See:") {
            for r in rest.split('—') {
                let r = r.trim();
                if !r.is_empty() {
                    cross_refs.push(r.to_string());
                }
            }
            i += 1;
            continue;
        }
        // A single full-width labeled/tabular box: one `┌ … ┐` opening a line.
        if t.starts_with('┌') && t.ends_with('┐') && t.matches('┌').count() == 1 {
            let (block, next) = parse_box(&lines, i);
            blocks.push(block);
            i = next;
            continue;
        }
        // Everything else (narrative, diagrams, side-by-side boxes) — verbatim.
        prose.push(t.to_string());
        i += 1;
    }

    Parsed {
        tagline,
        prose: collapse_blanks(&prose),
        blocks,
        cross_refs,
    }
}

/// Parse a box starting at `lines[start]` (its top border). Returns the block
/// and the index just past its bottom border (or past EOF if unterminated).
fn parse_box(lines: &[&str], start: usize) -> (Block, usize) {
    let top = lines[start].trim();
    let label = box_label(top);
    // Grid tables draw internal column joins with ┬ ┼ ┴.
    let grid = lines[start..]
        .iter()
        .take_while(|l| !l.trim().starts_with('└'))
        .any(|l| l.contains('┬') || l.contains('┼') || l.contains('┴'));

    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut text: Vec<String> = Vec::new();
    let mut i = start + 1;
    while i < lines.len() {
        let t = lines[i].trim();
        if t.starts_with('└') {
            i += 1;
            break;
        }
        // Only interior content lines (│ … │) carry data.
        if t.starts_with('│') {
            if let Some(raw) = strip_borders(t) {
                text.push(raw);
            }
            if let Some(cells) = split_row(t, grid) {
                rows.push(cells);
            }
        }
        i += 1;
    }
    (
        Block {
            label,
            kind: if grid { "grid".into() } else { "list".into() },
            rows,
            text: text.join("\n"),
        },
        i,
    )
}

/// The interior of a box line with `│` borders stripped and trimmed. `None`
/// for a blank or pure box-drawing separator line.
fn strip_borders(line: &str) -> Option<String> {
    let inner = line.trim().trim_start_matches('│').trim_end_matches('│');
    if inner.trim().is_empty() || inner.chars().all(|c| BOX_CHARS.contains(c)) {
        None
    } else {
        Some(inner.trim().to_string())
    }
}

/// Split one interior box line into cells. `None` if the line is a pure rule.
fn split_row(line: &str, grid: bool) -> Option<Vec<String>> {
    // Strip the outer │ borders.
    let inner = line.trim().trim_start_matches('│').trim_end_matches('│');
    if inner.trim().is_empty() {
        return None;
    }
    // A pure box-drawing separator row carries no data.
    if inner.chars().all(|c| BOX_CHARS.contains(c)) {
        return None;
    }
    let cells: Vec<String> = if grid {
        inner.split('│').map(|c| c.trim().to_string()).collect()
    } else {
        // Columns are separated by runs of 2+ spaces.
        split_on_gaps(inner)
    };
    let cells: Vec<String> = cells.into_iter().filter(|c| !c.is_empty()).collect();
    if cells.is_empty() {
        None
    } else {
        Some(cells)
    }
}

/// Split on runs of 2+ spaces, trimming each field.
fn split_on_gaps(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut spaces = 0usize;
    for c in s.chars() {
        if c == ' ' {
            spaces += 1;
        } else {
            if spaces >= 2 && !cur.trim().is_empty() {
                out.push(cur.trim().to_string());
                cur.clear();
            } else if spaces > 0 && !cur.is_empty() {
                // Single interior spaces are part of the field.
                cur.push(' ');
            }
            spaces = 0;
            cur.push(c);
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// Extract the label from a box top border: `┌─ LABEL ────…────┐`.
fn box_label(top: &str) -> String {
    let inner: String = top.chars().skip_while(|&c| c == '┌').collect();
    let inner = inner.trim_end_matches('┐');
    // Trim leading/trailing box-dashes and spaces; interior em-dashes (—) in a
    // label like `FINDING — …` are U+2014 and survive.
    inner.trim_matches(|c| c == '─' || c == ' ').to_string()
}

/// Extract the tagline (right descriptor) from `── TITLE ── tagline ──`.
fn header_tagline(rule: &str) -> String {
    let inner = rule.trim_matches(|c| c == '─' || c == ' ');
    // Segments are separated by runs of box-dashes.
    let segs: Vec<&str> = inner
        .split('─')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    if segs.len() <= 1 {
        String::new()
    } else {
        segs[1..].join(" ")
    }
}

fn collapse_blanks(lines: &[String]) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut blank = false;
    for l in lines {
        if l.is_empty() {
            if !blank && !out.is_empty() {
                out.push("");
            }
            blank = true;
        } else {
            out.push(l);
            blank = false;
        }
    }
    while out.last() == Some(&"") {
        out.pop();
    }
    out.join("\n")
}

fn chapter_num(title: &str) -> Option<u32> {
    title.split('.').next()?.trim().parse().ok()
}

/// Slug from a section title: take the short name before ` — `, kebab-case it.
fn slug(title: &str) -> String {
    let short = title.split(" — ").next().unwrap_or(title);
    let mut out = String::new();
    let mut dash = false;
    for c in short.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !out.is_empty() && !dash {
            out.push('-');
            dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// Integrity problems for `arieltune wiki doctor`.
pub fn integrity(sections: &[Section]) -> Vec<String> {
    let mut problems = Vec::new();
    let mut seen = std::collections::HashMap::new();
    for s in sections {
        *seen.entry(s.id.clone()).or_insert(0) += 1;
        if s.prose.trim().is_empty() && s.blocks.is_empty() {
            problems.push(format!("section '{}' ({}) is empty", s.id, s.title));
        }
    }
    for (id, n) in seen {
        if n > 1 {
            problems.push(format!("duplicate section id '{id}' ({n}×)"));
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
```
── A68H FCH ─────────────────────────── Bolton-D2H · discrete southbridge ──

A discrete AMD A68H southbridge, tied to the APU over a UMI x4 link.

┌─ CHIPSET ────────────────────────────────────────────────┐
│  Part             AMD A68H — Bolton-D2H, 65 nm   vendor 1022  │
│  Uplink           UMI x4 (PCIe 2.0-based)                     │
└───────────────────────────────────────────────────────────┘

See: Chapter 2 · Subsystem Internals — Chapter 3 · Security
```
";

    #[test]
    fn parses_tagline_prose_block_and_refs() {
        let p = parse_body(SAMPLE);
        assert_eq!(p.tagline, "Bolton-D2H · discrete southbridge");
        assert!(p.prose.contains("discrete AMD A68H southbridge"));
        assert_eq!(p.blocks.len(), 1);
        let b = &p.blocks[0];
        assert_eq!(b.label, "CHIPSET");
        assert_eq!(b.kind, "list");
        assert_eq!(
            b.rows[0],
            vec!["Part", "AMD A68H — Bolton-D2H, 65 nm", "vendor 1022"]
        );
        assert_eq!(b.rows[1], vec!["Uplink", "UMI x4 (PCIe 2.0-based)"]);
        assert!(b.text.contains("Part") && b.text.contains("Uplink"));
        assert_eq!(
            p.cross_refs,
            vec!["Chapter 2 · Subsystem Internals", "Chapter 3 · Security"]
        );
    }

    #[test]
    fn grid_table_splits_on_bars() {
        let body = "\
```
┌──────────┬─────────┬──────────┐
│  Chipset │ SATA    │ USB 3.0  │
├──────────┼─────────┼──────────┤
│  A68H    │ 4× 6G   │ 2        │
└──────────┴─────────┴──────────┘
```
";
        let p = parse_body(body);
        assert_eq!(p.blocks.len(), 1);
        assert_eq!(p.blocks[0].kind, "grid");
        assert_eq!(p.blocks[0].rows[0], vec!["Chipset", "SATA", "USB 3.0"]);
        assert_eq!(p.blocks[0].rows[1], vec!["A68H", "4× 6G", "2"]);
    }

    #[test]
    fn real_manual_projects_cleanly() {
        let secs = load();
        assert!(
            secs.len() >= 80,
            "expected many sections, got {}",
            secs.len()
        );
        assert!(
            secs.iter().any(|s| s.id == "ch1-a68h-fch"),
            "stable id present"
        );
        assert!(secs.iter().all(|s| !s.id.is_empty()));
        assert!(
            integrity(&secs).is_empty(),
            "no integrity problems: {:?}",
            integrity(&secs)
        );
        // Something got parsed into structured blocks.
        assert!(secs.iter().map(|s| s.blocks.len()).sum::<usize>() > 100);
    }
}
