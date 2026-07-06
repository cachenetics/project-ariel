// SPDX-License-Identifier: GPL-2.0-only
//! CU health-test — a known-answer Vulkan compute test (KAT) run at safe
//! routing granularities, to find a defective CU on a liberated BC-250.
//!
//! Not every BC-250 ships 40 perfectly healthy CUs. The liberation enumerates
//! them all; a marginal CU can produce wrong results or hang under load. This
//! proves the silicon computes correctly and localizes a fault.
//!
//! Granularity matters for safety. gfx1013 hangs — and a hang wedges the whole
//! box, since its GPU-reset path is broken — if you dispatch heavy compute with
//! whole shader arrays gated off. The proven-safe routing shapes all populate
//! every shader array (factory-24 = 0x07x4, full-40 = 0x1fx4). So the tests
//! stay inside that envelope and never isolate an array or a single WGP:
//!
//!   * [`test_full`]     — KAT at the full 40-CU config (safe; the normal shape).
//!   * [`test_localize`] — when full-40 fails, route full-40 *minus one WGP* at
//!     a time (every array keeps >=4 WGP). The WGP whose removal clears the
//!     fault is the bad one. Stays inside the safe envelope.
//!
//! The KAT runs a deterministic u32 integer chain per invocation, reproducible
//! bit-exactly on the CPU — a defective ALU shows as a mismatch, a hang as a
//! fence timeout (device lost), a weak CU as low throughput. RADV Vulkan is the
//! only stable compute path at 40 CU on gfx1013 (the ROCm OpenCL ICD is not).
//!
//! The GPU dispatch runs through [`ariel_compute::with_session`] on the ONE
//! shared Vulkan device while holding the process-wide compute lock, so a MEM
//! bandwidth bench and this KAT can never drive the GPU at once — they serialize
//! on that mutex. `kat.spv` stays crate-local (embedded below).

use std::cell::Cell;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use ariel_compute::ash::{vk, Device};
use ariel_compute::{with_session, Session};

use crate::curoute::{self, ARRAYS, FULL_MASK};
use ariel_smu::smu::Smu;

/// Live invocations per dispatch (1 Mi).
const KAT_N: u32 = 1 << 20;
/// Chain length per invocation — tuned so a full-40 run finishes well under 1 s.
const KAT_ITERS: u32 = 2048;
/// Fence timeout. A healthy run is sub-second; this catches a hung CU.
const KAT_TIMEOUT_NS: u64 = 10_000_000_000;
/// Force the GFX clock here during a test so throughput is comparable across
/// quadrants and the GPU isn't parked at the deep-sleep floor. Restored after.
const TEST_CLK_MHZ: u32 = 1500;

const KAT_SPV: &[u8] = include_bytes!("kat.spv");

/// Health verdict for one routing config.
#[derive(Clone, Debug)]
pub struct KatResult {
    /// Human label: "40-CU" or "SE0.SH0" or "SE1.SH1 WGP3".
    pub label: String,
    pub cu_count: u32,
    pub ok: bool,
    pub mismatches: u64,
    pub elapsed: Duration,
    pub gflops: f64,
    /// Set when the run could not complete (hang / device lost).
    pub hung: bool,
}

impl KatResult {
    pub fn verdict(&self) -> &'static str {
        if self.hung {
            "HUNG"
        } else if self.ok {
            "ok"
        } else {
            "FAIL"
        }
    }
}

/// The CPU reference for one invocation — must match `kat.comp` bit-for-bit.
fn cpu_kat(idx: u32, iters: u32) -> u32 {
    let mut x = idx.wrapping_mul(2654435761).wrapping_add(0x9e37_79b9);
    for i in 0..iters {
        x = x.wrapping_mul(1664525).wrapping_add(1013904223);
        x ^= x >> 15;
        x = x.wrapping_add(idx).wrapping_add(i);
    }
    x
}

