// SPDX-License-Identifier: GPL-2.0-only
//! Memory bandwidth + latency benchmark.
//!
//! A **Vulkan compute** bench (RADV is the BC-250's most reliable GPU backend).
//! Bandwidth = a grid-stride read kernel over a large GDDR6 buffer; latency = a
//! pointer-chase over a random permutation. A warmup pass drives the GPU to its
//! top DPM clock step before any measurement, so every metric is read at the
//! same (top) clock — not whatever level the governor happened to be at.
//!
//! The whole bench runs on the ONE shared Vulkan device owned by
//! [`ariel_compute`], inside a single [`ariel_compute::with_session`] — so it
//! holds the process-wide compute lock for its full duration and can never drive
//! the GPU concurrently with an APU KAT (or any other `ariel_compute` dispatch).
//! The bench keeps its own multi-dispatch / persistent-buffer / per-dispatch-
//! timing machinery (a one-shot `dispatch` can't host it); only device creation
//! moved onto the shared harness.
//!
//! Coordinating with aputune: aputune's app-driven autosleep FORCE-LOCKS the GFX
//! clock (deep-sleep 350 MHz when idle), which a load-driven warmup can't move —
//! so the scattered-read + latency phases (which run on the shader cores) would
//! read badly low. We touch aputune's poke file to wake it to the top setpoint
//! for the duration of the bench. No-op when aputune isn't present (the fence-
//! rate governor is then driven the old way).

use anyhow::Result;

#[derive(Clone, Copy, Debug)]
pub struct BenchResult {
    pub bandwidth_gbps: f64,
    /// Random-access read throughput (GB/s) — far lower than sequential, and the
    /// number that actually responds to timing changes.
    pub random_gbps: f64,
    pub latency_ns: f64,
    /// Mismatches found by the write/read-back integrity check (0 = stable).
    pub stability_errors: u64,
    /// Bytes covered by the integrity check.
    pub stability_bytes: u64,
}

/// Full bench (bandwidth + random + latency + integrity), via Vulkan compute.
/// Each metric is measured at the GPU's sustainable top clock (the warmup drives
/// the governor there; forcing the absolute max overheats this board), so results
/// are consistent. Errors if no usable Vulkan device is present.
pub fn run() -> Result<BenchResult> {
    vk::run()
}

/// Light probe: is a usable (non-software) Vulkan device present? For `doctor` —
/// does not run any kernels.
pub fn vulkan_available() -> bool {
    vk::available()
}

// ---- Vulkan compute bench -------------------------------------------------

mod vk {
    use std::io::Cursor;
    use std::time::{Duration, Instant};

    use anyhow::{anyhow, Result};
    use ash::vk;

    use super::BenchResult;

    const BW_SPV: &[u8] = include_bytes!("shaders/bandwidth.spv");
    const LAT_SPV: &[u8] = include_bytes!("shaders/latency.spv");
    const RAND_SPV: &[u8] = include_bytes!("shaders/random.spv");
    const STAB_WRITE_SPV: &[u8] = include_bytes!("shaders/stab_write.spv");
    const STAB_CHECK_SPV: &[u8] = include_bytes!("shaders/stab_check.spv");

    /// Quick integrity passes run as part of a normal bench (a few seeds).
    const STAB_PASSES: u32 = 3;

    /// M3: bounded fence wait. A GPU hang is EXPECTED when stressing marginal
    /// GDDR6 timings, and gfx1013 has no working GPU reset — an unbounded
    /// `wait_for_fences(.., u64::MAX)` would hang the thread forever WHILE holding
    /// the process-wide `ariel_compute` compute lock, wedging any later APU KAT
    /// too. Cap the wait (generous: exceeds cutest's 10s KAT budget so a slow but
    /// legitimate dispatch is never killed) and, on timeout, surface a device-lost
    /// error so the session unwinds and releases the lock instead of spinning.
    const FENCE_TIMEOUT_NS: u64 = 20_000_000_000; // 20 s

    /// Run the full bench on `ariel_compute`'s ONE shared device, holding the
    /// process-wide compute lock for the whole session (so MEM and an APU KAT
    /// serialize — R4). The instance/device/queue are the harness's; only the
    /// bench's own buffers/pipelines/timing live here.
    pub fn run() -> Result<BenchResult> {
        ariel_compute::with_session(|s| unsafe {
            bench_all(s.device, s.queue, s.queue_family, &s.memprops)
        })
    }

    /// Quick availability probe: loader + instance + at least one device.
    /// Delegates to the shared harness (same device-selection policy).
    pub fn available() -> bool {
        ariel_compute::vulkan_available()
    }

    fn find_mem(
        props: &vk::PhysicalDeviceMemoryProperties,
        type_bits: u32,
        flags: vk::MemoryPropertyFlags,
    ) -> Option<u32> {
        (0..props.memory_type_count).find(|&i| {
            type_bits & (1 << i) != 0
                && props.memory_types[i as usize]
                    .property_flags
                    .contains(flags)
        })
    }

