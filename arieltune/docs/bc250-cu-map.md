# BC-250 CU Map - shader-array topology and the dispatch model

Cachenetics BC-250 research. This documents the Cyan Skillfish (gfx1013) compute-unit
topology, the register triplet that gates CU enablement, and the empirical **dispatch
model** that predicts real throughput from a given CU routing - the "CU map."

It is contributed here so the map is public, dated, and reusable. It builds directly on
prior community work (see Acknowledgments in the repo `README.md`): the bc250-collective's
original enablement work, duggasco's CU-unlock research, and WinnieLV's live manager,
whose `apply_target_masks` register sequence this project ports.

## Topology

The BC-250 GPU (Cyan Skillfish, PCI `1002:13FE`, RDNA-class gfx1013) has:

- **2 shader engines** (SE0, SE1)
- **2 shader arrays per engine** (SH0, SH1) - 4 shader arrays total, grid order
  `(SE,SH) = (0,0) (0,1) (1,0) (1,1)`
- **5 WGP per shader array**, **2 CU per WGP** → 10 CU per array → **40 CU physical**

Stock firmware harvests the board to **24 CU** (WGP 0-2 per array dispatched).

### WGP → CU

```
WGP w  ->  CU (2w, 2w+1)
WGP 0 = CU 0,1     WGP 1 = CU 2,3     WGP 2 = CU 4,5     (stock active)
WGP 3 = CU 6,7     WGP 4 = CU 8,9                        (harvested stock)
```

A per-array WGP dispatch mask is 5 bits: `0x1F` = all 5 WGP = 10 CU (full),
`0x07` = WGP 0-2 = 6 CU (factory).

## The three registers (enumeration vs dispatch vs power-gate)

Enabling a CU requires moving **three** independent gates. Any one alone is a no-op.

| Register | Role | Stock | Full-40 |
|---|---|---|---|
| `CC_GC_SHADER_ARRAY_CONFIG` | **enumeration** harvest mask - what the driver reports/RADV issues to | harvested | cleared (0) |
| `SPI_PG_ENABLE_STATIC_WGP_MASK` | **dispatch** gate - which WGP the SPI actually routes waves to (per shader array) | `0x07` | `0x1F` |
| `RLC_PG_ALWAYS_ON_WGP_MASK` | **power-gate** override - keeps the routed WGP powered | harvested | `0x1F` |

- Clearing only `CC` is a no-op: RADV enumerates 40 but SPI never dispatches to the
  extra WGP.
- Setting only `SPI` is a no-op: the driver never generates work for un-enumerated CU.
- All three together: the silicon dispatches waves to all 40 CU.

Live application (WinnieLV's sequence, ported in `crates/apu/src/curoute.rs`): via `umr`,
per shader array. Boot-time application: the amdgpu patch
(`crates/apu/patches/.../0018-unlock-all-40-compute-units.patch`, module parameter
`bc250_cc_write_mode=3`, gated on PCI `0x13FE`).

## The dispatch model (the "CU map" finding)

**Routed CU count does NOT predict throughput.** Compute-bound throughput on gfx1013 is
gated by the **least-populated shader engine**. The two engines (SE0 = arrays `(0,0)+(0,1)`,
SE1 = arrays `(1,0)+(1,1)`) dispatch waves in parallel, so real throughput tracks the
weaker engine:

```
effective_CU = 4 × min(SE0_wgp, SE1_wgp)   =   2 × (CU of the least-populated engine)

  where SE0_wgp = per-array WGP of (0,0) + (0,1),  SE1_wgp = (1,0) + (1,1)
```

- **Compute-bound** (KAT / clpeak GFLOPS): `GFLOPS ≈ 44 × effective_CU` at 1500 MHz.
- The WGP split **within** an engine does not matter: `5/1` in an engine dispatches like
  `3/3` (both 6 WGP). Only the two per-engine totals count.
- Balanced engines (`SE0_wgp == SE1_wgp`): `effective_CU == routed_CU`, linear in routed CU.
- Imbalanced engines: the stronger engine is wasted down to the weaker one - a large
  penalty, not a small one.

Verified across a BC-250 fleet sweep (2026-07), including the permutations that separate
this model from the earlier "two smallest arrays" one:

| Per-array WGP | Engines (WGP) | Routed CU | effective_CU | measured GFLOPS |
|---|---|---|---|---|
| 5 / 5 / 5 / 5 | 10 / 10 | 40 | 40 | 1746 |
| 4 / 4 / 4 / 4 | 8 / 8 | 32 | 32 | balanced, linear |
| 3 / 3 / 3 / 3 | 6 / 6 | 24 | 24 | 1057 |
| 5 / 1 / 5 / 1 | 6 / 6 | 24 | **24** | **1058** |
| 5 / 5 / 1 / 1 | 10 / 2 | 24 | **8** | **357** |
| 5 / 4 / 2 / 1 | 9 / 3 | 24 | 12 | - |
| 4 / 2 / 1 / 1 | 6 / 2 | 16 | 8 | - |

The decisive pair is `5/1/5/1` vs `5/5/1/1`: **same four arrays**, but `5/1/5/1` splits the
two small arrays across engines (both engines 6 WGP -> 24 effective, 1058 GFLOPS) while
`5/5/1/1` stacks them in SE1 (weaker engine 2 WGP -> 8 effective, 357 GFLOPS). A
permutation-invariant "two smallest arrays" model predicts both at 8 and is wrong by 3×.
So **engine balance beats count**, and *where* an array sits (which engine) is what
matters, not its rank among the four. This supersedes the earlier "two smallest arrays",
"populated × shallowest-depth", and "linear in routed CU" models - all were
engine-balanced special cases.

### Memory-bound is different

Memory-bound work (e.g. llama.cpp prompt processing) does **not** follow the two-smallest
law. It scales as `(number of populated arrays) × (min WGP depth)` - a distinct regime.
Match the model to the workload.

## Safety

- **Never dispatch compute to a shape with an empty shader array.** An all-zero WGP mask
  on any of the 4 arrays hangs and wedges gfx1013 (observed twice on hardware). The
  empty-array compute case is untested and unsafe; `curoute.rs::apply()` refuses it before
  touching hardware.
- Localize a suspected-bad CU subtractively (full-40 minus one WGP), never by gating a
  whole array off.
- The AMD ROCm OpenCL ICD is not stable at 40 CU on gfx1013 (Vulkan/RADV paths are);
  keep `amdocl64.icd` disabled.

## Reading the live map

The live CU bitmap comes straight from amdgpu via the DRM `AMDGPU_INFO` ioctl
(`DEV_INFO` query `0x16`) - no libdrm dependency. `drm_amdgpu_info_device` field offsets
(validated on a real BC-250 by the original `cu_map.sh`):

```
num_shader_engines            @ 20
num_shader_arrays_per_engine  @ 24
cu_active_number              @ 48
cu_bitmap[4][4]               @ 56
```

`arieltune apu cu bench` measures `effective_cu` empirically on the box; the shader-array
shaping and warnings live in `crates/apu/src/curoute.rs::shape()`.

The standalone reference script that produced this map is included here:
[`cu_map.sh`](cu_map.sh) - read-only; queries the bitmap and prints the SE/SH map with
active/harvested counts.
