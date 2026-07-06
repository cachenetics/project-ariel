// SPDX-License-Identifier: GPL-2.0-only
//! The authored ASRock BC-250 OEM System Manual, embedded and parsed into a
//! chapter → section tree for the TUI. This is the human-authored, live-verified
//! manual (assembled from the per-chapter files) and the single source of truth
//! for the whole tool: [`crate::manual_data`] projects this same parse into the
//! structured records the CLI serves to agents.

/// The full assembled manual, embedded at build time.
pub const MANUAL_MD: &str = include_str!("bc250_manual.md");

/// One `##` section of a chapter.
pub struct Section {
    pub title: String,
    pub body: String,
}

/// One `#` chapter: its lead-in (preamble) plus its sections.
pub struct Chapter {
    pub title: String,
    pub preamble: String,
    pub sections: Vec<Section>,
}

/// Parse the embedded manual into chapters and sections.
pub fn load() -> Vec<Chapter> {
    parse(MANUAL_MD)
}

/// Heading level of a line (`#` count), or 0 if not an ATX heading.
fn heading_level(line: &str) -> usize {
    let h = line.chars().take_while(|c| *c == '#').count();
    if h >= 1 && line[h..].starts_with(' ') {
        h
    } else {
        0
    }
}

fn clean_chapter_title(raw: &str) -> String {
    if raw.contains("OEM System Manual") {
        "Overview".into()
    } else {
        raw.to_string()
    }
}

/// Fence-aware parse: `#`/`##` are only treated as headings outside fenced code
/// blocks (the manual has `#` shell comments and table headers inside ``` blocks).
pub fn parse(md: &str) -> Vec<Chapter> {
    let mut chapters: Vec<Chapter> = Vec::new();
    let mut in_fence = false;
    let mut chapter: Option<Chapter> = None;
    let mut section: Option<Section> = None;

    for line in md.lines() {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            in_fence = !in_fence;
        }
        let level = if in_fence { 0 } else { heading_level(line) };

        match level {
            1 => {
                if let Some(mut ch) = chapter.take() {
                    if let Some(s) = section.take() {
                        ch.sections.push(s);
                    }
                    chapters.push(ch);
                }
                chapter = Some(Chapter {
                    title: clean_chapter_title(line[1..].trim()),
                    preamble: String::new(),
                    sections: Vec::new(),
                });
            }
            2 => {
                if let (Some(ch), Some(s)) = (chapter.as_mut(), section.take()) {
                    ch.sections.push(s);
                }
                section = Some(Section {
                    title: line[2..].trim().to_string(),
                    body: String::new(),
                });
            }
            _ => {
                if let Some(sec) = section.as_mut() {
                    sec.body.push_str(line);
                    sec.body.push('\n');
                } else if let Some(ch) = chapter.as_mut() {
                    ch.preamble.push_str(line);
                    ch.preamble.push('\n');
                }
            }
        }
    }
    if let Some(mut ch) = chapter.take() {
        if let Some(s) = section.take() {
            ch.sections.push(s);
        }
        chapters.push(ch);
    }
    chapters
}
