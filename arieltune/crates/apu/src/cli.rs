// SPDX-License-Identifier: GPL-2.0-only
//! aputune — BC-250 APU liberation + tuner.
//!
//! One Rust tool that owns the whole Cyan Skillfish liberation surface:
//!
//!   patches              report per-patch kernel state (the 12-patch series)
//!   build                build the patch series into the system (kernel rebuild)
//!   liberate             guided build: gate, report, explicit tier choice
//!   cu / cumap           40-CU enable + the harvest map
//!   gpu                  GPU clock / DPM state control
//!   cpu                  CPU overclock/undervolt (boost clock + F/Vid curve)
//!   dpm                  the dynamic-clock daemon (control loop)
//!   tui                  interactive dashboard
//!   doctor               preflight: is this a BC-250, what's patched, what's live
//!
//! Detection is ground truth: aputune probes the booted kernel for each patch's
//! runtime tell, and if the silicon isn't liberated it can build the series in.

use anyhow::{Context, Result};
use clap::Subcommand;

// The SMU q0/q3 mailboxes live in ariel-smu (lifted in M2); the silicon-generic
// probes live in ariel-hal. This CLI USES them (no crate-local copies).
use crate::{
    cpu, cu, curoute, cutest, detect, dpm, gpuctl, kbuild, patches, persist, profile, telemetry,
};
use ariel_smu::ocq3;
use ariel_smu::smu::{self, Smu};

/// The `arieltune apu` subcommand tree (was `aputune`'s, verbatim minus `Tui` --
/// the TUI is the suite shell's APU tab now, not a subcommand). Every clamp /
/// ceiling / refusal / guard is preserved.
#[derive(Subcommand)]
pub enum Cmd {
    /// Report the per-patch state of the liberation series on the booted kernel.
    Patches {
        /// Show the embedded patch body for one id (e.g. 12).
        #[arg(long, value_name = "ID")]
        show: Option<String>,
    },
    /// Print the live CU harvest map.
    Cumap,
    /// 40-CU liberation control.
    Cu {
        #[command(subcommand)]
        action: CuCmd,
    },
    /// GPU clock control + app-driven power (deep-sleep / wake / autosleep).
    Gpu {
        #[command(subcommand)]
        action: GpuCmd,
    },
    /// CPU overclock/undervolt: boost clock + F/Vid curve + temp limits.
    Cpu {
        #[command(subcommand)]
        action: CpuCmd,
    },
    /// Build the liberation series into the system (patched kernel + arm 40-CU).
    Build {
        /// CachyOS PKGBUILD dir (or set APUTUNE_PKGBUILD).
        #[arg(long, value_name = "DIR")]
        pkgbuild: Option<std::path::PathBuf>,
        /// Deploy + install to a remote target (user@host) instead of locally.
        #[arg(long, value_name = "USER@HOST")]
        target: Option<String>,
        /// Actually execute (default: preview the plan only).
        #[arg(long)]
        run: bool,
    },
    /// Guided liberation: BC-250 gate + patch report, then an explicit tier
    /// choice (full 40-CU / tuning-only / inspect-only) before building.
    Liberate {
        /// CachyOS PKGBUILD dir (or set APUTUNE_PKGBUILD).
        #[arg(long, value_name = "DIR")]
        pkgbuild: Option<std::path::PathBuf>,
        /// Deploy + install to a remote target (user@host) instead of locally.
        #[arg(long, value_name = "USER@HOST")]
        target: Option<String>,
        /// Pick the tier without prompting.
        #[arg(long, value_enum, value_name = "TIER")]
        tier: Option<LiberateTier>,
        /// Actually execute the build (default: preview the plan only).
        #[arg(long)]
        run: bool,
    },
    /// Tuning profiles (bundle CU/GPU/CPU; draft-then-apply).
    Profile {
        #[command(subcommand)]
        action: ProfileCmd,
    },
    /// Preflight: BC-250 check, patch state, debugfs availability.
    Doctor {
        /// Print a single machine-readable JSON object and nothing else.
        #[arg(long)]
        json: bool,
        /// Exit non-zero unless this is a BC-250 with the full series live
        /// (the post-reboot / scripted check). Combines with --json.
        #[arg(long)]
        verify: bool,
    },
}

/// Liberation tiers — liberation is not all-or-nothing.
#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum LiberateTier {
    /// Patched kernel + route all 40 CUs (bc250_cc_write_mode=3).
    Full,
    /// Patched kernel with CU routing OFF (bc250_cc_write_mode=0): telemetry +
    /// GPU clock control + CPU OC without the 40-CU route (ROCm OpenCL is
    /// unstable at 40 CU on gfx1013).
    TuningOnly,
    /// No kernel changes; just report patch state (the decline path).
    InspectOnly,
}

#[derive(Subcommand)]
pub enum ProfileCmd {
    /// List built-in + custom profiles.
    List,
    /// Show one profile and what applying it would do.
    Show { name: String },
    /// Apply a profile. Dry-run (prints the plan) unless --write.
    Apply {
        name: String,
        #[arg(long)]
        write: bool,
    },
    /// Delete a custom profile.
    Delete { name: String },
}

#[derive(Subcommand)]
pub enum CuCmd {
    /// Show CU state (armed?, persisted?, harvest map).
    Status,
    /// Persist the 40-CU liberation (modprobe.d; takes effect next boot).
    Enable,
    /// Remove the persisted liberation (reverts to 24 CU next boot).
    Disable,
    /// Route all 40 CUs now (via umr). Persisted (survives reboot) like the TUI.
    RouteAll,
    /// Route factory dispatch (24 CU) now. Persisted (survives reboot).
    RouteFactory,
    /// Route per-array WGP masks (4 hex masks, e.g. 1f 1f 0f 1f). Persisted.
    /// An empty shader array is REFUSED (compute on it wedges gfx1013) unless
    /// --force-unsafe.
    Route {
        masks: Vec<String>,
        /// UNSAFE override: allow an empty shader array (compute-wedge class).
        #[arg(long)]
        force_unsafe: bool,
    },
    /// Toggle a single WGP, like the TUI's space key. Persisted.
    /// array 0..3 = SE0.SH0 / SE0.SH1 / SE1.SH0 / SE1.SH1; wgp 0..4.
    Toggle {
        array: u32,
        wgp: u32,
        /// UNSAFE override: allow the toggle to empty a shader array.
        #[arg(long)]
        force_unsafe: bool,
    },
    /// Bench a routing config's compute throughput: KAT GFLOPS at a pinned clock
    /// (compute-bound, isolates CU-scaling from memory bandwidth). Non-destructive
    /// — restores the live route after. No masks = bench the current route.
    /// Empty-shader-array shapes are refused (wedge class; no override — the
    /// bench DISPATCHES compute, which is exactly the hang).
    Bench { masks: Vec<String> },
    /// Persist the current live routing as the service profile + boot re-apply.
    RouteSave,
    /// Apply the saved service-profile routing (used by the boot unit). An
    /// unsafe (empty-array) saved profile is refused and factory-24 applied.
    RouteLoad,
    /// Un-arm the boot route re-apply (disable + remove arieltune-route.service).
    /// The saved route.json is kept; `cu route-save` re-arms.
    RouteForget,
    /// Health-test the CUs with a known-answer Vulkan compute test.
    ///
    /// Default: the full 40-CU config (correctness + throughput; safe).
    /// --localize: if a fault is found, route full-40 minus one WGP at a time
    /// to pinpoint the bad WGP (stays inside the safe routing envelope).
    Test {
        /// On a fault, run the subtractive sweep to localize the bad WGP.
        #[arg(long)]
        localize: bool,
        /// Force the subtractive sweep even with no fault (safety validation of
        /// the full-40-minus-one-WGP routing shape on a new kernel).
        #[arg(long, hide = true)]
        probe: bool,
    },
}

