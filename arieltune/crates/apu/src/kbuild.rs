// SPDX-License-Identifier: GPL-2.0-only
//! Build the liberation series into the system.
//!
//! aputune carries the 12-patch amdgpu series as data; this module reconstructs
//! a patched kernel package and installs it, following the validated flow:
//!
//!   1. materialize the embedded patches to the work dir
//!   2. extract + prepare the CachyOS source via the PKGBUILD (makepkg -o)
//!   3. apply each patch with `patch -p1` into the extracted tree
//!   4. rebuild the package: `makepkg -e --noextract --noprepare ...` (CC=gcc-15)
//!   5. install the package (locally or to a remote target), set the modprobe.d
//!      40-CU drop-in, rebuild initramfs, reboot
//!
//! Heavy + irreversible-ish (a ~30 min kernel build, a reboot), so `aputune
//! build` PREVIEWS the plan by default; pass `--run` to execute.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use crate::detect;
use crate::patches;

/// Post-reboot verification tuning (remote `--target --run` builds): an
/// initial grace so the online-poll can't catch the OLD system in the seconds
/// before `systemctl reboot` actually drops the link, then bounded polling.
const VERIFY_REBOOT_GRACE_S: u64 = 30;
/// Total wall-clock budget for the node to come back after the grace.
const VERIFY_POLL_TIMEOUT_S: u64 = 300;
/// Fixed delay between online polls.
const VERIFY_POLL_DELAY_S: u64 = 15;
/// Per-attempt ssh/scp ConnectTimeout.
const VERIFY_SSH_TIMEOUT_S: u64 = 10;
/// Where the running aputune binary is staged on the target (same arch as the
/// build host — this is exactly how the binary is already deployed).
const VERIFY_REMOTE_BIN: &str = "/tmp/aputune-verify";

pub struct BuildOpts {
    /// Directory holding the CachyOS PKGBUILD (+ source tarball, or makepkg
    /// fetches it). Required.
    pub pkgbuild_dir: Option<PathBuf>,
    /// Where to stage materialized patches.
    pub work_dir: PathBuf,
    /// Force gcc-15 (the toolchain the PMFW package was built with).
    pub cc: String,
    /// Deploy target `user@host`; None = install on this host.
    pub target: Option<String>,
    /// Value armed in the modprobe.d drop-in (`bc250_cc_write_mode`):
    /// 3 = route all 40 CUs, 0 = patched kernel only (tuning without routing).
    pub cc_mode: u32,
    /// Actually execute (default: preview only).
    pub run: bool,
}

impl Default for BuildOpts {
    fn default() -> Self {
        let work = std::env::var("APUTUNE_WORK_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
                PathBuf::from(home).join(".cache/aputune-build")
            });
        BuildOpts {
            pkgbuild_dir: std::env::var("APUTUNE_PKGBUILD").ok().map(PathBuf::from),
            work_dir: work,
            cc: std::env::var("APUTUNE_CC").unwrap_or_else(|_| "gcc-15".into()),
            target: None,
            cc_mode: 3,
            run: false,
        }
    }
}

/// Write every embedded patch to `<work>/patches/` and return the dir.
pub fn materialize_patches(work: &Path) -> Result<PathBuf> {
    let dir = work.join("patches");
    fs::create_dir_all(&dir).with_context(|| format!("mkdir {}", dir.display()))?;
    for p in patches::SERIES {
        // reconstruct the on-disk filename: <id>-<title-ish>.patch isn't stored,
        // so name them by ordinal; apply order is what matters.
        let name = format!("{}.patch", p.id);
        fs::write(dir.join(&name), p.body)
            .with_context(|| format!("write {}", dir.join(&name).display()))?;
    }
    Ok(dir)
}

/// One planned shell step.
struct Step {
    desc: String,
    /// Program + args.
    argv: Vec<String>,
    /// Working directory.
    cwd: PathBuf,
    /// Extra environment.
    env: Vec<(String, String)>,
}

fn step(desc: &str, cwd: &Path, argv: &[&str]) -> Step {
    Step {
        desc: desc.into(),
        argv: argv.iter().map(|s| s.to_string()).collect(),
        cwd: cwd.to_path_buf(),
        env: vec![],
    }
}

impl Step {
    fn with_env(mut self, k: &str, v: &str) -> Self {
        self.env.push((k.into(), v.into()));
        self
    }

    fn render(&self) -> String {
        let env: String = self.env.iter().map(|(k, v)| format!("{k}={v} ")).collect();
        format!("{}{}", env, self.argv.join(" "))
    }