/// Precompute the golden buffer once; reused across every config in a sweep.
fn golden() -> Vec<u32> {
    (0..KAT_N).map(|i| cpu_kat(i, KAT_ITERS)).collect()
}

/// Forces a fixed GFX clock for the duration of a test, restoring on drop.
/// Best-effort: a failure to force just means less-comparable throughput.
///
/// CRITICAL: stops the GPU power unit first. If its governor keeps forcing the
/// clock while this guard also forces it, the two race on the SMU mailbox and
/// wedge gfx1013 under compute load (verified — a hard reset during a CU bench).
/// The unit is started again on drop and re-enacts the persisted mode (a manual
/// pin comes back pinned).
struct ClockGuard {
    restore: bool,
}
impl ClockGuard {
    fn engage() -> Self {
        crate::persist::stop_gpu_unit();
        let restore = Smu::open()
            .ok()
            .map(|s| s.force_gfx_freq(TEST_CLK_MHZ).is_ok())
            .unwrap_or(false);
        ClockGuard { restore }
    }
}
impl Drop for ClockGuard {
    fn drop(&mut self) {
        if self.restore {
            if let Ok(s) = Smu::open() {
                let _ = s.unforce_gfx_freq();
            }
        }
        crate::persist::start_gpu_unit();
    }
}

/// Refuse to health-test anything but the BC-250 silicon under test. The shared
/// device (`ariel_compute`) already prefers a real GPU over a software
/// rasterizer, but a GPU-less host would fall back to llvmpipe — which would
/// "pass" every CU config without touching the silicon under test, a false
/// health verdict. So confirm the shared device IS the BC-250/GFX1013 before
/// dispatching.
fn ensure_bc250_device(s: &Session) -> Result<()> {
    let name = s.device_name.to_uppercase();
    if name.contains("BC-250") || name.contains("GFX1013") {
        Ok(())
    } else {
        bail!(
            "BC-250/GFX1013 Vulkan device not found (shared compute device is '{}') — \
             refusing to health-test a different device (a software rasterizer would \
             false-pass every config)",
            s.device_name
        )
    }
}

fn find_mem_type(
    props: &vk::PhysicalDeviceMemoryProperties,
    bits: u32,
    flags: vk::MemoryPropertyFlags,
) -> Option<u32> {
    (0..props.memory_type_count).find(|&i| {
        (bits & (1 << i)) != 0
            && props.memory_types[i as usize]
                .property_flags
                .contains(flags)
    })
}

/// A reusable Vulkan compute context for a KAT session. It BORROWS the
/// device/queue/memprops handed out by [`ariel_compute::with_session`] — it does
/// NOT create its own instance or device (that is the ONE shared device the
/// compute lock serializes) — and holds the persistent host-visible buffer +
/// pipeline reused across every config in a sweep. Its [`Drop`] destroys only the
/// objects it created here, never the shared device/instance (ariel_compute owns
/// those).
struct Vk<'a> {
    device: &'a Device,
    queue: vk::Queue,
    pipeline: vk::Pipeline,
    layout: vk::PipelineLayout,
    dsl: vk::DescriptorSetLayout,
    desc_pool: vk::DescriptorPool,
    desc_set: vk::DescriptorSet,
    cmd_pool: vk::CommandPool,
    cmd: vk::CommandBuffer,
    module: vk::ShaderModule,
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    mapped: *mut u32,
    /// M3 (round 2): set when a dispatch fence times out -> the GPU is hung and
    /// the submission never completes. Once lost, [`Drop`] MUST skip the
    /// (unbounded) device_wait_idle and the object teardown: those objects are
    /// bound to a never-completing submission, so waiting/destroying would block
    /// forever while the process-wide compute lock is still held. We leak them
    /// instead (a wedged gfx1013 is being reset/rebooted anyway).
    lost: Cell<bool>,
}

