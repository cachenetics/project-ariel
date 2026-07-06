// SPDX-License-Identifier: GPL-2.0-only
//! System-RAM integrity test — a built-in CPU memory check, the complement to
//! the GPU bench's buffer check.
//!
//! The GPU integrity test (`bench`) only exercises GPU-allocated buffers. But a
//! too-aggressive memory config corrupts CPU-side data too — page cache, the
//! kernel, files on their way to disk — which the GPU check never sees. (We
//! learned this the hard way: a config that benched "integrity OK" still flipped
//! a byte in a cached file.) This hammers a large slab of system RAM with
//! moving-inversion patterns so a config that silently corrupts the OS is caught
//! HERE, in a throwaway buffer, instead of on disk.
//!
//! No external deps (no `memtester`/`stressapptest`) — same philosophy as the
//! built-in Vulkan bench.

use std::hint::black_box;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};

pub struct SysMemResult {
    pub errors: u64,
    pub bytes: u64,
    pub passes: u32,
    pub secs: f64,
}

/// Odd multiplier (golden-ratio) so the per-cell pattern depends on the address —
/// catches address-decode faults as well as stuck/flipping bits.
const MIX: u64 = 0x9E37_79B9_7F4A_7C15;

#[inline]
fn xorshift(mut x: u64) -> u64 {
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    x
}

/// Allocate `mb` MiB as a `Vec<u64>`, halving on failure down to a floor so the
/// test still runs if the requested size doesn't fit alongside other workloads.
// try_reserve (fallible alloc) is deliberate — we must handle OOM gracefully, not
// panic, so the `vec![0; n]` clippy suggests is wrong here.
#[allow(clippy::slow_vector_initialization)]
fn alloc(mb: usize) -> Result<Vec<u64>> {
    for size in [mb, mb / 2, mb / 4, 256] {
        let n = size.saturating_mul(1024 * 1024 / 8);
        if n == 0 {
            continue;
        }
        let mut v: Vec<u64> = Vec::new();
        if v.try_reserve_exact(n).is_ok() {
            v.resize(n, 0);
            return Ok(v);
        }
    }
    bail!("could not allocate a system-RAM test buffer (out of memory?)");
}

/// Moving-inversions test over `mb` MiB of system RAM for at least `min_secs`.
///
/// Each pass: write an address-derived pattern, read it back and verify while
/// writing the bitwise inverse, then verify the inverse. A mismatch means RAM
/// gave back something other than what was written — i.e. the memory config is
/// corrupting data. The slab is far larger than CPU cache, so reads/writes hit
/// physical RAM. `black_box` keeps the compiler from eliding the verify.
pub fn test(mb: usize, min_secs: u64) -> Result<SysMemResult> {
    let mut buf = alloc(mb)?;
    let n = buf.len();
    let bytes = (n * 8) as u64;
    let mut errors: u64 = 0;
    let mut passes: u32 = 0;
    let mut seed: u64 = 0x1234_5678_9abc_def0;
    let start = Instant::now();

    loop {
        seed = xorshift(seed | 1);
        // write address-derived pattern
        for (i, c) in buf.iter_mut().enumerate() {
            *c = (i as u64).wrapping_mul(MIX) ^ seed;
        }
        // verify, then write the inverse
        for (i, c) in buf.iter_mut().enumerate() {
            let expect = (i as u64).wrapping_mul(MIX) ^ seed;
            if black_box(*c) != expect {
                errors += 1;
            }
            *c = !expect;
        }
        // verify the inverse
        for (i, c) in buf.iter().enumerate() {
            let expect = !((i as u64).wrapping_mul(MIX) ^ seed);
            if black_box(*c) != expect {
                errors += 1;
            }
        }
        passes += 1;
        if start.elapsed() >= Duration::from_secs(min_secs) {
            break;
        }
    }

    Ok(SysMemResult {
        errors,
        bytes,
        passes,
        secs: start.elapsed().as_secs_f64(),
    })
}
