// SPDX-License-Identifier: GPL-2.0-only
//! ariel-compute — a silicon-generic Vulkan compute harness.
//!
//! ONE lazily-created Vulkan device (instance + logical device + compute queue)
//! lives behind a process-wide mutex. Every [`dispatch`] serializes on that
//! mutex, so two callers can never drive the GPU at once:
//!
//!   a MEM bandwidth bench and an APU KAT cannot hit the GPU simultaneously —
//!   they serialize here.
//!
//! The harness is ASSET-FREE: it holds NO shaders. Callers pass SPIR-V bytes to
//! [`dispatch`] along with the storage buffers to bind and the push constants.
//! (M4's MEM bench and M5's APU KAT own their own SPIR-V and build on this.)
//!
//! `ash` dlopens the Vulkan loader at runtime; on a build host with no Vulkan,
//! device creation fails gracefully (`dispatch` returns `Err`). The mutex
//! serialization is independent of the device and is exercised by a GPU-free
//! unit test ([`with_compute_lock`]).

use std::sync::{Mutex, OnceLock};

use anyhow::{bail, Result};

// Re-export the exact `ash` version this harness links, so a consumer that runs
// its own multi-dispatch session over [`with_session`] uses the SAME Vulkan
// types (device/queue/memprops) — no version skew across the crate boundary.
pub use ash;

/// One storage buffer to bind for a dispatch. Buffers bind to descriptor set 0
/// in order: `buffers[i]` -> binding `i` (STORAGE_BUFFER, COMPUTE stage).
pub struct BufSpec {
    /// Buffer size in bytes.
    pub bytes: u64,
    /// Optional initial host->device contents. `None` = zero-initialised. If
    /// present, must be <= `bytes`.
    pub upload: Option<Vec<u8>>,
    /// Read this buffer back to the host after the dispatch completes.
    pub readback: bool,
}

impl BufSpec {
    /// A zeroed buffer of `bytes`, not read back (a scratch/output-only target
    /// the caller does not need to inspect).
    pub fn scratch(bytes: u64) -> Self {
        BufSpec {
            bytes,
            upload: None,
            readback: false,
        }
    }
    /// A buffer seeded with `data`, read back after the dispatch.
    pub fn io(data: Vec<u8>) -> Self {
        BufSpec {
            bytes: data.len() as u64,
            upload: Some(data),
            readback: true,
        }
    }
}

/// Result of a [`dispatch`]: per-input-buffer readback, in the same order as the
/// `buffers` argument. Entry `i` is `Some(bytes)` iff `buffers[i].readback` was
/// set, else `None`.
pub struct Readback {
    pub buffers: Vec<Option<Vec<u8>>>,
}

impl Readback {
    /// The first buffer that was read back (convenience for single-output
    /// kernels).
    pub fn first(&self) -> Option<&Vec<u8>> {
        self.buffers.iter().flatten().next()
    }
}

// Push constants are limited to the guaranteed-available 128-byte minimum so a
// dispatch never trips a device's maxPushConstantsSize.
const MAX_PUSH_BYTES: usize = 128;
// Descriptor bindings a single dispatch may bind (a soft cap; every device
// supports at least this many storage buffers).
const MAX_BUFFERS: usize = 8;
// Sane per-dimension workgroup-count cap. The Vulkan guaranteed minimum for
// maxComputeWorkGroupCount is 65535 per dimension; a caller passing a wild group
// count (the "huge group count -> hang" class) is refused up front rather than
// dispatched at the wedged GPU (gfx1013 has no working reset).
const MAX_GROUPS_PER_DIM: u32 = 65_535;
// Sane per-buffer byte cap (2 GiB). Guards against a bogus/overflowing BufSpec
// size that would exceed maxStorageBufferRange (or exhaust device memory) — well
// above anything a real bench allocates (largest is 256 MiB).
const MAX_BUFFER_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Process-wide compute state behind the serializing mutex.
struct ComputeState {
    ctx: Option<vk::Ctx>,
}