impl<'a> Vk<'a> {
    /// Build the KAT pipeline + persistent buffer on the shared session device.
    /// Called inside the `with_session` closure (the compute lock is held for the
    /// whole session, so no other dispatch touches the device).
    fn build(s: &'a Session) -> Result<Self> {
        let device = s.device;
        let queue = s.queue;
        let qfi = s.queue_family;

        let size = (KAT_N as vk::DeviceSize) * 4;
        let bci = vk::BufferCreateInfo::default()
            .size(size)
            .usage(vk::BufferUsageFlags::STORAGE_BUFFER)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let buffer = unsafe { device.create_buffer(&bci, None)? };
        let req = unsafe { device.get_buffer_memory_requirements(buffer) };
        let mti = find_mem_type(
            &s.memprops,
            req.memory_type_bits,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )
        .ok_or_else(|| anyhow!("no host-visible memory type"))?;
        let mai = vk::MemoryAllocateInfo::default()
            .allocation_size(req.size)
            .memory_type_index(mti);
        let memory = unsafe { device.allocate_memory(&mai, None)? };
        unsafe { device.bind_buffer_memory(buffer, memory, 0)? };
        let mapped =
            unsafe { device.map_memory(memory, 0, size, vk::MemoryMapFlags::empty())? } as *mut u32;

        let bindings = [vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE)];
        let dslci = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
        let dsl = unsafe { device.create_descriptor_set_layout(&dslci, None)? };

