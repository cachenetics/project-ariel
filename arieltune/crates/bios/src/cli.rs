// SPDX-License-Identifier: GPL-2.0-only
//! The BIOS CLI: the non-TUI projection of the BC-250 BIOS surface.
//!
//! memtune handles the memory side; this is the rest of the BIOS surface: the
//! full AMD CBS + OEM Setup catalogue, organized into one categorized, searchable
//! tree. Current values are read from the live EFI variables; AmdSetup settings
//! are editable (written back to the EFI variable, applied on reboot). OEM Setup
//! settings are boot-service-only (the OS can't SetVariable them) AND often
//! suppressed in the menu — but `oem-set` changes them anyway via SMM (no flash
//! rig; see below).
//!
//! For a setting to actually take effect, AGESA needs its APCB *enable bit* set
//! (the value lives in AmdSetup, the enable bit lives in the APCB in flash). The
//! `apcb` commands read and flip those enable bits via in-system flashrom — the
//! effective-write surface.
//!
//! OEM `Setup` is changed by `oem-set` through the `smiflash` SMM driver: it
//! appends a new entry to the variable's NVAR update chain in flash (the
//! firmware's own update mechanism), bypassing the boot-service variable lock,
//! with no external programmer. Needs `smiflash.ko` loaded (smi_port=0xB0).
//!
//! Ported verbatim from biostune's `main.rs` (minus the process entrypoint and
//! the `Tui` variant — the suite owns TUI launch). Every gate/force/dirty check
//! is preserved. Board detection is delegated to `bc250_board` (the lifted
//! Compat/WriteClass/Gate).

use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};

use crate::catalog::Setting;
use crate::efivar::EfiVars;
use crate::flash::Medium;
use crate::{
    apcb, boot, catalog, descriptions, dirty, effect, efivar, flash, gate, nvram, oem, risk, smm,
};
use bc250_board::{Compat, WriteClass};

/// The two global BIOS flags (flattened into `arieltune bios`).
#[derive(Args)]
pub struct Global {
    /// For `apcb` commands: operate on a BIOS image file instead of the live SPI.
    #[arg(long, global = true, value_name = "FILE")]
    pub image: Option<String>,

    /// Read current values from the SPI flash (slower) instead of efivarfs. This
    /// recovers the OEM `Setup` values that efivarfs hides — so OEM settings show
    /// their real current value, not the catalogue default.
    #[arg(long = "from-flash", global = true)]
    pub from_flash: bool,
}

#[derive(Subcommand)]
pub enum Cmd {
    /// List categories and how many settings each holds.
    Categories,
    /// Print settings, optionally filtered by name or category (case-insensitive).
    Dump {
        #[arg(long)]
        filter: Option<String>,
    },
    /// Show one setting: current value, options, default, category.
    Get { name: String },
    /// Change settings as NAME=VAL (value or option label). Dry-run unless --write.
    Set {
        #[arg(required = true, value_name = "NAME=VAL")]
        assignments: Vec<String>,
        /// Actually write to AmdSetup (applies on reboot).
        #[arg(long)]
        write: bool,
        /// Required for caution/brick/one-way settings or an unrecognised BIOS.
        #[arg(long)]
        force: bool,
    },
    /// The APCB enable-bit layer — the *effective* surface. AGESA applies a CBS
    /// setting only if its APCB token's enable bit is on (the value lives in
    /// AmdSetup). Read with `status`; flip a token with `enable`/`disable`.
    Apcb {
        #[command(subcommand)]
        action: ApcbCmd,
    },
    /// Stage a NO-FLASH OEM-Setup change as NAME=VAL, applied at the next boot
    /// (boot-time setup_var; recoverable by NVRAM clear, no SPI rig). Dry-run
    /// unless --arm. After it applies, run `oem-clear`.
    OemStage {
        #[arg(required = true, value_name = "NAME=VAL")]
        assignments: Vec<String>,
        /// Actually stage files + set the one-shot boot entry (then reboot yourself).
        #[arg(long)]
        arm: bool,
        /// Required for known-dangerous settings (e.g. IOMMU can black-screen the BC-250).
        #[arg(long)]
        force: bool,
    },
    /// Tear down any staged one-shot OEM-Setup boot (clear BootNext + entry + files).
    OemClear,
    /// Change an OEM `Setup` value as NAME=VAL via SMM — no flash rig, bypasses the
    /// variable lock (writes the NVAR store directly through the smiflash driver).
    /// Dry-run unless --apply. Needs `smiflash.ko` loaded (smi_port=0xB0).
    OemSet {
        /// One or more NAME=VAL (value or option label), OEM Setup settings only.
        assignments: Vec<String>,
        /// Actually perform the SMM write (persists; applies on reboot).
        #[arg(long)]
        apply: bool,
        /// Required for known-dangerous settings (e.g. IOMMU can black-screen the BC-250).
        #[arg(long)]
        force: bool,
    },
    /// Environment + varstore checks.
    Doctor,
    /// Manage the smiflash SMM driver that powers OEM `Setup` editing.
    Driver {
        #[command(subcommand)]
        action: DriverCmd,
    },
    /// Verify a field actually DOES something, via independent downstream
    /// observables (topology / PCIe link / GPU power+clock / IOMMU / VRAM) —
    /// not a variable read-back. Snapshot before a change, reboot, snapshot
    /// after, then diff: a changed observable means the field is LIVE.
    Effect {
        #[command(subcommand)]
        action: EffectCmd,
    },
}