#[derive(Subcommand)]
pub enum GpuCmd {
    /// Show GPU power state: setpoints (top/deep) + current clock.
    Status,
    /// Pin the GFX clock to a fixed frequency (MHz).
    Force { mhz: u32 },
    /// Release the GFX clock lock + clear the manual clock (BAPM/autosleep resume).
    Unforce,
    /// Transiently pin the GFX clock (MHz) for the duration of a task, e.g. a
    /// benchmark: stops the GPU power unit and forces the clock, WITHOUT
    /// persisting or changing the power mode. Pair with `gpu unpin`.
    Pin { mhz: u32 },
    /// End a `gpu pin`: release the clock and restart the GPU power unit (which
    /// re-enacts the persisted mode). No persistence change.
    Unpin,
    /// Enact the persisted GPU power mode from power.json: manual pin, governor,
    /// autosleep, or released. Run by aputune-gpu.service (the ONE GPU power
    /// unit); waits out the boot-settle window first.
    ApplyBoot,
    /// Alias for apply-boot, kept so an old aputune-gpu-clock.service on an
    /// un-migrated host still re-applies the persisted mode.
    ApplySavedForce,
    /// Set the GPU voltage (mV) at the high clock via amdgpu overdrive
    /// (pp_od_clk_voltage) — SMU-safe, live, clamped to the driver's OD_RANGE.
    /// Persisted + re-applied at boot. Use for undervolting the GPU rail.
    Vid { mv: u32 },
    /// Reset the GPU voltage to the stock SMU-managed curve (overdrive restore).
    UnforceVid,
    /// Force the active/top clock (your thermal cap). App calls this on work.
    /// Refused while a manual pin (possibly a heat-pin) is persisted, unless
    /// --override-pin.
    Wake {
        /// Override an active persisted manual pin (e.g. a heat-safety pin).
        #[arg(long)]
        override_pin: bool,
    },
    /// Force the deep-sleep clock (saves ~20 W). App calls this when idle.
    /// Refused while a manual pin is persisted, unless --override-pin.
    DeepSleep {
        /// Override an active persisted manual pin.
        #[arg(long)]
        override_pin: bool,
    },
    /// Set the governor's high (top/active) tier clock (MHz). Persisted; applied
    /// live if the governor is running.
    SetTop { mhz: u32 },
    /// Set the governor's mid tier clock (MHz). Persisted; live if running.
    SetMid { mhz: u32 },
    /// Set the governor's idle tier clock (MHz). Persisted; live if running.
    SetIdle { mhz: u32 },
    /// Set the governor's deep-sleep tier clock (MHz). Persisted; live if running.
    SetDeep { mhz: u32 },
    /// Set ONLY the GPU temp cap (C) — the governor demotes to hold it — without
    /// touching the CPU OC. Persisted (re-applied at boot).
    TempCap { c: u32 },
    /// Make the fence-rate governor the persistent auto mode (survives reboot).
    GovernorOn,
    /// Turn the auto governor off: released (native DPM) becomes the persistent
    /// mode.
    GovernorOff,
    /// Make poke-driven autosleep the persistent auto mode (survives reboot).
    /// The workload pokes via `aputune gpu poke` on each unit of work.
    AutosleepOn,
    /// Touch the poke file so `autosleep` keeps the clock at top (call per request).
    Poke,
    /// Poke-driven auto-sleep daemon: force top while poked, release to BAPM when
    /// idle (so non-poking GPU work is never starved).
    Autosleep {
        #[arg(long, default_value_t = 30)]
        idle: u64,
        /// Aggressive: force the deep clock (350) when idle instead of releasing
        /// to BAPM — saves ~20 W but STARVES any GPU user that doesn't poke.
        #[arg(long = "deep-force")]
        deep_force: bool,
    },
    /// Activity-driven 3-state governor (+ deep-sleep): auto GPU DVFS from the
    /// ring fence-rate. Runs until stopped (`gpu apply-boot` runs this
    /// in-process when power.json says governor).
    Governor,
    /// Coarse sysfs performance level: low|high|auto|profile_standard.
    /// Refused while a manual pin is persisted, unless --override-pin.
    Level {
        level: String,
        /// Override an active persisted manual pin.
        #[arg(long)]
        override_pin: bool,
    },
    /// Dump the telemetry node (patch 11).
    Telemetry,
}

#[derive(Subcommand)]
pub enum CpuCmd {
    /// Live CPU state: current Vid, per-core freq, temp limit, P-states.
    Status,
    /// Apply a CPU overclock/undervolt (boost clock + F/Vid curve scale + temps).
    ///
    /// The curve scale (-50..0) undervolts; a boost/scale combo whose predicted
    /// Vid exceeds 1.325 V is refused (raising clock without undervolt is unsafe).
    Set {
        /// Max CPU boost clock (MHz), 2800..4100. Unset = keep current.
        #[arg(long)]
        boost: Option<u32>,
        /// F/Vid curve scale (-50..0); deeper = more undervolt. Unset = keep current.
        #[arg(long, allow_negative_numbers = true)]
        scale: Option<i32>,
        /// CPU temperature limit (C). Unset = keep current.
        #[arg(long = "cpu-temp")]
        cpu_temp: Option<u32>,
        /// GPU temperature limit (C). Unset = keep current.
        #[arg(long = "gpu-temp")]
        gpu_temp: Option<u32>,
    },
    /// Undervolt at the current boost: pick the curve scale for a Vid cap (mV).
    Undervolt {
        /// Target boost clock (MHz).
        #[arg(long, default_value_t = 3500)]
        boost: u32,
        /// Vid cap (mV) to undervolt toward.
        vid_cap: u32,
    },
    /// Force an absolute CPU Vid (mV). Hard-capped at 1.325 V.
    Vid { mv: u32 },
    /// Release a forced CPU Vid back to the SMU curve.
    VidAuto,
    /// Restore CPU to firmware defaults (and drop the saved OC + boot service).
    Restore,
    /// Re-apply the persisted CPU OC (run by arieltune-cpu-oc.service at boot).
    ApplySaved,
    /// Auto-detect: climb the boost clock under torture, undervolting via the
    /// curve to stay under a Vid cap, and report the highest stable point.
    Detect {
        #[arg(long = "frequency", default_value_t = 4100)]
        target: u32,
        #[arg(long = "vid", default_value_t = 1275)]
        vid_cap: u32,
        #[arg(long, default_value_t = 90)]
        temp: u32,
        #[arg(long, default_value_t = 100)]
        step: u32,
        #[arg(long, default_value_t = 8)]
        dwell: u64,
        /// Save the highest stable point as a custom profile of this name.
        #[arg(long, value_name = "NAME")]
        save: Option<String>,
    },
}

/// Run one `arieltune apu` subcommand. The suite bin owns SIGPIPE, `Cli::parse`,
/// and the bare-`arieltune apu` launch-to-tab; a subcommand WITH args lands here.
/// There is no `Tui` / `None` arm: the interactive dashboard is the suite shell's
/// APU tab, reached by bare `arieltune apu`, not a subcommand.
pub fn run(cmd: Cmd) -> Result<()> {
    match cmd {
        Cmd::Patches { show } => cmd_patches(show),
        Cmd::Cumap => cmd_cumap(),
        Cmd::Cu { action } => cmd_cu(action),
        Cmd::Gpu { action } => cmd_gpu(action),
        Cmd::Cpu { action } => cmd_cpu(action),
        Cmd::Build {
            pkgbuild,
            target,
            run,
        } => {
            let mut opts = kbuild::BuildOpts::default();
            if pkgbuild.is_some() {
                opts.pkgbuild_dir = pkgbuild;
            }
            opts.target = target;
            opts.run = run;
            kbuild::build(opts)
        }
        Cmd::Liberate {
            pkgbuild,
            target,
            tier,
            run,
        } => cmd_liberate(pkgbuild, target, tier, run),
        Cmd::Profile { action } => cmd_profile(action),
        Cmd::Doctor { json, verify } => cmd_doctor(json, verify),
    }
}