fn compute() -> &'static Mutex<ComputeState> {
    static COMPUTE: OnceLock<Mutex<ComputeState>> = OnceLock::new();
    COMPUTE.get_or_init(|| Mutex::new(ComputeState { ctx: None }))
}

/// Run `f` while holding the process-wide compute lock — the exact mutex
/// [`dispatch`] takes. No device is created, so this exercises the
/// serialization guarantee on any host (GPU or not). Test/utility hook.
pub fn with_compute_lock<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    // Ignore poisoning: a panicked prior holder must not wedge the harness for
    // the rest of the process; the state behind the lock is reconstructable.
    let _guard = compute().lock().unwrap_or_else(|p| p.into_inner());
    f()
}

/// Asset-free compute dispatch. Serializes on the process-wide compute lock,
/// lazily creating the Vulkan device on first use.
///
/// - `spirv`   — the compute shader (SPIR-V bytes). The harness holds none.
/// - `groups`  — workgroup counts `[x, y, z]` for `vkCmdDispatch`.
/// - `push`    — push-constant bytes (<= 128), or empty for none.
/// - `buffers` — storage buffers to bind at set 0, binding 0..N.
///
/// Returns the readback of every buffer marked `readback`.
pub fn dispatch(
    spirv: &[u8],
    groups: [u32; 3],
    push: &[u8],
    buffers: &[BufSpec],
) -> Result<Readback> {
    // ---- input validation (before touching the GPU) ----
    if spirv.is_empty() {
        bail!("dispatch: empty SPIR-V");
    }
    if !spirv.len().is_multiple_of(4) {
        bail!(
            "dispatch: SPIR-V length {} is not a multiple of 4",
            spirv.len()
        );
    }
    if push.len() > MAX_PUSH_BYTES {
        bail!(
            "dispatch: {} push-constant bytes exceeds the {MAX_PUSH_BYTES}-byte cap",
            push.len()
        );
    }
    if buffers.is_empty() {
        bail!("dispatch: at least one storage buffer is required");
    }
    if buffers.len() > MAX_BUFFERS {
        bail!(
            "dispatch: {} buffers exceeds the {MAX_BUFFERS}-binding cap",
            buffers.len()
        );
    }
    // Refuse an out-of-range workgroup count before touching the GPU (huge group
    // count -> hang class; gfx1013 has no working reset to recover from it).
    for (dim, &g) in groups.iter().enumerate() {
        if g > MAX_GROUPS_PER_DIM {
            bail!(
                "dispatch: workgroup count {g} on dim {dim} exceeds the {MAX_GROUPS_PER_DIM} cap"
            );
        }
    }
    for (i, b) in buffers.iter().enumerate() {
        if b.bytes == 0 {
            bail!("dispatch: buffer {i} has zero size");
        }
        if b.bytes > MAX_BUFFER_BYTES {
            bail!(
                "dispatch: buffer {i} size ({} B) exceeds the {MAX_BUFFER_BYTES}-byte cap",
                b.bytes
            );
        }
        if let Some(data) = &b.upload {
            if data.len() as u64 > b.bytes {
                bail!(
                    "dispatch: buffer {i} upload ({} B) exceeds its size ({} B)",
                    data.len(),
                    b.bytes
                );
            }
        }
    }

    let mut guard = compute().lock().unwrap_or_else(|p| p.into_inner());
    if guard.ctx.is_none() {
        guard.ctx = Some(vk::Ctx::create()?);
    }
    let ctx = guard.ctx.as_ref().unwrap();
    // SAFETY: the ctx (instance/device/queue) is valid for the whole call, and
    // the process-wide lock guarantees no other dispatch touches the device.
    unsafe { ctx.run(spirv, groups, push, buffers) }
}

/// True if a usable Vulkan device can be reached (loader + instance + at least
/// one physical device). Does not create the logical device or run a kernel.
pub fn vulkan_available() -> bool {
    vk::available()
}

