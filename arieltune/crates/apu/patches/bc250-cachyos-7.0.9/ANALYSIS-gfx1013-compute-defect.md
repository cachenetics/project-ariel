# BC-250 gfx1013: compute defect analysis, and the GabiWar/production patch comparison

**Date:** 2026-08-05
**Hardware:** AMD BC-250 blade — Cyan Skillfish APU, gfx1013, GC IP 10.1.3, 16 GB unified GDDR6
**Kernel:** `linux-cachyos-bore-7.0.9` (hard pin — see [Kernel pin](#kernel-pin))
**Production repo:** bc-250-rocm on the internal Gitea (Fabian/Dani)
**GabiWar repo:** bc250-rocm-gabriwar on the internal Gitea (Gabriel Duarte Guerra)
**Ariel patch series:** `arieltune/crates/apu/patches/bc250-cachyos-7.0.9`

---

## Evidence key

This document distinguishes what was checked against source or captured data
from what remains inference. Claims are tagged:

| Tag | Meaning |
|---|---|
| **[V]** | Verified in this session against kernel source, a checksum, or a captured log. Path and line given. |
| **[R]** | Reported by an upstream investigator, not independently reproduced here. Source given. |
| **[H]** | Hypothesis. Consistent with the evidence, not established. |
| **[X]** | Checked and **disproven**. |

---

## Executive summary

ROCm inference is production-stable on this hardware. The unsolved defect is
narrower than "ROCm is broken": **any workload with a second heavy GPU phase
fails** — training backward pass, img2img reverse pass, multi-step generation.
The first phase completes; the next one dies. **[R]**

When the failure surfaces as a page fault rather than a HIP error, the fault is
always an `SQC (inst)` read — the shader *instruction* cache. **[R]** That
places the defect before the fault: the wave's program counter was already
wrong when it was issued.

The dominant fault signature is not a random address. Across nine faults on
separate boots with ASLR active, seven are identical in shape: top byte `0xff`,
low 20 bits `0xbb000`, with only bits [39:20] moving. **[R]** The process's own
code objects live at `0x7f...`. `0x7f` → `0xff` is **one bit** — bit 47, the
canonical-form boundary — and the top byte of a 48-bit shader address is exactly
`COMPUTE_PGM_HI`, an 8-bit register carrying bits [47:40]. **[H]** So the
address is not garbage; it is a plausible entry point with a single wrong bit in
the one register that holds that bit.

A prior version of this document asserted that the driver *assembles* a bad
address from a corrupted MEC queue and dispatches to it. **That is wrong and is
retracted** — see [Retracted](#retracted-the-src_data-narrative). The code in
question is fault *reporting*, verified against source.

Two patches are added to the Ariel series as a result. Neither claims a fix:
patch `17` is the measurement that isolates the defect, patch `18` is
containment so measuring it does not cost a blade.

---

## Retracted: the `src_data` narrative

The earlier analysis centred on these two lines in `gmc_v10_0_process_interrupt()`:

```c
addr  = (u64)entry->src_data[0] << 12;          /* bits [43:12] */
addr |= ((u64)entry->src_data[1] & 0xf) << 44;  /* bits [47:44] */
```

and claimed the MEC compute queue returns corrupted `src_data[]`, from which the
driver "blindly assembles a random address and dispatches to it".

**This is disproven on three independent counts. [X]**

### 1. `src_data[]` is IH ring data, not compute queue data

`/var/tmp/linux-6.19/drivers/gpu/drm/amd/amdgpu/amdgpu_ih.c:289` — the entry is
decoded from the **interrupt handler ring**:

```c
entry->src_data[0] = dw[4];
entry->src_data[1] = dw[5];
```

Those are dwords 4 and 5 of a 32-byte interrupt vector written by the GPU's IH
block, fed by GMC/UTCL2 **after** a translation was already rejected. The
AQL/HSA compute queue is a different structure, in a different address space,
with a different consumer. **[V]**

### 2. `addr` never reaches hardware

Every use of `addr` in that function, from
`<internal-storage>/iommu/artifacts/v73/host-snapshot-snap-8201b059/source/gmc_v10_0.c`:

| Call | Purpose |
|---|---|
| `amdgpu_gmc_filter_faults(adev, ..., addr, ...)` | de-duplicate repeat faults |
| `amdgpu_vm_handle_fault(adev, ..., addr, ...)` | retry path only — attempts to *page in* |
| `amdgpu_vm_update_fault_cache(adev, ..., addr, ...)` | record for debugfs |
| `dev_err(... "in page starting at address 0x%016llx" ...)` | print |

It is never written to a register and never becomes a PC. The function is
registered as `.process` in `gmc_v10_0_irq_funcs` — an interrupt handler. **[V]**

The ordering settles it: the GPU faulted → the IH block recorded it → the driver
decoded it. A handler cannot cause the fault it is being notified about.

### 3. The reported address is faithful

A plausible objection is that the flag bits overlap the address bits, corrupting
the report. They do not.
`/var/tmp/linux-6.19/drivers/gpu/drm/amd/amdgpu/amdgpu_gmc.h:89-92`:

```c
#define AMDGPU_GMC9_FAULT_SOURCE_DATA_RETRY 0x80
#define AMDGPU_GMC9_FAULT_SOURCE_DATA_READ  0x40
#define AMDGPU_GMC9_FAULT_SOURCE_DATA_WRITE 0x20
#define AMDGPU_GMC9_FAULT_SOURCE_DATA_EXE   0x10
```

Flags occupy bits [7:4]; the address takes `src_data[1] & 0xf`, bits [3:0]. No
overlap. The reported address is what actually faulted. **[V]**

### Also retracted: the KFD queue-reset fix

The earlier proposal to call `amdgpu_amdkfd_interrupt(adev, entry)` on the
corruption signature does not work. `kgd2kfd_interrupt()` filters via
`interrupt_is_wanted()` and routes to `kfd_int_process_v10.c`, which handles **SQ
interrupts only** (AUTO/INST/ERROR encodings). KFD does not handle GMC page
faults; the vector would be silently dropped. **[V]**
(`/var/tmp/linux-6.19/drivers/gpu/drm/amd/amdkfd/kfd_device.c:1121`,
`kfd_int_process_v10.c`)

Additionally, the proposed detection predicate `(addr >> 44) == 0xF` is unsafe:
bits [47:44] = `0xF` is a **legal** canonical high address — that is precisely
what `AMDGPU_GMC_HOLE_START`/`HOLE_END` sign-extension exists to handle — so the
check would also misfire on genuine faults.

---

## The actual evidence

### Fault corpus

Nine faults, separate boots, ASLR active. Source: GabiWar
`docs/08-address-corruption.md`. **[R]**

| address | high byte | low 20 bits | client |
|---|---|---|---|
| `0xffb5506bb000` | `0xff` | `0xbb000` | SQC (inst) |
| `0xffe8464bb000` | `0xff` | `0xbb000` | SQC (inst) |
| `0xff14126bb000` | `0xff` | `0xbb000` | SQC (inst) |
| `0xff550dabb000` | `0xff` | `0xbb000` | SQC (inst) |
| `0xff26f76bb000` | `0xff` | `0xbb000` | SQC (inst) |
| `0xfffd1e6bb000` | `0xff` | `0xbb000` | SQC (inst) |
| `0xff64aa2bb000` | `0xff` | `0xbb000` | SQC (inst) |
| `0x81cf24049000` | `0x81` | `0x49000` | SQC (inst) |
| `0x1f837bd19000` | `0x1f` | `0x19000` | SQC (inst) |

Seven of nine share both the top byte and the low 20 bits across boots with
randomisation active. The two outliers differ in *both* fields, which is
consistent with them being a different code object rather than a different bug —
but with n=2 that is not established. **[H]**

### Fault status decode

Identical every occurrence: **[R]**

```
[gfxhub] page fault (src_id:0 ring:88 vmid:8)
  from client 0x1b (UTCL2)
  GCVM_L2_PROTECTION_FAULT_STATUS: 0x008012B0
  Faulty UTCL2 client ID: SQC (inst) (0x9)
  MORE_FAULTS: 0x0   WALKER_ERROR: 0x0
  PERMISSION_FAULTS: 0xb   MAPPING_ERROR: 0x0   RW: 0x0
```

`SQC (inst)` is the instruction cache; `RW: 0x0` is a read. `MAPPING_ERROR: 0x0`
with `WALKER_ERROR: 0x0` means the VA was inside the aperture and the walk
completed; the PTE then denied access. For a bogus address that is exactly what
you expect, so **this decode confirms the address is bad but does not by itself
discriminate why**. It should not be over-read — the per-bit semantics of
`PERMISSION_FAULTS` were not pinned down in this session.

### The bit-47 observation

This is new analysis from this session and the reason patch `17` exists. **[H]**

```
0x7f = 0111 1111        expected top byte (user code objects)
0xff = 1111 1111        observed top byte
       ^
       bit 47 — canonical-form boundary
```

The top byte of a 48-bit address is bits [47:40], and the top bit of that byte is
bit 47. Substituting `0x7f` back yields addresses in the process's normal range:

```
0xffb5506bb000  ->  0x7fb5506bb000
0xff64aa2bb000  ->  0x7f64aa2bb000
0x1f837bd19000  ->  0x7f837bd19000
```

The register split matches exactly:

```
COMPUTE_PGM_LO  (0x1bac)   address bits [39:8]    <- stays plausible
COMPUTE_PGM_HI  (0x1bad)   address bits [47:40]   <- 8 bits wide, holds the wrong byte
```

GabiWar disassembled the MEC firmware writing both, at two sites: **[R]**

```
lsrd r12, r11, #40           ; bits [47:40]
lsrd r11, r11, #8            ; bits [39:8]
stw  r11, reg[r0, #0x2e0c]   ; COMPUTE_PGM_LO
stw  r12, reg[r0, #0x2e0d]   ; COMPUTE_PGM_HI
```

This reframes the problem from "garbage address" to "one bit set that should be
clear", which is a far more tractable target.

### Two prior dismissals that were themselves wrong

Recorded because they cost real time. Both from GabiWar's own retrospective. **[R]**

- The corrected address was compared against `kernel_obj` from AQL packets and
  found far away. Invalid: `kernel_obj` points at the **kernel descriptor**, not
  the entry point. Code starts at `kernel_obj + kernel_code_entry_byte_offset`.
  The right comparison was never made.
- The corrected address was found inside a mapped `renderD128` region and
  presented as confirmation. Invalid: **85% of that range is mapped** in this
  process, so landing in a mapping carries almost no information.

### What is ruled out

| Ruled out | Basis |
|---|---|
| MEC firmware divergence | 98.49% identical to navi10 on an aligned diff, **zero divergent register accesses**. An earlier 76.5% figure was a byte-offset artifact mistaking shifted code for absent code. **[R]** |
| MEC firmware *version* | Production runs actual `navi10_mec.bin` renamed to `cyan_skillfish2_mec.bin` — MD5 `9f6ed115127318fcddef8090bcd7af9d`, 268,160 bytes, byte-identical. Stock CachyOS firmware preserved as `.cs-orig` (MD5 `01a5687ad3997ead6fc090aee2ff76df`, 268,592 bytes). **[V]** |
| Corruption in the AQL packet | Failing dispatch carried `kernel_obj=0x7f21c2a2a540` — ordinary and uncorrupted. Whatever goes wrong happens after the runtime hands over. **[R]** |
| Kernel class / size / dtype | BLAS, elementwise and reduction all fail; the exact failing shape works in isolation; fp16/fp32/bf16 all fail. **[R]** |
| Late code-object loading | Failing dispatches have a correct `kernel_obj`. Note this tests the *packet field*, not whether the memory at that address read back correctly. **[R]** |
| `AMD_SERIALIZE_KERNEL=3` as a fix | Does not force completion signals. **[R]** |

### A conflict worth recording

The `arieltune` hardware manual (Chapter 6) documents the MEC as
*silicon-defective* for non-atomic global stores, concluding "MEC is dead, use
the GFX ring". That finding is from Vulkan/RADV work.

It **conflicts** with two things established here: ROCm inference is
production-stable through the MEC path, and GabiWar's aligned firmware diff found
zero divergent register accesses. Both cannot be fully true as stated. The most
likely reconciliation is that the manual's finding is real but narrower than
"MEC is dead" — a specific access pattern RADV hit, which the ROCm dispatch path
mostly avoids. **[H]** This is unresolved and should not be treated as settled in
either direction.

---

## Repository comparison

Two independent investigations — no shared files, no copied code, no
cross-references. Different communities (BC-250 Discord: neoney/anrp/wtfuzz vs
git.sudx.de: Fabian/Dani) converging on the same hardware truths.

| | Production (`bc-250-rocm`) | GabiWar (`bc250-rocm-gabriwar`) |
|---|---|---|
| **Goal** | General ROCm compute (llama.cpp inference) | Stable Diffusion via ComfyUI |
| **Commits** | 5 | 1 |
| **Patch format** | Python scripts (in-place source edit) | Unified `.patch` diffs |
| **Kernel files** | `gfx_v10_0.c`, `gmc_v10_0.c`, `amdgpu_gmc.c` | `amdgpu_ttm.c`, `gmc_v10_0.c` |
| **User-space** | HIP tests, watchdog, install pipeline, custom rocBLAS gfx1013 kernels, SGEMM benchmarks | ComfyUI warmup node, Tensile safeStoi, rocBLAS artifacts |
| **Docs** | 1 comprehensive README | 11 focused markdown docs + README |
| **CU config** | 40 CU unlock | stock 24 CU |

### The one shared fix

Both independently disabled `flush_pasid_uses_kiq` in `gmc_v10_0_hw_init()`:

```c
/* Stock */
adev->gmc.flush_pasid_uses_kiq = !amdgpu_emu_mode;

/* GabiWar — unconditional */
adev->gmc.flush_pasid_uses_kiq = false;

/* Ariel patch 14 — version-guarded */
if (gc_ver >= IP_VERSION(10,1,0) && gc_ver < IP_VERSION(10,2,0))
        adev->gmc.flush_pasid_uses_kiq = false;
else
        adev->gmc.flush_pasid_uses_kiq = !amdgpu_emu_mode;
```

GabiWar's A/B is worth recording because it is clean: stock module reproduces
`timeout waiting for kiq fence` → `TLB flush failed for PASID 5` → CP
unrecoverable → `resume of IP block <sdma_v5_0> failed -110`. Patched module runs
a full SD pipeline with zero errors. Reverting reproduces the failure. **[R]**

Ariel's version is version-guarded and pairs with patches 14/15 covering
`gmc_v10_0_flush_gpu_tlb()`, `amdgpu_gmc_flush_gpu_tlb_pasid()` and
`fw_reg_write_reg_wait()`, plus dead-GPU detection. Redundant with GabiWar's,
not conflicting; the guarded version is retained.

### Conflict matrix

| GabiWar patch | Conflicts? | Disposition |
|---|---|---|
| `01-amdgpu-ttm-null-check` | No — `amdgpu_ttm.c`, untouched by the series | **Adopted as patch 18** |
| `0001-drm-amdgpu-guard-against-NULL-pages` | Same fix, submission-formatted | Basis for patch 18 |
| `02-amdgpu-flush-pasid-kiq` | Same line as Ariel patch 14 | Not adopted — patch 14 is guarded and broader |
| `03-tensile-client-safestoi` | No — userspace C++ | Not adopted — only relevant when tuning Tensile |

| Ariel patch | Conflicts with GabiWar? | Retained? |
|---|---|---|
| `13` GFXOFF disable | No — GabiWar never touches `gfx_v10_0.c` | Yes. GabiWar's continuous pipeline keeps the GPU busy enough that GFXOFF never engages; that masks the problem rather than fixing it, and training has idle gaps. |
| `14` `gmc_v10_0.c` KIQ + dead-GPU | Overlaps GabiWar `02` | Yes — guarded, broader, has dead-GPU detection |
| `15` `amdgpu_gmc.c` KIQ | No | Yes |
| `12`/`16` CU unlock | No | Yes — optional feature, out of scope for this defect |

---

## What was added to the Ariel series

### Patch 17 — `gmc_v10_0.c`: gfx1013 instruction-fetch fault probe

Diagnostic. Report-only: no control-flow change, no register reads, no locks.

Logs, for gfx1013 only, the raw interrupt vector and the bit-47-cleared
reconstruction, so the candidate can be checked against the faulting process's
GPU VA map offline:

```
BC-250 probe: src_data[0]=... src_data[1]=... retry=. exe=. write=.
BC-250 probe: fault 0x................ bit47=. candidate 0x................
```

Gated by `amdgpu.bc250_fault_probe` (default on, `0` disables). The module
parameter doubles as the runtime tell for `arieltune apu patches`.

**It deliberately does not read `COMPUTE_PGM_LO/HI`.** Those are per-pipe CP
registers behind GRBM indexing and `srbm_mutex`; this handler runs in hard-IRQ
context, so reading them there would deadlock. Reading them from userspace via
debugfs `amdgpu_regs` is independently known to hang this board. **[R]** That
measurement needs a process-context path and is the next step, not this patch.

### Patch 18 — `amdgpu_ttm.c`: guard NULL `ttm->pages[]` on unpopulate

Containment, by Gabriel Duarte Guerra, carried unmodified in substance.

`amdgpu_ttm_tt_unpopulate()` writes `pages[i]->mapping` with no NULL check. A BO
left partially populated by an aborted GPU command panics the kernel on cleanup:

```
RIP: 0010:amdgpu_ttm_tt_unpopulate+0x77/0xd0 [amdgpu]
CR2: 0000000000000018
Kernel panic - not syncing: Fatal exception
```

`CR2 = 0x18` is the offset of `struct page::mapping`, confirming `pages[i]` was
NULL. On the BC-250 this turned every compute fault into a dead blade — four hard
hangs in one session upstream, with unclean shutdowns and data loss. **[R]**

This is why it ships alongside patch 17: without it, measuring the defect costs a
reboot instead of producing a log. The missing check is upstream code and is worth
upstreaming independently of the BC-250.

### Validation performed

- Base tree identified by reverse-applying series patch `14` against the v73 host
  snapshot — clean, so the snapshot is the series base + patch 14. **[V]**
- Both patches generated as real unified diffs against that tree, not hand-written.
- Both apply with `patch -p1 --forward --fuzz=0` (the exact invocation
  `kbuild.rs` uses), and correctly **reject** re-application. **[V]**
- `cargo test -p apu` — 34 passed, including `patch_steps_apply_with_zero_fuzz`. **[V]**
- `arieltune apu patches` lists both with correct tells. **[V]**

---

## What was deliberately not adopted

| Item | Reason |
|---|---|
| Fault-handler retry / `-EAGAIN` | `entry` is a hardware interrupt vector describing a fault that already happened. Re-reading returns the same data. Fixes nothing. |
| Address rewriting in the fault handler | Would require the detection predicate `(addr >> 44) == 0xF`, which also matches legal canonical high addresses. Actively dangerous. |
| `amdgpu_amdkfd_interrupt()` queue reset | KFD's v10 interrupt path handles SQ interrupts only; the vector is dropped. **[V]** |
| ComfyUI warmup node (~90 dummy dispatches) | Works by changing timing/layout. Useful as *evidence* that this is a race, not as a fix. |
| `HIP_LAUNCH_BLOCKING=1`, `AMD_SERIALIZE_KERNEL=3` | Same — reduces the window, hides the symptom. Keep for bisection, not for production. |
| Tensile `safeStoi` | Only relevant when tuning Tensile. |
| GabiWar rocBLAS artifacts | Production has its own gfx1013 kernels and validated SGEMM benchmarks in `/data/rocm/`. |

The common thread: every one of these changes timing or suppresses reporting.
None of them explains why bit 47 is set.

---

## Next steps

1. **Deploy 17 + 18**, reproduce the failing workload, collect `candidate`
   addresses from dmesg.
2. **Correlate** each candidate against the faulting process's GPU VA map. If
   candidates consistently resolve to a mapped code object, the defect is
   isolated to the top byte of the shader program address — and the remaining
   question is narrow: who sets bit 47.
3. **Read `COMPUTE_PGM_LO/HI` at fault time from process context.** If `PGM_HI`
   reads `0xff` while `PGM_LO` is correct, the root cause is pinned. This needs a
   deferred work item, not the IRQ handler.
4. **Open question if confirmed:** does bit 47 originate in the CP's read of the
   kernel descriptor, in the descriptor's own `kernel_code_entry_byte_offset` (a
   *signed* 64-bit field in the AMDHSA ABI — a sign-extension bug there would set
   exactly this bit), or in the `HSA_OVERRIDE_GFX_VERSION=10.1.0` path that makes
   ROCr emit gfx1010 objects for gfx1013? The third is worth testing early
   because it is free.

---

## Kernel pin

Pin to `linux-cachyos-bore-7.0.9`. Do not build the series against `7.0.11+` —
those regress the BC-250 SDMA path.

**Operator note on `HSA_ENABLE_SDMA=0`:** the claim that SDMA is simply broken on
this hardware is not accurate. Kernel 7.0.9 has working SDMA. It is disabled
because it is the last thing you want to change while chasing a ROCm defect — it
has been validated under specific workloads, offers no performance benefit here,
and is highly niche to high-memory-churn work.

---

## Credits
- **Studebaker** (https://github.com/cachenetics/project-ariel)
- **Fabian** (https://git.sudx.de/BC250/ROCm)) and **Dani** — production repo, fundamental research
- **Gabriel Duarte Guerra** ([GabriWar](https://github.com/GabriWar)) — fault corpus, MEC firmware disassembly and aligned diff, TTM NULL guard (patch 18), and the retrospective on his own two bad dismissals, which is the most useful part of that repo
- **neoney** (BC-250 Discord) — KIQ bypass discovery NIXOS Maniac
- **anrp**, **wtfuzz** (BC-250 Discord) — additional findings