fn cmd_profile(action: ProfileCmd) -> Result<()> {
    match action {
        ProfileCmd::List => {
            for p in profile::all() {
                println!("{:<14} {}", p.name, p.description);
            }
        }
        ProfileCmd::Show { name } => {
            let p = profile::find(&name).ok_or_else(|| anyhow::anyhow!("no profile '{name}'"))?;
            println!("{} — {}\n", p.name, p.description);
            for l in p.plan_lines() {
                println!("  {l}");
            }
        }
        ProfileCmd::Apply { name, write } => {
            let p = profile::find(&name).ok_or_else(|| anyhow::anyhow!("no profile '{name}'"))?;
            println!("profile '{}' — {}", p.name, p.description);
            for l in p.plan_lines() {
                println!("  {l}");
            }
            if !write {
                println!("\n(dry-run; pass --write to apply)");
                return Ok(());
            }
            // Validate the WHOLE profile before any actuation — a bad CPU point
            // or GPU mode must not leave a half-applied profile behind.
            p.validate()?;
            let oc = ocq3::OcQ3::open_checked().ok();
            // CPU + GPU go through the shared state machine (same paths as the
            // CLI `cpu set` / `gpu force|unforce`, persistence included).
            p.apply(oc.as_ref())?;
            // CU arming last (persisted/reboot action) — after live actuation
            // succeeded, so an apply failure can't leave the modprobe.d changed.
            match p.cu_40 {
                Some(true) => {
                    cu::enable_persist()?;
                    println!("armed 40-CU: {} (reboot to apply)", cu::MODPROBE_CONF);
                }
                Some(false) => {
                    cu::disable_persist()?;
                    println!("disarmed 40-CU (reboot to apply)");
                }
                None => {}
            }
            println!("applied '{}'", p.name);
        }
        ProfileCmd::Delete { name } => {
            profile::delete(&name)?;
            println!("deleted custom profile '{name}'");
        }
    }
    Ok(())
}

fn cmd_cu(action: CuCmd) -> Result<()> {
    match action {
        CuCmd::Status => {
            println!("40-CU liberation:");
            println!("  module param armed: {}", yn(cu::liberation_armed()));
            println!("  modprobe.d persisted: {}", yn(cu::persist_present()));
            match cu::map() {
                Some(m) => print!("{}", cu::render(&m)),
                None => println!("  harvest map: unavailable (amdgpu not queryable)"),
            }
            // Live SPI dispatch route — what the GPU actually runs on RIGHT NOW.
            // Distinct from the harvest map above (driver CU enumeration): a box
            // can be 40-CU "liberated" yet dispatch to only a subset, so report
            // the live route explicitly rather than let the harvest count mislead.
            println!("live routing (SPI dispatch):");
            match curoute::current_masks() {
                Ok(m) => {
                    let s = curoute::shape(&m);
                    println!(
                        "  masks: [{:02x} {:02x} {:02x} {:02x}]  ({:?} WGP/array)",
                        m[0], m[1], m[2], m[3], s.per_array_wgp
                    );
                    // Compute throughput is gated by the TWO smallest arrays
                    // (eff_CU = 4 x (w1+w2)); bench for the real GFLOPS.
                    println!(
                        "  routed: {}/40 CU ({} arrays, {} effective)",
                        s.cu, s.populated, s.effective_cu
                    );
                    for w in s.warnings() {
                        println!("  ! {w}");
                    }
                }
                Err(e) => println!("  unavailable ({e})"),
            }
            println!(
                "  boot route re-apply: {}",
                if persist::route_enabled() {
                    format!("armed ({})", persist::ROUTE_UNIT)
                } else {
                    "not armed".to_string()
                }
            );
        }
        CuCmd::Enable => {
            cu::enable_persist()?;
            println!("persisted: {}", cu::MODPROBE_CONF);
            println!("now run `mkinitcpio -P` (or your initramfs tool) and reboot.");
            if !detect::report()
                .rows
                .iter()
                .any(|r| r.id == "12" && matches!(r.state, detect::State::Present))
            {
                println!(
                    "WARNING: the 40-CU kernel patch (12) is not detected live — \
                     enable will be a no-op until the patched kernel is built in (`arieltune apu build`)."
                );
            }
        }
        CuCmd::Disable => {
            cu::disable_persist()?;
            println!(
                "removed: {} (reverts to 24 CU next boot)",
                cu::MODPROBE_CONF
            );
        }
        CuCmd::RouteAll => {
            curoute::enable_all()?;
            let p = persist_route();
            println!("routed all 40 CUs{}", persist_tag(p));
        }
        CuCmd::RouteFactory => {
            curoute::factory()?;
            let p = persist_route();
            println!("routed factory dispatch (24 CU){}", persist_tag(p));
        }
        CuCmd::Route {
            masks,
            force_unsafe,
        } => {
            let m = parse_masks(&masks)?;
            if force_unsafe {
                eprintln!("WARNING: --force-unsafe — empty-shader-array wedge guard bypassed");
                curoute::apply_forced(m)?;
            } else {
                curoute::apply(m)?;
            }
            let p = persist_route();
            let s = curoute::shape(&m);
            println!(
                "routed {} CUs: [{:02x}, {:02x}, {:02x}, {:02x}]{}",
                s.cu,
                m[0],
                m[1],
                m[2],
                m[3],
                persist_tag(p)
            );
            for w in s.warnings() {
                eprintln!("WARNING: {w}");
            }
        }
        CuCmd::Toggle {
            array,
            wgp,
            force_unsafe,
        } => {
            anyhow::ensure!(
                array < 4,
                "array must be 0..3 (SE0.SH0/SE0.SH1/SE1.SH0/SE1.SH1)"
            );
            anyhow::ensure!(wgp < 5, "wgp must be 0..4");
            if force_unsafe {
                eprintln!("WARNING: --force-unsafe — empty-shader-array wedge guard bypassed");
            }
            let m = curoute::toggle_wgp(array as usize, wgp, force_unsafe)?;
            let p = persist_route();
            let s = curoute::shape(&m);
            println!(
                "toggled SE{}.SH{} W{wgp} -> {} CU routed{}",
                array / 2,
                array % 2,
                s.cu,
                persist_tag(p)
            );
            for w in s.warnings() {
                eprintln!("WARNING: {w}");
            }
        }
        CuCmd::Bench { masks } => {
            let m = if masks.is_empty() {
                curoute::current_masks()?
            } else {
                parse_masks(&masks)?
            };
            let s = curoute::shape(&m);
            for w in s.warnings() {
                eprintln!("WARNING: {w}");
            }
            // bench_masks refuses an empty-array shape (the wedge class).
            let r = cutest::bench_masks(m)?;
            let per = if s.cu > 0 {
                r.gflops / s.cu as f64
            } else {
                0.0
            };
            println!(
                "bench [{:02x} {:02x} {:02x} {:02x}]: {} CU routed  —  {:.0} GFLOPS  ({:.1} /CU)  ({})",
                m[0],
                m[1],
                m[2],
                m[3],
                s.cu,
                r.gflops,
                per,
                if r.hung {
                    "HUNG".to_string()
                } else if r.ok {
                    "correct".to_string()
                } else {
                    format!("{} mismatches", r.mismatches)
                }
            );
        }
        CuCmd::RouteSave => {
            let m = curoute::save_profile()?;
            let armed = persist::enable_route().is_ok();
            println!(
                "saved service routing: {:x?} ({} CU) -> {}{}",
                m,
                curoute::cu_count(&m),
                curoute::PROFILE_PATH,
                if armed {
                    " (boot re-apply armed)"
                } else {
                    " (boot re-apply NOT armed)"
                }
            );
        }
        CuCmd::RouteLoad => {
            let m = curoute::apply_saved()?;
            println!(
                "applied service routing: {:x?} ({} CU)",
                m,
                curoute::cu_count(&m)
            );
        }
        CuCmd::RouteForget => {
            persist::disable_route();
            println!(
                "boot route re-apply disarmed (unit removed); {} kept — `cu route-save` re-arms",
                curoute::PROFILE_PATH
            );
        }
        CuCmd::Test { localize, probe } => cmd_cu_test(localize, probe)?,
    }
    Ok(())
}