/// A borrow of the shared Vulkan device, handed to a [`with_session`] closure.
///
/// This is the low-level escape hatch for a consumer whose GPU work is a whole
/// SESSION of many dispatches on persistent buffers with its own timing — the
/// asset-free one-shot [`dispatch`] can't host that. MEM's bandwidth/latency
/// bench is the first such consumer: it needs device-local buffers, per-dispatch
/// timing, two pipelines with a barrier, and thousands of dispatches on the same
/// buffers. It runs all of that here, on the ONE shared device, while holding the
/// process-wide compute lock — so it still cannot touch the GPU concurrently with
/// an APU KAT (or any other [`dispatch`]).
pub struct Session<'a> {
    /// The shared logical device (same handle every session sees).
    pub device: &'a ash::Device,
    /// A compute-capable queue on that device.
    pub queue: ash::vk::Queue,
    /// The queue family index `queue` was taken from.
    pub queue_family: u32,
    /// Memory-type properties, for picking device-local / host-visible types.
    pub memprops: ash::vk::PhysicalDeviceMemoryProperties,
    /// The physical device's name (`VkPhysicalDeviceProperties.deviceName`). A
    /// caller whose correctness depends on running on specific silicon (e.g. the
    /// APU CU health-test, which must REFUSE a software rasterizer that would
    /// false-pass every routing config) checks this before dispatching.
    pub device_name: &'a str,
}

/// Run `f` with a borrow of the shared Vulkan device, holding the process-wide
/// compute lock for the whole closure.
///
/// Lazily creates the ONE device on first use (shared with [`dispatch`]). The
/// entire closure runs under the same serializing mutex `dispatch` takes, so a
/// MEM bench (a long session here) and an APU KAT serialize — they can never
/// drive the GPU at once. Returns `Err` (without wedging the lock) if no usable
/// Vulkan device can be created on this host.
pub fn with_session<F, R>(f: F) -> Result<R>
where
    F: FnOnce(&Session) -> Result<R>,
{
    let mut guard = compute().lock().unwrap_or_else(|p| p.into_inner());
    if guard.ctx.is_none() {
        guard.ctx = Some(vk::Ctx::create()?);
    }
    let ctx = guard.ctx.as_ref().unwrap();
    let sess = ctx.session();
    f(&sess)
}

// ---- Vulkan backend --------------------------------------------------------

mod vk {
    use std::ffi::CStr;
    use std::io::Cursor;

    use anyhow::{anyhow, bail, Result};
    use ash::vk;

    use super::{BufSpec, Readback};

    /// M3: bounded fence wait for a one-shot dispatch (20 s). Generous enough that
    /// a slow but legitimate kernel is never killed (exceeds cutest's 10 s KAT
    /// budget), but finite so a wedged GPU can't hold the compute lock forever.
    const DISPATCH_FENCE_TIMEOUT_NS: u64 = 20_000_000_000;

    /// The lazily-created device context (one per process).
    pub struct Ctx {
        // Field order matters for drop: destroyed top-down by explicit Drop below.
        _entry: ash::Entry,
        instance: ash::Instance,
        device: ash::Device,
        queue: vk::Queue,
        qf: u32,
        memprops: vk::PhysicalDeviceMemoryProperties,
        dev_name: String,
    }