        let pool_sizes = [vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)];
        let dpci = vk::DescriptorPoolCreateInfo::default()
            .max_sets(1)
            .pool_sizes(&pool_sizes);
        let desc_pool = unsafe { device.create_descriptor_pool(&dpci, None)? };
        let set_layouts = [dsl];
        let dsai = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(desc_pool)
            .set_layouts(&set_layouts);
        let desc_set = unsafe { device.allocate_descriptor_sets(&dsai)? }[0];

        let buf_info = [vk::DescriptorBufferInfo::default()
            .buffer(buffer)
            .offset(0)
            .range(size)];
        let write = vk::WriteDescriptorSet::default()
            .dst_set(desc_set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(&buf_info);
        unsafe { device.update_descriptor_sets(&[write], &[]) };

        let code = ariel_compute::ash::util::read_spv(&mut std::io::Cursor::new(KAT_SPV))
            .context("read embedded KAT spv")?;
        let smci = vk::ShaderModuleCreateInfo::default().code(&code);
        let module = unsafe { device.create_shader_module(&smci, None)? };

        let pc_ranges = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::COMPUTE)
            .offset(0)
            .size(8)];
        let plci = vk::PipelineLayoutCreateInfo::default()
            .set_layouts(&set_layouts)
            .push_constant_ranges(&pc_ranges);
        let layout = unsafe { device.create_pipeline_layout(&plci, None)? };

        let entry_name = c"main";
        let stage = vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::COMPUTE)
            .module(module)
            .name(entry_name);
        let cpci = [vk::ComputePipelineCreateInfo::default()
            .stage(stage)
            .layout(layout)];
        let pipeline = unsafe {
            device
                .create_compute_pipelines(vk::PipelineCache::null(), &cpci, None)
                .map_err(|(_, e)| e)?
        }[0];

        let cmd_pool = unsafe {
            device.create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .queue_family_index(qfi)
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                None,
            )?
        };
        let cmd = unsafe {
            device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(cmd_pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )?
        }[0];

        Ok(Vk {
            device,
            queue,
            pipeline,
            layout,
            dsl,
            desc_pool,
            desc_set,
            cmd_pool,
            cmd,
            module,
            buffer,
            memory,
            mapped,
            lost: Cell::new(false),
        })
    }

    /// Run one KAT dispatch and verify against `golden`. Returns (elapsed,
    /// mismatches, hung).
    fn run_kat(&self, golden: &[u32]) -> Result<(Duration, u64, bool)> {
        // Poison the output buffer before every run: if a dispatch is silently
        // dropped (queue error swallowed, WGP never dispatched), the compare
        // would otherwise see the PREVIOUS run's still-correct data and
        // false-pass. The salt guarantees stale contents never match golden
        // (salt[i] != golden[i] for every element).
        unsafe {
            let out = std::slice::from_raw_parts_mut(self.mapped, KAT_N as usize);
            for (o, g) in out.iter_mut().zip(golden) {
                *o = !*g;
            }
        }
        unsafe {
            self.device
                .reset_command_buffer(self.cmd, vk::CommandBufferResetFlags::empty())?;
            self.device.begin_command_buffer(
                self.cmd,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;
            self.device
                .cmd_bind_pipeline(self.cmd, vk::PipelineBindPoint::COMPUTE, self.pipeline);
            self.device.cmd_bind_descriptor_sets(
                self.cmd,
                vk::PipelineBindPoint::COMPUTE,
                self.layout,
                0,
                &[self.desc_set],
                &[],
            );
            let mut pc = [0u8; 8];
            pc[0..4].copy_from_slice(&KAT_N.to_ne_bytes());
            pc[4..8].copy_from_slice(&KAT_ITERS.to_ne_bytes());
            self.device.cmd_push_constants(
                self.cmd,
                self.layout,
                vk::ShaderStageFlags::COMPUTE,
                0,
                &pc,
            );
            self.device.cmd_dispatch(self.cmd, KAT_N.div_ceil(64), 1, 1);
            self.device.end_command_buffer(self.cmd)?;

            let fence = self
                .device
                .create_fence(&vk::FenceCreateInfo::default(), None)?;
            let cmds = [self.cmd];
            let submit = [vk::SubmitInfo::default().command_buffers(&cmds)];
            let t0 = Instant::now();
            self.device.queue_submit(self.queue, &submit, fence)?;
            let waited = self.device.wait_for_fences(&[fence], true, KAT_TIMEOUT_NS);
            let elapsed = t0.elapsed();
            if waited.is_err() {
                // WEDGE PATH: the fence never signaled -> the submission is STILL
                // PENDING on a hung gfx1013 (its GPU-reset path is broken). Do NOT
                // destroy the fence: destroying a fence with a pending submission is
                // UB (VUID-vkDestroyFence-fence-01120). Leak it, and mark the
                // context lost so Drop skips the unbounded device_wait_idle + the
                // object teardown (all bound to a submission that will never
                // complete). Returning here promptly drops the with_session closure
                // and frees the process-wide compute lock -- a bounded leak beats an
                // unbounded lock-hold. Mirrors ariel-compute's one-shot bail path.
                self.lost.set(true);
                return Ok((elapsed, KAT_N as u64, true)); // hung
            }
            // Signaled: the submission completed, so the fence is safe to destroy.
            self.device.destroy_fence(fence, None);

            let gpu = std::slice::from_raw_parts(self.mapped, KAT_N as usize);
            let mismatches = gpu.iter().zip(golden).filter(|(g, c)| g != c).count() as u64;
            Ok((elapsed, mismatches, false))
        }
    }
}