fn kat_row(h: &cutest::KatResult) {
    println!(
        "  {:<14} {:>3} CU  {:>7.0} ms  {:>8.0} GFLOPS  {:>9}  {}",
        h.label,
        h.cu_count,
        h.elapsed.as_secs_f64() * 1000.0,
        h.gflops,
        h.mismatches,
        h.verdict()
    );
}

fn cmd_cu_test(localize: bool, probe: bool) -> Result<()> {
    if !cutest::available() {
        anyhow::bail!(
            "health-test needs umr (routing) and a Vulkan ICD (RADV). \
             install: pacman -S umr vulkan-radeon vulkan-icd-loader"
        );
    }

    if probe {
        // Safety validation: exercise the subtractive routing shape (full-40
        // minus one WGP) regardless of fault state. Every config keeps all four
        // shader arrays populated.
        println!("PROBE: subtractive sweep (full-40 minus one WGP) — safety check\n");
        cutest::test_localize(kat_row)?;
        return Ok(());
    }

    println!("CU health-test — known-answer Vulkan compute (KAT)\n");
    let full = cutest::test_full()?;
    kat_row(&full);

    if full.ok {
        println!("\nall 40 CUs healthy.");
        return Ok(());
    }

    println!("\na CU is faulty ({} mismatches).", full.mismatches);
    if !localize {
        println!("re-run with --localize to pinpoint the bad WGP.");
        std::process::exit(1);
    }

    println!("\nlocalizing — routing full-40 minus one WGP at a time:");
    // In the subtractive sweep, ok==true means removing that WGP cleared the
    // fault, i.e. that WGP holds the bad CU.
    let sweep = cutest::test_localize(|h| {
        println!(
            "  drop {:<16} {:>8.0} GFLOPS  {:>9} mismatch  {}",
            h.label.trim_start_matches('-'),
            h.gflops,
            h.mismatches,
            if h.hung {
                "HUNG"
            } else if h.ok {
                "<- fault clears (suspect)"
            } else {
                "still faulty"
            }
        );
    })?;
    let suspects: Vec<_> = sweep
        .iter()
        .filter(|h| h.ok)
        .map(|h| h.label.trim_start_matches('-').to_string())
        .collect();
    println!();
    if suspects.is_empty() {
        println!("could not isolate a single WGP (multiple bad, or intermittent)");
    } else {
        println!("bad WGP: {}", suspects.join(", "));
        println!("route around it: edit the routing in the TUI, then `cu route-save`");
    }
    std::process::exit(1);
}