    impl Ctx {
        pub fn create() -> Result<Self> {
            unsafe {
                let entry = ash::Entry::load().map_err(|e| anyhow!("load Vulkan loader: {e}"))?;
                let app = vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_1);
                let ici = vk::InstanceCreateInfo::default().application_info(&app);
                let instance = entry
                    .create_instance(&ici, None)
                    .map_err(|e| anyhow!("create_instance: {e}"))?;

                // From here, any failure must destroy the instance before returning.
                let build = (|| {
                    let pd = pick_device(&instance)?;
                    let dev_name = {
                        let p = instance.get_physical_device_properties(pd);
                        CStr::from_ptr(p.device_name.as_ptr())
                            .to_string_lossy()
                            .into_owned()
                    };
                    let qf = compute_queue_family(&instance, pd)?;
                    let qprio = [1.0f32];
                    let qci = vk::DeviceQueueCreateInfo::default()
                        .queue_family_index(qf)
                        .queue_priorities(&qprio);
                    let qcis = [qci];
                    let dci = vk::DeviceCreateInfo::default().queue_create_infos(&qcis);
                    let device = instance
                        .create_device(pd, &dci, None)
                        .map_err(|e| anyhow!("create_device: {e}"))?;
                    let queue = device.get_device_queue(qf, 0);
                    let memprops = instance.get_physical_device_memory_properties(pd);
                    Ok::<_, anyhow::Error>((device, queue, qf, memprops, dev_name))
                })();
                match build {
                    Ok((device, queue, qf, memprops, dev_name)) => Ok(Ctx {
                        _entry: entry,
                        instance,
                        device,
                        queue,
                        qf,
                        memprops,
                        dev_name,
                    }),
                    Err(e) => {
                        instance.destroy_instance(None);
                        Err(e)
                    }
                }
            }
        }