    struct Buf {
        buffer: vk::Buffer,
        memory: vk::DeviceMemory,
    }

    unsafe fn make_buffer(
        device: &ash::Device,
        memprops: &vk::PhysicalDeviceMemoryProperties,
        size: u64,
        host_visible: bool,
    ) -> Result<Buf> {
        let bci = vk::BufferCreateInfo::default()
            .size(size)
            .usage(vk::BufferUsageFlags::STORAGE_BUFFER)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let buffer = device.create_buffer(&bci, None)?;
        let req = device.get_buffer_memory_requirements(buffer);
        // Prefer device-local; if host_visible, require it (mappable). On this APU
        // device-local+host-visible types exist, so we get both.
        let want = if host_visible {
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT
        } else {
            vk::MemoryPropertyFlags::DEVICE_LOCAL
        };
        let mt = find_mem(memprops, req.memory_type_bits, want)
            .or_else(|| {
                find_mem(
                    memprops,
                    req.memory_type_bits,
                    vk::MemoryPropertyFlags::DEVICE_LOCAL,
                )
            })
            .or_else(|| {
                find_mem(
                    memprops,
                    req.memory_type_bits,
                    vk::MemoryPropertyFlags::HOST_VISIBLE,
                )
            })
            .ok_or_else(|| anyhow!("no suitable memory type"))?;
        let ai = vk::MemoryAllocateInfo::default()
            .allocation_size(req.size)
            .memory_type_index(mt);
        let memory = device
            .allocate_memory(&ai, None)
            .map_err(|e| anyhow!("allocate {size} bytes: {e}"))?;
        device.bind_buffer_memory(buffer, memory, 0)?;
        Ok(Buf { buffer, memory })
    }

    unsafe fn destroy_buf(device: &ash::Device, b: &Buf) {
        device.destroy_buffer(b.buffer, None);
        device.free_memory(b.memory, None);
    }