fn cmd_gpu(action: GpuCmd) -> Result<()> {
    match action {
        GpuCmd::Status => {
            let cfg = dpm::PowerConfig::load_or_default();
            println!("GPU power (app-driven):");
            println!("  top (active):     {} MHz", cfg.top_mhz);
            println!("  deep (idle):      {} MHz", cfg.deep_mhz);
            // The heat-safety state MUST be visible: whether a manual pin is
            // armed, which auto controller owns the clock otherwise, and any
            // persisted voltage override.
            match cfg.force_mhz {
                Some(m) => println!("  manual pin:       {m} MHz (force_mhz ARMED — wins at boot)"),
                None => println!("  manual pin:       none"),
            }
            println!("  auto mode:        {}", gpuctl::mode_name(cfg.auto_mode));
            match cfg.force_vid_mv {
                Some(v) => println!("  forced GPU Vid:   {v} mV (persisted)"),
                None => println!("  forced GPU Vid:   none (SMU-managed)"),
            }
            // Prefer the telemetry node (live QueryGfxclk); fall back to sysfs.
            let cur = Smu::open()
                .ok()
                .and_then(|s| s.telemetry())
                .and_then(|t| {
                    t.lines().find_map(|l| {
                        l.split_once("GfxClk:")
                            .and_then(|(_, r)| r.split_whitespace().next().map(String::from))
                    })
                })
                .or_else(|| telemetry::current_sclk_mhz().map(|m| m.to_string()));
            match cur {
                Some(c) => println!("  current GfxClk:    {c} MHz"),
                None => println!("  current GfxClk:    unknown"),
            }
        }
        GpuCmd::Force { mhz } => {
            // The shared state-machine transition (gpuctl::force): voltage floor
            // raised BEFORE the clock (raise failure = clock untouched), persist,
            // then the single unit re-enacts the manual pin.
            let out = gpuctl::force(mhz)?;
            if let Some((old, floor)) = out.vid_raised {
                println!(
                    "note: raised GPU undervolt {old} -> {floor} mV (safe floor for {} MHz)",
                    out.set_mhz
                );
            }
            let note = if out.held {
                "manual mode — sticks across reboots"
            } else {
                "LIVE-ONLY — will NOT survive a reboot"
            };
            if out.set_mhz != mhz {
                println!(
                    "GFX clock forced to {} MHz (clamped from {mhz}); {note}",
                    out.set_mhz
                );
            } else {
                println!("GFX clock forced to {} MHz; {note}", out.set_mhz);
            }
            if !out.held {
                eprintln!(
                    "WARNING: could not install the boot re-apply service — this pin is \
                     REBOOT-UNSAFE. Re-run as root, or the clock reverts to auto on reboot."
                );
            }
        }
        GpuCmd::Unforce => {
            // Shared transition: release clock + voltage, clear ONLY the manual
            // pin — the persisted auto mode is preserved, not clobbered to
            // governor (releasing a pin must not flip autosleep/released boxes).
            let mode = gpuctl::unforce()?;
            println!(
                "GFX clock + voltage released — auto mode ({}) resumed",
                gpuctl::mode_name(mode)
            );
        }
        GpuCmd::Pin { mhz } => {
            // Stop the GPU power unit FIRST (its daemon unforces on stop), then
            // hard-force — transient, no persistence, no mode change. On ANY
            // failure after the stop, the unit is started again before returning:
            // a dead GPU power unit means no governor and no heat-pin.
            persist::stop_gpu_unit();
            let pin = || -> Result<u32> {
                let target = mhz.clamp(smu::SCLK_MIN_MHZ, smu::SCLK_MAX_MHZ);
                // Same crash class as force: a persisted undervolt sized for a
                // lower clock must be raised (transiently — not persisted)
                // before the clock lands on it.
                let cfg = dpm::PowerConfig::load_or_default();
                if let Some((old, floor)) = gpuctl::required_vid_raise(&cfg, target) {
                    anyhow::ensure!(
                        telemetry::od_set_vddc(target, floor),
                        "cannot raise GPU undervolt {old} -> {floor} mV for {target} MHz \
                         (overdrive write failed) — refusing to pin onto a starved rail"
                    );
                    println!(
                        "note: raised GPU voltage {old} -> {floor} mV for the pin (transient)"
                    );
                }
                Smu::open()?.force_gfx_freq(target)
            };
            match pin() {
                Ok(set) => println!(
                    "pinned {set} MHz (transient — GPU power unit stopped; run `gpu unpin` after)"
                ),
                Err(e) => {
                    persist::start_gpu_unit();
                    return Err(e.context("pin failed (GPU power unit restarted)"));
                }
            }
        }
        GpuCmd::Unpin => {
            Smu::open()?.unforce_gfx_freq()?;
            persist::start_gpu_unit();
            println!("unpinned — GPU power unit restarted (persisted mode resumes)");
        }
        GpuCmd::ApplyBoot | GpuCmd::ApplySavedForce => gpu_apply_boot()?,
        GpuCmd::Vid { mv } => {
            // Set the GPU voltage at the TOP clock via amdgpu overdrive
            // (pp_od_clk_voltage). amdgpu serializes the SMU write, so it's safe
            // live under load — no governor coordination, no MP1 race. Use the
            // configured top clock (not the live one) so it never pins the max
            // clock low. Persist so it re-applies at boot.
            let _lock = dpm::ConfigLock::acquire();
            let mut cfg = dpm::PowerConfig::load_or_default();
            // Floor at the highest reachable clock: top, a manual force, OR the
            // governor high tier (writable independently of top_mhz).
            let clk = cfg.effective_floor_clock(&dpm::GovernorConfig::load_or_default());
            let floor = telemetry::min_gfx_vddc(clk);
            if mv < floor {
                anyhow::bail!(
                    "{mv} mV is below the safe floor {floor} mV for {clk} MHz — that undervolt \
                     would crash the GPU. Raise the voltage, or lower the clock first (set-top)."
                );
            }
            if !telemetry::od_set_vddc(clk, mv) {
                anyhow::bail!(
                    "voltage set failed — overdrive off (amdgpu.ppfeaturemask) or need root, \
                     or {mv} mV is outside the OD_RANGE VDDC"
                );
            }
            cfg.force_vid_mv = Some(mv);
            cfg.save()?;
            // Boot re-apply rides the single GPU power unit (apply-boot applies
            // the saved voltage first); no unit restart needed for a live set.
            println!(
                "GPU voltage set to {mv} mV at the high clock{}",
                if persist::gpu_unit_installed() {
                    " — persisted, re-applied at boot"
                } else {
                    " (live now; run a gpu mode command to install the boot unit)"
                }
            );
        }
        GpuCmd::UnforceVid => {
            // Restore the stock voltage curve (SMU-managed) via amdgpu overdrive.
            if !telemetry::od_reset() {
                anyhow::bail!("voltage reset failed — overdrive off or need root");
            }
            let _lock = dpm::ConfigLock::acquire();
            let mut cfg = dpm::PowerConfig::load_or_default();
            cfg.force_vid_mv = None;
            cfg.save()?;
            println!("GPU voltage released — stock SMU-managed curve restored");
        }
        GpuCmd::Wake { override_pin } => {
            let cfg = dpm::PowerConfig::load_or_default();
            guard_manual_pin(&cfg, override_pin, "wake")?;
            Smu::open()?.force_gfx_freq(cfg.top_mhz)?;
            println!("woke to {} MHz", cfg.top_mhz);
        }
        GpuCmd::DeepSleep { override_pin } => {
            let cfg = dpm::PowerConfig::load_or_default();
            guard_manual_pin(&cfg, override_pin, "deep-sleep")?;
            Smu::open()?.force_gfx_freq(cfg.deep_mhz)?;
            println!("deep-sleep at {} MHz", cfg.deep_mhz);
        }
        GpuCmd::SetTop { mhz } => {
            // Also feed the governor's high tier so the setpoint is live under auto.
            let _lock = dpm::ConfigLock::acquire();
            let mut cfg = dpm::PowerConfig::load_or_default();
            cfg.set_top(mhz)?;
            let mut g = dpm::GovernorConfig::load_or_default();
            g.high_mhz = mhz;
            // An inverted ladder makes thermal demotion RAISE the clock — refuse.
            g.validate_ladder()?;
            // Raising the clock onto a stale low undervolt is the classic crash
            // (e.g. 887 mV set for 1500 is far too low for 2230). The floor keys
            // off the HIGHEST clock the GPU can now reach: the new top, the
            // governor high tier, or a manual force.
            let clk = cfg
                .effective_top_mhz()
                .max(g.high_mhz)
                .max(cfg.force_mhz.unwrap_or(0));
            let floor = telemetry::min_gfx_vddc(clk);
            if let Some(v) = cfg.force_vid_mv {
                if v < floor {
                    cfg.force_vid_mv = Some(floor);
                    let _ = telemetry::od_set_vddc(clk, floor);
                    println!(
                        "note: raised GPU undervolt {v} -> {floor} mV (safe floor for {clk} MHz)"
                    );
                }
            }
            cfg.save()?;
            g.save()?;
            persist::reload_if_auto();
            println!("top/high setpoint = {mhz} MHz (persisted)");
        }
        GpuCmd::SetMid { mhz } => {
            let mut g = dpm::GovernorConfig::load_or_default();
            g.mid_mhz = mhz;
            g.validate_ladder()?;
            g.save()?;
            persist::reload_if_auto();
            println!("mid setpoint = {mhz} MHz (persisted)");
        }
        GpuCmd::SetIdle { mhz } => {
            let mut g = dpm::GovernorConfig::load_or_default();
            g.idle_mhz = mhz;
            g.validate_ladder()?;
            g.save()?;
            persist::reload_if_auto();
            println!("idle setpoint = {mhz} MHz (persisted)");
        }
        GpuCmd::SetDeep { mhz } => {
            let _lock = dpm::ConfigLock::acquire();
            let mut cfg = dpm::PowerConfig::load_or_default();
            cfg.set_deep(mhz)?;
            let mut g = dpm::GovernorConfig::load_or_default();
            g.deep_mhz = mhz;
            g.validate_ladder()?;
            cfg.save()?;
            g.save()?;
            persist::reload_if_auto();
            println!("deep setpoint = {mhz} MHz (persisted)");
        }
        GpuCmd::TempCap { c } => {
            // Change ONLY the GPU temp cap: same cpu.json the TUI + `cpu set` use,
            // applied live via queue-3 (clamped to the safe hw backstop), persisted
            // for boot, and picked up by a running governor for its demotion target.
            // Range-checked BEFORE persisting: an out-of-range cap (0 disables the
            // soft cap; 150 fails CpuOc::validate) would poison cpu.json and break
            // the boot cpu-apply.
            anyhow::ensure!(
                (60..=ocq3::TEMP_MAX_C).contains(&c),
                "gpu temp cap {c} C outside 60-{} C",
                ocq3::TEMP_MAX_C
            );
            let mut pt = cpu::CpuOc::load_or_default();
            pt.gpu_temp_c = c;
            pt.save()?;
            if let Ok(oc) = ocq3::OcQ3::open_checked() {
                oc.set_gpu_max_temp(c)?;
            }
            let _ = persist::enable_cpu_oc();
            persist::reload_if_auto();
            println!("gpu temp cap = {c} C (persisted; governor demotes to hold it)");
        }
        GpuCmd::GovernorOn => {
            // Release any live pin for instant effect, clear the persisted manual
            // clock (a stale force_mhz would re-arm the pin at the next boot and
            // shadow the governor), then make the single unit enact governor mode.
            let _ = Smu::open().and_then(|s| s.unforce_gfx_freq());
            let _lock = dpm::ConfigLock::acquire();
            let mut cfg = dpm::PowerConfig::load_or_default();
            cfg.force_mhz = None;
            cfg.auto_mode = dpm::AutoMode::Governor;
            cfg.save()?;
            persist::log_transition("governor-on");
            persist::apply_mode()?;
            println!("auto governor: ON (persists across reboot)");
        }
        GpuCmd::GovernorOff => {
            let _ = Smu::open().and_then(|s| s.unforce_gfx_freq());
            let _lock = dpm::ConfigLock::acquire();
            let mut cfg = dpm::PowerConfig::load_or_default();
            cfg.force_mhz = None;
            cfg.auto_mode = dpm::AutoMode::Released;
            cfg.save()?;
            persist::log_transition("governor-off -> released");
            persist::apply_mode()?;
            println!("auto governor: OFF — released (native DPM)");
        }
        GpuCmd::AutosleepOn => {
            // Poke-driven autosleep becomes the persisted auto mode; the unit
            // restart starts the daemon (it applies its idle state on start).
            let _lock = dpm::ConfigLock::acquire();
            let mut cfg = dpm::PowerConfig::load_or_default();
            cfg.force_mhz = None;
            cfg.auto_mode = dpm::AutoMode::Autosleep;
            cfg.save()?;
            persist::log_transition("autosleep-on");
            persist::apply_mode()?;
            println!("autosleep: ON (poke-driven; persists across reboot)");
        }
        GpuCmd::Poke => {
            dpm::poke()?;
        }
        GpuCmd::Autosleep { idle, deep_force } => dpm::autosleep(idle as f64, deep_force)?,
        GpuCmd::Governor => dpm::governor(dpm::GovernorConfig::load_or_default())?,
        GpuCmd::Level {
            level,
            override_pin,
        } => {
            let cfg = dpm::PowerConfig::load_or_default();
            guard_manual_pin(&cfg, override_pin, "level")?;
            smu::set_performance_level(&level)?;
            println!("performance level set: {level}");
        }
        GpuCmd::Telemetry => match Smu::open()?.telemetry() {
            Some(t) => print!("{t}"),
            None => println!("telemetry node not present (patch 11)"),
        },
    }
    Ok(())
}