    fn execute(&self) -> Result<()> {
        let mut cmd = Command::new(&self.argv[0]);
        cmd.args(&self.argv[1..]).current_dir(&self.cwd);
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
        let status = cmd
            .status()
            .with_context(|| format!("spawn: {}", self.render()))?;
        if !status.success() {
            bail!("step failed ({}): {}", status, self.render());
        }
        Ok(())
    }
}

/// Validate a `user@host` deploy target: both halves non-empty and limited to
/// `[A-Za-z0-9_.-]`. The target is interpolated into an scp/ssh shell line, so
/// anything wider (spaces, quotes, `;`) is refused outright.
fn valid_target(t: &str) -> Result<()> {
    let ok_part = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c))
    };
    match t.split_once('@') {
        Some((user, host)) if ok_part(user) && ok_part(host) => Ok(()),
        _ => bail!("invalid --target '{t}': must be user@host with only letters, digits, . _ -"),
    }
}

/// Shell-quote a string for interpolation into an `sh -c` line: single-quoted,
/// with embedded single quotes escaped as `'\''`.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// ssh argv against a (pre-validated) target: plain argv — no shell — with a
/// fixed ConnectTimeout and BatchMode so an auth prompt fails instead of
/// hanging the verification loop.
fn ssh_argv(target: &str, remote: &[&str]) -> Vec<String> {
    let mut v = vec![
        "ssh".to_string(),
        "-o".into(),
        format!("ConnectTimeout={VERIFY_SSH_TIMEOUT_S}"),
        "-o".into(),
        "BatchMode=yes".into(),
        target.to_string(),
    ];
    v.extend(remote.iter().map(|s| s.to_string()));
    v
}

/// scp argv to push a local file to `<target>:<remote_path>` (same ssh options).
fn scp_argv(local: &Path, target: &str, remote_path: &str) -> Vec<String> {
    vec![
        "scp".to_string(),
        "-o".into(),
        format!("ConnectTimeout={VERIFY_SSH_TIMEOUT_S}"),
        "-o".into(),
        "BatchMode=yes".into(),
        local.display().to_string(),
        format!("{target}:{remote_path}"),
    ]
}