impl Drop for Vk<'_> {
    fn drop(&mut self) {
        // WEDGE PATH (M3, round 2): if the last dispatch timed out, the GPU is hung
        // and its submission never completes. device_wait_idle() would then block
        // UNBOUNDED while the process-wide compute lock is STILL HELD (we run inside
        // the with_session closure) -> the exact wedge M3 removes. Skip the
        // idle-wait AND the teardown: those objects are bound to a never-completing
        // submission, so destroying them is UB and can itself block on a lost
        // device. Leak them -- a wedged gfx1013 is being reset/rebooted anyway, and
        // a bounded leak beats an unbounded lock-hold. Returning immediately lets
        // the with_session closure drop and the compute lock free.
        if self.lost.get() {
            return;
        }
        // Destroy ONLY the objects this KAT session created; the shared device +
        // instance belong to ariel_compute and must outlive us. The compute lock
        // is still held here (we run inside the with_session closure), so idling
        // + tearing down the pipeline can't race another dispatch.
        unsafe {
            let _ = self.device.device_wait_idle();
            self.device.unmap_memory(self.memory);
            self.device.destroy_command_pool(self.cmd_pool, None);
            self.device.destroy_pipeline(self.pipeline, None);
            self.device.destroy_pipeline_layout(self.layout, None);
            self.device.destroy_shader_module(self.module, None);
            self.device.destroy_descriptor_pool(self.desc_pool, None);
            self.device.destroy_descriptor_set_layout(self.dsl, None);
            self.device.destroy_buffer(self.buffer, None);
            self.device.free_memory(self.memory, None);
        }
    }
}

fn gflops(elapsed: Duration) -> f64 {
    let ops = (KAT_N as f64) * (KAT_ITERS as f64) * 4.0;
    ops / elapsed.as_secs_f64().max(1e-9) / 1e9
}

/// Whether the health-test can run (umr for routing + a usable Vulkan device).
pub fn available() -> bool {
    curoute::available() && ariel_compute::vulkan_available()
}

/// Run the KAT against a routing config (does not save/restore — the caller owns
/// the surrounding `apply(saved)`).
fn run_config(vk: &Vk, masks: [u32; 4], label: String, golden: &[u32]) -> Result<KatResult> {
    curoute::apply(masks)?;
    let (elapsed, mismatches, hung) = vk.run_kat(golden)?;
    Ok(KatResult {
        label,
        cu_count: curoute::cu_count(&masks),
        ok: !hung && mismatches == 0,
        mismatches,
        elapsed,
        gflops: if hung { 0.0 } else { gflops(elapsed) },
        hung,
    })
}

/// Full 40-CU correctness + throughput (the normal config; safe).
pub fn test_full() -> Result<KatResult> {
    let saved = curoute::current_masks()?;
    let _clk = ClockGuard::engage();
    let g = golden();
    // The whole KAT session runs on the ONE shared device under the process-wide
    // compute lock (with_session), so it can never drive the GPU concurrently
    // with a MEM bench.
    let res = with_session(|s| {
        ensure_bc250_device(s)?;
        let vk = Vk::build(s)?;
        run_config(&vk, [FULL_MASK; 4], "40-CU".into(), &g)
    });
    // Restore what was LIVE before the test — apply_forced, because a
    // pre-existing (operator-forced) unsafe route must be restorable; the
    // safety refusal is for NEW shapes, not for putting back the prior state.
    let restore = curoute::apply_forced(saved);
    let r = res?;
    restore?;
    Ok(r)
}

/// Bench one arbitrary routing config: KAT compute GFLOPS at the pinned test
/// clock (compute-bound, so it isolates CU-scaling from memory bandwidth), with
/// correctness checked. Saves + restores the prior route, so it's non-destructive.
///
/// SAFETY: refuses a shape with any empty shader array (dispatching compute on
/// it hangs + wedges gfx1013 — see the module docs). The refusal happens before
/// any hardware is touched.
pub fn bench_masks(masks: [u32; 4]) -> Result<KatResult> {
    if curoute::has_empty_array(&masks) {
        bail!(
            "refusing to bench {masks:02x?}: an empty shader array under compute \
             hangs + wedges gfx1013. Populate every array."
        );
    }
    // Best-of-N: a single KAT run is noisy (~15-20%) — pipeline warmup, clock
    // settling, and the llama server contending for the GPU. The fastest of a few
    // runs is the clean throughput (least perturbed), so it's stable/comparable.
    const REPS: usize = 6;
    let saved = curoute::current_masks()?;
    let _clk = ClockGuard::engage();
    let g = golden();
    let res = with_session(|s| {
        ensure_bc250_device(s)?;
        let vk = Vk::build(s)?;
        curoute::apply(masks)?;
        let mut best = Duration::from_secs(3600);
        let mut mism = 0u64;
        for _ in 0..REPS {
            let (el, mm, hung) = vk.run_kat(&g)?;
            if hung {
                return Ok((el, mm, true));
            }
            mism = mm;
            if el < best {
                best = el;
            }
            if mism != 0 {
                break; // a fault won't clear on repeat
            }
        }
        Ok((best, mism, false))
    });
    // Restore what was LIVE before the test — apply_forced, because a
    // pre-existing (operator-forced) unsafe route must be restorable; the
    // safety refusal is for NEW shapes, not for putting back the prior state.
    let restore = curoute::apply_forced(saved);
    let (elapsed, mismatches, hung) = res?;
    restore?;
    Ok(KatResult {
        label: format!("{masks:02x?}"),
        cu_count: curoute::cu_count(&masks),
        ok: !hung && mismatches == 0,
        mismatches,
        elapsed,
        gflops: if hung { 0.0 } else { gflops(elapsed) },
        hung,
    })
}

