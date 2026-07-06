// SPDX-License-Identifier: GPL-2.0-only
//! The WIKI CLI: projects the embedded manual into clean structured records for
//! agents (chapters / list / get / search / safety / export / doctor). Ported
//! verbatim from wikitune's `main.rs` (minus the process entrypoint and the `Tui`
//! variant -- the suite owns TUI launch). SIGPIPE is set by the suite binary.

use anyhow::{Context, Result};
use bc250_catalog::manual_data::{self, Block, Section};
use clap::Subcommand;

#[derive(Subcommand)]
pub enum Cmd {
    /// List chapters and their section counts.
    Chapters,
    /// List sections (id + title), optionally within one chapter.
    List {
        #[arg(long)]
        chapter: Option<u32>,
    },
    /// Show one section by id -- readable text, or `--json` for the record.
    Get {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Full-text search across every section (title, tagline, prose, blocks).
    Search {
        query: String,
        #[arg(long)]
        json: bool,
    },
    /// List every safety CAUTION across the manual.
    Safety {
        #[arg(long)]
        json: bool,
    },
    /// Export sections as structured records for RAG ingestion or tooling.
    Export {
        /// `jsonl` (one record per line), `json` (one document), or `md`.
        #[arg(long, default_value = "jsonl")]
        format: ExportFormat,
        #[arg(long)]
        chapter: Option<u32>,
        /// Write to a file instead of stdout.
        #[arg(short, long)]
        out: Option<String>,
    },
    /// Check that the embedded manual projects cleanly (ids, empty sections).
    Doctor,
}

#[derive(Clone, Copy, Debug)]
pub enum ExportFormat {
    Jsonl,
    Json,
    Markdown,
}

impl std::str::FromStr for ExportFormat {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "jsonl" | "ndjson" => Ok(ExportFormat::Jsonl),
            "json" => Ok(ExportFormat::Json),
            "md" | "markdown" => Ok(ExportFormat::Markdown),
            other => Err(format!("unknown format '{other}' (want jsonl|json|md)")),
        }
    }
}

fn in_chapter(s: &Section, ch: Option<u32>) -> bool {
    ch.map(|n| s.chapter_num == Some(n)).unwrap_or(true)
}