/// Run an argv and capture stdout; a non-zero exit is an error.
fn run_capture(argv: &[String]) -> Result<String> {
    let out = Command::new(&argv[0])
        .args(&argv[1..])
        .output()
        .with_context(|| format!("spawn: {}", argv.join(" ")))?;
    if !out.status.success() {
        bail!("command failed ({}): {}", out.status, argv.join(" "));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The target's running kernel (`uname -r`) — captured BEFORE the install so
/// the post-reboot check can prove the node booted a DIFFERENT kernel.
fn remote_uname(target: &str) -> Result<String> {
    let k = run_capture(&ssh_argv(target, &["uname", "-r"]))?
        .trim()
        .to_string();
    if k.is_empty() {
        bail!("empty `uname -r` from {target}");
    }
    Ok(k)
}

/// Poll the rebooting target back online (`ssh ... true` every
/// VERIFY_POLL_DELAY_S), bounded by VERIFY_POLL_TIMEOUT_S total. Output is
/// swallowed — a dozen "Connection refused" polls are expected, not news.
fn poll_target_online(target: &str) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(VERIFY_POLL_TIMEOUT_S);
    loop {
        let argv = ssh_argv(target, &["true"]);
        let up = Command::new(&argv[0])
            .args(&argv[1..])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if up {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!(
                "{target} did not come back online within {VERIFY_POLL_TIMEOUT_S}s of the \
                 reboot — check its console (a failed boot may be sitting in the bootloader \
                 or a rescue shell)"
            );
        }
        std::thread::sleep(Duration::from_secs(VERIFY_POLL_DELAY_S));
    }
}

/// Parse the target's `doctor --json` output.
fn parse_doctor_json(s: &str) -> Result<detect::DoctorJson> {
    serde_json::from_str(s.trim())
        .with_context(|| format!("unparseable `doctor --json` output: {s:?}"))
}

/// Post-reboot verification for a remote `--run` build: wait out the reboot
/// grace, poll the node back online, push THIS binary (build host and BC-250
/// targets are the same arch), run `doctor --json` on the target, and assert
/// the node booted a NEW kernel with the full series live.
fn verify_remote(target: &str, pre_kernel: &str) -> Result<()> {
    valid_target(target)?;
    println!(
        "\nverify: waiting {VERIFY_REBOOT_GRACE_S}s for {target} to actually go down, then \
         polling every {VERIFY_POLL_DELAY_S}s (up to {VERIFY_POLL_TIMEOUT_S}s)"
    );
    std::thread::sleep(Duration::from_secs(VERIFY_REBOOT_GRACE_S));
    poll_target_online(target)?;
    let me = std::env::current_exe().context("resolve the running aputune binary")?;
    run_capture(&scp_argv(&me, target, VERIFY_REMOTE_BIN))
        .with_context(|| format!("copy aputune to {target}:{VERIFY_REMOTE_BIN}"))?;
    let doctor_out = run_capture(&ssh_argv(
        target,
        &["sudo", VERIFY_REMOTE_BIN, "doctor", "--json"],
    ));
    // Best-effort cleanup; never masks a doctor failure.
    let rm = ssh_argv(target, &["rm", "-f", VERIFY_REMOTE_BIN]);
    let _ = Command::new(&rm[0]).args(&rm[1..]).output();
    let d = parse_doctor_json(&doctor_out.context("run `doctor --json` on the target")?)?;
    if d.kernel == pre_kernel {
        bail!(
            "{target} came back on the OLD kernel ({pre_kernel}) — the new kernel did not \
             boot (bootloader fallback?); expected `uname -r` != {pre_kernel}"
        );
    }
    if !d.fully {
        bail!(
            "{target} booted {} but the series is NOT fully live ({}/{} patches) — \
             run `arieltune apu patches` on the target for the per-patch detail",
            d.kernel,
            d.present,
            d.total
        );
    }
    println!(
        "verified: {target} booted {}, {}/{} patches live",
        d.kernel, d.present, d.total
    );
    Ok(())
}

/// Resolve the single extracted `cachyos-*` dir under `<pkgbuild>/src` in Rust
/// (no shell glob). Fails on zero matches (extraction didn't happen) AND on
/// multiple (a stale second tree would make `patch -d` ambiguous).
fn extracted_src(pkgbuild: &Path) -> Result<PathBuf> {
    let src = pkgbuild.join("src");
    let entries = fs::read_dir(&src)
        .with_context(|| format!("read {} (did makepkg -o run?)", src.display()))?;
    let mut hits: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_dir()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with("cachyos-"))
                    .unwrap_or(false)
        })
        .collect();
    match hits.len() {
        0 => bail!("no extracted cachyos-* dir under {}", src.display()),
        1 => Ok(hits.remove(0)),
        n => bail!(
            "{n} cachyos-* dirs under {} — ambiguous; remove the stale ones",
            src.display()
        ),
    }
}

/// The extract step (makepkg -o). Integrity checks are NOT skipped: a kernel
/// source that fails its checksums must stop the build, not get patched and
/// installed anyway.
fn extract_step(pkgbuild: &Path) -> Step {
    step(
        "extract + prepare CachyOS source (makepkg -o)",
        pkgbuild,
        &["makepkg", "-o", "--nodeps", "--noconfirm"],
    )
}