#[derive(Subcommand)]
pub enum EffectCmd {
    /// Capture an observable fingerprint to a file (--out) or stdout.
    Snapshot {
        #[arg(long, value_name = "FILE")]
        out: Option<String>,
    },
    /// Diff two fingerprints; reports which observables changed (LIVE) or none (INERT).
    Diff { a: String, b: String },
    /// Sample GPU sclk/power/temp for N seconds and report the peak (drive a GPU
    /// load in parallel — this is the dynamic observable for PPT/TDC/EDC).
    Gpu {
        #[arg(long, default_value_t = 15)]
        secs: u32,
    },
}

#[derive(Subcommand)]
pub enum DriverCmd {
    /// Build + install the smiflash module for THIS kernel via DKMS (rebuilds
    /// automatically on kernel upgrades). Needs root, dkms, and kernel headers;
    /// on a BC-250 it builds on the board.
    Build,
    /// Load the smiflash module with the FADT SW-SMI command port (auto-detected).
    Load,
    /// Report whether the driver is loaded/installed + the detected SMI port.
    Status,
    /// Unload the driver.
    Unload,
}

#[derive(Subcommand)]
pub enum ApcbCmd {
    /// List the APCB CBS tokens and whether each is enabled.
    Status,
    /// Turn a CBS token's enable bit ON (id like 0x1501). Dry-run unless --write.
    Enable {
        id: String,
        #[arg(long)]
        write: bool,
        /// Required to flash on an unrecognised BIOS (brick-class path).
        #[arg(long)]
        force: bool,
    },
    /// Turn a CBS token's enable bit OFF. Dry-run unless --write.
    Disable {
        id: String,
        #[arg(long)]
        write: bool,
        /// Required to flash on an unrecognised BIOS (brick-class path).
        #[arg(long)]
        force: bool,
    },
}

/// Dispatch a BIOS CLI subcommand. Builds the value source (efivarfs or the flash
/// NVAR store) from the two globals exactly as biostune did, then dispatches.
pub fn run(global: Global, cmd: Cmd) -> Result<()> {
    let medium = || match &global.image {
        Some(p) => Medium::File(p.into()),
        None => Medium::Live,
    };
    // The current-value source: efivarfs (fast) or the flash NVAR store (slower,
    // but recovers the OEM `Setup` values efivarfs hides).
    let efi = || -> Result<EfiVars> {
        if global.from_flash {
            Ok(EfiVars::from_nvram(&medium().read_image()?))
        } else {
            Ok(EfiVars::read())
        }
    };
    match cmd {
        Cmd::Categories => categories(),
        Cmd::Dump { filter } => dump(filter.as_deref(), &efi()?),
        Cmd::Get { name } => get(&name, &efi()?),
        Cmd::Set {
            assignments,
            write,
            force,
        } => set(&assignments, write, force),
        Cmd::Apcb { action } => apcb_cmd(&medium(), action),
        Cmd::OemStage {
            assignments,
            arm,
            force,
        } => oem_stage(&assignments, arm, force),
        Cmd::OemClear => {
            boot::clear(&boot::find_esp()?)?;
            println!("cleared any staged one-shot OEM-Setup boot.");
            Ok(())
        }
        Cmd::OemSet {
            assignments,
            apply,
            force,
        } => oem_set_cmd(&assignments, apply, force),
        Cmd::Doctor => doctor(),
        Cmd::Driver { action } => driver_cmd(action),
        Cmd::Effect { action } => match action {
            EffectCmd::Snapshot { out } => effect::snapshot(out.as_deref()),
            EffectCmd::Diff { a, b } => effect::diff(&a, &b),
            EffectCmd::Gpu { secs } => effect::gpu(secs),
        },
    }
}

