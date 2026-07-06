// SPDX-License-Identifier: GPL-2.0-only
//! The MEM CLI: the non-TUI surface of the BC-250 GDDR6 memory-timing tuner.
//!
//! Ported verbatim from memtune's `main.rs` (minus the process entrypoint and the
//! `Tui` variant — the suite owns TUI launch). Every CMOS/timing safety path is
//! preserved: writes stage into CMOS and take effect on the NEXT boot (ABL trains
//! them); a re-stamp re-arms training without changing timings; dry-run unless
//! `--write`; auto-backup before a write.
//!
//! It shows the knobs, explains them, lets you change them by hand and measure
//! the result, and snaps back to a known-good config. No autotuning: tuning is
//! manual, and the community shares what trains and scores on their boards.

use anyhow::{bail, Context, Result};
use clap::Subcommand;

use crate::config::{signature_name, MemConf, FIELDS, SIG_ABL, SIG_LINUX_TOOL};
use crate::{bench, cmos, config, sysmem, tune, umc};

#[derive(Subcommand)]
pub enum Cmd {
    /// Print the current CMOS memory config.
    Dump,
    /// Stage the recommended tune (the known-good 1750 MHz config). Dry-run unless --write.
    Recommended {
        /// Actually write to CMOS (applies on next reboot).
        #[arg(long)]
        write: bool,
    },
    /// Stage individual timings as KEY=VAL. Dry-run unless --write.
    Set {
        #[arg(required = true, value_name = "KEY=VAL")]
        assignments: Vec<String>,
        #[arg(long)]
        write: bool,
    },
    /// Measure current memory bandwidth + latency (Vulkan).
    Bench,
    /// Explain how the bandwidth + latency benchmark works.
    Explain,
    /// Save the current CMOS config to a file (or an auto-named backup).
    Backup {
        #[arg(value_name = "FILE")]
        path: Option<String>,
    },
    /// Restore a CMOS config from a backup file. Dry-run unless --write.
    Restore {
        #[arg(value_name = "FILE")]
        path: String,
        #[arg(long)]
        write: bool,
    },
    /// Preflight checks: hardware, /dev/port, Vulkan, current config, serial.
    Doctor,
    /// Read the live UMC (memory controller) registers (advanced; needs bc250_smu).
    Umc,
    /// System-RAM integrity test (CPU-side) — catches corruption the GPU bench misses.
    Memtest {
        /// Size of the test slab in MiB (default 2048).
        #[arg(long, default_value_t = 2048)]
        mb: usize,
        /// Minimum seconds to run (more = more stress; default 30).
        #[arg(long, default_value_t = 30)]
        secs: u64,
    },
    // NOTE: memtune's `uninstall` verb is intentionally DROPPED here — uninstall
    // is suite-level, see M7.
}

/// Dispatch a MEM CLI subcommand (memtune's `run` body for the kept commands).
pub fn run(cmd: Cmd) -> Result<()> {
    // Sweep orphaned state files from removed features (best-effort).
    tune::cleanup_stale();
    match cmd {
        Cmd::Dump => dump(),
        Cmd::Recommended { write } => apply_recommended_cmd(write),
        Cmd::Set { assignments, write } => apply_set(&assignments, write),
        Cmd::Bench => bench_cmd(),
        Cmd::Explain => {
            print!("{}", EXPLAIN);
            Ok(())
        }
        Cmd::Backup { path } => backup_cmd(path),
        Cmd::Restore { path, write } => restore_cmd(&path, write),
        Cmd::Doctor => doctor(),
        Cmd::Umc => umc::dump(),
        Cmd::Memtest { mb, secs } => memtest_cmd(mb, secs),
    }
}