/// Seconds since boot, from /proc/uptime.
fn proc_uptime_secs() -> Option<f64> {
    std::fs::read_to_string("/proc/uptime")
        .ok()?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// Refuse an app-level clock override (`wake` / `deep-sleep` / `level`) while a
/// persisted manual pin is armed: those commands would silently push the clock
/// off a live heat-pin. `--override-pin` acknowledges and proceeds.
fn guard_manual_pin(cfg: &dpm::PowerConfig, override_pin: bool, what: &str) -> Result<()> {
    if let Some(m) = cfg.force_mhz {
        if override_pin {
            eprintln!(
                "WARNING: overriding the persisted {m} MHz manual pin with `{what}` \
                 (--override-pin) — the pin re-asserts on the next unit restart/boot"
            );
        } else {
            anyhow::bail!(
                "a manual clock pin is persisted at {m} MHz (possibly a heat-safety pin) — \
                 `{what}` would silently override it. Release it first (`gpu unforce`) or \
                 pass --override-pin."
            );
        }
    }
    Ok(())
}

/// `gpu apply-boot` — the single aputune-gpu.service entrypoint (also the
/// `apply-saved-force` alias for un-migrated hosts). Enacts the persisted GPU
/// power mode from power.json:
///
///   force_mhz Some  -> pin it (manual mode; exit 0, RemainAfterExit holds)
///   Governor        -> run the fence-rate governor (blocks forever)
///   Autosleep       -> run the poke-driven autosleep daemon (blocks forever)
///   Released        -> release to native DPM (exit 0)
///
/// HEAT-SAFETY INVARIANT: a persisted manual pin (e.g. a 350 MHz heat-pin) MUST
/// come back pinned after a reboot — the manual branch is unconditional on a
/// migrated host and always wins over auto_mode.
fn gpu_apply_boot() -> Result<()> {
    // Fail-SAFE load: a power.json that EXISTS but does not parse may have held
    // a heat-pin — it must never default to governor-at-top (dpm::boot_plan).
    let load = dpm::load_for_boot();
    let plan = dpm::boot_plan(&load);
    let cfg = match &load {
        dpm::BootLoad::Loaded(c) => c.clone(),
        _ => dpm::PowerConfig::default(),
    };
    // Boot-settle: forcing/governing a just-booted GPU (sdma ring still resetting)
    // has wedged the box, so wait out an uptime window before touching the SMU
    // (a no-op on a live restart already past it). The window is mode-aware: a
    // manual pin to a LOW clock is a single, gentle SMU write — nothing like the
    // governor's climb-to-run load burst — so it uses a much shorter settle to
    // minimise the heat-exposure window before a heat-pin lands. Auto modes and
    // HIGH manual pins keep the full settle (forcing a high clock early is the
    // wedge-prone case).
    const SETTLE_S: f64 = 120.0;
    const SETTLE_MANUAL_LOW_S: f64 = 15.0;
    const LOW_PIN_MHZ: u32 = 1000; // <= idle tier: low wedge risk
    let settle = match plan {
        dpm::BootPlan::Manual(m) if m <= LOW_PIN_MHZ => SETTLE_MANUAL_LOW_S,
        dpm::BootPlan::FailSafePin(_) => SETTLE_MANUAL_LOW_S,
        _ => SETTLE_S,
    };
    match proc_uptime_secs() {
        Some(up) if up >= settle => {}
        Some(up) => {
            let wait = settle - up;
            eprintln!(
                "arieltune apu apply-boot: boot-settle — waiting {wait:.0}s (uptime {up:.0}s)"
            );
            std::thread::sleep(std::time::Duration::from_secs_f64(wait));
        }
        // Fail CLOSED: with /proc/uptime unreadable we can't prove the settle
        // window has passed — sleep the FULL window rather than skip it (an
        // early SMU force on a just-booted GPU is the wedge class).
        None => {
            eprintln!(
                "arieltune apu apply-boot: /proc/uptime unreadable — sleeping the full \
                 {settle:.0}s settle (fail-closed)"
            );
            std::thread::sleep(std::time::Duration::from_secs_f64(settle));
        }
    }
    // CORRUPT power.json: pin the deep/idle-safe LOW clock and stop — the lost
    // config may have been a heat-pin, so no auto controller is started.
    if let dpm::BootPlan::FailSafePin(m) = plan {
        eprintln!(
            "arieltune apu apply-boot: power.json is CORRUPT — fail-safe: pinning {m} MHz, \
             no auto controller (repair or delete {} and re-set the mode)",
            dpm::CONFIG_PATH
        );
        persist::log_transition(&format!(
            "apply-boot: corrupt power.json -> fail-safe pin {m}"
        ));
        let set = Smu::open()?.force_gfx_freq(m)?;
        println!("fail-safe: GFX clock pinned at {set} MHz");
        return Ok(());
    }
    // Persisted GPU voltage FIRST, via amdgpu overdrive (SMU-safe), at the
    // effective top clock (governor top or a higher manual force) so it never
    // pins the max clock low AND never undervolts below what the actual running
    // clock needs.
    if let Some(mv) = cfg.force_vid_mv {
        let clk = cfg.effective_floor_clock(&dpm::GovernorConfig::load_or_default());
        if telemetry::od_set_vddc(clk, mv) {
            println!("re-applied GPU voltage {mv} mV at {clk} MHz (overdrive)");
        } else {
            eprintln!("GPU voltage re-apply failed (overdrive off?)");
        }
    }
    // Manual pin wins over everything (the heat-safety invariant above).
    if let dpm::BootPlan::Manual(m) = plan {
        // Un-migrated-host guard (apply-saved-force alias): never re-force the
        // clock while ANY legacy GPU unit (governor OR autosleep) is still
        // enabled — two writers on the SMU is the wedge class. A migrated host
        // has no legacy units, so the pin is applied unconditionally there.
        if persist::any_legacy_gpu_unit_enabled() {
            println!(
                "legacy GPU unit(s) still enabled — skipping manual clock re-apply \
                 (un-migrated host)"
            );
            return Ok(());
        }
        let set = Smu::open()?.force_gfx_freq(m)?;
        println!("manual mode: GFX clock pinned at {set} MHz (power.json force_mhz)");
        return Ok(());
    }
    // Same guard for the auto modes: on an un-migrated host the legacy
    // governor/autosleep unit still owns auto — don't start a second daemon.
    if persist::any_legacy_gpu_unit_enabled() {
        println!(
            "legacy GPU unit(s) still enabled — leaving auto mode to them \
             (un-migrated host; a gpu mode command migrates)"
        );
        return Ok(());
    }
    match cfg.auto_mode {
        dpm::AutoMode::Governor => dpm::governor(dpm::GovernorConfig::load_or_default()),
        dpm::AutoMode::Autosleep => dpm::autosleep(30.0, false),
        dpm::AutoMode::Released => {
            Smu::open()?.unforce_gfx_freq()?;
            println!("released: GFX clock left to native DPM (BAPM)");
            Ok(())
        }
    }
}

fn cmd_cpu(action: CpuCmd) -> Result<()> {
    let oc = ocq3::OcQ3::open_checked()
        .context("queue-3 OC mailbox not reachable (need root + a BC-250)")?;
    match action {
        CpuCmd::Status => {
            let l = cpu::live(&oc);
            println!("CPU (live, via SMU queue 3):");
            println!(
                "  current Vid:    {}",
                l.cur_vid_mv
                    .map(|v| format!("{v} mV"))
                    .unwrap_or("?".into())
            );
            println!(
                "  GPU Vid:        {}",
                oc.gpu_voltage_mv()
                    .map(|v| format!("{v} mV"))
                    .unwrap_or("?".into())
            );
            if !l.cores.is_empty() {
                let s: Vec<String> = l.cores.iter().map(|c| format!("{c}")).collect();
                println!("  core freq MHz:  {}", s.join("  "));
            }
            if !l.pstates.is_empty() {
                let s: Vec<String> = l.pstates.iter().map(|c| format!("{c}")).collect();
                println!("  P-state MHz:    {}", s.join("  "));
            }
            println!(
                "  CPU temp limit: {}",
                l.cpu_temp_max
                    .map(|t| format!("{t} C"))
                    .unwrap_or("?".into())
            );
            println!(
                "  boot re-apply:  {}",
                if persist::cpu_oc_enabled() {
                    format!("armed ({})", persist::CPU_OC_UNIT)
                } else {
                    "not armed".to_string()
                }
            );
        }
        CpuCmd::Set {
            boost,
            scale,
            cpu_temp,
            gpu_temp,
        } => {
            // Preserve unspecified fields: load the current config and override
            // ONLY the flags that were given, so `cpu set --boost N` never clobbers
            // the GPU temp cap / curve / CPU temp. Same non-clobber contract as the
            // TUI (which edits one field and keeps the rest).
            let prev = cpu::CpuOc::load_or_default();
            let mut pt = prev.clone();
            if let Some(b) = boost {
                pt.boost_mhz = b;
            }
            if let Some(s) = scale {
                pt.curve_scale = s;
            }
            if let Some(c) = cpu_temp {
                pt.cpu_temp_c = c;
            }
            if let Some(g) = gpu_temp {
                pt.gpu_temp_c = g;
            }
            // Direction-aware staging vs the previously-persisted point, so
            // lowering boost/shallowing the curve never transiently overvolts.
            pt.apply_from(&oc, Some(&prev))?;
            println!(
                "applied: boost {} MHz, curve scale {} (~{} mV @ boost), temp {}/{} C",
                pt.boost_mhz,
                pt.curve_scale,
                pt.predicted_vid_mv(),
                pt.cpu_temp_c,
                pt.gpu_temp_c,
            );
            if pt.boost_mhz > ocq3::BOOST_WARN_MHZ {
                println!(
                    "note: {} MHz is aggressive — stress-test thoroughly.",
                    pt.boost_mhz
                );
            }
            // Persist so it survives reboots: save the config + install the boot
            // service that re-applies it (aputune cpu apply-saved).
            pt.save()?;
            match persist::enable_cpu_oc() {
                Ok(()) => println!("persisted — re-applied on every boot"),
                Err(e) => println!("(live only — could not install boot service: {e})"),
            }
        }
        CpuCmd::Undervolt { boost, vid_cap } => {
            anyhow::ensure!(
                vid_cap <= ocq3::VID_CEILING_MV,
                "vid cap {vid_cap} exceeds {} mV ceiling",
                ocq3::VID_CEILING_MV
            );
            let scale = cpu::safe_scale_for(boost, vid_cap);
            let pt = cpu::CpuOc {
                boost_mhz: boost,
                curve_scale: scale,
                ..Default::default()
            };
            pt.apply(&oc)?;
            println!(
                "undervolt: boost {boost} MHz, curve scale {scale} (~{} mV predicted @ boost)",
                pt.predicted_vid_mv()
            );
        }
        CpuCmd::Vid { mv } => {
            oc.force_cpu_vid_mv(mv)?;
            println!(
                "forced CPU Vid {mv} mV (capped at {} mV)",
                ocq3::VID_CEILING_MV
            );
        }
        CpuCmd::VidAuto => {
            oc.unforce_cpu_vid()?;
            println!("released CPU Vid to the SMU curve");
        }
        CpuCmd::Restore => {
            cpu::restore_stock(&oc)?;
            let _ = cpu::clear_saved();
            persist::disable_cpu_oc();
            println!("CPU restored to firmware defaults (saved OC + boot service removed)");
        }
        CpuCmd::ApplySaved => {
            if cpu::saved_exists() {
                let pt = cpu::CpuOc::load_or_default();
                pt.apply(&oc)?;
                println!(
                    "re-applied saved CPU OC: boost {} MHz, scale {} (~{} mV), temp {}/{} C",
                    pt.boost_mhz,
                    pt.curve_scale,
                    pt.predicted_vid_mv(),
                    pt.cpu_temp_c,
                    pt.gpu_temp_c,
                );
            } else {
                println!("no saved CPU OC — nothing to apply");
            }
        }
        CpuCmd::Detect {
            target,
            vid_cap,
            temp,
            step,
            dwell,
            save,
        } => {
            // Reject a bad save-name before spending a full stress sweep.
            if let Some(name) = &save {
                profile::valid_name(name)?;
            }
            let opts = cpu::SweepOpts {
                target_mhz: target,
                vid_cap_mv: vid_cap,
                temp_c: temp,
                step,
                dwell_s: dwell,
            };
            println!(
                "CPU auto-detect: climb to {target} MHz, undervolt to <= {vid_cap} mV, \
                 {dwell}s/point, temp {temp}C  (Ctrl-C aborts + restores stock)\n"
            );
            let (points, best) = cpu::detect(&oc, &opts)?;
            println!(
                "{:>8}  {:>6}  {:>8}  {:<7} {:>6}  note",
                "boost", "scale", "Vid", "result", "maxT"
            );
            for p in &points {
                println!(
                    "{:>6}MHz  {:>6}  {:>6} mV  {:<7} {:>5.0}C  {}",
                    p.boost_mhz,
                    p.scale,
                    p.measured_vid_mv.map(|v| v as i64).unwrap_or(-1),
                    if p.stable { "stable" } else { "FAIL" },
                    p.max_temp_c,
                    p.note
                );
            }
            match best {
                Some(pt) => {
                    println!(
                        "\nhighest stable: boost {} MHz @ curve scale {} (~{} mV)",
                        pt.boost_mhz,
                        pt.curve_scale,
                        pt.predicted_vid_mv()
                    );
                    if let Some(name) = save {
                        let prof = profile::Profile {
                            name: name.clone(),
                            description: format!(
                                "cpu-detect: boost {} MHz, curve scale {} (stability-swept)",
                                pt.boost_mhz, pt.curve_scale
                            ),
                            cu_40: None,
                            gpu: None,
                            cpu: Some(pt),
                        };
                        let path = profile::save(&prof)?;
                        println!("saved profile '{name}' -> {}", path.display());
                    }
                }
                None => println!("\nno stable point found in range"),
            }
        }
    }
    Ok(())
}

fn cmd_patches(show: Option<String>) -> Result<()> {
    if let Some(id) = show {
        match patches::SERIES.iter().find(|p| p.id == id) {
            Some(p) => {
                println!("# {} — {}\n# touches: {}\n", p.id, p.title, p.touches);
                print!("{}", p.body);
            }
            None => println!("no such patch: {id}"),
        }
        return Ok(());
    }

    let rep = detect::report();
    println!(
        "BC-250: {}    series: {} patches    {}",
        if rep.is_bc250 { "yes" } else { "NO" },
        patches::count(),
        if rep.fully_patched() {
            "FULLY PATCHED"
        } else {
            "NOT fully patched"
        }
    );
    if rep.dbg_dir.is_none() {
        println!("note: amdgpu debugfs not found (mount debugfs / run as root for full detection)");
    }
    println!();
    for r in &rep.rows {
        println!("{} {:<5} {}", r.state.glyph(), r.id, r.title);
    }
    let missing = rep.missing();
    if !missing.is_empty() {
        println!(
            "\n{} patch(es) absent — run `arieltune apu build` to build the series into the system.",
            missing.len()
        );
    }
    Ok(())
}

/// Doctor-style series summary: liveness count + per-patch rows.
fn print_series(rep: &detect::Report) {
    let present = rep
        .rows
        .iter()
        .filter(|r| matches!(r.state, detect::State::Present | detect::State::Inferred))
        .count();
    println!("  liberation series: {present}/{} live", patches::count());
    for r in &rep.rows {
        println!("  {} {:<5} {}", r.state.glyph(), r.id, r.title);
    }
}

/// Ask which liberation tier to apply. Defaults to inspect-only (decline) on
/// empty input, EOF, or a non-tty stdin — a kernel build must never start by
/// accident.
fn prompt_tier() -> Result<LiberateTier> {
    use std::io::{BufRead, IsTerminal, Write};
    println!("liberation tiers:");
    println!("  1) full          patched kernel + route all 40 CUs (bc250_cc_write_mode=3)");
    println!("  2) tuning-only   patched kernel, CU routing OFF (bc250_cc_write_mode=0):");
    println!("                   telemetry + GPU clock + CPU OC without the 40-CU route");
    println!("                   (ROCm OpenCL is unstable at 40 CU on gfx1013)");
    println!("  3) inspect-only  report patch state, change nothing  [default]");
    if !std::io::stdin().is_terminal() {
        println!("stdin is not a tty — taking the default (inspect-only)");
        return Ok(LiberateTier::InspectOnly);
    }
    print!("choose [1/2/3, default 3]: ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    if std::io::stdin().lock().read_line(&mut line)? == 0 {
        // EOF: decline.
        println!();
        return Ok(LiberateTier::InspectOnly);
    }
    match line.trim() {
        "1" | "full" => Ok(LiberateTier::Full),
        "2" | "tuning-only" | "tuning" => Ok(LiberateTier::TuningOnly),
        "" | "3" | "inspect-only" | "inspect" => Ok(LiberateTier::InspectOnly),
        other => anyhow::bail!("unrecognized choice '{other}' — nothing done"),
    }
}

/// `aputune liberate` — orchestration over `kbuild::build`: gate on the
/// silicon, short-circuit if already liberated, then an explicit tier choice.
/// Preview-first like `build`: without --run the chosen tier's plan is printed
/// and nothing executes.
fn cmd_liberate(
    pkgbuild: Option<std::path::PathBuf>,
    target: Option<String>,
    tier: Option<LiberateTier>,
    run: bool,
) -> Result<()> {
    // Gate first: never offer a kernel build on non-BC-250 silicon.
    if !ariel_hal::ariel_apu_present() {
        anyhow::bail!("not a BC-250 (PCI 1002:13fe not found) — nothing to liberate");
    }
    let rep = detect::report();
    if rep.fully_patched() {
        println!("liberation series already live — nothing to do.");
        print_series(&rep);
        return Ok(());
    }
    let tier = match tier {
        Some(t) => t,
        None => prompt_tier()?,
    };
    match tier {
        LiberateTier::InspectOnly => {
            println!("inspect-only — no kernel changes.");
            print_series(&rep);
            println!("\nre-run `arieltune apu liberate` (or `arieltune apu build`) when ready.");
            Ok(())
        }
        LiberateTier::Full | LiberateTier::TuningOnly => {
            let mut opts = kbuild::BuildOpts::default();
            if pkgbuild.is_some() {
                opts.pkgbuild_dir = pkgbuild;
            }
            opts.target = target;
            opts.run = run;
            opts.cc_mode = if tier == LiberateTier::Full { 3 } else { 0 };
            if tier == LiberateTier::TuningOnly {
                println!(
                    "tuning-only: same patched kernel, bc250_cc_write_mode=0 \
                     (40-CU routing stays off; `arieltune apu cu enable` arms it later)"
                );
            }
            kbuild::build(opts)
        }
    }
}

fn cmd_cumap() -> Result<()> {
    match cu::map() {
        Some(m) => print!("{}", cu::render(&m)),
        None => println!("could not query amdgpu CU map (need a working amdgpu + render node)"),
    }
    Ok(())
}

fn cmd_doctor(json: bool, verify: bool) -> Result<()> {
    let rep = detect::report();
    // The verify gate: usable after a reboot and by scripts (non-zero exit via
    // the anyhow flow unless the box is a BC-250 with the full series live).
    let verify_gate = |rep: &detect::Report| -> Result<()> {
        if verify && !(rep.is_bc250 && rep.fully_patched()) {
            anyhow::bail!(
                "doctor --verify failed: is_bc250={} fully_patched={}",
                rep.is_bc250,
                rep.fully_patched()
            );
        }
        Ok(())
    };
    if json {
        // Machine mode: exactly one JSON object on stdout, nothing else, so
        // callers (incl. kbuild's post-install verification) parse it cleanly.
        let d = detect::DoctorJson::from_report(&rep);
        println!("{}", serde_json::to_string(&d)?);
        return verify_gate(&rep);
    }
    println!("arieltune apu doctor");
    println!("  BC-250 (PCI 1002:13fe): {}", yn(rep.is_bc250));
    println!(
        "  amdgpu debugfs: {}",
        rep.dbg_dir
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "not found".into())
    );
    let present = rep
        .rows
        .iter()
        .filter(|r| matches!(r.state, detect::State::Present | detect::State::Inferred))
        .count();
    println!("  liberation series: {present}/{} live", patches::count());
    match cu::map() {
        Some(m) => println!("  CUs active: {}/{}", m.active, m.possible),
        None => println!("  CUs active: unknown (amdgpu not queryable)"),
    }
    // Boot re-apply arming state — which persisted tunables come back on reboot.
    let cfg = dpm::PowerConfig::load_or_default();
    println!(
        "  boot units: gpu {}  cpu-oc {}  route {}",
        if persist::gpu_unit_installed() {
            "installed"
        } else {
            "absent"
        },
        if persist::cpu_oc_enabled() {
            "armed"
        } else {
            "off"
        },
        if persist::route_enabled() {
            "armed"
        } else {
            "off"
        },
    );
    match cfg.force_mhz {
        Some(m) => println!("  GPU mode: manual pin {m} MHz (wins at boot)"),
        None => println!("  GPU mode: auto ({})", gpuctl::mode_name(cfg.auto_mode)),
    }
    if !rep.fully_patched() {
        println!(
            "\n  -> not fully liberated. `arieltune apu patches` for detail, `arieltune apu build` to apply."
        );
    }
    verify_gate(&rep)
}

fn yn(b: bool) -> &'static str {
    if b {
        "yes"
    } else {
        "no"
    }
}

/// Parse 4 per-array hex WGP masks (e.g. "1f 1f 0f 1f"); only the low 5 bits matter.
fn parse_masks(masks: &[String]) -> anyhow::Result<[u32; 4]> {
    anyhow::ensure!(
        masks.len() == 4,
        "need 4 masks (SE0.SH0 SE0.SH1 SE1.SH0 SE1.SH1)"
    );
    let mut m = [0u32; 4];
    for (i, s) in masks.iter().enumerate() {
        let raw = u32::from_str_radix(s.trim_start_matches("0x"), 16)
            .map_err(|_| anyhow::anyhow!("bad hex mask: {s}"))?;
        m[i] = raw & curoute::FULL_MASK;
    }
    Ok(m)
}

/// Persist the just-applied CU routing so it survives reboots — snapshot the live
/// masks to the profile + arm the boot re-apply service. Same as the TUI's apply.
fn persist_route() -> bool {
    curoute::save_profile().is_ok() && persist::enable_route().is_ok()
}

fn persist_tag(persisted: bool) -> &'static str {
    if persisted {
        " — persisted (survives reboot)"
    } else {
        " (live only — could not persist)"
    }
}