/// The BIOS-compatibility verdict for a *live* write, or `None` for an off-board
/// (`--image FILE`) edit that never touches a real board.
fn live_compat(m: &Medium) -> Option<Compat> {
    match m {
        Medium::Live => Some(bc250_board::detect_bios().compat),
        Medium::File(_) => None,
    }
}

fn parse_id(s: &str) -> Result<u16> {
    let s = s.trim();
    if let Some(h) = s.strip_prefix("0x") {
        Ok(u16::from_str_radix(h, 16)?)
    } else {
        Ok(s.parse::<u16>()?)
    }
}

fn apcb_cmd(m: &Medium, action: ApcbCmd) -> Result<()> {
    let image = m.read_image()?;
    let addr = apcb::locate(&image)?;
    let a = apcb::extract(&image, addr)?;
    match action {
        ApcbCmd::Status => {
            let h = a.header();
            let toks = a.cbsg_tokens();
            let on = toks.iter().filter(|t| t.enabled()).count();
            println!(
                "APCB @0x{addr:x}  v0x{:x} header 0x{:x} total 0x{:x} checksum 0x{:02x} [{}]  [{}]",
                h.version,
                h.header_size,
                h.total_size,
                h.checksum,
                if a.verify_checksum() { "ok" } else { "BAD" },
                m.label(),
            );
            for g in a.groups() {
                println!("  group {:<5} {}B body  {}", g.name(), g.body_size, g.label);
            }
            let stripped = a.stripped_groups();
            if !stripped.is_empty() {
                println!(
                    "  stripped: {} (their tokens are cosmetic)",
                    stripped.join(", ")
                );
            }
            println!("\n{} CBS tokens, {on} enabled:\n", toks.len());
            for t in &toks {
                println!(
                    "  token 0x{:04x}  {}  (flag 0x{:02x}, value 0x{:02x}) @ CBSG+0x{:x}",
                    t.id(),
                    if t.enabled() { "ENABLED " } else { "disabled" },
                    t.flag,
                    t.value,
                    t.offset
                );
            }
            Ok(())
        }
        ApcbCmd::Enable { id, write, force } => {
            apcb_set_enable(m, &image, addr, a, &id, true, write, force)
        }
        ApcbCmd::Disable { id, write, force } => {
            apcb_set_enable(m, &image, addr, a, &id, false, write, force)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn apcb_set_enable(
    m: &Medium,
    image: &[u8],
    addr: usize,
    mut a: apcb::Apcb,
    id: &str,
    enable: bool,
    write: bool,
    force: bool,
) -> Result<()> {
    let id = parse_id(id)?;
    let tok = a
        .cbs_token(id)
        .with_context(|| format!("CBS token 0x{id:04x} not found in this APCB"))?;
    println!(
        "token 0x{id:04x} @ CBSG+0x{:x}: {} -> {}",
        tok.offset,
        if tok.enabled() { "enabled" } else { "disabled" },
        if enable { "enabled" } else { "disabled" }
    );
    if tok.enabled() == enable {
        println!("already in the requested state — nothing to do.");
        return Ok(());
    }
    a.set_token_enable(id, enable)?;
    if !write {
        println!("\ndry-run — nothing flashed. Re-run with --write (then reboot).");
        return Ok(());
    }
    // Brick-class path (flashrom): gate on BIOS-version compatibility for a live
    // board. Off-board (`--image`) edits skip the gate.
    gate::preflight_flash(live_compat(m), force)?;
    if let Some(bk) = flash::backup_before(m, image)? {
        eprintln!("backed up image to {}", bk.display());
    }
    let mut new = image.to_vec();
    apcb::splice(&mut new, addr, &a)?;
    m.write_image(image, &new)?;
    println!("\nflashed. Reboot to apply (ABL3 re-reads the APCB at cold boot).");
    println!("POC-first: validate ONE token end-to-end before trusting this broadly.");
    Ok(())
}

fn value_str(efi: &EfiVars, s: &Setting) -> String {
    match efi.value(s) {
        Some(v) => format!("{} (0x{v:x})", s.label_for(v)),
        None => "—".into(),
    }
}

fn categories() -> Result<()> {
    let settings = catalog::load();
    let cats = catalog::by_category(&settings);
    let comps = catalog::by_compartment(&settings, &cats);
    println!(
        "{} settings, {} categories, {} compartments:\n",
        settings.len(),
        cats.len(),
        comps.len()
    );
    for (comp, cat_idxs) in &comps {
        let total: usize = cat_idxs.iter().map(|&ci| cats[ci].1.len()).sum();
        println!("{comp}  ({total})");
        for &ci in cat_idxs {
            let (name, idx) = &cats[ci];
            println!("  {:>4}  {}", idx.len(), name);
        }
    }
    Ok(())
}

fn dump(filter: Option<&str>, efi: &EfiVars) -> Result<()> {
    let settings = catalog::load();
    let f = filter.map(|s| s.to_lowercase());
    let mut n = 0;
    for s in &settings {
        if let Some(f) = &f {
            if !s.name.to_lowercase().contains(f) && !s.category.to_lowercase().contains(f) {
                continue;
            }
        }
        println!(
            "  [{}] {:<40} = {:<22} {}",
            s.category,
            s.name,
            value_str(efi, s),
            s.value_space()
        );
        n += 1;
    }
    eprintln!(
        "\n{n} setting(s){}",
        filter
            .map(|f| format!(" matching '{f}'"))
            .unwrap_or_default()
    );
    Ok(())
}

fn get(name: &str, efi: &EfiVars) -> Result<()> {
    let settings = catalog::load();
    let matches: Vec<&Setting> = settings
        .iter()
        .filter(|s| {
            s.name.eq_ignore_ascii_case(name)
                || s.name.to_lowercase().contains(&name.to_lowercase())
        })
        .collect();
    if matches.is_empty() {
        anyhow::bail!("no setting matching '{name}'");
    }
    for s in matches {
        let (desc, _) = descriptions::describe(s);
        println!("{}  [{}]", s.name, s.category);
        println!("  {desc}");
        println!("  current : {}", value_str(efi, s));
        if let Some(d) = &s.default {
            println!("  default : {d}");
        }
        println!("  values  : {}", s.value_space());
        println!(
            "  storage : {} +0x{:x} ({}-bit)",
            s.varstore, s.offset, s.bits
        );
        println!();
    }
    Ok(())
}

/// Resolve a NAME=VAL OEM-Setup assignment to a validated (setting, value).
fn resolve_oem(name: &str, val: &str) -> Result<(Setting, u8)> {
    let settings = catalog::load();
    let s = settings
        .iter()
        .find(|s| s.name.eq_ignore_ascii_case(name) && s.varstore == "Setup")
        .or_else(|| settings.iter().find(|s| s.name.eq_ignore_ascii_case(name)))
        .with_context(|| format!("unknown setting: {name}"))?;
    if s.varstore != "Setup" {
        bail!(
            "{}: that's a CBS setting — use `set` (AmdSetup); oem-set is for OEM Setup",
            s.name
        );
    }
    if s.width() != 1 {
        bail!("{}: only 1-byte OEM settings are supported", s.name);
    }
    let v = resolve_value(s, val)?;
    if let Some([lo, hi]) = s.range {
        if v < lo || v > hi {
            bail!("{}: {v} out of range {lo}..={hi}", s.name);
        }
    } else if !s.options.is_empty() && !s.options.iter().any(|(o, _)| *o == v) {
        bail!("{}: {v} not a valid option ({})", s.name, s.value_space());
    }
    Ok((s.clone(), v as u8))
}

/// Change OEM `Setup` values via SMM (NVAR-append) — no flash rig, bypasses the
/// boot-service variable lock. Dry-run unless `apply`.
fn oem_set_cmd(assignments: &[String], apply: bool, force: bool) -> Result<()> {
    if assignments.is_empty() {
        bail!("nothing to set — give one or more NAME=VAL");
    }
    let mut plan = Vec::new();
    for a in assignments {
        let (name, val) = a.split_once('=').context("expected NAME=VAL")?;
        plan.push(resolve_oem(name.trim(), val)?);
    }

    let _ = smm::load(); // best-effort auto-load if the module is installed
    let smm = smm::Smm::open().context(
        "OEM-set needs the smiflash SMM driver — run `arieltune bios driver build` \
         (or `arieltune bios driver load`)",
    )?;

    for (s, v) in &plan {
        let (cur, _) = oem::oem_read(&smm, "Setup", nvram::SETUP_SIZE, s.offset)
            .with_context(|| format!("reading current {}", s.name))?;
        let risk = risk::assess(s);
        let tag = if risk.risk.tag().is_empty() {
            String::new()
        } else {
            format!("   [{}]", risk.risk.tag())
        };
        println!(
            "  {}: {} (0x{:02x}) -> {} (0x{:02x})   [Setup +0x{:x}]{tag}",
            s.name,
            s.label_for(cur as u32),
            cur,
            s.label_for(*v as u32),
            v,
            s.offset,
        );
    }

    if !apply {
        println!(
            "\ndry-run — nothing written. Re-run with --apply to perform the SMM write \
             (persists; applies on reboot). Recovery if needed: NVRAM clear or reflash the backup."
        );
        return Ok(());
    }

    // OEM-set is a flash-class SMM write at a catalogue offset — gate on the
    // BIOS version (offsets must match) and on each field's risk.
    let chosen: Vec<&Setting> = plan.iter().map(|(s, _)| s).collect();
    gate::preflight(
        Some(bc250_board::detect_bios().compat),
        WriteClass::FlashClass,
        &chosen,
        force,
    )?;

    println!(
        "\napplying via SMM — all {} OEM field(s) in ONE NVAR entry...",
        plan.len()
    );
    let edits: Vec<(usize, u8)> = plan.iter().map(|(s, v)| (s.offset, *v)).collect();
    match oem::oem_set(&smm, "Setup", nvram::SETUP_SIZE, &edits).context("OEM batch set")? {
        Some(free) => {
            dirty::mark();
            for (s, v) in &plan {
                println!("  {} -> {}", s.name, s.label_for(*v as u32));
            }
            println!("committed in one NVAR entry @0x{free:x}.");
            println!(
                "\nREBOOT NOW to apply. The store is pending-reboot: don't change other settings\n\
                 until you reboot (arieltune blocks variable writes meanwhile, to protect the\n\
                 store). The reboot applies the changes and clears this state."
            );
        }
        None => println!("\nno changes applied (all already at target)."),
    }
    Ok(())
}

/// Manage the smiflash SMM driver (powers OEM `Setup` editing).
fn driver_cmd(action: DriverCmd) -> Result<()> {
    match action {
        DriverCmd::Build => {
            smm::build().context("building the smiflash driver via DKMS")?;
            println!("\nsmiflash built + installed. Load it: sudo arieltune bios driver load");
            Ok(())
        }
        DriverCmd::Status => {
            println!(
                "smiflash driver : {}",
                if smm::Smm::available() {
                    "loaded [ok]"
                } else {
                    "not loaded"
                }
            );
            match smm::smi_cmd_port() {
                Ok(p) => println!("FADT SMI_CMD    : 0x{p:x}"),
                Err(e) => println!("FADT SMI_CMD    : unknown ({e})"),
            }
            println!("module          : {}", smm::install_state());
            Ok(())
        }
        DriverCmd::Load => {
            smm::load().context("loading smiflash driver")?;
            let p = smm::smi_cmd_port().unwrap_or(0);
            println!("smiflash loaded (smi_port=0x{p:x}) — OEM editing is now available.");
            Ok(())
        }
        DriverCmd::Unload => {
            smm::unload().context("unloading smiflash driver")?;
            println!("smiflash unloaded.");
            Ok(())
        }
    }
}

/// Resolve a NAME=VAL value against a setting: accept a number, a hex (0x..), or
/// an option label.
fn resolve_value(s: &Setting, raw: &str) -> Result<u32> {
    let raw = raw.trim();
    if let Some(h) = raw.strip_prefix("0x") {
        return Ok(u32::from_str_radix(h, 16)?);
    }
    if let Ok(n) = raw.parse::<u32>() {
        return Ok(n);
    }
    s.options
        .iter()
        .find(|(_, l)| l.eq_ignore_ascii_case(raw))
        .map(|(v, _)| *v)
        .with_context(|| {
            format!(
                "{}: '{raw}' is not a number or a known option ({})",
                s.name,
                s.value_space()
            )
        })
}

fn set(assignments: &[String], write: bool, force: bool) -> Result<()> {
    if write && dirty::is_dirty() {
        anyhow::bail!("{}", dirty::why());
    }
    let settings = catalog::load();
    let mut edits = Vec::new();
    let mut chosen: Vec<&Setting> = Vec::new();
    for a in assignments {
        let (name, val) = a.split_once('=').context("expected NAME=VAL")?;
        let name = name.trim();
        // prefer an editable (AmdSetup) match
        let s = settings
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(name) && efivar::is_writable(&s.varstore))
            .or_else(|| settings.iter().find(|s| s.name.eq_ignore_ascii_case(name)))
            .with_context(|| format!("unknown setting: {name}"))?;
        if !efivar::is_writable(&s.varstore) {
            anyhow::bail!(
                "{}: OEM/boot-service var — use `oem-set {}=VAL` (SMM path), not `set`",
                s.name,
                s.name,
            );
        }
        let v = resolve_value(s, val)?;
        // validate
        if let Some([lo, hi]) = s.range {
            if v < lo || v > hi {
                anyhow::bail!("{}: {v} out of range {lo}..={hi}", s.name);
            }
        } else if !s.options.is_empty() && !s.options.iter().any(|(o, _)| *o == v) {
            anyhow::bail!("{}: {v} not a valid option ({})", s.name, s.value_space());
        }
        let a = risk::assess(s);
        let tag = if a.risk.tag().is_empty() {
            String::new()
        } else {
            format!("   [{}]", a.risk.tag())
        };
        println!("  {} -> {}{tag}", s.name, s.label_for(v));
        edits.push(efivar::Edit {
            offset: s.offset,
            width: s.width(),
            value: v,
        });
        chosen.push(s);
    }
    if !write {
        println!("\ndry-run — nothing written. Re-run with --write (then reboot to apply).");
        return Ok(());
    }
    // AmdSetup is a firmware-native SetVariable (NVRAM-clear recoverable), but a
    // wrong-BIOS offset or a brick/one-way setting still needs the gate.
    gate::preflight(
        Some(bc250_board::detect_bios().compat),
        WriteClass::EfiVar,
        &chosen,
        force,
    )?;
    let backup = efivar::write_amdsetup(&edits)?;
    println!(
        "\nwrote {} change(s); backup at {}",
        edits.len(),
        backup.display()
    );
    println!("reboot to apply (note: AGESA applies the APCB copy for some settings, so a write can be cosmetic).");
    Ok(())
}

fn oem_stage(assignments: &[String], arm: bool, force: bool) -> Result<()> {
    eprintln!(
        "note: `oem-stage` (boot-shell setup_var) is SUPERSEDED and usually INEFFECTIVE — the\n\
         \x20     firmware locks OEM `Setup` at EndOfDxe before the boot shell runs (verified on\n\
         \x20     P3.00). Use `oem-set NAME=VAL --apply` (the SMM path) instead.\n"
    );
    let settings = catalog::load();
    let mut edits = Vec::new();
    let mut chosen: Vec<&Setting> = Vec::new();
    for a in assignments {
        let (name, val) = a.split_once('=').context("expected NAME=VAL")?;
        let name = name.trim();
        let s = settings
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(name) && s.varstore == "Setup")
            .or_else(|| settings.iter().find(|s| s.name.eq_ignore_ascii_case(name)))
            .with_context(|| format!("unknown setting: {name}"))?;
        if s.varstore != "Setup" {
            anyhow::bail!(
                "{}: that's a CBS setting — use `set` (AmdSetup) for it; this path is for OEM Setup",
                s.name
            );
        }
        if s.width() != 1 {
            anyhow::bail!(
                "{}: only 1-byte OEM settings are supported by the boot helper",
                s.name
            );
        }
        let v = resolve_value(s, val)?;
        if let Some([lo, hi]) = s.range {
            if v < lo || v > hi {
                anyhow::bail!("{}: {v} out of range {lo}..={hi}", s.name);
            }
        } else if !s.options.is_empty() && !s.options.iter().any(|(o, _)| *o == v) {
            anyhow::bail!("{}: {v} not a valid option ({})", s.name, s.value_space());
        }
        let risk = risk::assess(s);
        let tag = if risk.risk.tag().is_empty() {
            String::new()
        } else {
            format!("   [{}]", risk.risk.tag())
        };
        println!(
            "  {} -> {} (Setup +0x{:x} = 0x{:02x}){tag}",
            s.name,
            s.label_for(v),
            s.offset,
            v
        );
        edits.push(boot::SetupEdit {
            name: s.name.clone(),
            offset: s.offset,
            value: v as u8,
        });
        chosen.push(s);
    }

    let nsh = boot::startup_nsh(&edits);
    println!("\n--- startup.nsh (runs at next boot, then resets back to Linux) ---\n{nsh}");

    if !arm {
        println!(
            "dry-run — nothing staged. Re-run with --arm to stage files + set the one-shot boot."
        );
        return Ok(());
    }
    gate::preflight(
        Some(bc250_board::detect_bios().compat),
        WriteClass::FlashClass,
        &chosen,
        force,
    )?;
    let esp = boot::find_esp()?;
    boot::stage(&esp, &edits)?;
    let num = boot::arm(&esp)?;
    println!(
        "\narmed one-shot boot Boot{num}. REBOOT to apply — it writes the setting at boot and resets \
         back to Linux (no flash). If it doesn't return, power-cycle the board (BootNext is one-shot, \
         so it falls back to normal boot). After it applies, run `arieltune bios oem-clear`."
    );
    Ok(())
}

fn doctor() -> Result<()> {
    let ok = |b: bool| if b { "[ok]" } else { "[fail]" };
    println!("arieltune bios doctor\n");

    let settings = catalog::load();
    let cats = catalog::by_category(&settings);
    println!(
        "[ok] catalogue: {} settings, {} categories",
        settings.len(),
        cats.len()
    );

    let b = bc250_board::detect_bios();
    let bc250 = b.compat != Compat::NotBc250;
    println!(
        "{} board: {}",
        ok(bc250),
        if b.product.is_empty() {
            "unknown"
        } else {
            &b.product
        }
    );
    println!(
        "{} BIOS: {} ({}) — {}",
        match b.compat {
            Compat::Verified => "[ok]",
            Compat::ProbableAsrock => "[ok]",
            _ => "[!!]",
        },
        if b.bios_version.is_empty() {
            "unknown"
        } else {
            &b.bios_version
        },
        if b.bios_vendor.is_empty() {
            "?"
        } else {
            &b.bios_vendor
        },
        b.compat.label(),
    );
    if matches!(b.compat, Compat::UnknownBios | Compat::NotBc250) {
        println!(
            "     writes are gated: catalogue offsets are only proven on ASRock BC-250 \
             P2/P3/P5 firmware."
        );
    }

    let efidir = std::path::Path::new("/sys/firmware/efi/efivars").is_dir();
    println!("{} efivarfs mounted", ok(efidir));

    let efi = EfiVars::read();
    println!(
        "{} AmdSetup readable (AMD CBS values)",
        ok(efi.has("AmdSetup"))
    );
    if efi.has("Setup") {
        println!("[ok] Setup readable (OEM values)");
    } else {
        println!("     note: Setup not exposed at runtime (boot-service-only on the BC-250) —");
        println!("           load smiflash.ko (below) for live OEM values + editing.");
    }

    // SMM OEM-edit capability (the smiflash driver + a live read through it)
    let smm_ok = smm::Smm::available();
    println!(
        "{} smiflash SMM driver (OEM `oem-set` editing, no flash rig)",
        ok(smm_ok)
    );
    if smm_ok {
        match smm::Smm::open().and_then(|s| {
            oem::oem_read(&s, "Setup", nvram::SETUP_SIZE, 0xDA).map_err(std::io::Error::other)
        }) {
            Ok((v, _)) => println!(
                "     live OEM read OK — Above 4G Decoding = {}",
                if v == 0 { "Disabled" } else { "Enabled" }
            ),
            Err(e) => println!("     [warn] driver present but SMM read failed: {e}"),
        }
    } else {
        println!("     build: arieltune bios driver build; load: arieltune bios driver load");
    }

    if dirty::is_dirty() {
        println!("[!!] NVRAM store is SMM-dirty — an OEM edit is pending a reboot.");
        println!("     REBOOT before changing any other setting (variable writes are blocked).");
    }

    // sample a known setting to prove decode works
    if let Some(s) = settings
        .iter()
        .find(|s| s.varstore == "AmdSetup" && !s.options.is_empty())
    {
        println!("     sample: {} = {}", s.name, value_str(&efi, s));
    }
    Ok(())
}