fn memtest_cmd(mb: usize, secs: u64) -> Result<()> {
    println!("system-RAM integrity test: {mb} MiB, >={secs}s (CPU-side, catches what the GPU bench misses)...");
    let r = sysmem::test(mb, secs)?;
    let mib = r.bytes / (1024 * 1024);
    let gibps = (r.bytes as f64 * r.passes as f64 * 3.0) / r.secs / 1e9; // ~3 buffer passes/iter
    if r.errors == 0 {
        println!(
            "integrity    : OK — 0 errors over {} pass(es) of {mib} MiB in {:.0}s (~{gibps:.0} GB/s)",
            r.passes, r.secs
        );
    } else {
        println!(
            "integrity    : {} ERRORS over {} pass(es) of {mib} MiB — this config is UNSTABLE (corrupts system RAM)",
            r.errors, r.passes
        );
    }
    Ok(())
}

fn dump() -> Result<()> {
    let conf = MemConf::from_bytes(cmos::read_config()?);
    let sig = conf.signature();
    println!("Signature  : 0x{:08X} — {}", sig, signature_name(sig));
    println!("             {}", config::signature_help(sig));
    let ck = conf.get("Checksum").unwrap_or(0) as u16;
    let ck_ok = if conf.checksum_valid() {
        "valid"
    } else {
        "INVALID"
    };
    println!("Checksum   : 0x{ck:04X} ({ck_ok})");
    let clock = conf.get("ClockSpeed").unwrap_or(0);
    let bw = clock as f64 * 8.0 / 1000.0 * 256.0 / 8.0;
    println!("ClockSpeed : {clock} MHz  (~{bw:.0} GB/s @256-bit)\n");
    for fl in FIELDS {
        if matches!(fl.name, "Signature" | "Checksum" | "ClockSpeed") {
            continue;
        }
        let v = conf.get_field(fl);
        let marker = if fl.name == "UMA_SIZE" {
            " MB"
        } else if v == fl.lo {
            " (minimum)"
        } else if v == fl.hi {
            " (maximum)"
        } else {
            ""
        };
        println!("  {:<12}: {:>5}{}", fl.name, v, marker);
    }
    Ok(())
}

fn apply_recommended_cmd(write: bool) -> Result<()> {
    let current = MemConf::from_bytes(cmos::read_config()?);
    let mut draft = current.clone();
    draft.apply_recommended();
    commit(current, draft, write)
}

fn apply_set(assignments: &[String], write: bool) -> Result<()> {
    let current = MemConf::from_bytes(cmos::read_config()?);
    let mut draft = current.clone();
    for a in assignments {
        let (k, v) = a
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("bad KEY=VAL: '{a}'"))?;
        let f = MemConf::field(k).ok_or_else(|| anyhow::anyhow!("unknown field '{k}'"))?;
        if matches!(f.name, "Signature" | "Checksum") {
            bail!("'{k}' is managed automatically, not settable");
        }
        let val: u32 = v
            .parse()
            .map_err(|_| anyhow::anyhow!("bad value '{v}' for {k}"))?;
        if val < f.lo || val > f.hi {
            bail!("{k}={val} out of range {}..{}", f.lo, f.hi);
        }
        draft.set(k, val);
    }
    commit(current, draft, write)
}