/// Steps AFTER extraction: patch apply (argv `patch`, no shell), rebuild,
/// install. `src` is the resolved extracted tree; for the preview (before
/// extraction exists) a `src/cachyos-*` hint path stands in.
fn post_extract_plan(opts: &BuildOpts, patch_dir: &Path, src: &Path) -> Result<Vec<Step>> {
    let pkgbuild = opts
        .pkgbuild_dir
        .clone()
        .context("no PKGBUILD dir set (pass --pkgbuild <dir> or APUTUNE_PKGBUILD)")?;
    let mut steps = Vec::new();

    // 1. apply each embedded patch into the extracted tree. Plain argv — no
    //    shell, no glob, no redirect (`-i` replaces `< file`); `--forward` so a
    //    re-run over an already-patched tree fails cleanly instead of
    //    reverse-prompting; `--fuzz=0` so a hunk that no longer matches the
    //    source exactly FAILS LOUDLY instead of fuzzy-applying at an offset —
    //    a silently mis-placed hunk builds a kernel that only LOOKS patched.
    for p in patches::SERIES {
        let patch_file = patch_dir.join(format!("{}.patch", p.id));
        steps.push(Step {
            desc: format!("apply {} ({})", p.id, p.title),
            argv: vec![
                "patch".into(),
                "-p1".into(),
                "--forward".into(),
                "--fuzz=0".into(),
                "-d".into(),
                src.display().to_string(),
                "-i".into(),
                patch_file.display().to_string(),
            ],
            cwd: pkgbuild.clone(),
            env: vec![],
        });
    }

    // 2. rebuild the package without re-extracting / re-preparing.
    steps.push(
        step(
            "build patched kernel package (makepkg -e)",
            &pkgbuild,
            &[
                "makepkg",
                "-e",
                "--noextract",
                "--noprepare",
                "--noconfirm",
                "--nodeps",
                "--force",
            ],
        )
        .with_env("CC", &opts.cc)
        .with_env("HOSTCC", &opts.cc)
        .with_env("CXX", &opts.cc.replace("gcc", "g++"))
        .with_env("HOSTCXX", &opts.cc.replace("gcc", "g++")),
    );

    // 3. install + arm. Local vs remote. The shell lines glob the built
    //    package, so every interpolated path is quoted; the remote target was
    //    validated to [\w.-]+@[\w.-]+ up front.
    let pb_q = sh_quote(&pkgbuild.display().to_string());
    let mode = opts.cc_mode;
    let install_sh = if let Some(tgt) = &opts.target {
        valid_target(tgt)?;
        format!(
            "set -e; \
             scp {pb_q}/linux-cachyos-*.pkg.tar.zst {tgt}:/tmp/; \
             ssh {tgt} 'sudo pacman -U --noconfirm /tmp/linux-cachyos-*.pkg.tar.zst && \
               printf \"options amdgpu bc250_cc_write_mode={mode}\\n\" | sudo tee /etc/modprobe.d/aputune-40cu.conf && \
               sudo mkinitcpio -P && sudo systemctl reboot'"
        )
    } else {
        format!(
            "set -e; \
             sudo pacman -U --noconfirm {pb_q}/linux-cachyos-*.pkg.tar.zst; \
             printf 'options amdgpu bc250_cc_write_mode={mode}\\n' | sudo tee /etc/modprobe.d/aputune-40cu.conf; \
             sudo mkinitcpio -P; \
             echo 'reboot to load the liberated kernel'"
        )
    };
    steps.push(Step {
        desc: format!(
            "install package + arm cc_write_mode={mode}{} + initramfs{}",
            if mode == 3 { " (40-CU)" } else { "" },
            opts.target
                .as_ref()
                .map(|t| format!(" + reboot ({t})"))
                .unwrap_or_default()
        ),
        argv: vec!["sh".into(), "-c".into(), install_sh],
        cwd: pkgbuild.clone(),
        env: vec![],
    });

    Ok(steps)
}