        /// Borrow the device/queue/memprops for a caller-driven session (see
        /// [`super::with_session`]). Valid only while the compute lock is held.
        pub fn session(&self) -> super::Session<'_> {
            super::Session {
                device: &self.device,
                queue: self.queue,
                queue_family: self.qf,
                memprops: self.memprops,
                device_name: &self.dev_name,
            }
        }

        /// Record + submit one dispatch, returning the requested readbacks.
        ///
        /// # Safety
        /// The caller holds the process-wide compute lock (no concurrent device
        /// use) and `self` is a valid context.
        pub unsafe fn run(
            &self,
            spirv: &[u8],
            groups: [u32; 3],
            push: &[u8],
            specs: &[BufSpec],
        ) -> Result<Readback> {
            let device = &self.device;

            // -- storage buffers (host-visible so we can upload/readback) --
            let mut bufs: Vec<HostBuf> = Vec::with_capacity(specs.len());
            for (i, s) in specs.iter().enumerate() {
                let b = make_host_buffer(device, &self.memprops, s.bytes)
                    .map_err(|e| anyhow!("buffer {i}: {e}"))?;
                if let Some(data) = &s.upload {
                    let ptr = device
                        .map_memory(b.memory, 0, s.bytes, vk::MemoryMapFlags::empty())
                        .map_err(|e| anyhow!("map buffer {i}: {e}"))?
                        as *mut u8;
                    std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len());
                    device.unmap_memory(b.memory);
                }
                bufs.push(b);
            }

            // -- descriptor + pipeline layout --
            let bindings: Vec<vk::DescriptorSetLayoutBinding> = (0..specs.len() as u32)
                .map(|i| {
                    vk::DescriptorSetLayoutBinding::default()
                        .binding(i)
                        .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                        .descriptor_count(1)
                        .stage_flags(vk::ShaderStageFlags::COMPUTE)
                })
                .collect();
            let dsl = device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                None,
            )?;
            let set_layouts = [dsl];

            // Push-constant size must be a multiple of 4 (Vulkan requirement).
            let push_size = push.len().div_ceil(4) * 4;
            let push_ranges = [vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::COMPUTE)
                .offset(0)
                .size(push_size as u32)];
            let mut plci = vk::PipelineLayoutCreateInfo::default().set_layouts(&set_layouts);
            if push_size > 0 {
                plci = plci.push_constant_ranges(&push_ranges);
            }
            let playout = device.create_pipeline_layout(&plci, None)?;

            // -- pipeline from the caller's SPIR-V --
            let code = ash::util::read_spv(&mut Cursor::new(spirv))?;
            let module = device
                .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&code), None)?;
            let stage = vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::COMPUTE)
                .module(module)
                .name(c"main");
            let pci = vk::ComputePipelineCreateInfo::default()
                .stage(stage)
                .layout(playout);
            let pipe = device
                .create_compute_pipelines(vk::PipelineCache::null(), &[pci], None)
                .map_err(|(_, e)| anyhow!("create_compute_pipelines: {e}"))?[0];
            device.destroy_shader_module(module, None);

            // -- descriptor set --
            let pool_sizes = [vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(specs.len() as u32)];
            let dpool = device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(1)
                    .pool_sizes(&pool_sizes),
                None,
            )?;
            let set = device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(dpool)
                    .set_layouts(&set_layouts),
            )?[0];
            let infos: Vec<[vk::DescriptorBufferInfo; 1]> = bufs
                .iter()
                .map(|b| {
                    [vk::DescriptorBufferInfo::default()
                        .buffer(b.buffer)
                        .offset(0)
                        .range(vk::WHOLE_SIZE)]
                })
                .collect();
            let writes: Vec<vk::WriteDescriptorSet> = infos
                .iter()
                .enumerate()
                .map(|(i, info)| {
                    vk::WriteDescriptorSet::default()
                        .dst_set(set)
                        .dst_binding(i as u32)
                        .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                        .buffer_info(info)
                })
                .collect();
            device.update_descriptor_sets(&writes, &[]);

            // -- command buffer: bind, push, dispatch --
            let cpool = device.create_command_pool(
                &vk::CommandPoolCreateInfo::default().queue_family_index(self.qf),
                None,
            )?;
            let cmd = device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(cpool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )?[0];
            let fence = device.create_fence(&vk::FenceCreateInfo::default(), None)?;

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
            if push_size > 0 {
                // Pad to the (multiple-of-4) range size the layout declared.
                let mut padded = push.to_vec();
                padded.resize(push_size, 0);
                device.cmd_push_constants(cmd, playout, vk::ShaderStageFlags::COMPUTE, 0, &padded);
            }
            device.cmd_dispatch(cmd, groups[0], groups[1], groups[2]);
            device.end_command_buffer(cmd)?;

            let cmds = [cmd];
            let submit = vk::SubmitInfo::default().command_buffers(&cmds);
            device.queue_submit(self.queue, &[submit], fence)?;
            // M3: bounded fence wait. An unbounded wait on a GPU hang (expected on
            // marginal timings; gfx1013 reset is broken) would spin forever while
            // holding the process-wide compute lock, wedging any later APU KAT.
            // 20 s exceeds cutest's 10 s KAT budget so a slow-but-live dispatch is
            // never killed; on timeout we bail the whole session so the lock frees.
            //
            // The bail DELIBERATELY leaks the Vulkan objects created above (fence,
            // pipeline, pools, buffers): the fence has a STILL-PENDING submission on
            // a hung device, so destroying it is UB (VUID-vkDestroyFence-fence-
            // 01120), and every other object is bound to that never-completing
            // submission. On a wedged gfx1013 (being reset/rebooted anyway) a
            // bounded leak beats blocking teardown -- the same wedge-path tradeoff
            // as cutest's Vk::drop. Freeing them here is NOT safe, so we don't.
            if device
                .wait_for_fences(&[fence], true, DISPATCH_FENCE_TIMEOUT_NS)
                .is_err()
            {
                bail!("dispatch fence timed out (GPU hang / device lost)");
            }

            // -- readback --
            let mut out: Vec<Option<Vec<u8>>> = Vec::with_capacity(specs.len());
            for (s, b) in specs.iter().zip(bufs.iter()) {
                if s.readback {
                    let ptr =
                        device.map_memory(b.memory, 0, s.bytes, vk::MemoryMapFlags::empty())?
                            as *const u8;
                    let mut v = vec![0u8; s.bytes as usize];
                    std::ptr::copy_nonoverlapping(ptr, v.as_mut_ptr(), s.bytes as usize);
                    device.unmap_memory(b.memory);
                    out.push(Some(v));
                } else {
                    out.push(None);
                }
            }

            // -- teardown (device is idle: we waited on the fence) --
            device.destroy_fence(fence, None);
            device.destroy_command_pool(cpool, None);
            device.destroy_descriptor_pool(dpool, None);
            device.destroy_pipeline(pipe, None);
            device.destroy_pipeline_layout(playout, None);
            device.destroy_descriptor_set_layout(dsl, None);
            for b in &bufs {
                device.destroy_buffer(b.buffer, None);
                device.free_memory(b.memory, None);
            }

            Ok(Readback { buffers: out })
        }
    }

    impl Drop for Ctx {
        fn drop(&mut self) {
            unsafe {
                let _ = self.device.device_wait_idle();
                self.device.destroy_device(None);
                self.instance.destroy_instance(None);
            }
        }
    }

    struct HostBuf {
        buffer: vk::Buffer,
        memory: vk::DeviceMemory,
    }

    unsafe fn make_host_buffer(
        device: &ash::Device,
        memprops: &vk::PhysicalDeviceMemoryProperties,
        size: u64,
    ) -> Result<HostBuf> {
        let bci = vk::BufferCreateInfo::default()
            .size(size)
            .usage(vk::BufferUsageFlags::STORAGE_BUFFER)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let buffer = device.create_buffer(&bci, None)?;
        let req = device.get_buffer_memory_requirements(buffer);
        // Host-visible + coherent so uploads/readbacks map without explicit
        // flush. A pure-compute harness values correctness over peak bandwidth;
        // a device-local fast path is M4's concern if it ever needs one.
        let want = vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT;
        let mt = find_mem(memprops, req.memory_type_bits, want)
            .or_else(|| {
                find_mem(
                    memprops,
                    req.memory_type_bits,
                    vk::MemoryPropertyFlags::HOST_VISIBLE,
                )
            })
            .ok_or_else(|| anyhow!("no host-visible memory type"))?;
        let ai = vk::MemoryAllocateInfo::default()
            .allocation_size(req.size)
            .memory_type_index(mt);
        let memory = device
            .allocate_memory(&ai, None)
            .map_err(|e| anyhow!("allocate {size} bytes: {e}"))?;
        device.bind_buffer_memory(buffer, memory, 0)?;
        Ok(HostBuf { buffer, memory })
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

    unsafe fn pick_device(instance: &ash::Instance) -> Result<vk::PhysicalDevice> {
        let pds = instance
            .enumerate_physical_devices()
            .map_err(|e| anyhow!("enumerate devices: {e}"))?;
        if pds.is_empty() {
            bail!("no Vulkan physical devices");
        }
        // Prefer a real GPU (skip llvmpipe/CPU); fall back to whatever's there.
        let mut fallback = None;
        for &pd in &pds {
            let p = instance.get_physical_device_properties(pd);
            let name = CStr::from_ptr(p.device_name.as_ptr())
                .to_string_lossy()
                .to_lowercase();
            if fallback.is_none() {
                fallback = Some(pd);
            }
            if !name.contains("llvmpipe") && !name.contains("software") && !name.contains("cpu") {
                return Ok(pd);
            }
        }
        fallback.ok_or_else(|| anyhow!("no usable Vulkan device"))
    }

    unsafe fn compute_queue_family(
        instance: &ash::Instance,
        pd: vk::PhysicalDevice,
    ) -> Result<u32> {
        instance
            .get_physical_device_queue_family_properties(pd)
            .iter()
            .position(|q| q.queue_flags.contains(vk::QueueFlags::COMPUTE))
            .map(|i| i as u32)
            .ok_or_else(|| anyhow!("no compute queue family"))
    }

    /// Loader + instance + at least one device, with no logical device created.
    pub fn available() -> bool {
        unsafe {
            let Ok(entry) = ash::Entry::load() else {
                return false;
            };
            let app = vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_1);
            let ici = vk::InstanceCreateInfo::default().application_info(&app);
            let Ok(instance) = entry.create_instance(&ici, None) else {
                return false;
            };
            let ok = instance
                .enumerate_physical_devices()
                .map(|p| !p.is_empty())
                .unwrap_or(false);
            instance.destroy_instance(None);
            ok
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};
    use std::thread;

    /// The core acceptance: concurrent lock holders are mutually exclusive.
    /// Runs WITHOUT a GPU — it exercises only the serializing mutex that
    /// `dispatch` shares (via `with_compute_lock`).
    #[test]
    fn compute_lock_serializes() {
        const THREADS: usize = 8;
        const ITERS: usize = 2000;

        let inside = Arc::new(AtomicBool::new(false));
        let violations = Arc::new(AtomicUsize::new(0));
        let counter = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(THREADS));

        let mut handles = Vec::new();
        for _ in 0..THREADS {
            let inside = Arc::clone(&inside);
            let violations = Arc::clone(&violations);
            let counter = Arc::clone(&counter);
            let barrier = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                barrier.wait(); // maximise contention: all threads start together
                for _ in 0..ITERS {
                    with_compute_lock(|| {
                        // If mutual exclusion holds, nobody is ever already inside.
                        if inside.swap(true, Ordering::SeqCst) {
                            violations.fetch_add(1, Ordering::SeqCst);
                        }
                        // Widen the critical-section window so a missing lock races.
                        for _ in 0..32 {
                            std::hint::spin_loop();
                        }
                        counter.fetch_add(1, Ordering::SeqCst);
                        inside.store(false, Ordering::SeqCst);
                    });
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(
            violations.load(Ordering::SeqCst),
            0,
            "two threads were inside the compute lock at once"
        );
        assert_eq!(counter.load(Ordering::SeqCst), THREADS * ITERS);
    }

    /// Input validation happens before any GPU access, so these assert without a
    /// device present.
    #[test]
    fn dispatch_rejects_bad_input() {
        // empty SPIR-V
        assert!(dispatch(&[], [1, 1, 1], &[], &[BufSpec::scratch(16)]).is_err());
        // SPIR-V not a multiple of 4
        assert!(dispatch(&[1, 2, 3], [1, 1, 1], &[], &[BufSpec::scratch(16)]).is_err());
        // no buffers
        assert!(dispatch(&[0, 0, 0, 0], [1, 1, 1], &[], &[]).is_err());
        // oversized push
        let big = vec![0u8; MAX_PUSH_BYTES + 4];
        assert!(dispatch(&[0, 0, 0, 0], [1, 1, 1], &big, &[BufSpec::scratch(16)]).is_err());
        // upload larger than the buffer
        let over = BufSpec {
            bytes: 4,
            upload: Some(vec![0u8; 8]),
            readback: false,
        };
        assert!(dispatch(&[0, 0, 0, 0], [1, 1, 1], &[], &[over]).is_err());
    }

    /// `with_session` takes the SAME process-wide compute lock as `dispatch` and
    /// `with_compute_lock`. Whether or not a device exists on this (possibly
    /// GPU-less) host, it must release the lock afterwards so the serialization
    /// primitive keeps working. No GPU assertion — we only prove the lock is
    /// reusable after a session attempt (a leaked lock would deadlock the second
    /// acquire below).
    #[test]
    fn with_session_releases_the_lock() {
        // On a GPU host this runs the (empty) closure; on a build host it returns
        // Err from device creation. Either way the lock must be freed.
        let _ = with_session(|_s| Ok(()));
        let v = with_compute_lock(|| 7);
        assert_eq!(v, 7);
    }

    /// M3 (round 2): a session whose closure BAILS (as the KAT / one-shot dispatch
    /// does on a fence timeout) must still free the process-wide compute lock --
    /// otherwise a single wedged dispatch would hold it forever and every later
    /// GPU op would deadlock. GPU-free: proves the lock is reusable after an Err.
    #[test]
    fn with_session_err_still_frees_lock() {
        let _ = with_session(|_s| -> Result<()> { bail!("simulated fence timeout") });
        // A leaked lock would deadlock this second acquire.
        let v = with_compute_lock(|| 99);
        assert_eq!(v, 99);
    }

    #[test]
    fn bufspec_helpers() {
        let s = BufSpec::scratch(64);
        assert_eq!(s.bytes, 64);
        assert!(s.upload.is_none());
        assert!(!s.readback);

        let io = BufSpec::io(vec![1, 2, 3, 4]);
        assert_eq!(io.bytes, 4);
        assert!(io.readback);
    }
}