/// Dispatch a WIKI CLI subcommand against the embedded manual's structured projection.
pub fn run(cmd: Cmd) -> Result<()> {
    let sections = manual_data::load();

    match cmd {
        Cmd::Chapters => {
            let mut cur = String::new();
            let mut n = 0usize;
            for s in &sections {
                if s.chapter != cur {
                    if !cur.is_empty() {
                        println!("{cur}  ({n} sections)");
                    }
                    cur = s.chapter.clone();
                    n = 0;
                }
                n += 1;
            }
            if !cur.is_empty() {
                println!("{cur}  ({n} sections)");
            }
        }

        Cmd::List { chapter } => {
            for s in sections.iter().filter(|s| in_chapter(s, chapter)) {
                println!("{:<28} {}", s.id, s.title);
            }
        }

        Cmd::Get { id, json } => {
            let s = sections
                .iter()
                .find(|s| s.id == id)
                .with_context(|| format!("no section '{id}'"))?;
            if json {
                println!("{}", serde_json::to_string_pretty(s)?);
            } else {
                print_section(s);
            }
        }

        Cmd::Search { query, json } => {
            let q = query.to_lowercase();
            let hits: Vec<&Section> = sections.iter().filter(|s| matches(s, &q)).collect();
            if json {
                println!("{}", serde_json::to_string_pretty(&hits)?);
            } else if hits.is_empty() {
                println!("no matches for '{query}'");
            } else {
                for s in hits {
                    println!("{:<28} {}  [{}]", s.id, s.title, s.chapter);
                }
            }
        }

        Cmd::Safety { json } => {
            let mut items = Vec::new();
            for s in &sections {
                for b in s.blocks.iter().filter(|b| b.label.contains("CAUTION")) {
                    items.push((s, b));
                }
            }
            if json {
                let recs: Vec<_> = items
                    .iter()
                    .map(|(s, b)| {
                        serde_json::json!({
                            "section": s.id,
                            "title": s.title,
                            "chapter": s.chapter,
                            "caution": b.text.replace('\n', " "),
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&recs)?);
            } else {
                for (s, b) in items {
                    println!(
                        "[{}] {}\n    {}\n",
                        s.chapter,
                        s.title,
                        b.text.replace('\n', " ")
                    );
                }
            }
        }

        Cmd::Export {
            format,
            chapter,
            out,
        } => {
            let sel: Vec<&Section> = sections.iter().filter(|s| in_chapter(s, chapter)).collect();
            let rendered = render_export(&sel, format)?;
            match out {
                Some(path) => {
                    std::fs::write(&path, rendered).with_context(|| format!("writing {path}"))?;
                    eprintln!("wrote {} sections to {path}", sel.len());
                }
                None => print!("{rendered}"),
            }
        }

        Cmd::Doctor => {
            let problems = manual_data::integrity(&sections);
            println!("manual: {} sections", sections.len());
            if problems.is_empty() {
                println!("[ok] no integrity problems");
            } else {
                for p in &problems {
                    println!("[fail] {p}");
                }
                std::process::exit(1);
            }
        }
    }

    Ok(())
}

fn matches(s: &Section, q: &str) -> bool {
    s.title.to_lowercase().contains(q)
        || s.tagline.to_lowercase().contains(q)
        || s.prose.to_lowercase().contains(q)
        || s.id.contains(q)
        || s.blocks.iter().any(|b| {
            b.label.to_lowercase().contains(q)
                || b.rows
                    .iter()
                    .any(|r| r.iter().any(|c| c.to_lowercase().contains(q)))
        })
}

fn render_export(sel: &[&Section], fmt: ExportFormat) -> Result<String> {
    Ok(match fmt {
        ExportFormat::Jsonl => {
            let mut out = String::new();
            for s in sel {
                out.push_str(&serde_json::to_string(s)?);
                out.push('\n');
            }
            out
        }
        ExportFormat::Json => serde_json::to_string_pretty(sel)?,
        ExportFormat::Markdown => {
            let mut out = String::from("# ASRock BC-250 — OEM System Manual\n\n");
            for s in sel {
                out.push_str(&format!("## {}\n\n", s.title));
                if !s.tagline.is_empty() {
                    out.push_str(&format!("*{}*\n\n", s.tagline));
                }
                if !s.prose.is_empty() {
                    out.push_str(&s.prose);
                    out.push_str("\n\n");
                }
                for b in &s.blocks {
                    out.push_str(&format!("**{}**\n\n", b.label));
                    if narrative(b) {
                        out.push_str(&b.text);
                        out.push('\n');
                    } else {
                        for r in &b.rows {
                            out.push_str(&format!("- {}\n", r.join(" · ")));
                        }
                    }
                    out.push('\n');
                }
                if !s.cross_refs.is_empty() {
                    out.push_str(&format!("See also: {}\n\n", s.cross_refs.join("; ")));
                }
                out.push_str("---\n\n");
            }
            out
        }
    })
}

/// Print a section as readable text: title, tagline, prose, aligned blocks, refs.
fn print_section(s: &Section) {
    println!("# {}", s.title);
    if !s.tagline.is_empty() {
        println!("  {}", s.tagline);
    }
    println!("  {} · {}\n", s.chapter, s.id);
    if !s.prose.is_empty() {
        println!("{}\n", s.prose);
    }
    for b in &s.blocks {
        print_block(b);
    }
    if !s.cross_refs.is_empty() {
        println!("See also: {}", s.cross_refs.join("; "));
    }
}

/// Narrative boxes (CAUTION / FINDING) are prose, not columns -- render as text.
fn narrative(b: &Block) -> bool {
    b.label.contains("CAUTION") || b.label.starts_with("FINDING")
}

fn print_block(b: &Block) {
    println!("── {} ──", b.label);
    if narrative(b) {
        for l in b.text.lines() {
            println!("  {l}");
        }
        println!();
        return;
    }
    let ncols = b.rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let mut w = vec![0usize; ncols];
    for r in &b.rows {
        for (i, c) in r.iter().enumerate() {
            w[i] = w[i].max(c.chars().count());
        }
    }
    for r in &b.rows {
        let mut line = String::from("  ");
        for (i, c) in r.iter().enumerate() {
            if i + 1 < r.len() {
                line.push_str(&format!("{:<width$}  ", c, width = w[i]));
            } else {
                line.push_str(c);
            }
        }
        println!("{}", line.trim_end());
    }
    println!();
}