/// Show the diff and, if `write`, stamp + flush to CMOS.
fn commit(current: MemConf, mut draft: MemConf, write: bool) -> Result<()> {
    let mut changed = 0;
    for fl in FIELDS {
        if matches!(fl.name, "Signature" | "Checksum") {
            continue;
        }
        let (a, b) = (current.get_field(fl), draft.get_field(fl));
        if a != b {
            println!("  {:<12}: {:>5} -> {:>5}", fl.name, a, b);
            changed += 1;
        }
    }
    // Even with no timing changes, re-writing is meaningful when the stored
    // signature isn't already applied ($ABL) or pending (LINUX_TOOL): re-stamping
    // tells ABL to (re)train the config on next boot and mark it $ABL. This is how
    // you clear a CMOS_BAD/WDT_FIRED state without changing any timings.
    let sig = current.signature();
    let needs_restamp = sig != SIG_ABL && sig != SIG_LINUX_TOOL;
    if changed == 0 && !needs_restamp {
        println!("no changes (signature already {}).", signature_name(sig));
        return Ok(());
    }
    if changed == 0 {
        println!(
            "timings already match; signature is {} — re-stamping so ABL trains it on reboot.",
            signature_name(sig)
        );
    }
    if !write {
        let what = if changed == 0 {
            "re-stamp the signature".to_string()
        } else {
            format!("{changed} change(s)")
        };
        println!("\n{what} — dry run. Re-run with --write to commit (applies on reboot).");
        return Ok(());
    }
    match cmos::auto_backup() {
        Ok(p) => println!("backed up current config -> {}", p.display()),
        Err(e) => eprintln!("(auto-backup failed: {e} — continuing)"),
    }
    draft.stamp(SIG_LINUX_TOOL);
    cmos::write_config(&draft.buf)?;
    println!("\nWRITTEN. Reboot to apply; then `dump` and check the signature ($ABL = trained, CMOS_BAD = rejected).");
    Ok(())
}

const EXPLAIN: &str = "\
arieltune mem — how the benchmark works

BANDWIDTH (GB/s)
  A Vulkan compute kernel does a grid-stride READ over a large device-local
  GDDR6 buffer (256 MiB), repeated many times. The buffer is far bigger than the
  GPU's L2, so re-reads hit GDDR6 — this measures real memory read bandwidth,
  not cache. Only one lane writes a result, so the traffic is ~pure read, and we
  take the best (fastest) dispatch and compute:

      GB/s = bytes_read / seconds        (bytes_read = buffer_size x iterations)

  The 'theoretical' figure is the bus ceiling: 256-bit x (clock x 8) / 8
  (e.g. 1750 MHz -> 14.0 Gbps/pin -> ~448 GB/s). Measured is typically ~97% of
  it (real-world efficiency).

WARMUP (before every metric)
  The BC-250's DPM governor (the userspace dpm-daemon) clocks the GPU by FENCE
  RATE, not load: it only climbs to its top step (2230 MHz) at >=80 submitted
  jobs/sec. A few big bench dispatches peg the memory bus but submit slowly, so
  the governor would hold the core at the mid step (1500 MHz) and every metric
  would read a clock low. So we first run a throttled stream of dispatches
  (~200/sec, like a real GPU workload) until the live clock confirms it reached
  the top step — then bandwidth, random, and latency are all measured there. We
  never force the clock (forcing the absolute max overheats this board); the
  daemon keeps full thermal authority and we just give it the signal to climb.

LATENCY (ns/access)
  A single GPU thread pointer-chases a random permutation: idx = next[idx], for
  ~1,000,000 dependent hops over an 8 MiB chain. Each read depends on the
  previous one and the order is random, so the prefetcher can't hide it — the
  time per hop is the true dependent-access latency:

      ns/access = seconds / hops