/// Entry point for `aputune build`.
pub fn build(opts: BuildOpts) -> Result<()> {
    // Validate the deploy target BEFORE any heavy work.
    if let Some(tgt) = &opts.target {
        valid_target(tgt)?;
    }
    let pkgbuild = opts.pkgbuild_dir.clone().context(
        "no PKGBUILD dir set (pass --pkgbuild <dir> or APUTUNE_PKGBUILD); \
             it must hold a CachyOS linux-cachyos-* PKGBUILD",
    )?;
    if !pkgbuild.join("PKGBUILD").exists() {
        bail!("no PKGBUILD in {}", pkgbuild.display());
    }
    fs::create_dir_all(&opts.work_dir)
        .with_context(|| format!("mkdir {}", opts.work_dir.display()))?;
    let patch_dir = materialize_patches(&opts.work_dir)?;
    println!(
        "materialized {} patches -> {}",
        patches::count(),
        patch_dir.display()
    );

    let extract = extract_step(&pkgbuild);

    if !opts.run {
        // Preview: the source isn't extracted yet, so a `src/cachyos-*` hint
        // stands in for the tree the run resolves in Rust after extraction.
        let hint = pkgbuild.join("src/cachyos-<resolved-after-extract>");
        let mut steps = vec![extract];
        steps.extend(post_extract_plan(&opts, &patch_dir, &hint)?);
        // A remote run ends with an in-Rust verification pass — show it in the
        // plan so the count and the last step aren't a surprise.
        let n = steps.len() + usize::from(opts.target.is_some());
        println!("\n=== build plan (preview; pass --run to execute) ===");
        for (i, s) in steps.iter().enumerate() {
            println!("[{}/{}] {}", i + 1, n, s.desc);
            println!("      $ {}", s.render());
        }
        if let Some(tgt) = &opts.target {
            println!(
                "[{n}/{n}] verify post-reboot: poll {tgt} back online (<= {VERIFY_POLL_TIMEOUT_S}s), \
                 push this arieltune binary, `doctor --json`, assert new kernel + full series"
            );
        }
        println!(
            "\nbuild host needs: makepkg, {}, base-devel, ~25 GB free, ~30 min.",
            opts.cc
        );
        return Ok(());
    }

    // Pre-install kernel anchor for the post-reboot verification: remote via
    // ssh (an unreachable target fails HERE, before the ~30 min build), local
    // from procfs (for the Change-D style after-reboot instruction).
    let pre_kernel = match &opts.target {
        Some(tgt) => remote_uname(tgt)
            .with_context(|| format!("capture pre-install `uname -r` from {tgt}"))?,
        None => ariel_hal::running_kernel(),
    };
    println!("pre-install kernel: {pre_kernel}");

    // Run: extract first, THEN resolve the concrete cachyos-* tree in Rust
    // (fails on 0 or >1 matches) and drive the remaining steps against it.
    println!("\n[1/?] {}", extract.desc);
    println!("      $ {}", extract.render());
    extract.execute()?;
    let src = extracted_src(&pkgbuild)?;
    let steps = post_extract_plan(&opts, &patch_dir, &src)?;
    let n = steps.len() + 1;
    for (i, s) in steps.iter().enumerate() {
        println!("\n[{}/{}] {}", i + 2, n, s.desc);
        println!("      $ {}", s.render());
        s.execute()?;
    }
    // Report the concrete source tree the package was built from.
    println!("built from {}", src.display());
    if let Some(tgt) = &opts.target {
        // The target is rebooting — prove it comes back on the NEW kernel with
        // the full series live before calling the build done.
        verify_remote(tgt, &pre_kernel)?;
        println!("\ndone.");
    } else {
        // Local install: we can't verify across our own reboot in-process, so
        // hand the operator the exact post-reboot check.
        println!("\ndone. reboot to load the liberated kernel, then verify:");
        println!("  sudo arieltune apu doctor --verify");
        println!(
            "  (expect kernel != {pre_kernel}, {}/{} patches live)",
            patches::count(),
            patches::count()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_validation() {
        for ok in ["user@host", "user@example.com", "a-b@host.lan", "u_1@h-2.x"] {
            assert!(valid_target(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "nouser",
            "@host",
            "user@",
            "user@host;rm -rf /",
            "user@host x",
            "us er@host",
            "user@host$(reboot)",
            "user@host'",
        ] {
            assert!(valid_target(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn shell_quoting() {
        assert_eq!(sh_quote("plain"), "'plain'");
        assert_eq!(sh_quote("a b"), "'a b'");
        assert_eq!(sh_quote("a'b"), r"'a'\''b'");
    }

    #[test]
    fn patch_steps_apply_with_zero_fuzz() {
        let opts = BuildOpts {
            pkgbuild_dir: Some(PathBuf::from("/tmp/pkg")),
            ..Default::default()
        };
        let steps =
            post_extract_plan(&opts, Path::new("/tmp/patches"), Path::new("/tmp/src")).unwrap();
        let patch_steps: Vec<_> = steps.iter().filter(|s| s.argv[0] == "patch").collect();
        assert_eq!(patch_steps.len(), patches::count());
        for s in patch_steps {
            assert!(
                s.argv.contains(&"--fuzz=0".to_string()),
                "missing --fuzz=0: {}",
                s.render()
            );
            assert!(s.argv.contains(&"--forward".to_string()));
        }
    }

    #[test]
    fn ssh_scp_argv_shapes() {
        let v = ssh_argv("user@host", &["uname", "-r"]);
        assert_eq!(v[0], "ssh");
        assert!(v.contains(&format!("ConnectTimeout={VERIFY_SSH_TIMEOUT_S}")));
        assert!(v.contains(&"BatchMode=yes".to_string()));
        // Target precedes the remote command (nothing after it but the argv).
        let t = v.iter().position(|s| s == "user@host").unwrap();
        assert_eq!(&v[t + 1..], ["uname", "-r"]);

        let s = scp_argv(Path::new("/proc/self/exe"), "user@host", VERIFY_REMOTE_BIN);
        assert_eq!(s[0], "scp");
        assert_eq!(s.last().unwrap(), "user@host:/tmp/aputune-verify");
        assert!(s.contains(&"/proc/self/exe".to_string()));
    }

    #[test]
    fn doctor_json_parsing() {
        let d = parse_doctor_json(
            "{\"is_bc250\":true,\"kernel\":\"6.12.4-aputune\",\"present\":12,\
             \"total\":12,\"fully\":true}\n",
        )
        .unwrap();
        assert!(d.is_bc250);
        assert_eq!(d.kernel, "6.12.4-aputune");
        assert_eq!(d.present, 12);
        assert_eq!(d.total, 12);
        assert!(d.fully);
        assert!(parse_doctor_json("not json").is_err());
        assert!(parse_doctor_json("").is_err());
    }
}