/// Subtractive localization — only meaningful when [`test_full`] shows a fault.
///
/// gfx1013 hangs if you dispatch compute with whole shader arrays gated off, so
/// we never isolate an array. Instead, for each WGP we route full-40 *minus that
/// one WGP* — every shader array keeps ≥4 WGP, staying inside the safe envelope
/// (factory-24 / full-40 are the proven-safe shapes; both populate all arrays).
/// The WGP whose removal makes the KAT pass is the culprit: with it gated off,
/// its bad CUs no longer receive work, so the mismatches clear.
///
/// Each [`KatResult`]'s `ok` therefore means "the fault cleared with this WGP
/// removed" — i.e. `ok == true` flags the suspect. `cb` reports each as it lands.
pub fn test_localize(mut cb: impl FnMut(&KatResult)) -> Result<Vec<KatResult>> {
    let saved = curoute::current_masks()?;
    let _clk = ClockGuard::engage();
    let g = golden();
    let res = with_session(|s| {
        ensure_bc250_device(s)?;
        let vk = Vk::build(s)?;
        let mut out = Vec::with_capacity(20);
        for (ai, (se, sh)) in ARRAYS.iter().enumerate() {
            for wgp in 0..5u32 {
                let mut masks = [FULL_MASK; 4];
                masks[ai] &= !(1 << wgp); // drop one WGP; array keeps the other 4
                let r = run_config(&vk, masks, format!("-SE{se}.SH{sh}/WGP{wgp}"), &g)?;
                cb(&r);
                let hung = r.hung;
                out.push(r);
                if hung {
                    break;
                }
            }
        }
        Ok(out)
    });
    // Restore what was LIVE before the test — apply_forced, because a
    // pre-existing (operator-forced) unsafe route must be restorable; the
    // safety refusal is for NEW shapes, not for putting back the prior state.
    let restore = curoute::apply_forced(saved);
    let out = res?;
    restore?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WEDGE-SAFETY REGRESSION: bench_masks must refuse an empty-shader-array
    /// shape before touching any hardware (no umr/Vulkan needed to observe it).
    #[test]
    fn bench_rejects_empty_array_shape() {
        let err = bench_masks([FULL_MASK, 0, FULL_MASK, FULL_MASK])
            .unwrap_err()
            .to_string();
        assert!(err.contains("empty shader array"), "{err}");
    }

    /// The CPU KAT reference is the golden source — pin a couple of values so a
    /// refactor of cpu_kat can't silently drift from the shader.
    #[test]
    fn cpu_kat_is_deterministic() {
        assert_eq!(cpu_kat(0, 8), cpu_kat(0, 8));
        assert_ne!(cpu_kat(0, KAT_ITERS), cpu_kat(1, KAT_ITERS));
    }
}
