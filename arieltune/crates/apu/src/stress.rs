// SPDX-License-Identifier: GPL-2.0-only
//! Deterministic CPU torture for stability detection.
//!
//! The unit of work is a *fixed* compute block with a reproducible checksum: at
//! a stable clock every block yields the same value. An unstable overclock
//! produces a wrong checksum (a silent compute error) — which is exactly the
//! failure we want to catch, and catches it directly rather than waiting for a
//! crash. We compute the reference once at a known-good clock, then every block
//! on every test thread must match it.
//!
//! No external `stress` dependency — the torture is in-process.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crate::telemetry;

/// Iterations per block — heavy enough (~tens of ms) to sample thermals between
/// blocks, light enough to react to the stop flag promptly.
const BLOCK_ITERS: u64 = 40_000_000;

/// One fixed-work compute block. Deterministic on stable silicon: a wrapping
/// integer mixer interleaved with f64 FMA folded back to bits. Marked
/// `inline(never)` so the optimizer can't fold it to a constant.
#[inline(never)]
pub fn block_checksum() -> u64 {
    let mut h: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut f: f64 = 1.000_000_001;
    let mut i: u64 = 0;
    while i < BLOCK_ITERS {
        h = h
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        h ^= h >> 29;
        // FP work that exercises the FPU/SIMD path; folded into the hash.
        f = f.mul_add(1.000_000_000_3, 0.000_000_5);
        if f > 2.0 {
            f -= 1.0;
        }
        h = h.wrapping_add(f.to_bits().rotate_left((i & 63) as u32));
        i += 1;
    }
    h ^ f.to_bits()
}

pub struct StressResult {
    /// True if every block on every thread matched the reference.
    pub stable: bool,
    /// True if the run was cut short by the thermal ceiling.
    pub thermal_abort: bool,
    /// Highest junction temperature observed (C), 0 if unreadable.
    pub max_temp_c: f64,
    /// Total blocks completed across all threads (throughput proxy — reported
    /// for future sweep tooling; not consumed by the current CLI output).
    #[allow(dead_code)]
    pub blocks: u64,
}

/// Run the torture for `dwell` across `threads` cores, comparing each block to
/// `reference`. Aborts early on `temp_ceiling` or when `stop` is set.
pub fn torture(
    threads: usize,
    dwell: Duration,
    reference: u64,
    temp_ceiling: f64,
    stop: &'static AtomicBool,
) -> StressResult {
    let unstable = Arc::new(AtomicBool::new(false));
    let blocks = Arc::new(AtomicU64::new(0));
    let deadline = Instant::now() + dwell;

    let mut handles = Vec::new();
    for _ in 0..threads.max(1) {
        let unstable = unstable.clone();
        let blocks = blocks.clone();
        handles.push(thread::spawn(move || {
            while Instant::now() < deadline
                && !stop.load(Ordering::Relaxed)
                && !unstable.load(Ordering::Relaxed)
            {
                if block_checksum() != reference {
                    unstable.store(true, Ordering::Relaxed);
                    break;
                }
                blocks.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }

    // Main thread samples thermals while the workers run.
    let mut max_temp = 0.0f64;
    let mut thermal_abort = false;
    while Instant::now() < deadline && !stop.load(Ordering::Relaxed) {
        if let Some(t) = telemetry::junction_temp_c() {
            if t > max_temp {
                max_temp = t;
            }
            if t >= temp_ceiling {
                thermal_abort = true;
                stop.store(true, Ordering::Relaxed);
                break;
            }
        }
        if unstable.load(Ordering::Relaxed) {
            break;
        }
        thread::sleep(Duration::from_millis(200));
    }

    for h in handles {
        let _ = h.join();
    }

    StressResult {
        stable: !unstable.load(Ordering::Relaxed) && !thermal_abort,
        thermal_abort,
        max_temp_c: max_temp,
        blocks: blocks.load(Ordering::Relaxed),
    }
}