RANDOM (GB/s) and INTEGRITY are also measured: random is a scattered read (the
timing-sensitive number — it's what responds to a tweak), and integrity writes a
pattern across 768 MiB and reads it back (0 errors = stable). Tune for random +
latency; a config with integrity errors is not stable, don't keep it.

All run via Vulkan compute (RADV) — no external tools.
";

fn bench_cmd() -> Result<()> {
    let conf = MemConf::from_bytes(cmos::read_config()?);
    let clock = conf.get("ClockSpeed").unwrap_or(0);
    let est = clock as f64 * 8.0 / 1000.0 * 256.0 / 8.0;
    println!("running Vulkan memory bench (bandwidth + random + latency + integrity)...");
    let r = bench::run()?;
    println!("ClockSpeed   : {clock} MHz  (theoretical ~{est:.0} GB/s @256-bit)");
    println!(
        "bandwidth    : {:.1} GB/s  ({:.0}% of theoretical, sequential read)",
        r.bandwidth_gbps,
        r.bandwidth_gbps / est * 100.0
    );
    if r.random_gbps > 0.0 {
        println!(
            "random       : {:.1} GB/s  (scattered read — the timing-sensitive number)",
            r.random_gbps
        );
    }
    if r.latency_ns > 0.0 {
        println!("latency      : {:.0} ns / access", r.latency_ns);
    }
    if r.stability_bytes > 0 {
        let mib = r.stability_bytes / (1024 * 1024);
        if r.stability_errors == 0 {
            println!("integrity    : OK — 0 errors over {mib} MiB written + verified");
        } else {
            println!(
                "integrity    : {} ERRORS over {mib} MiB — this config is UNSTABLE",
                r.stability_errors
            );
        }
    }
    Ok(())
}

fn backup_cmd(path: Option<String>) -> Result<()> {
    let conf = MemConf::from_bytes(cmos::read_config()?);
    match path {
        Some(p) => {
            std::fs::write(&p, conf.to_hex()).with_context(|| format!("write {p}"))?;
            println!("saved current config -> {p}");
        }
        None => {
            let p = cmos::auto_backup()?;
            println!("saved current config -> {}", p.display());
        }
    }
    Ok(())
}

fn restore_cmd(path: &str, write: bool) -> Result<()> {
    let hex = std::fs::read_to_string(path).with_context(|| format!("read {path}"))?;
    let restored = MemConf::from_hex(&hex)
        .ok_or_else(|| anyhow::anyhow!("{path} is not a valid 28-byte hex config"))?;
    let current = MemConf::from_bytes(cmos::read_config()?);
    commit(current, restored, write)
}

fn is_bc250() -> bool {
    std::fs::read_dir("/sys/bus/pci/devices")
        .map(|rd| {
            rd.flatten().any(|e| {
                std::fs::read_to_string(e.path().join("device"))
                    .map(|s| s.trim() == "0x13fe")
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

fn doctor() -> Result<()> {
    println!("arieltune mem doctor — preflight checks\n");
    let mut hard_fail = false;

    match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/port")
    {
        Ok(_) => println!("  [ok]   /dev/port accessible (root + CONFIG_DEVPORT)"),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            println!("  [FAIL] /dev/port: permission denied — run with sudo");
            hard_fail = true;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!("  [FAIL] /dev/port missing — kernel needs CONFIG_DEVPORT");
            hard_fail = true;
        }
        Err(e) => {
            println!("  [FAIL] /dev/port: {e}");
            hard_fail = true;
        }
    }

    if is_bc250() {
        println!("  [ok]   BC-250 GPU detected (1002:13FE, gfx1013)");
    } else {
        println!("  [warn] no BC-250 (1002:13FE) found — this tool is BC-250-specific");
    }

    if bench::vulkan_available() {
        println!("  [ok]   Vulkan device available (built-in benchmark)");
    } else {
        println!("  [FAIL] no Vulkan device — can't run the benchmark");
        hard_fail = true;
    }

    if !hard_fail {
        if let Ok(buf) = cmos::read_config() {
            let c = MemConf::from_bytes(buf);
            let ck = if c.checksum_valid() {
                "valid"
            } else {
                "INVALID"
            };
            let sig = c.signature();
            let tag = if config::signature_ok(sig) {
                "[ok]  "
            } else {
                "[info]"
            };
            println!(
                "  {tag} current: {} MHz, signature {}, checksum {ck}",
                c.get("ClockSpeed").unwrap_or(0),
                signature_name(sig)
            );
            println!("         {}", config::signature_help(sig));
        }
    }

    let cmdline = std::fs::read_to_string("/proc/cmdline").unwrap_or_default();
    if cmdline.contains("console=ttyS") {
        println!("  [ok]   serial console configured (console=ttyS in cmdline)");
    } else {
        println!("  [info] no serial console (console=ttyS) — handy for watching a risky reboot");
    }

    println!();
    if hard_fail {
        bail!("not ready — fix the [FAIL] items above");
    }
    println!("ready.");
    Ok(())
}