    unsafe fn bench_all(
        device: &ash::Device,
        queue: vk::Queue,
        qf: u32,
        memprops: &vk::PhysicalDeviceMemoryProperties,
    ) -> Result<BenchResult> {
        // Shared: descriptor layout (2 storage buffers), pipeline layout (8B push).
        let bindings = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(1)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
        ];
        let dsl = device.create_descriptor_set_layout(
            &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
            None,
        )?;
        let set_layouts = [dsl];
        let push = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::COMPUTE)
            .offset(0)
            .size(8)];
        let playout = device.create_pipeline_layout(
            &vk::PipelineLayoutCreateInfo::default()
                .set_layouts(&set_layouts)
                .push_constant_ranges(&push),
            None,
        )?;

        let pool_sizes = [vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(16)];
        let dpool = device.create_descriptor_pool(
            &vk::DescriptorPoolCreateInfo::default()
                .max_sets(8)
                .pool_sizes(&pool_sizes),
            None,
        )?;

        let bw_pipe = make_pipeline(device, playout, BW_SPV)?;
        let rand_pipe = make_pipeline(device, playout, RAND_SPV)?;
        let lat_pipe = make_pipeline(device, playout, LAT_SPV)?;
        let stab_write_pipe = make_pipeline(device, playout, STAB_WRITE_SPV)?;
        let stab_check_pipe = make_pipeline(device, playout, STAB_CHECK_SPV)?;

        let cpool = device.create_command_pool(
            &vk::CommandPoolCreateInfo::default()
                .queue_family_index(qf)
                .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
            None,
        )?;
        let cmd = device.allocate_command_buffers(
            &vk::CommandBufferAllocateInfo::default()
                .command_pool(cpool)
                .level(vk::CommandBufferLevel::PRIMARY)
                .command_buffer_count(1),
        )?[0];
        let fence = device.create_fence(&vk::FenceCreateInfo::default(), None)?;

        // Warm the GPU to its top DPM clock step ONCE, up front, so every metric
        // below is measured at the same (top) clock instead of whatever level the
        // governor happened to be at. The streaming bandwidth kernel is the load.
        warm_to_peak(
            device, queue, memprops, dsl, dpool, playout, bw_pipe, cmd, fence,
        )?;
        // Now pre-warmed: bandwidth is a short measurement burst, random + latency
        // run warm (latency keeps its own front-load between samples).
        let bandwidth = run_bandwidth(
            device, queue, memprops, dsl, dpool, playout, bw_pipe, cmd, fence,
        )?;
        // `bw_pipe` doubles as the front-load that holds the clock high while the
        // (low-load) random + latency samples run.
        //
        // Re-confirm the top clock right before random. Its samples are slow and
        // low-load, so its own front-load can HOLD run but can't reliably CLIMB back
        // if the clock sagged after bandwidth — which made random intermittently read
        // at idle. A fresh warm_to_peak (proven-safe, exits in ~1s when already warm)
        // guarantees we enter the random samples at run; its front-load then holds it.
        warm_to_peak(
            device, queue, memprops, dsl, dpool, playout, bw_pipe, cmd, fence,
        )?;
        // (No stderr warning here: it would write raw to the terminal mid-TUI
        // render and corrupt the display. We now warm to aputune's actual top
        // clock, and the measured panel already shows the live clock, so a
        // "not at run" note is both redundant and harmful to the UI.)
        let random = run_random(
            device, queue, memprops, dsl, dpool, playout, rand_pipe, bw_pipe, cmd, fence,
        )?;
        let latency = run_latency(
            device, queue, memprops, dsl, dpool, playout, lat_pipe, bw_pipe, cmd, fence,
        )?;
        let (stability_errors, stability_bytes) = run_stability(
            device,
            queue,
            memprops,
            dsl,
            dpool,
            playout,
            stab_write_pipe,
            stab_check_pipe,
            cmd,
            fence,
            STAB_PASSES,
        )?;

        // cleanup
        device.destroy_fence(fence, None);
        device.destroy_command_pool(cpool, None);
        device.destroy_pipeline(bw_pipe, None);
        device.destroy_pipeline(rand_pipe, None);
        device.destroy_pipeline(lat_pipe, None);
        device.destroy_pipeline(stab_write_pipe, None);
        device.destroy_pipeline(stab_check_pipe, None);
        device.destroy_descriptor_pool(dpool, None);
        device.destroy_pipeline_layout(playout, None);
        device.destroy_descriptor_set_layout(dsl, None);

        Ok(BenchResult {
            bandwidth_gbps: bandwidth,
            random_gbps: random,
            latency_ns: latency,
            stability_errors,
            stability_bytes,
        })
    }

    unsafe fn make_pipeline(
        device: &ash::Device,
        layout: vk::PipelineLayout,
        spv: &[u8],
    ) -> Result<vk::Pipeline> {
        let code = ash::util::read_spv(&mut Cursor::new(spv))?;
        let module = device
            .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&code), None)?;
        let stage = vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::COMPUTE)
            .module(module)
            .name(c"main");
        let ci = vk::ComputePipelineCreateInfo::default()
            .stage(stage)
            .layout(layout);
        let pipe = device
            .create_compute_pipelines(vk::PipelineCache::null(), &[ci], None)
            .map_err(|(_, e)| anyhow!("create_compute_pipelines: {e}"))?[0];
        device.destroy_shader_module(module, None);
        Ok(pipe)
    }

    unsafe fn alloc_set(
        device: &ash::Device,
        dpool: vk::DescriptorPool,
        dsl: vk::DescriptorSetLayout,
        b0: vk::Buffer,
        b1: vk::Buffer,
    ) -> Result<vk::DescriptorSet> {
        let layouts = [dsl];
        let set = device.allocate_descriptor_sets(
            &vk::DescriptorSetAllocateInfo::default()
                .descriptor_pool(dpool)
                .set_layouts(&layouts),
        )?[0];
        let i0 = [vk::DescriptorBufferInfo::default()
            .buffer(b0)
            .offset(0)
            .range(vk::WHOLE_SIZE)];
        let i1 = [vk::DescriptorBufferInfo::default()
            .buffer(b1)
            .offset(0)
            .range(vk::WHOLE_SIZE)];
        let writes = [
            vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(&i0),
            vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(1)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(&i1),
        ];
        device.update_descriptor_sets(&writes, &[]);
        Ok(set)
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn submit_timed(
        device: &ash::Device,
        queue: vk::Queue,
        cmd: vk::CommandBuffer,
        fence: vk::Fence,
        pipe: vk::Pipeline,
        playout: vk::PipelineLayout,
        set: vk::DescriptorSet,
        push: &[u8],
        groups: u32,
    ) -> Result<f64> {
        device.reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty())?;
        device.begin_command_buffer(cmd, &vk::CommandBufferBeginInfo::default())?;
        device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::COMPUTE, pipe);
        device.cmd_bind_descriptor_sets(
            cmd,
            vk::PipelineBindPoint::COMPUTE,
            playout,
            0,
            &[set],
            &[],
        );
        device.cmd_push_constants(cmd, playout, vk::ShaderStageFlags::COMPUTE, 0, push);
        device.cmd_dispatch(cmd, groups, 1, 1);
        device.end_command_buffer(cmd)?;

        let cmds = [cmd];
        let submit = vk::SubmitInfo::default().command_buffers(&cmds);
        device.reset_fences(&[fence])?;
        let t = Instant::now();
        device.queue_submit(queue, &[submit], fence)?;
        // M3: bounded wait — on a GPU hang, bail the whole session (the fence still
        // has a pending submission, so we must NOT reuse it; unwinding tears down
        // the session and frees the compute lock).
        device
            .wait_for_fences(&[fence], true, FENCE_TIMEOUT_NS)
            .map_err(|e| anyhow!("bench dispatch fence timed out (GPU hang / device lost): {e}"))?;
        Ok(t.elapsed().as_secs_f64())
    }

    /// Allocate the largest device-local buffer that fits (256 -> 128 -> 64 MiB).
    unsafe fn alloc_biggest(
        device: &ash::Device,
        memprops: &vk::PhysicalDeviceMemoryProperties,
    ) -> Result<(Buf, u64)> {
        for mb in [256u64, 128, 64] {
            let sz = mb * 1024 * 1024;
            if let Ok(b) = make_buffer(device, memprops, sz, false) {
                return Ok((b, sz));
            }
        }
        Err(anyhow!("could not allocate a device-local data buffer"))
    }

    /// Ask the APU governor (if it's the active GPU governor) to hold the top clock.
    ///
    /// The governor's app-driven autosleep FORCE-LOCKS the GFX clock (deep-sleep
    /// 350 MHz when idle) via the SMU — a forced clock can't be moved by load, so
    /// our fence-rate warmup can't climb on its own. WRITING the governor's poke
    /// file wakes it to the top setpoint (~2230), exactly what we measure at.
    /// No-op when no governor is installed, so a standalone bench still drives the
    /// clock the old way.
    ///
    /// This is deliberately FILE IPC, not an in-process clock pin: the governor is
    /// a separate daemon and the ONLY SMU clock writer (design R3/R5). Pinning the
    /// clock in-process here would put a SECOND writer on the SMU and race the
    /// governor — the exact double-writer hazard. The poke path is the shared
    /// `apu::POKE_PATH`/`apu::POKE_DIR` const, so the reader and writer can't drift.
    fn poke_aputune() {
        if std::path::Path::new(apu::POKE_DIR).is_dir() {
            let _ = std::fs::write(apu::POKE_PATH, b"");
        }
    }

    /// The GFX clock the bench warms to and measures at. We bench at whatever
    /// clock aputune holds the GPU at — its configured top setpoint
    /// (`power.json` `top_mhz`) — NOT a fixed ~2230. A governor capped at e.g.
    /// 1500 would otherwise make `warm_to_peak` drive load for its full cap
    /// trying to reach an unreachable 2230, then measure "low". Falls back to the
    /// board's stock run bin (~2230) when aputune isn't installed (standalone).
    fn expected_run_mhz() -> u16 {
        const STOCK_RUN: u16 = 2230;
        std::fs::read_to_string("/var/lib/aputune/power.json")
            .ok()
            .and_then(|s| {
                s.split("\"top_mhz\"").nth(1).and_then(|r| {
                    r.split(|c: char| !c.is_ascii_digit())
                        .find(|t| !t.is_empty())
                        .and_then(|n| n.parse::<u16>().ok())
                })
            })
            .filter(|&m| m >= 350)
            .unwrap_or(STOCK_RUN)
    }

    /// Read the live GFX clock TWICE and report whether both land in the expected
    /// top DPM bin (aputune's top setpoint). A single `gpu_metrics` read can tear
    /// while a compute job is in flight (returning garbage); requiring two sane,
    /// in-window reads rejects that, so the warmup never false-positives "at run"
    /// and measures at a low clock. The window brackets the target: slack below
    /// for a step-under, headroom above rejects torn-high garbage.
    fn clock_confirmed_run() -> bool {
        let target = expected_run_mhz();
        let lo = target.saturating_sub(150);
        let hi = target.saturating_add(170);
        let sane = |c: u16| (lo..=hi).contains(&c);
        match (crate::metrics::read(), crate::metrics::read()) {
            (Some(a), Some(b)) => sane(a.gfxclk_mhz) && sane(b.gfxclk_mhz),
            _ => false,
        }
    }

    /// Drive the DPM governor up to its TOP clock step ("run" ~2230 MHz on this
    /// board) before any measurement, so every metric is read at the same top clock.
    ///
    /// The BC-250's userspace `dpm-daemon` governs by FENCE RATE, not GPU occupancy:
    /// it climbs idle(1000)->mid(1500) at >=5 fence-advances/s and mid->run(2230)
    /// only at >=80/s sustained ~1.5s (real GPU load like llama generation runs
    /// ~315/s). A few big bench dispatches signal only ~13 fences/s, so they peg the
    /// GPU's memory bus while the governor still thinks it's lightly loaded and holds
    /// it at 1500. So we don't fight the governor — we give it the signal it wants:
    /// a burst of MANY TINY dispatches (one fence each, thousands/s) until it
    /// legitimately climbs to run. The daemon keeps full thermal authority (it won't
    /// climb if too hot). Once at run it only descends after 30s under 10/s, so the
    /// measurements that follow stay pinned at 2230.
    ///
    /// We confirm via the real GFX clock (`gpu_metrics`) and exit as soon as it's at
    /// run for CONFIRM_SECS — near-instant when already warm. The cap is generous
    /// (WARM_CAP) so that right after a reboot we WAIT OUT the governor's own startup:
    /// on this board `dpm-daemon` only starts once llama-server is healthy, which can
    /// be 30-60s after boot. Until it's governing, driving load can't move the clock;
    /// once it is, the clock climbs and we bench. So the wait is adaptive — it lasts
    /// exactly until the clock can reach run, not a fixed guess. If `gpu_metrics`
    /// isn't readable (or no governor ever appears) we drive load for the full cap
    /// then measure at whatever clock is achievable.
    #[allow(clippy::too_many_arguments)]
    unsafe fn warm_to_peak(
        device: &ash::Device,
        queue: vk::Queue,
        memprops: &vk::PhysicalDeviceMemoryProperties,
        dsl: vk::DescriptorSetLayout,
        dpool: vk::DescriptorPool,
        playout: vk::PipelineLayout,
        pipe: vk::Pipeline,
        cmd: vk::CommandBuffer,
        fence: vk::Fence,
    ) -> Result<()> {
        // Moderate per-dispatch work (~a few ms each) so the submit/fence loop runs
        // at a few HUNDRED fences/s — past the 80/s the governor needs to climb to
        // "run", but near real GPU load (llama generation is ~315/s). A tiny
        // dispatch instead hits thousands/s, which the BC-250's compute queue can't
        // take — it hangs the queue. A MIN_PERIOD floor hard-caps the rate even if a
        // dispatch comes back faster than expected.
        let load = make_buffer(device, memprops, 128 * 1024 * 1024, false)?;
        let out = make_buffer(device, memprops, 256, false)?;
        let set = alloc_set(device, dpool, dsl, load.buffer, out.buffer)?;
        let n = (128 * 1024 * 1024 / 16) as u32; // vec4 count
        let groups: u32 = 1024;
        let mut push = [0u8; 8];
        push[0..4].copy_from_slice(&n.to_ne_bytes());
        push[4..8].copy_from_slice(&16u32.to_ne_bytes()); // iters/thread (~5ms/dispatch)

        // Generous so a post-reboot bench waits out the governor's startup (dpm-daemon
        // can take 30-60s to begin governing after boot); exits the instant the clock
        // confirms run, so a warm bench pays almost none of it.
        const WARM_CAP_SECS: f64 = 45.0;
        // Hold the streaming load at confirmed-run for this long before trusting
        // it. Beyond confirming the GFX clock, this gives the memory/SoC-fabric
        // DPM state time to ramp under sustained load — after a deep idle (aputune
        // just released to BAPM, or a wake) the fabric lags the GFX clock, and a
        // too-short hold makes the first run's scattered-read + latency read low.
        const CONFIRM_SECS: f64 = 2.5;
        const POLL_SECS: f64 = 0.1;
        const MIN_PERIOD: Duration = Duration::from_millis(3); // <=~330 fences/s
                                                               // If aputune owns the clock, ask it to hold the top setpoint up front —
                                                               // otherwise its force-lock pins us at deep-sleep and this loop spins the
                                                               // full WARM_CAP without ever confirming run.
        poke_aputune();
        let start = Instant::now();
        let mut last_poll = start;
        let mut last_poke = start;
        let mut at_run: Option<Instant> = None;
        loop {
            // Each submit waits for its fence, so the clock reads below happen in the
            // idle gap between dispatches (avoids the gpu_metrics-during-compute race).
            let iter = Instant::now();
            let _ = submit_timed(device, queue, cmd, fence, pipe, playout, set, &push, groups)?;
            if let Some(rest) = MIN_PERIOD.checked_sub(iter.elapsed()) {
                std::thread::sleep(rest); // throttle: never hammer the queue
            }
            if start.elapsed().as_secs_f64() >= WARM_CAP_SECS {
                break;
            }
            // Keep aputune's poke fresh so it stays at top for the whole warmup
            // (and the measurement phases that follow, well inside its idle window).
            if last_poke.elapsed().as_secs_f64() >= 2.0 {
                last_poke = Instant::now();
                poke_aputune();
            }
            if last_poll.elapsed().as_secs_f64() >= POLL_SECS {
                last_poll = Instant::now();
                if clock_confirmed_run() {
                    if at_run
                        .get_or_insert_with(Instant::now)
                        .elapsed()
                        .as_secs_f64()
                        >= CONFIRM_SECS
                    {
                        break; // confirmed at the top clock step
                    }
                } else {
                    at_run = None; // not (yet) at run — restart the confirm timer
                }
            }
        }
        destroy_buf(device, &load);
        destroy_buf(device, &out);
        Ok(())
    }

    /// Sequential-read bandwidth, measured by riding the throughput up to its peak.
    /// `warm_to_peak` gets the clock to the top step, but the BC-250 keeps ramping
    /// for ~20-40s after a COLD BOOT (DPM steps settle, then memory/SoC/thermal/
    /// contention), so a fixed short window taken right after a reboot reads several
    /// GB/s low — which is why the post-reboot auto-bench used to come out below a
    /// second manual run. So we track the best (fastest) dispatch and stop only once
    /// it hasn't improved for STALE_SECS: near-instant when already warm, but it
    /// rides the post-boot climb to the true peak. Hard-capped by MAX.
    #[allow(clippy::too_many_arguments)]
    unsafe fn run_bandwidth(
        device: &ash::Device,
        queue: vk::Queue,
        memprops: &vk::PhysicalDeviceMemoryProperties,
        dsl: vk::DescriptorSetLayout,
        dpool: vk::DescriptorPool,
        playout: vk::PipelineLayout,
        pipe: vk::Pipeline,
        cmd: vk::CommandBuffer,
        fence: vk::Fence,
    ) -> Result<f64> {
        let (data, bytes) = alloc_biggest(device, memprops)?;
        let out = make_buffer(device, memprops, 256, false)?;
        let set = alloc_set(device, dpool, dsl, data.buffer, out.buffer)?;

        let n = (bytes / 16) as u32; // vec4 count
        let iters: u32 = 128;
        let groups: u32 = 1024; // 1024*64 = 65536 invocations
        let mut push = [0u8; 8];
        push[0..4].copy_from_slice(&n.to_ne_bytes());
        push[4..8].copy_from_slice(&iters.to_ne_bytes());

        // Ride the throughput to its plateau: track the best (fastest) dispatch and
        // stop once it hasn't improved by >0.05% for STALE_SECS (after a MIN floor),
        // capped by MAX. Warm -> exits in ~MIN+STALE; cold boot -> rides the ramp to
        // the true peak. The continuous dispatches also keep the clock pinned where
        // warm_to_peak left it.
        const MIN_SECS: f64 = 2.0;
        const STALE_SECS: f64 = 4.0;
        const MAX_SECS: f64 = 30.0;
        let start = Instant::now();
        let mut best = f64::INFINITY;
        let mut last_improve = start;
        loop {
            let t = submit_timed(device, queue, cmd, fence, pipe, playout, set, &push, groups)?;
            if t < best {
                if t < best * 0.9995 {
                    last_improve = Instant::now(); // a real (>0.05%) improvement
                }
                best = t;
            }
            let el = start.elapsed().as_secs_f64();
            if el >= MAX_SECS
                || (el >= MIN_SECS && last_improve.elapsed().as_secs_f64() >= STALE_SECS)
            {
                break;
            }
        }
        let moved = (n as f64) * 16.0 * (iters as f64);
        let gbps = moved / best / 1e9;

        destroy_buf(device, &data);
        destroy_buf(device, &out);
        Ok(gbps)
    }

    /// Random-access read throughput. Scattered reads are LOW throughput, so each
    /// dispatch is slow and the fence rate alone can fall under the governor's
    /// descend threshold — letting the clock sag mid-measurement (random reads low,
    /// while bandwidth/latency look fine). Same problem latency has, same fix: keep
    /// the GPU under a continuous front-load (`load_pipe`) and interleave random
    /// samples, taking the best. The sustained load holds the top clock.
    #[allow(clippy::too_many_arguments)]
    unsafe fn run_random(
        device: &ash::Device,
        queue: vk::Queue,
        memprops: &vk::PhysicalDeviceMemoryProperties,
        dsl: vk::DescriptorSetLayout,
        dpool: vk::DescriptorPool,
        playout: vk::PipelineLayout,
        rand_pipe: vk::Pipeline,
        load_pipe: vk::Pipeline,
        cmd: vk::CommandBuffer,
        fence: vk::Fence,
    ) -> Result<f64> {
        poke_aputune(); // keep aputune at top for the scattered-read phase
        let (data, bytes) = alloc_biggest(device, memprops)?;
        let out = make_buffer(device, memprops, 256, false)?;
        let rand_set = alloc_set(device, dpool, dsl, data.buffer, out.buffer)?;

        let n = (bytes / 16) as u32; // vec4 count
        let iters: u32 = 4096; // scattered reads per thread
        let groups: u32 = 1024; // 65536 threads
        let mut push = [0u8; 8];
        push[0..4].copy_from_slice(&n.to_ne_bytes());
        push[4..8].copy_from_slice(&iters.to_ne_bytes());

        // Front-load buffer streamed between samples to hold the clock (as latency).
        let (load, load_bytes) = alloc_biggest(device, memprops)?;
        let load_set = alloc_set(device, dpool, dsl, load.buffer, out.buffer)?;
        let load_n = (load_bytes / 16) as u32;
        let load_groups: u32 = 1024;
        let mut load_push = [0u8; 8];
        load_push[0..4].copy_from_slice(&load_n.to_ne_bytes());
        load_push[4..8].copy_from_slice(&64u32.to_ne_bytes());

        let start = Instant::now();
        let mut best = f64::INFINITY;
        loop {
            // front-load: hold the clock high, then sample random at that clock
            let _ = submit_timed(
                device,
                queue,
                cmd,
                fence,
                load_pipe,
                playout,
                load_set,
                &load_push,
                load_groups,
            )?;
            let t = submit_timed(
                device, queue, cmd, fence, rand_pipe, playout, rand_set, &push, groups,
            )?;
            best = best.min(t);
            if start.elapsed().as_secs_f64() >= 2.5 {
                break;
            }
        }
        let moved = (groups as f64) * 64.0 * (iters as f64) * 16.0;
        let gbps = moved / best / 1e9;

        destroy_buf(device, &data);
        destroy_buf(device, &out);
        destroy_buf(device, &load);
        Ok(gbps)
    }

    /// Pointer-chase latency. A single-thread chase is low-load, so a load-based
    /// GPU governor (amdgpu DPM, or a userspace one like the BC-250 dpm-daemon)
    /// downclocks during the measurement and the number comes out high/variable.
    /// We don't fight the governor: instead we keep the GPU under continuous load
    /// (`load_pipe`) and interleave SHORT latency samples in a ~3s window, taking
    /// the best. The sustained load holds the top clock; each short sample (well
    /// under the governor's reaction time) catches it.
    #[allow(clippy::too_many_arguments)]
    unsafe fn run_latency(
        device: &ash::Device,
        queue: vk::Queue,
        memprops: &vk::PhysicalDeviceMemoryProperties,
        dsl: vk::DescriptorSetLayout,
        dpool: vk::DescriptorPool,
        playout: vk::PipelineLayout,
        lat_pipe: vk::Pipeline,
        load_pipe: vk::Pipeline,
        cmd: vk::CommandBuffer,
        fence: vk::Fence,
    ) -> Result<f64> {
        // Latency is the slow, low-load phase — keep aputune at top for it.
        poke_aputune();
        // 8 MiB chain (>> caches), random permutation cycle.
        let n: usize = 2 * 1024 * 1024; // uints
        let chain = make_buffer(device, memprops, (n * 4) as u64, true)?;
        let ptr = device.map_memory(chain.memory, 0, (n * 4) as u64, vk::MemoryMapFlags::empty())?
            as *mut u32;
        let perm = permutation_cycle(n);
        std::ptr::copy_nonoverlapping(perm.as_ptr(), ptr, n);
        device.unmap_memory(chain.memory);

        let out = make_buffer(device, memprops, 256, false)?;
        let lat_set = alloc_set(device, dpool, dsl, chain.buffer, out.buffer)?;

        // The front-load buffer: a big buffer streamed by `load_pipe` to keep the
        // GPU busy (so the governor holds the top clock) between latency samples.
        let (load, load_bytes) = {
            let mut made = None;
            for mb in [256u64, 128, 64] {
                let sz = mb * 1024 * 1024;
                if let Ok(b) = make_buffer(device, memprops, sz, false) {
                    made = Some((b, sz));
                    break;
                }
            }
            made.ok_or_else(|| anyhow!("could not allocate a front-load buffer"))?
        };
        let load_set = alloc_set(device, dpool, dsl, load.buffer, out.buffer)?;
        let load_n = (load_bytes / 16) as u32;
        let load_groups: u32 = 1024;
        let mut load_push = [0u8; 8];
        load_push[0..4].copy_from_slice(&load_n.to_ne_bytes());
        load_push[4..8].copy_from_slice(&64u32.to_ne_bytes()); // ~50ms of load per dispatch

        // Short enough that one sample finishes before the governor reacts.
        let steps: u32 = 200_000;
        let mut lat_push = [0u8; 8];
        lat_push[0..4].copy_from_slice(&steps.to_ne_bytes());

        let start = Instant::now();
        let mut best = f64::INFINITY;
        loop {
            // front-load: hold the clock high
            let _ = submit_timed(
                device,
                queue,
                cmd,
                fence,
                load_pipe,
                playout,
                load_set,
                &load_push,
                load_groups,
            )?;
            // sample latency at that clock
            let t = submit_timed(
                device, queue, cmd, fence, lat_pipe, playout, lat_set, &lat_push, 1,
            )?;
            best = best.min(t);
            if start.elapsed().as_secs_f64() >= 3.0 {
                break;
            }
        }
        let ns = best / (steps as f64) * 1e9;

        destroy_buf(device, &chain);
        destroy_buf(device, &out);
        destroy_buf(device, &load);
        Ok(ns)
    }

    /// Integrity test: write a deterministic pattern across a big GDDR6 buffer,
    /// read it back, count mismatches. Repeated over `passes` seeds. Returns
    /// (total mismatches, total bytes verified). A trained-but-unstable config
    /// shows up here as non-zero errors.
    #[allow(clippy::too_many_arguments)]
    unsafe fn run_stability(
        device: &ash::Device,
        queue: vk::Queue,
        memprops: &vk::PhysicalDeviceMemoryProperties,
        dsl: vk::DescriptorSetLayout,
        dpool: vk::DescriptorPool,
        playout: vk::PipelineLayout,
        write_pipe: vk::Pipeline,
        check_pipe: vk::Pipeline,
        cmd: vk::CommandBuffer,
        fence: vk::Fence,
        passes: u32,
    ) -> Result<(u64, u64)> {
        let (data, bytes) = {
            let mut made = None;
            for mb in [256u64, 128, 64] {
                let sz = mb * 1024 * 1024;
                if let Ok(b) = make_buffer(device, memprops, sz, false) {
                    made = Some((b, sz));
                    break;
                }
            }
            made.ok_or_else(|| anyhow!("could not allocate a device-local data buffer"))?
        };
        // Host-visible counter buffer so we can read the mismatch count back.
        let counter = make_buffer(device, memprops, 256, true)?;
        let ptr =
            device.map_memory(counter.memory, 0, 256, vk::MemoryMapFlags::empty())? as *mut u32;
        let set = alloc_set(device, dpool, dsl, data.buffer, counter.buffer)?;

        let n = (bytes / 4) as u32; // uint count
        let groups: u32 = 1024;
        let mut errors: u64 = 0;
        let mut tested: u64 = 0;
        for p in 0..passes {
            let seed = 0xA5A5_A5A5u32 ^ p.wrapping_mul(0x9E37_79B9);
            let mut push = [0u8; 8];
            push[0..4].copy_from_slice(&n.to_ne_bytes());
            push[4..8].copy_from_slice(&seed.to_ne_bytes());
            std::ptr::write_volatile(ptr, 0u32); // reset counter
            submit_writecheck(
                device, queue, cmd, fence, write_pipe, check_pipe, playout, set, &push, groups,
            )?;
            errors += std::ptr::read_volatile(ptr) as u64;
            tested += (n as u64) * 4;
        }

        device.unmap_memory(counter.memory);
        destroy_buf(device, &data);
        destroy_buf(device, &counter);
        Ok((errors, tested))
    }

    /// Write pass then check pass in one command buffer, with a barrier between so
    /// the read-back sees real memory (not the writer's caches).
    #[allow(clippy::too_many_arguments)]
    unsafe fn submit_writecheck(
        device: &ash::Device,
        queue: vk::Queue,
        cmd: vk::CommandBuffer,
        fence: vk::Fence,
        write_pipe: vk::Pipeline,
        check_pipe: vk::Pipeline,
        playout: vk::PipelineLayout,
        set: vk::DescriptorSet,
        push: &[u8],
        groups: u32,
    ) -> Result<()> {
        device.reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty())?;
        device.begin_command_buffer(cmd, &vk::CommandBufferBeginInfo::default())?;
        device.cmd_bind_descriptor_sets(
            cmd,
            vk::PipelineBindPoint::COMPUTE,
            playout,
            0,
            &[set],
            &[],
        );
        device.cmd_push_constants(cmd, playout, vk::ShaderStageFlags::COMPUTE, 0, push);
        device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::COMPUTE, write_pipe);
        device.cmd_dispatch(cmd, groups, 1, 1);
        let bar = [vk::MemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::SHADER_WRITE)
            .dst_access_mask(vk::AccessFlags::SHADER_READ)];
        device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::COMPUTE_SHADER,
            vk::PipelineStageFlags::COMPUTE_SHADER,
            vk::DependencyFlags::empty(),
            &bar,
            &[],
            &[],
        );
        device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::COMPUTE, check_pipe);
        device.cmd_dispatch(cmd, groups, 1, 1);
        device.end_command_buffer(cmd)?;

        let cmds = [cmd];
        let submit = vk::SubmitInfo::default().command_buffers(&cmds);
        device.reset_fences(&[fence])?;
        device.queue_submit(queue, &[submit], fence)?;
        // M3: bounded wait — see FENCE_TIMEOUT_NS. Bail on hang so the compute lock
        // is released instead of the thread spinning forever on a wedged GPU.
        device
            .wait_for_fences(&[fence], true, FENCE_TIMEOUT_NS)
            .map_err(|e| {
                anyhow!("integrity dispatch fence timed out (GPU hang / device lost): {e}")
            })?;
        Ok(())
    }

    /// A single Hamiltonian cycle over 0..n (so the chase never short-cycles),
    /// shuffled so consecutive hops jump unpredictably (defeats the prefetcher).
    fn permutation_cycle(n: usize) -> Vec<u32> {
        // Fisher-Yates over [1..n] with a cheap LCG, then link 0->p[0]->p[1]...->0.
        let mut order: Vec<u32> = (1..n as u32).collect();
        let mut state: u64 = 0x9e3779b97f4a7c15;
        for i in (1..order.len()).rev() {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let j = (state >> 33) as usize % (i + 1);
            order.swap(i, j);
        }
        let mut next = vec![0u32; n];
        let mut cur = 0u32;
        for &node in &order {
            next[cur as usize] = node;
            cur = node;
        }
        next[cur as usize] = 0; // close the cycle
        next
    }

    #[cfg(test)]
    pub fn permutation_cycle_test(n: usize) -> Vec<u32> {
        permutation_cycle(n)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn permutation_is_a_single_cycle() {
        let n = 4096;
        let next = vk_permutation_for_test(n);
        // walking from 0 must visit all n nodes before returning to 0
        let mut seen = vec![false; n];
        let mut cur = 0usize;
        for _ in 0..n {
            assert!(!seen[cur], "cycle repeats early");
            seen[cur] = true;
            cur = next[cur] as usize;
        }
        assert_eq!(cur, 0, "chain must close back to 0");
        assert!(seen.iter().all(|&s| s));
    }

    // re-expose the private fn for the test
    fn vk_permutation_for_test(n: usize) -> Vec<u32> {
        super::vk::permutation_cycle_test(n)
    }
}
