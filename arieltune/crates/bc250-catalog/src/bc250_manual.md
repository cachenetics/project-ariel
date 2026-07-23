# ASRock BC-250 — OEM System Manual

```
┌──────────────────────────────────────────────────────────────────────────────────────┐
│                                                                                      │
│      A S R o c k   B C - 2 5 0                             OEM  SYSTEM  MANUAL       │
│      AMD Oberon APU · Harvested SoC · Open Carrier Board                             │
│                                                                                      │
├──────────────────────────────────────────────────────────────────────────────────────┤
│                                                                                      │
│      CPU     6C/12T Zen 2              3.2 GHz base · 4.0 GHz boost                  │
│      GPU     40 CU RDNA 1.x            gfx1013 · GC 10.1.3                           │
│      MEM     16 GB GDDR6 unified       256-bit · 8 channels                          │
│      SoC     Family 17h Model 47h      PCI 1002:13FE rev 00                          │
│      BOARD   ASRock BC-250 carrier     A68H FCH · Bolton-D2H                         │
│                                                                                      │
│      One die.  CPU and GPU on one unified memory pool.                               │
│                                                                                      │
└──────────────────────────────────────────────────────────────────────────────────────┘

The ASRock BC-250 is a Zen 2 + RDNA 1.x accelerator built on a harvested AMD Oberon
die, mounted on an open carrier board.  CPU and GPU sit on one package and share a
single 16 GB pool of GDDR6 over the on-die Data Fabric — a true unified-memory
architecture.  The GPU is gfx1013, an RDNA 1.x part unique to this silicon; the
console-derived SoC has no on-die Fusion Controller Hub, so the carrier provides a
discrete AMD A68H (Bolton-D2H) southbridge over a UMI link.

This manual documents what the board IS and how it is built — components, buses,
registers, firmware, configuration, safety limits.  It is a static reference:
measured and dynamic behaviour belongs to the companion tools.

── CHAPTERS ───────────────────────────────────────────────────────── read outside-in ──

  1  Carrier Board            4  System Configuration
  2  Subsystem Internals      5  Driver Stack
  3  Security & Trust         6  Compute Stack
```

## Overview

```
── OVERVIEW ────────────────────────────────────────────────────── system at a glance ──

Zen 2 + RDNA 1.x on one harvested AMD Oberon die, on an open carrier board.  CPU
and GPU share a single 16 GB GDDR6 pool over the on-die Data Fabric — a true
unified-memory architecture.

┌─ SILICON ────────────────────────────────────────────────────────────────────────────┐
│  SoC               AMD Oberon — Family 17h Model 47h           PCI 1002:13FE rev 00  │
│  CPU               6C/12T Zen 2 — SMU-managed          3.2 GHz base · 4.0 GHz boost  │
│  GPU               40 CU RDNA 1.x — gfx1013 · GC 10.1.3      stock 24 CU · patch 40  │
│  Memory            16 GB GDDR6 on-package                      256-bit · 8 channels  │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ CARRIER ────────────────────────────────────────────────────────────────────────────┐
│  Board             ASRock BC-250                                                     │
│  Chipset           AMD A68H FCH (Bolton-D2H) — discrete southbridge over UMI         │
│  Display           DP 1.4 on board I/O (disabled for headless operation)             │
│  Network           Realtek RTL8111H Gigabit Ethernet                                 │
│  Storage           M.2 2280 — NVMe (PCIe Gen2 x2) or SATA, via NXP protocol mux      │
│  USB               USB 3.0 / 2.0 / 1.1 (on-die + FCH)                                │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ FIRMWARE ───────────────────────────────────────────────────────────────────────────┐
│  SPI flash         16 MB — UEFI (BIOS P3.00), PSP, SMU, ABL, APCB                    │
│  VBIOS             113-AMDRBN-003                                                    │
└──────────────────────────────────────────────────────────────────────────────────────┘

The die exposes on the order of three dozen IP blocks; the full enumeration is
in Chapter 2, Subsystem Internals.

                        See: Chapter 1 · Carrier Board — Chapter 2 · Subsystem Internals
```

## Hardware Tree

```
── HARDWARE TREE ────────────────────────────────────────── carrier → APU → IP blocks ──

The whole board, outside-in.  Every node below is broken down in full in its
chapter.

ASRock BC-250 Carrier Board
│
├── AMD BC-250 APU (Family 17h Model 47h, Oberon)          PCI 1002:13FE rev 00
│   │
│   ├── CPU — Zen 2 (microcode 0x8407007)
│   │   ├── 6C / 12T (8 physical, 2 fused — one per CCX, harvest bin)
│   │   ├── 2 CCX × 3 active cores, 4 MB L3 each (8 MB total)
│   │   ├── per-core 32 KB L1D + 32 KB L1I + 512 KB L2
│   │   └── ISA: SSE4A, AVX, AVX2, FMA, AES-NI, SHA-NI, CLWB, SEV/SEV-ES (no AVX-512)
│   │
│   ├── GPU — GC 10.1.3, RDNA 1.x (gfx1013)
│   │   ├── 40 CU on die (stock harvest 24 active; kernel patch enables the fused 16)
│   │   ├── 4 shader arrays × 10 CU, 20 WGP (5/array), 2 SE, 2,560 SPs; Wave64 native
│   │   ├── 256 VGPR/SIMD, 64 KB LDS/WGP
│   │   ├── firmware ME 0x63 · PFP 0x94 · CE 0x25 · RLC 0x0D · MEC 0x90
│   │   ├── native IMAGE_BVH_INTERSECT_RAY (gfx1013-only in RDNA 1)
│   │   └── SDMA0 / SDMA1 5.0.1 — DMA engines (fw 0x34)
│   │
│   ├── MEMORY
│   │   ├── UMC 8.1.1 ×2 — 8 channels, 256-bit GDDR6 (8 chips × 2 GB)
│   │   ├── MMHUB 2.0.3 · ATHUB 2.0.3 · SYSTEMHUB 2.1.0 · HDP 5.0.1
│   │   └── L2IMU — L2 invalidation management
│   │
│   ├── INTERCONNECT
│   │   ├── DF 3.5.0 — Data Fabric (central interconnect)
│   │   ├── NBIF 2.1.1 — PCIe endpoint + IOMMU · IOHC — I/O hub
│   │   └── PCIE 4.2.0 · PCS — PCIe PHY
│   │
│   ├── SECURITY
│   │   └── MP0 / PSP 11.0.8 — ARM Cortex-A5 (SOS, ABL0-4; APCB at BIOS 0xAB1000)
│   │
│   ├── POWER / CLOCK / THERMAL
│   │   ├── MP1 / SMU 11.0.8 — Xtensa LX, fw 88.6.0 (0x00580600), 5 mailbox queues
│   │   ├── SMUIO 11.0.8 — power-gate sequencer, GPIO (8 tiles)
│   │   ├── THM 11.0.1 — on-die thermal · FUSE 11.0.1 — harvest data
│   │   └── CLK GFXCLK 0x16C00 · SOCCLK 0x16E00 · MCLK 0x17000 · LCLK 0x17E00
│   │
│   ├── INTERRUPTS — OSSSYS / IH 5.0.1
│   ├── DISPLAY (headless) — DMU 2.0.3 (DP 1.4) · DIO · DAZ
│   ├── I/O — ACP 4.0.0 (audio) · USB 4.5.0 ×2 · CCP (AES/SHA/RSA/RNG)
│   └── DEBUG — DBGU_NBIO/IO 3.0 · DFX / DFX_DAP 2.0 (JTAG)
│
├── AMD A68H FCH (Bolton-D2H, discrete southbridge, 1022:78xx) — over UMI
│   ├── PCIe Gen2 x2 → M.2 NVMe (via NXP mux)
│   ├── PCIe Gen1 x1 → RTL8111H GbE (r8169)
│   ├── SATA III (1022:7801) → M.2 SATA mode (via NXP mux)
│   ├── USB — XHCI 1022:7814 · EHCI ×2 (1022:7808) · OHCI ×3 (1022:7807/7809)
│   ├── LPC (1022:780e) → NCT6686D Super I/O
│   ├── PCI bridge (1022:780f) · Hudson PCIe ports 0/1 (1022:43a0 / 43a1)
│   └── SMBus (1022:780b) / I2C → VRM, sensors, SPD
│
├── Power Delivery (VRM)
│   ├── ISL69247 — main controller (VddGfx + VCore), PMBUS
│   ├── ISL95712 — secondary (VddNb / VSoC)
│   └── ISL99360 — smart power stages · inputs: PCIe 8-pin + Molex 8-pin
│
├── SPI Flash
│   ├── Winbond W25Q128JV — 16 MB (BIOS, PSP, SMU, ABL, APCB)
│   └── Macronix MX25L4006E — 512 KB (Super I/O firmware)
│
└── Debug Headers
    ├── J2 — JTAG / HDT+ (20-pin, unpopulated)
    ├── J4004 — SPI flash programming (populated)
    ├── I2C_HEADER1 — PMBUS to VRM · TPMS1 — LPC + SMBus (18-pin)
    └── AUTO_PWRON1 / CLRCMOS1 — jumpers

                        See: Chapter 1 · Carrier Board — Chapter 2 · Subsystem Internals
```

## CPU — Zen 2

```
── CPU ─────────────────────────────────────────────────────────────── Zen 2 · 6C/12T ──

A 6-core / 12-thread Zen 2 complex, harvested from an 8-core die with one core
fused off per CCX.  Clocking is owned by the SMU — there is no Linux cpufreq path
on this part.

  ┌─ CCX 0 ─────────────────────────────┐    ┌─ CCX 1 ─────────────────────────────┐
  │  C0     C1     C2     C3 (fused)    │    │  C0     C1     C2     C3 (fused)    │
  │           L3  4 MB shared           │    │           L3  4 MB shared           │
  └─────────────────────────────────────┘    └─────────────────────────────────────┘
     8 physical cores · 2 fused (one per CCX, harvest bin) · 8 MB L3 total

┌─ SPECIFICATIONS ─────────────────────────────────────────────────────────────────────┐
│  Cores / Threads   6C / 12T (8 physical, 2 fused)                                    │
│  Topology          2 CCX × 3 active cores; 4 MB L3 per CCX (8 MB total)              │
│  Cache (per core)  32 KB L1D + 32 KB L1I + 512 KB L2                                 │
│  Clock             3.2 GHz base · 4.0 GHz boost (SMU-managed, 100 MHz refclk)        │
│  ISA               x86-64, SSE4A, AVX, AVX2, FMA, AES-NI, SHA-NI, CLWB, SEV/SEV-ES   │
│                    not present: AVX-512                                              │
│  Microcode         0x8407007                                                         │
└──────────────────────────────────────────────────────────────────────────────────────┘

                                              See: Chapter 2 · Subsystem Internals → CPU
```

## GPU — RDNA 1.x (gfx1013)

```
── GPU ─────────────────────────────────────────────── RDNA 1.x · gfx1013 · GC 10.1.3 ──

An RDNA 1.x graphics core unique to this silicon.  The die carries forty compute
units; the stock BC-250 harvest fuses the part down to twenty-four — a kernel
patch liberates all forty.  Native IMAGE_BVH_INTERSECT_RAY (gfx1013-only within
RDNA 1), but no dedicated RT block, no Wave32 mode, no cooperative-matrix (WMMA).

  ┌─ SE 0 ──────────────────────────────┐    ┌─ SE 1 ──────────────────────────────┐
  │  SA 0    10 CU · 5 WGP              │    │  SA 2    10 CU · 5 WGP              │
  │  SA 1    10 CU · 5 WGP              │    │  SA 3    10 CU · 5 WGP              │
  └─────────────────────────────────────┘    └─────────────────────────────────────┘
       2 SE · 4 shader arrays × 10 CU · 20 WGP (5/array) · 2,560 SP · Wave64

┌─ SPECIFICATIONS ─────────────────────────────────────────────────────────────────────┐
│  Compute Units     40 on die — stock harvest 24 active; patch enables the fused 16   │
│  Shader Layout     4 shader arrays × 10 CU · 20 WGP (5/array) · 2 SE · 2,560 SP      │
│  Wavefront         Wave64 native (no Wave32)                                         │
│  Register File     256 VGPR/SIMD · 106 SGPR                                          │
│  LDS               64 KB per WGP (32 KB per CU)                                      │
│  Cache             L0 16 KB/CU → L1 128 KB/SA → L2 4 MB → GDDR6 (no Inf. Cache)      │
│  Clock domain      GFXCLK — SMN 0x16C00                                              │
│  Firmware          ME 0x63 · PFP 0x94 · CE 0x25 · RLC 0x0D · MEC 0x90                │
│  DMA               SDMA0 / SDMA1 5.0.1 (fw 0x34)                                     │
└──────────────────────────────────────────────────────────────────────────────────────┘

                                                          See: Chapter 6 · Compute Stack
```

## Memory — 16 GB Unified GDDR6

```
── MEMORY ─────────────────────────────────────────────────────── 16 GB unified GDDR6 ──

CPU and GPU share one 16 GB GDDR6 pool through the Data Fabric — the same
controllers, the same physical DRAM.  Eight 2 GB chips present a 256-bit bus
across eight channels.  The BIOS carves a small framebuffer region at boot; the
remainder is a shared system pool visible to both processors.

┌─ SPECIFICATIONS ─────────────────────────────────────────────────────────────────────┐
│  Capacity          16 GB GDDR6 on-package (8 chips × 2 GB)                           │
│  Controller        UMC 8.1.1 ×2 — 8 channels                                         │
│  Bus               256-bit (16 sub-channels × 16-bit)                                │
│  Apertures         VRAM carve-out (BIOS UMA, default 256 MB) + shared system pool    │
│  Timings           set from APCB tokens at boot; overridable via extended CMOS       │
│  Hubs              MMHUB 2.0.3 · ATHUB 2.0.3 · SYSTEMHUB 2.1.0 · HDP 5.0.1           │
└──────────────────────────────────────────────────────────────────────────────────────┘

     ┌──────────┐        ┌─────────────┐        ┌──────────────┐
     │  Zen 2   │<──────>│ Data Fabric │<──────>│ RDNA 1.x GPU │
     │  6 cores │        │  DF 3.5.0   │        │    40 CU     │
     └──────────┘        └──────┬──────┘        └──────────────┘
                                │
                          ┌─────┴─────┐
                          │   MMHUB   │
                          └─────┬─────┘
                     ┌──────────┴──────────┐
                ┌────┴────┐           ┌────┴────┐
                │  UMC 0  │           │  UMC 1  │
                │ 4 chan  │           │ 4 chan  │
                └────┬────┘           └────┬────┘
                     │                     │
                 GDDR6 8 GB            GDDR6 8 GB
              (4 chips × 2 GB)      (4 chips × 2 GB)

                   See: Chapter 2 → UMC / Memory — Chapter 4 → BIOS memory configuration
```

## Interconnect — Data Fabric

```
── INTERCONNECT ────────────────────────────────────────────── Data Fabric · DF 3.5.0 ──

The Data Fabric is the coherent spine of the SoC — every CPU ↔ GPU ↔ memory ↔ I/O
transaction crosses it.  PCIe reaches the outside through NBIF and the on-die
root complex; the System Management Network (SMN) is the internal register bus,
reached from the host through PCI configuration space on the root complex.

                        ┌─────────────────────┐
     Zen 2 CPU ─────────┤                     ├───────── GC 10.1.3 (gfx1013)
                        │     DATA FABRIC     │
     UMC 0 / 1 ─────────┤      DF 3.5.0       ├───────── MMHUB / ATHUB
                        │       (FCLK)        │
     MP0 · MP1 ─────────┤                     ├───────── NBIF → IOHC → PCIe / FCH
                        └─────────────────────┘

┌─ SPECIFICATIONS ─────────────────────────────────────────────────────────────────────┐
│  Data Fabric       DF 3.5.0 — coherent interconnect (FCLK)                           │
│  PCIe              NBIF 2.1.1 (endpoint + IOMMU) · IOHC · PCIE 4.2.0 · PCS (PHY)     │
│  SMN access        PCI cfg 0000:00:00.0 — offset 0xB8 (index) / 0xBC (data)          │
└──────────────────────────────────────────────────────────────────────────────────────┘

                                                    See: Chapter 2 → Interconnect Fabric
```

## Security — Platform Security Processor

```
── SECURITY ─────────────────────────────────────────────────────────────── PSP · MP0 ──

An ARM Cortex-A5 (the MP0 block) runs ahead of the x86 cores and is the root of
the platform's firmware trust chain: secure boot, key management, firmware
loading.  It runs a Trusted OS from the SPI flash; the AGESA configuration block
(APCB) and the ABL boot-loader stages live alongside it.

     power-on → PSP (SOS) → ABL0–4 → UEFI → OS   (x86 cores released last)

┌─ SPECIFICATIONS ─────────────────────────────────────────────────────────────────────┐
│  Core              ARM Cortex-A5 — MP0 / PSP 11.0.8                                  │
│  Firmware          Trusted OS (SOS) + ABL stages 0–4                                 │
│  PSP directory     BIOS 0x8E0000                                                     │
│  APCB              BIOS 0xAB1000 — 235 CBS configuration tokens                      │
└──────────────────────────────────────────────────────────────────────────────────────┘

                                                       See: Chapter 3 · Security & Trust
```

## Power, Clock & Thermal — System Management Unit

```
── POWER · CLOCK · THERMAL ──────────────────────────────────────────────── SMU · MP1 ──

The SMU (the MP1 block) is the board's power and thermal controller: a Tensilica
Xtensa LX core running PSP-verified firmware.  It owns the clock and voltage
domains, commands the VRM over the SVI2 serial bus, and manages the power-island
gates.  The OS reaches it only through five hardware mailbox queues — every clock
or voltage change is a request the firmware may honor, clamp, or refuse.

┌─ SPECIFICATIONS ─────────────────────────────────────────────────────────────────────┐
│  Core              Tensilica Xtensa LX — MP1 / SMU 11.0.8                            │
│  Firmware          88.6.0 (0x00580600) — PSP-verified                                │
│  Mailboxes         5 queues (Q0–Q4) — command / response / argument triples          │
│  Power gating      SMUIO 11.0.8 — 8 power-island tiles, GPIO                         │
│  Thermal / fuse    THM 11.0.1 · FUSE 11.0.1                                          │
│  Clock domains     GFXCLK 0x16C00 · SOCCLK 0x16E00 · MCLK 0x17000 · LCLK 0x17E00     │
│  VRM link          SVI2 serial bus                                                   │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ CAUTION ────────────────────────────────────────────────────────────────────────────┐
│  Never send Q0 0x04 / 0x2E, or untested Q3 handlers — hang; power-cycle needed.      │
│  Never exceed one mailbox message per 100 ms.                                        │
│  No mailbox traffic during sustained compute.                                        │
└──────────────────────────────────────────────────────────────────────────────────────┘

                                           See: Chapter 2 → SMU / Power, Clock & Thermal
```

## Chipset — A68H FCH (Bolton-D2H)

```
── CHIPSET ──────────────────────────────────────────────────── A68H FCH · Bolton-D2H ──

The carrier provides a discrete AMD A68H (Bolton-D2H) Fusion Controller Hub over
a UMI link — the console-derived Oberon SoC has no on-die FCH.  The UMI link is
transparent to PCI enumeration, so the FCH functions appear on the root bus; they
are a distinct silicon family (Hudson/Bolton 1022:78xx plus the 43a0/43a1 PCIe
bridges) from the on-die Ariel blocks (1022:13xx).

   Oberon SoC ══ UMI ══ A68H FCH ──┬── PCIe Gen2 x2 ─────→ M.2 NVMe (via NXP mux)
                                   ├── PCIe Gen1 x1 ─────→ RTL8111H GbE
                                   ├── SATA III ─────────→ M.2 SATA (via NXP mux)
                                   ├── USB — XHCI · EHCI ×2 · OHCI ×3
                                   ├── LPC ──────────────→ NCT6686D Super I/O
                                   └── SMBus / I2C ──────→ VRM · sensors · SPD

┌─ DEVICE FUNCTIONS ───────────────────────────────────────────────────────────────────┐
│  USB 3.0           XHCI — 1022:7814                                                  │
│  USB 2.0           EHCI ×2 — 1022:7808                                               │
│  USB 1.1           OHCI ×3 — 1022:7807 / 7809                                        │
│  SATA              1022:7801 — AHCI, SATA III                                        │
│  SMBus             1022:780b — PIIX4 adapter, 5 sub-buses                            │
│  LPC               1022:780e — bridge to NCT6686D Super I/O                          │
│  PCI bridge        1022:780f — FCH PCI bridge                                        │
│  PCIe bridges      1022:43a0 / 43a1 — Hudson PCIe ports 0 / 1                        │
└──────────────────────────────────────────────────────────────────────────────────────┘

                                                    See: Chapter 1 · Carrier Board → FCH
```

## Power Delivery — VRM

```
── POWER DELIVERY ─────────────────────────────────────────────────────────────── VRM ──

A multi-phase digital VRM feeds the APU, commanded by the SMU over SVI2.  The
board takes both a PCIe 8-pin and a Molex 8-pin input.

   PCIe 8-pin (J1000) ──┬──→ ISL69247 ──→ ISL99360 smart stages ──→ VddGfx + VCore
   Molex 8-pin ─────────┤
                        └──→ ISL95712 ─────────────────────────────→ VddNb / VSoC
                                 ↑
                          SMU over SVI2

┌─ COMPONENTS ─────────────────────────────────────────────────────────────────────────┐
│  Main controller   ISL69247 — VddGfx + VCore (PMBUS-accessible)                      │
│  Secondary         ISL95712 — VddNb / VSoC                                           │
│  Power stages      ISL99360 — smart (integrated FET + driver)                        │
│  Inputs            PCIe 8-pin (J1000) + Molex Micro-Fit 8-pin                        │
│  Command path      SMU → SVI2 → controllers                                          │
│  Monitoring        I2C_HEADER1 (PMBUS) · NCT6686D                                    │
└──────────────────────────────────────────────────────────────────────────────────────┘

                                                         See: Chapter 1 → Power Delivery
```

## Board I/O — Storage, Network, Super I/O

```
── BOARD I/O ────────────────────────────────────────── storage · network · super I/O ──

┌─ STORAGE ────────────────────────────────────────────────────────────────────────────┐
│  Slot              M.2 2280                                                          │
│  Modes             NVMe (PCIe Gen2 x2) or SATA — selected by NXP protocol mux        │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ NETWORK ────────────────────────────────────────────────────────────────────────────┐
│  Controller        Realtek RTL8111H GbE — 1000BASE-T                                 │
│  PCI ID            10ec:8168 — driver r8169                                          │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ SUPER I/O ──────────────────────────────────────────────────────────────────────────┐
│  Controller        Nuvoton NCT6686D — over LPC                                       │
│  Functions         temps, voltage ADCs, fan PWM, watchdog                            │
│  Firmware          own 512 KB flash — Macronix MX25L4006E                            │
└──────────────────────────────────────────────────────────────────────────────────────┘

                                            See: Chapter 1 → Storage, Network, Super I/O
```

## Firmware & Configuration

```
── FIRMWARE & CONFIGURATION ────────────────────────────────── SPI flash · boot chain ──

┌─ FIRMWARE STORE ─────────────────────────────────────────────────────────────────────┐
│  SPI flash         Winbond W25Q128JV — 16 MB                                         │
│  Contents          UEFI · PSP directory · SMU · ABL · APCB                           │
│  BIOS              P3.00                                                             │
│  VBIOS             113-AMDRBN-003                                                    │
└──────────────────────────────────────────────────────────────────────────────────────┘

  BOOT CHAIN     PSP → ABL0–4 → UEFI → OS

  CONFIG FLOW    APCB flash tokens → AmdSetup / Setup EFI variables
                                   → kernel parameters

                        See: Chapter 3 → Firmware map — Chapter 4 · System Configuration
```

## Debug & Recovery

```
── DEBUG & RECOVERY ─────────────────────────────────────────────── headers · jumpers ──

┌─ HEADERS ────────────────────────────────────────────────────────────────────────────┐
│  J2                JTAG / HDT+ — 20-pin, 1.27 mm (unpopulated)                       │
│  J4004             SPI flash programming (populated) — external BIOS / recovery      │
│  I2C_HEADER1       PMBUS to the VRM controllers                                      │
│  TPMS1             LPC + SMBus — 18-pin                                              │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ JUMPERS ────────────────────────────────────────────────────────────────────────────┐
│  AUTO_PWRON1       auto power-on                                                     │
│  CLRCMOS1          clear CMOS                                                        │
└──────────────────────────────────────────────────────────────────────────────────────┘

                                               See: Chapter 1 → Debug & Recovery Headers
```


# 1. Carrier Board

```
── CARRIER BOARD ──────────────────────────────────────── open carrier · discrete FCH ──

The ASRock BC-250 carrier is the open board the Oberon APU is mounted on: it
supplies power, clocking, I/O breakout, board management, and the debug and recovery
access the console-derived SoC does not expose on its own.  Unlike a modern socketed
AMD platform, the carrier reverts to a discrete external FCH — the A68H (Bolton-D2H)
southbridge over UMI — because the silicon was designed for a console motherboard,
not a standard platform.

┌─ BOARD ──────────────────────────────────────────────────────────────────────────────┐
│  Board            ASRock BC-250 open carrier              "Robin" — ASRock codename  │
│  FCH              AMD A68H — Bolton-D2H, 65 nm, vendor 1022       UMI x4 · PCIe 2.0  │
│  VRM              multi-phase digital — ISL69247 + ISL95712 + ISL99360  rated 220 W  │
│  Super I/O        Nuvoton NCT6686D — EC-mode strapped                   HWM + UART2  │
│  Storage          M.2 2280 — NVMe over FCH PCIe Gen2 x2                              │
│  Network          Realtek RTL8111H Gigabit Ethernet                           r8169  │
│  BIOS flash       Winbond W25Q128JV — 16 MiB SPI                                     │
│  SIO flash        Macronix MX25L4006E — 512 KiB (NCT6686D firmware)          SIO1_R  │
│  Board mgmt       libAsrCore v1.70.0 — needs a detection shim on this board          │
└──────────────────────────────────────────────────────────────────────────────────────┘

   POWER   PCIe 8-pin (J1000) ──┬── 12 V ──→ VRM 220 W ──→ APU rails
           Molex 8-pin ─────────┘            ISL69247 · ISL95712 · ISL99360 stages
           (J2000/J2001)                     SMU-commanded over SVI2

   I/O     Oberon APU ══ UMI x4 ══ A68H FCH ──┬── PCIe Gen2 x2 ──→ M.2 NVMe
                                              ├── PCIe Gen1 x1 ──→ RTL8111H GbE
                                              ├── SATA III (AHCI)
                                              ├── USB — XHCI · EHCI ×2 · OHCI ×3
                                              ├── SMBus — 5 buses → VRM · clock gen
                                              └── LPC ───────────→ NCT6686D Super I/O
                                                                   (EC mode · HWM+UART2)

 See: Ch 2 · Subsystem Internals — Ch 3 · Security & Trust — Ch 4 · System Configuration
```

## A68H FCH — Fusion Controller Hub

```
── A68H FCH ─────────────────────────────────────── Bolton-D2H · discrete southbridge ──

A discrete AMD A68H (Bolton-D2H, 65 nm) southbridge, tied to the APU over a UMI x4
link (PCIe 2.0-based): USB, SATA/AHCI, SMBus, the LPC bridge to the Super I/O, the
FCH GPIO array — and the only working watchdog on the board (the TCO timer).  AMD
folded the southbridge onto the APU die from Carrizo (2015) onward; this
console-derived carrier is architecturally unusual in reverting to an external FCH.

┌─ CHIPSET ────────────────────────────────────────────────────────────────────────────┐
│  Part             AMD A68H — Bolton-D2H, 65 nm                          vendor 1022  │
│  Uplink           UMI x4 (PCIe 2.0-based)                                            │
│  PCIe downstream  Gen2 x2 → M.2 NVMe · Gen1 x1 → RTL8111H GbE        Hudson bridges  │
│  MMIO window      0xFED80000 — 64 KB                                                 │
│  LPC bridge       PCI 00:14.3 — to NCT6686D Super I/O                 also on TPMS1  │
│  Watchdog         FCH TCO (sp5100_tco) — /dev/watchdog0            only working WDT  │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ PCI FUNCTIONS ──────────────────────────────────────────────────────────────────────┐
│  1022:780b  SMBus controller 00:14.0            piix4_smbus — 5 buses                │
│  1022:780e  LPC bridge       00:14.3            ISA bridge to NCT6686D               │
│  1022:780f  PCI bridge       00:14.4            subtractive decode                   │
│  1022:7801  SATA controller  00:11.0            ahci — SATA III · 6 Gbps             │
│  1022:7807  USB OHCI (1.1)   00:12.0 · 00:13.0  ohci-pci  (×2)                       │
│  1022:7808  USB EHCI (2.0)   00:12.2 · 00:13.2  ehci-pci  (×2)                       │
│  1022:7809  USB OHCI (1.1)   00:14.5            ohci-pci                             │
│  1022:7814  USB XHCI (3.0)   00:10.0            xhci_hcd                             │
│  1022:43a0  PCIe bridge      00:15.0            Hudson port 0 → M.2 NVMe (Gen2 x2)   │
│  1022:43a1  PCIe bridge      00:15.1            Hudson port 1 → GbE (Gen2-cap x1)    │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ BOLTON FAMILY — A68H = BOLTON-D2H IN THE 65 nm LINE ────────────────────────────────┐
│  Chipset  Codename     SATA    USB 3.0   RAID     PCIe      UMI                      │
│  A55      Bolton-D1    6× 6G   0         no       4× Gen2   x4 Gen2                  │
│  A58      Bolton-D2L   4× 6G   0         no       4× Gen2   x4 Gen2                  │
│  A68H     Bolton-D2H   4× 6G   2         0/1/10   4× Gen2   x4 Gen2  ← this board    │
│  A78      Bolton-D3    6× 6G   2         0/1/10   4× Gen2   x4 Gen2                  │
│  A88X     Bolton-D4    8× 6G   4         0/1/10   4× Gen2   x4 Gen2                  │
└──────────────────────────────────────────────────────────────────────────────────────┘

All external USB is provided by the FCH — the two on-die USB 4.5.0 controllers are
covered in Chapter 2.  One xHCI 3.0 controller plus the legacy EHCI / OHCI
companions (PCI IDs above); port-enable and over-current mapping live in the FCH
MMIO USB control block at window offset 0x200:

┌─ USB PORT CONTROL — MMIO WINDOW +0x200 ──────────────────────────────────────────────┐
│  +0x038    port-enable bitmask                                                       │
│  +0x058    over-current (OC) pin map                                                 │
└──────────────────────────────────────────────────────────────────────────────────────┘

The 64 KB MMIO window holds the base registers, IOMUX, USB control, GPIO, SMBus, a
DesignWare I2C block, and the TCO watchdog / timer block:

┌─ MMIO WINDOW — 0xFED80000 · 64 KB ───────────────────────────────────────────────────┐
│  Offset   Size    Region                  Notes                                      │
│  0x000    0x100   FCH base registers      vendor ID 0x1022 · PMIO config             │
│  0x100    0x100   IOMUX                   one byte per pin — bits[7:6] = function    │
│  0x200    0x100   USB port control        +0x038 port-enable · +0x058 OC-pin map     │
│  0x300    0x100   Address decoder         LPC ranges · IOAPIC base · PMIO            │
│  0x400    0x100   (unknown)               6 active regs — possibly AcpiBlk           │
│  0x500    0x300   GPIO control array      pins 0-191 · stride 4 B                    │
│  0x800    0x100   GPIO interrupt map      header + 53 entries                        │
│  0x900    0x100   SMBus status (8 ch)     8 × 32-byte blocks · +0x0C = bus status    │
│  0xA00    0x100   SMBus0 controller       HST_STS · HST_CNT · XMIT_SLVA · data       │
│  0xC00    0x100   I2C controller          DesignWare — 3 buses at stride 0x20        │
│  0x1000   0x100   WDT / timer             FCH timer / TCO watchdog (0xFED80B00)      │
└──────────────────────────────────────────────────────────────────────────────────────┘

The PMIO enable register at 0x24-0x27 reads 0xFED80001 (bit 0 = enable); 0x2000 and
above reads all 0xFF (unmapped).

FCH GPIO — the control array is at 0xFED80500; pin N has a 32-bit register at
0x500 + N*4.  Only byte 0 is writable, and the hardware mirrors the written byte
across all four bytes (a read returns e.g. 0xC5C5C5C5).  IOMUX is one byte per pin
at 0xFED80100 + N.

┌─ GPIO REGISTERS ─────────────────────────────────────────────────────────────────────┐
│  GPIO control register — byte 0              IOMUX byte — 0xFED80100 + pin           │
│    bit 7     OE — output enable (1 = out)      bits 7:6  func 0=GPIO · 1-3=periph    │
│    bit 6     OUT — output drive (OE = 1)       bit 5     pull-up enable              │
│    bit 5     PU — pull-up enable               bit 4     pull-down enable            │
│    bit 4     PD — pull-down enable             bits 3:0  reserved                    │
│    bits 3:1  reserved                                                                │
│    bit 0     IN — input status (read)                                                │
└──────────────────────────────────────────────────────────────────────────────────────┘

Write protocol: read the 32-bit register, extract byte 0, modify, write back as a
32-bit value.  Valid array: pins 0-191 (768 bytes); requires /dev/mem.  Confirmed
GPIO-mode pins (IOMUX func = 0): 36-39, 66, 68-71, 103, 114, 162-164, 167-170,
185-186.

┌─ CAUTION — FCH GPIO ─────────────────────────────────────────────────────────────────┐
│  This is the Hudson/A68H base 0x500, NOT the 0x1500 used by the mainline             │
│  gpio-amd-fch driver (which targets the A55E FCH and reads all zeros here).          │
│                                                                                      │
│  Never write FCH GPIO pins 36, 38, or 66 — each asserts a power-sequencing /         │
│  NVMe PERST# line: immediate board power-off (kernel panic, drive dropped from       │
│  the bus).  Pins 37 and 39 sit in the same cluster and are unverified — treat        │
│  as unsafe.  Read access is safe on all pins.                                        │
└──────────────────────────────────────────────────────────────────────────────────────┘

FCH TCO watchdog — the board's only working hang-recovery mechanism (the NCT6686D
LDN 8 watchdog is unreachable in EC mode).  The kick is a single MMIO store, so it
survives kernel hangs and most GPU deadlocks; it is FCH-side and does not touch the
GFX/SDMA path, so it does not interfere with compute.

┌─ TCO WATCHDOG ───────────────────────────────────────────────────────────────────────┐
│  Driver           Linux sp5100_tco                                                   │
│  Device           /dev/watchdog0 (246:0)                                             │
│                   legacy alias /dev/watchdog (10:130) — same timer                   │
│  MMIO             TCO block at 0xFED80B00 within the FCH window                      │
│  Timeout          1-65535 s (driver min/max_timeout)          module opt heartbeat=  │
│  On expiry        FCH asserts a hardware reset                       full cold boot  │
└──────────────────────────────────────────────────────────────────────────────────────┘

Arm / kick / disarm: load sp5100_tco (optionally heartbeat= in /etc/modprobe.d/);
a daemon opens /dev/watchdog0 ONCE and writes a byte periodically; to disarm
cleanly, write the magic 'V' byte to the held-open descriptor before closing.

┌─ CAUTION — SINGLE-OPEN WATCHDOG ─────────────────────────────────────────────────────┐
│  The device is single-open and expect_close is per-file-handle.  A separate          │
│  printf V > /dev/watchdog0 returns EBUSY and does NOT disarm — the timer stays       │
│  armed and the board hard-resets at timeout.                                         │
│                                                                                      │
│  /dev/watchdog (10:130) and /dev/watchdog0 (246:0) are two nodes onto the SAME       │
│  timer — the single-open rule spans both; use one node consistently                  │
│  (/dev/watchdog0 preferred).                                                         │
└──────────────────────────────────────────────────────────────────────────────────────┘

      See: SMBus / I2C topology · Board Management — this chapter — Ch 4 · Configuration
```

## Power Delivery — VRM

```
── POWER DELIVERY ─────────────────────────────────────────────────────── VRM · 220 W ──

A multi-phase digital VRM (rated 220 W) converts the 12 V board input to the APU
rails and auxiliaries.  The main controller drives smart power stages; a secondary
controller handles the SoC / Northbridge rail.  The SMU inside the APU is the sole
writer of voltage setpoints, commanding the controllers over the SVI2 2-wire serial
bus — host access over PMBus is telemetry-only.

┌─ COMPONENTS ─────────────────────────────────────────────────────────────────────────┐
│  Main controller  ISL69247 (PUA1) — VddGfx + VCore                    primary rails  │
│  Secondary        ISL95712 (PUIO1) — VddNb / VSoC                     SoC / NB rail  │
│  Power stages     ISL99360 (PUA11+) — hi/lo FET + driver + current sense             │
│  Rating           220 W                                                              │
│  Inputs           PCIe 8-pin — J1000, primary 12 V                                   │
│                   Molex Micro-Fit 8-pin — J2000 / J2001                              │
│  Setpoint bus     SVI2 — SMU → controllers                      not host-observable  │
│  Telemetry        PMBus over SMBus port 1 (I2C_HEADER1)                   read-only  │
└──────────────────────────────────────────────────────────────────────────────────────┘

   PCIe 8-pin (J1000) ──┬── 12 V ──┬── ISL69247 ──→ ISL99360 stages ──┬─→ VCore
   Molex 8-pin ─────────┘          │    (PUA1)        (PUA11+)        ├─→ VddGfx
   (J2000/J2001)                   │                                  └─→ VDDCR_SOC
                                   └── ISL95712 ─────────────────────────→ VddNb / VSoC
                                        (PUIO1)

        SMU ══ SVI2 ══→ controllers       setpoints — the SMU is the sole writer
        host ── PMBus ─→ I2C_HEADER1      telemetry only

┌─ RAIL ASSIGNMENT ────────────────────────────────────────────────────────────────────┐
│  VDDCR_CPU      Zen 2 core voltage (VCore)                                 ISL69247  │
│  VDDCR_GFX      RDNA GPU core — follows the VF curve                       ISL69247  │
│  VDDCR_SOC      SoC uncore                                                 ISL69247  │
│  VddNb / VSoC   SoC / Northbridge (aux)                                    ISL95712  │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ PMBUS TELEMETRY — LINEAR11 (VOUT: LINEAR16) ────────────────────────────────────────┐
│  0x88   READ_VIN            input voltage                                            │
│  0x8B   READ_VOUT           output voltage                                 Linear16  │
│  0x8C   READ_IOUT           output current                                           │
│  0x8D   READ_TEMPERATURE_1  controller temperature                                   │
│  0x96   READ_POUT           output power                                             │
└──────────────────────────────────────────────────────────────────────────────────────┘

Read telemetry over the kernel i2c-dev interface on port 1, e.g.
i2ctransfer -y 1 w1@0xNN 0x88 then i2ctransfer -y 1 r2@0xNN.  Note: the device
answering PMBus on I2C_HEADER1 at 0x60 physically identifies as an Infineon
XDPE132G5C (PMBus Direct mode, PAGE 0 = standby/aux, PAGE 1 = SoC core VDD_CR);
the ISL69247 is the datasheet / decompilation reference for the main controller —
the physical part on this header is the Infineon device.

┌─ CAUTION — READ-ONLY ────────────────────────────────────────────────────────────────┐
│  All host-side VRM access is read-only monitoring.  Do NOT issue PMBus writes —      │
│  the SMU overwrites any setpoint on its next SVI2 frame (within milliseconds),       │
│  and a write races the live control loop.  Voltage tuning is out of scope for        │
│  this manual; it belongs to the companion tune application.                          │
└──────────────────────────────────────────────────────────────────────────────────────┘

        See: I2C_HEADER1 device map — this chapter — Ch 2 → SMU / Power, Clock & Thermal
```

## Super I/O — Nuvoton NCT6686D

```
── SUPER I/O ───────────────────────────────────────────────────── NCT6686D · EC mode ──

A Nuvoton NCT6686D hardware-monitor part, board-strapped into EC (Embedded
Controller) mode at power-on — which permanently locks the traditional SuperIO
index/data config path.  The hardware monitor and UART2 work; the watchdog / GPIO /
PWM logical devices are gated behind the HWM page protocol and unreachable.  The
chip runs its own firmware from a dedicated 512 KiB Macronix flash.

┌─ SUPER I/O ──────────────────────────────────────────────────────────────────────────┐
│  Part             Nuvoton NCT6686D                 chip ID 0xD441 · rev 0xBC · UIO1  │
│  Mode             EC — board-strapped at power-on                cannot switch back  │
│  HWM window       0x0A20 page-select · 0x0A21 index · 0x0A22 data                    │
│  SIO config       ports 0x2E / 0x2F — chip ID at reg 0x20/0x21                       │
│                   LDN config regs locked — read 0xFF in EC mode                      │
│  UART2            base 0x2F8 · IRQ 3                             ttyS1 · 115200 8N1  │
│  SIO firmware     Macronix MX25L4006E — 512 KiB                              SIO1_R  │
│  Access           root / CAP_SYS_RAWIO (raw I/O port access)                         │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ LOGICAL DEVICES — FROM SIO SCAN ────────────────────────────────────────────────────┐
│  LDN 0x02   UART-A    base 0x03F8    COM1 / ttyS0 — active by default, IRQ 4         │
│  LDN 0x03   UART-B    base 0x02F8    COM2 / ttyS1 — debug headers, IRQ 3, BIOS-off   │
│  LDN 0x05   KBC       base 0x0060    keyboard controller                             │
│  LDN 0x08   WDT       —              locked in EC mode                               │
│  LDN 0x0A   ACPI      —              embedded-controller interface                   │
│  LDN 0x0B   HWM/EC    base 0x0A20    hardware monitor — fully accessible             │
└──────────────────────────────────────────────────────────────────────────────────────┘

The 0x2E/0x2F unlock sequence (write 0x87 twice to enter, 0xAA to exit) is
effectively a no-op here, but the chip ID and the EC HWM page interface are readable
regardless.  Chip-ID detection map: 0x73B0 = W83627DHG · 0xB3D2 = NCT5525D ·
0xD441 = NCT6686D (this board).

HWM access is paged: write the page (0-12) to 0x0A20, the register offset to 0x0A21,
then read / write data at 0x0A22.

┌─ HWM PAGED REGISTER MAP ─────────────────────────────────────────────────────────────┐
│  Page 0    system config + chip ID (0x20-0x21 = 0xD441) · 0x39+ fan PWM / sys        │
│  Page 1    temperature sensors — 9 channels, 0.5 degC resolution                     │
│  Page 4    voltage / ADC inputs (4) — adc_10 · adc_11 · adc_12 · adc_14              │
│  Page 6    extended chip config / version info                                       │
│  Page 9    smart-fan mode config (5 fan channels)                                    │
│  Page 10   thermal limits — warning / critical thresholds                            │
│  Page 11   fan curve points — 2 fans, 7-point curves, writable                       │
│  Page 12   factory calibration data                                                  │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ TEMPERATURE CHANNELS — PAGE 1 (REGISTER LAYOUT, NOT READINGS) ──────────────────────┐
│  0x00   CPU                      0x12   Chipset (A68H FCH)                           │
│  0x02   near-GPU (PCB)           0x14   PCH                                          │
│  0x04   Board / ambient          0x16   Hotspot — GPU junction (die hotspot)         │
│  0x10   VRM                      0x18   Inlet                                        │
│                                  0x1A   Outlet                                       │
└──────────────────────────────────────────────────────────────────────────────────────┘

Each channel is an integer byte plus a fractional byte at the next index (the
fractional byte's MSB indicates the +0.5 degC step).  Voltage ADC scaling factors
are not documented.

┌─ FAN CONTROL — PAGE 11 · PWM DUTY 0-255 · WRITABLE ──────────────────────────────────┐
│  Fan 0    temp-point base 0x00 · PWM-duty base 0x08                                  │
│  Fan 1    temp-point base 0x18 · PWM-duty base 0x20                                  │
└──────────────────────────────────────────────────────────────────────────────────────┘

Each fan channel is a 7-point temperature-to-duty curve.  Page 10 holds the
warning / critical thresholds; page 9 the smart-fan mode config.  Fan-speed
readback concatenates a high and low counter byte into a 16-bit tach count.

Reading temperatures, the libAsrCore way: AsrLibGetRobinTemperature enters SIO
extended mode (outb(0x2E,0x87) twice), reads the chip ID at index 0x20, selects
LDN 0x0B, enables the HWM block (reg 0x30: set bit 0, clear bit 1), then per sensor
writes 0x0A20=0xFF, 0x0A20=0x01 (page 1), the offset to 0x0A21, and reads 0x0A22;
it exits with outb(0x2E,0xAA).

┌─ CAUTION — SIO FIRMWARE ROM ─────────────────────────────────────────────────────────┐
│  Never flash the NCT6686D firmware ROM (the 512 KiB Macronix MX25L4006E, SIO1_R)     │
│  — flashing it bricks the Super I/O permanently.  This chip is distinct from the     │
│  16 MiB BIOS flash.                                                                  │
└──────────────────────────────────────────────────────────────────────────────────────┘

Upstream Linux does not directly support the NCT6686D; the in-kernel nct6683 driver
(EC-mode predecessor, force=true) is read-only, and the out-of-tree nct6687d driver
uses the same 0x0A20 path with full fan PWM read / write.

Bringing up ttyS1 (COM2) — the BIOS activates only UART1 (ttyS0 / COM1).  ttyS1
requires TWO independent steps, because LPC COM decode is gated at the FCH:

┌─ LPC COM DECODE — PCI 00:14.3 CONFIG OFFSET 0x44 ────────────────────────────────────┐
│  bit 6    COM1 decode — 0x3F8-0x3FF                             default 1 (enabled)  │
│  bit 7    COM2 decode — 0x2F8-0x2FF                            default 0 (disabled)  │
└──────────────────────────────────────────────────────────────────────────────────────┘

  1  Configure NCT6686D LDN 3 — base 0x2F8, IRQ 3, active = 1.
  2  Set bit 7 of PCI 00:14.3 offset 0x44 so the FCH forwards 0x2F8-0x2FF to LPC.

Until bit 7 is set, 0x2F8 reads 0xFF even after the LDN is enabled.  After both
steps /dev/ttyS1 is a working 16550A at 115200 8N1; the kernel may still need the
port type set via TIOCSSERIAL (the 8250 driver marks it uart:unknown at boot).

                      See: A68H FCH (LPC decode) · ASRock Carrier Library — this chapter
```

## SMBus / I2C topology

```
── SMBUS / I2C ────────────────────────────────────────── PIIX4 · 5 ports · 2 headers ──

The A68H exposes a PIIX4-type SMBus controller with five bus ports (0-4).  The
board's VRM and clock-generator devices are not on the internal PIIX4 buses — they
sit on two external headers (I2C_HEADER1 / Bus 1 and I2C_HEADER2 / Bus 2), so the
OS-visible PIIX4 buses enumerate but return no device responses.  Access is via the
kernel i2c-dev module (/dev/i2c-N); i2cdetect -l lists adapters, i2cdetect -y N
enumerates.  SMBus READS are non-destructive.

┌─ PORTS ──────────────────────────────────────────────────────────────────────────────┐
│  Port 0   chipset-internal                                      typical FCH routing  │
│  Port 1   VRM controllers — PMBus                                   via I2C_HEADER1  │
│  Port 2   clock generator                                     addr found at runtime  │
│  Port 3   (verify via I2C scan)                                          unverified  │
│  Port 4   (verify via I2C scan)                                          unverified  │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ I2C_HEADER1 (BUS 1) — VRM + POWER MONITOR ──────────────────────────────────────────┐
│  0x60   Infineon XDPE132G5C multi-phase VRM     PMBus Direct mode                    │
│         — PAGE 0 standby/aux · PAGE 1 SoC core VDD_CR                                │
│  0x5D   VDDM VRM — Renesas / ISL family         proprietary · 0xBEEF fill            │
│  0x4F   power monitor                           identity TBD                         │
│  0x0C   SMBus ARA — Alert Response Address      no active alerts                     │
│  0x28   HAZARD — clock-stretches indefinitely   see CAUTION below                    │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ I2C_HEADER2 / DDC (BUS 2) — BOARD IDENTITY · CLOCK GEN · DDC ───────────────────────┐
│  0x37   board identity hash                     32-byte static · unique per board    │
│  0x3A   clock generator config                  5 config bytes (0x00-0x04)           │
│  0x4A   DIMM slot A temp — MCP9808-compatible   empty socket                         │
│  0x4B   DIMM slot B temp — MCP9808-compatible   empty socket                         │
│  0x50   monitor EDID EEPROM (DDC)               reflects attached display            │
│  0x54   DDC extended / secondary EEPROM         sparse, mostly zeros                 │
└──────────────────────────────────────────────────────────────────────────────────────┘

The board identity hash at 0x37 is a 32-byte read-only value fixed at manufacturing.
One observed value:

   7ec3ec56338af1790247c0050692a3526148f62a2d53559e38a125805e8830ac

Bus 2 is electrically independent of Bus 1.

┌─ CAUTION — SMBUS WRITES · ADDRESS 0x28 ──────────────────────────────────────────────┐
│  SMBus WRITES can reconfigure the VRM or clock generator and are dangerous —         │
│  treat host-side SMBus as read-only.                                                 │
│                                                                                      │
│  Any transaction to address 0x28 on Bus 1 makes the device ACK then hold SCL low     │
│  indefinitely, wedging the entire bus — all Bus 1 devices, including the 0x60        │
│  VRM, become inaccessible.  Recovery is a BOARD POWER-CYCLE; restarting the I2C      │
│  host does not release the bus.  Keep 0x28 in a blocked-address set.  Bus 2 is       │
│  unaffected.                                                                         │
└──────────────────────────────────────────────────────────────────────────────────────┘

                                    See: Power Delivery · Clock generator — this chapter
```

## Storage — M.2 NVMe

```
── STORAGE ────────────────────────────────────────────────────────── M.2 2280 · NVMe ──

A single M.2 2280 slot on an A68H PCIe downstream port.  The fitted drive is a
per-install choice, not a fixed carrier property; a Gen4-native SSD negotiates down
to Gen2 x2 through the chipset.

┌─ STORAGE ────────────────────────────────────────────────────────────────────────────┐
│  Slot             M.2 2280                                                           │
│  Link             FCH PCIe Gen2 x2 — via a Hudson bridge                             │
│  Reference unit   SK hynix P41 — PCI 03:00.0             vendor 0x1c5c · dev 0x1959  │
└──────────────────────────────────────────────────────────────────────────────────────┘

The NVMe PERST# line is part of the FCH GPIO power-sequencing cluster (pins
36 / 38 / 66) — see the FCH GPIO caution.

                                                            See: A68H FCH — this chapter
```

## Network — Gigabit Ethernet

```
── NETWORK ──────────────────────────────────────────────────── RTL8111H · 1000BASE-T ──

On-board network is a Realtek RTL8111H Gigabit Ethernet controller, enumerated over
PCIe from an A68H downstream port.

┌─ NETWORK ────────────────────────────────────────────────────────────────────────────┐
│  Controller       Realtek RTL8111H — 1000BASE-T                                      │
│  Link             FCH PCIe Gen1 x1                                                   │
│  Driver           r8169-class (mainline Linux)                                       │
└──────────────────────────────────────────────────────────────────────────────────────┘

                                                            See: A68H FCH — this chapter
```

## Clock generator

```
── CLOCK GENERATOR ─────────────────────────────────────────────── SMBus Bus 2 · 0x3A ──

An SMBus-attached clock generator supplies board-level reference clocks — likely
including the GDDR6 MCLK reference the UMC PLL multiplies.  It sits on SMBus port 2
(I2C_HEADER2) at address 0x3A; the address is not fixed in silicon but is selected
at runtime by the ASRock carrier library from a board database.

┌─ CLOCK GENERATOR ────────────────────────────────────────────────────────────────────┐
│  Bus / address    I2C Bus 2 (I2C_HEADER2) — address 0x3A                             │
│  Config           5 static config bytes at 0x00-0x04                      rest 0x00  │
│  Part             not confirmed — candidates: IDT 5V9885T,                           │
│                   Renesas RC32500-series, or similar 5-byte clock-buffer IC          │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ REGISTER DUMP — BYTE READS ─────────────────────────────────────────────────────────┐
│  0x00   0x41        0x03   0x9A                                                      │
│  0x01   0x97        0x04   0x8F                                                      │
│  0x02   0x6A        0x05-0x3F   0x00                                                 │
└──────────────────────────────────────────────────────────────────────────────────────┘

The five bytes are consistent across reads — static PLL / oscillator configuration,
not volatile status.

Address discovery (libAsrCore): TSMBus::GetClockGenAddress() (decompiled address
0x001510d0) is a one-line accessor returning the byte at TSMBus + 0x37a.  That byte
is populated during TSMBus::DetectSMBus() (0x00151300), which walks a TSMBusConfig
tree via a TInterpreter script engine; on a matching board it copies three bytes
from the config record:

┌─ TSMBus CONFIG RECORD BYTES ─────────────────────────────────────────────────────────┐
│  +0x378    status / ACK                                                              │
│  +0x379    error mask                                                                │
│  +0x37a    clock-gen SMBus address                                                   │
└──────────────────────────────────────────────────────────────────────────────────────┘

The address is therefore board-specific.

┌─ CAUTION — READ-ONLY ────────────────────────────────────────────────────────────────┐
│  SMBus writes to the clock generator can reconfigure clocking and are                │
│  potentially destructive — treat access as read-only.                                │
└──────────────────────────────────────────────────────────────────────────────────────┘

                       See: SMBus / I2C topology · ASRock Carrier Library — this chapter
```

## SPI Flash

```
── SPI FLASH ───────────────────────────────────────────────── W25Q128JV + MX25L4006E ──

Two SPI flash devices.  The 16 MiB part holds the entire platform firmware image
(UEFI/BIOS, PSP directory, SMU, ABL, APCB); the 512 KiB part is the NCT6686D
Super I/O's private firmware and is electrically separate.

IDENTIFY BY DESIGNATOR + CAPACITY, NOT BRAND.  AsRock second-sources both sockets,
so the manufacturer varies between board builds - some ship a Winbond W25Q128JV
(16 MiB) BIOS + a Macronix MX25L4006E (512 KiB) SIO, others ship a Macronix
MX25L12872F (16 MiB) BIOS + a Winbond (512 KiB) SIO, and the two silkscreen labels
BIOS_A1 and SIO1_R can even appear to sit over the "wrong" brand relative to this
manual's photos.  The role follows the DESIGNATOR and the SIZE, never the logo:
BIOS_A1 = the 16 MiB (128 Mbit) chip; SIO1_R = the 512 KiB (4 Mbit) chip.  flashrom
reports the true capacity on probe - a 16384 kB detect is the BIOS flash whatever
name it prints, a 512 kB detect is the SIO flash.  Do NOT decide "which chip is the
BIOS" by whether it is the Winbond or the Macronix one.

┌─ FLASH DEVICES ──────────────────────────────────────────────────────────────────────┐
│  BIOS flash       Winbond W25Q128JV — 16 MiB          UEFI · PSP · SMU · ABL · APCB  │
│  SIO flash        Macronix MX25L4006E — 512 KiB                NCT6686D fw · SIO1_R  │
│  Programming      J4004 header (BIOS flash) — external          board off / standby  │
└──────────────────────────────────────────────────────────────────────────────────────┘

The BIOS flash is reachable for external programming only through J4004 (see Debug
& Recovery Headers).  During boot the SoC reads the flash over a separate internal
SPI bus in single-IO mode (opcode 0x03, 4 bytes per transaction); J4004 carries
only the flash MISO output on that bus and cannot sniff or interpose on live
PSP-to-flash traffic.  The full firmware region map is in Chapter 3.

A USB / EFI (or DOS / in-band) BIOS updater can only ever reach the 16 MiB BIOS_A1
chip.  Every such tool - flashrom -p internal, an AMI AFU utility, an ".efi"
updater on a FAT32 stick - writes through the FCH SPI100 controller, which is
hardwired to the BIOS_A1 footprint.  The 512 KiB SIO flash lives on the LPC bus
behind the NCT6686D; no SPI-master signal reaches it, so no in-band flasher can
touch it (confirmed LPC-only - see TPMS1 below).  If a board stops POSTing after a
USB / EFI flash, the updater bricked the BIOS image on the 16 MiB chip, NOT the SIO
part - recover by external SPI reflash of BIOS_A1 (below), not a CMOS pull.

┌─ CAUTION — SIO FLASH ────────────────────────────────────────────────────────────────┐
│  Never flash the Macronix MX25L4006E (the Super I/O ROM) — it bricks the             │
│  NCT6686D permanently, and it is distinct from the 16 MiB BIOS flash.                │
└──────────────────────────────────────────────────────────────────────────────────────┘

   See: Debug & Recovery Headers — this chapter — Ch 3 · Security & Trust (firmware map)
```

## Debug & Recovery Headers

```
── DEBUG & RECOVERY ─────────────────────────────────────────────── headers · jumpers ──

The carrier exposes four headers and two jumpers for debug and recovery.  The most
important is J4004, the SPI programming header — the only known recovery path from
a PSP brick.

┌─ HEADERS & JUMPERS ──────────────────────────────────────────────────────────────────┐
│  J2            JTAG / AMD HDT+ debug    20-pin · 1.27 mm      unpopulated            │
│  J4004         SPI flash programming    2×4 · 2.54 mm         populated · 7/8 pads   │
│  I2C_HEADER1   PMBus / I2C to VRM       SMBus-routed          read-only telemetry    │
│  I2C_HEADER2   DDC / board identity     SMBus Bus 2           board ID · clock gen   │
│  TPMS1         LPC / TPM                18-pin 2×9 · 2.0 mm   pin 13 keyed           │
│  AUTO_PWRON1   jumper — 2-pin           CLRCMOS1              jumper — 2-pin         │
└──────────────────────────────────────────────────────────────────────────────────────┘

J4004 breaks out the Winbond W25Q128JV (16 MiB) BIOS flash.  The pin assignment is
a CUSTOM ASROCK LAYOUT, not the conventional 8-pin programmer pinout — verified by
continuity to the SO-8 package.  A white triangle marks pin 1.

      top row    5   6   7   8
      bot row    1   2   3  [4]          [4] = unpopulated pad — no through-hole pin
                 ^
                 white triangle = pin 1

┌─ J4004 PINOUT — CUSTOM ASROCK LAYOUT ────────────────────────────────────────────────┐
│  Pin        Flash SO-8   Signal      Notes                                           │
│  1 (tri)    8            VCC 3.3 V   keyed pin — a reversed cable shorts VCC ↔ GND   │
│  2          1            CS#         chip select — active low                        │
│  3          2            MISO        data from flash                                 │
│  4          —            —           unpopulated pad (no through-hole pin)           │
│  5          4            GND         ground                                          │
│  6          6            CLK         SPI clock                                       │
│  7          5            MOSI        data to flash                                   │
│  8          —            strap       DNP resistor, ~10k to GND — do not connect      │
└──────────────────────────────────────────────────────────────────────────────────────┘

WP# (SO-8 pin 3) and HOLD# (SO-8 pin 7) are NOT broken out — both are tied high via
on-board 10k pull-ups, so the flash is permanently out of write-protect / hold and
an external programmer cannot exercise hardware write-protect through J4004.  The
silkscreen shows both BIOS_A1 and BIOS_S_A1 on the single footprint — a
BOM-flexibility marking (two qualified part numbers for one socket), not a
dual-chip layout.  Only shared power rails cross to TPMS1 (J4004 pin 1 ↔ TPMS1
pin 15 = 3.3 V; J4004 pin 5 ↔ TPMS1 pin 12 = GND); no SPI signals bridge to TPMS1.

PSP-brick recovery — when a board fails POST (no display, no UART), external SPI
reflash is the only known recovery; once the PSP rejects the SPI image, in-band
reflash is impossible.  The programmer we document is a Raspberry Pi Pico 2 (RP2350)
running pico-serprog (the libreboot serprog firmware): 3.3 V-native, no level shifter
for this 3.3 V flash, and no external power needed for the target.

Wire five lines — the four SPI signals plus a common ground — and DO NOT connect
J4004 pin 1 (VCC).  The flash is powered by the board's own 3.3 V rail, so the Pico
drives logic only.  Leaving VCC unconnected also means a mis-oriented connector can
no longer short VCC ↔ GND, and it is what lets the same rig flash a live board.

┌─ PICO 2 → J4004 WIRING (pico-serprog GPIO map) ──────────────────────────────────────┐
│  Pico 2         Signal    J4004 pin    Flash SO-8                                    │
│  GP2  (phys 4)  CLK        6            6                                            │
│  GP3  (phys 5)  MOSI       7            5                                            │
│  GP4  (phys 6)  MISO       3            2                                            │
│  GP5  (phys 7)  CS#        2            1                                            │
│  GND  (phys 3)  GND        5            4                                            │
│  ---            VCC        1            LEAVE UNCONNECTED (board powers the flash)   │
└──────────────────────────────────────────────────────────────────────────────────────┘

Power state.  Because the Pico never sources VCC, there is no rail contention, so any
of these work:

  * PSU ON, board OFF (S5 standby) — the recommended default.  The 3.3VSB rail powers
    the flash while the FCH SPI master is held in reset, so the Pico owns the bus
    cleanly.  You do NOT need the board fully unplugged, and you do NOT supply 3.3 V
    from the Pico.
  * PSU ON, board BOOTED — also works.  Post-boot the FCH has shadowed the BIOS into
    DRAM and leaves the ROM bus idle, so the Pico can drive it without a power-cycle.
    Use this when cold-cycling is awkward; board-off is the more deterministic path.

┌─ PICO 2 REFLASH PROCEDURE ───────────────────────────────────────────────────────────┐
│  1  Flash pico-serprog.uf2 to the Pico 2 once (hold BOOTSEL, drag-drop the .uf2);    │
│     it re-enumerates as a USB CDC serial port at /dev/ttyACM0.                       │
│  2  Wire the five lines above.  Leave J4004 pin 1 (VCC) unconnected.                 │
│  3  Leave the PSU on.  Board OFF (default) or booted — either is fine.               │
│  4  Read twice and diff — the two dumps MUST match before you trust the backup:      │
│        flashrom -p serprog:dev=/dev/ttyACM0,spispeed=16M -r dump1.bin                │
│        flashrom -p serprog:dev=/dev/ttyACM0,spispeed=16M -r dump2.bin                │
│        cmp dump1.bin dump2.bin                                                       │
│  5  Write the known-good image:                                                      │
│        flashrom -p serprog:dev=/dev/ttyACM0,spispeed=16M -w known-good.bin           │
│  6  Disconnect the Pico and cold-boot.  If probe or verify fails, lower the clock    │
│     (spispeed=4M) and retry — a marginal jumper wire shows up as a flaky probe.      │
└──────────────────────────────────────────────────────────────────────────────────────┘

flashrom auto-detects the 16 MiB Winbond W25Q128JV.  The flash has a SINGLE SPI port —
there is no "separate internal bus."  External access works because the Pico drives that
one bus while the FCH master is either tri-stated (board off) or idle (booted).  The same
electrical fact means J4004 CAN observe live PSP-to-flash traffic — passive sniffing just
needs a tap firmware instead of serprog (see the recon pico2-spi-tap tooling), not a
different header.

┌─ CAUTION - PROGRAMMER VCC (CH341A undervolt / 5 V hazard) ───────────────────────────┐
│  A stock CH341A drives VCC and the SPI lines at ~5 V and can BRICK a 3.3 V part;     │
│  even a "3.3 V-modded" unit sags to ~3.0-3.1 V under load on long clip leads, so an  │
│  erase / write lays down marginal cells that verify at the programmer's own slow     │
│  read yet fail the PSP's cold read - and repeated low-V writes degrade the chip.     │
│  Feed the chip a clean 3.3 V (bench PSU on the VCC pin, programmer VCC left off),    │
│  keep leads short, lower spispeed, and verify at a DIFFERENT speed than you wrote.   │
│  Prefer the Pico 2 serprog rig above - 3.3 V-native, no 5 V hazard.                  │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ CAUTION — WRONG-CHIP FLASH ─────────────────────────────────────────────────────────┐
│  Never flash the secondary Macronix MX25L4006E (the Super I/O ROM) — it bricks       │
│  the NCT6686D permanently.  Recovery from a bad NVRAM / config is a CMOS battery     │
│  pull (CLRCMOS1), not a reflash.                                                     │
└──────────────────────────────────────────────────────────────────────────────────────┘

TPMS1 exposes the full LPC bus (LAD[3:0], LCLK, LFRAME#, LRESET#), SMBus, and
power — the same LPC bus as the NCT6686D.  Confirmed LPC-only: no SPI signals
bridge to the BIOS flash.

┌─ TPMS1 PINOUT — 18-PIN · 2×9 · 2.0 mm ───────────────────────────────────────────────┐
│  pin  1  PCICLK                pin  2  GND                                           │
│  pin  3  FRAME                 pin  4  SMB_CLK_MAIN                                  │
│  pin  5  PCIRST#               pin  6  SMB_DATA_MAIN                                 │
│  pin  7  LAD3                  pin  8  LAD2                                          │
│  pin  9  3V                    pin 10  LAD1                                          │
│  pin 11  LAD0                  pin 12  GND                                           │
│  pin 13  (key / n-c)           pin 14  S_PWRDWN#                                     │
│  pin 15  3VSB                  pin 16  SERIRQ#                                       │
│  pin 17  GND                   pin 18  GND                                           │
└──────────────────────────────────────────────────────────────────────────────────────┘

J2 (JTAG / AMD HDT+, 20-pin, 1.27 mm, bottom side, unpopulated) provides CPU
halt / resume / single-step, register and memory access, breakpoints, and trace.
It requires a 1.27 mm header soldered onto the pads.  TEST18 / TEST19 / DBRDY0 are
left floating.

┌─ J2 PINOUT — JTAG / HDT+ · 20-PIN · 1.27 mm · UNPOPULATED ───────────────────────────┐
│  pin  1  VDDIO                 pin  2  TCK                                           │
│  pin  3  GND                   pin  4  TMS                                           │
│  pin  5  GND                   pin  6  TDI                                           │
│  pin  7  GND                   pin  8  TDO                                           │
│  pin  9  TRST_L                pin 10  PWROK_BUF                                     │
│  pin 11  DBRDY3                pin 12  RESET_L                                       │
│  pin 13  DBRDY2                pin 14  DBRDY0                                        │
│  pin 15  DBRDY1                pin 16  DBREQ_L                                       │
│  pin 17  GND                   pin 18  TEST19                                        │
│  pin 19  VDDIO                 pin 20  TEST18                                        │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ JUMPERS ────────────────────────────────────────────────────────────────────────────┐
│  AUTO_PWRON1   pins 1-2   auto power-on when 12 V applied                            │
│                pins 2-3   wait for power button                                      │
│  CLRCMOS1      pins 1-2   power CMOS from CR2032 (default)                           │
│                pins 2-3   clear CMOS / NVRAM to UEFI defaults                        │
└──────────────────────────────────────────────────────────────────────────────────────┘

── REFLASHED A GOOD IMAGE AND STILL NO POST ─────────────────────────── PSP boot loop ──

A board that will not POST even after an external reflash with a "verified" image is a
different failure from a plain bad flash, and the fix is not "flash it again."  Read
the signs first - they say WHERE the board is stuck:

  * an RGB / mouse LED lights but no keyboard enumerates and there is no display
    -> USB has +5 V VBUS (the LED is just power) but there is NO USB host controller:
    the x86 cores never left reset.  On this SoC the PSP boots first and releases the
    x86 cores; if x86 never starts, the PSP never got there.
  * a STEADY ~1 s blink on the NIC link LED, or a red<->green LED cycling on a regular
    cadence, with no DHCP lease -> that cadence is a RESET-LOOP HEARTBEAT: the PSP is
    bootlooping and resetting the board about once a second.

Net: x86 is held in reset and the PSP is looping on the SPI image.  Two root causes
produce exactly this, and they are separated by MEASUREMENT, not by flashing again.

  A. THE FLASH CONTENT IS NOT ACTUALLY GOOD (tool-verify lies).  A programmer's
     "verify OK" only means the bytes read back match the bytes IT SENT - never that
     the chip matches a working board.  Two ways that bites, both worse on the 16 MiB
     Macronix MX25L12872F (JEDEC C2 20 18) because it is uncommon and often mis-ID'd:
       - wrong chip profile -> wrong erase / program / addressing -> the top of the
         chip (reset vector, $PSP directory @ 0x8E0000, NVRAM) is mangled while verify
         passes against the tool's own wrong address map.  Force the exact part in
         flashrom (-c "..."); never auto-detect this one.
       - partial image -> a BIOS-volume-only file leaves the PSP / APCB / EFI-NVRAM
         regions as the corrupted post-poke state.  A full image is EXACTLY
         16,777,216 bytes and carries the $PSP magic near 0x8E0000.
     PROVE it: read the chip back and byte-compare (sha256 / cmp) against a KNOWN-GOOD
     FULL 16 MiB dump from a working board - not against your own source file.

  B. THE BOARD'S OWN 3.3 V RAIL IS MARGINAL (reads clean on the bench, browns out in
     circuit).  The SPI flash, the NCT6686D, the NIC PHY and the PSP's early I/O all
     run off the board 3.3 V / 3.3VSB rail.  On a CH341A the chip runs off the
     programmer's clean regulated 3.3 V, so it reads / writes / verifies perfectly; in
     the board it runs off the derived rail.  If that rail sags, is noisy, or
     collapses, the PSP's cold read of the flash fails and the board resets - and a
     rail that browns out and recovers on a ~1 s cycle IS the heartbeat on the NIC /
     LED.  A verify-clean chip that will not boot in-circuit is the textbook signature
     of a board-power fault, not a flash-content fault.
     MEASURE it (board powered, in the boot loop - NOT on the programmer):
       - 3.3 V at J4004 pin 1 -> GND (pin 5), or 3V / 3.3VSB at TPMS1 pin 9 / pin 15
         -> GND (pin 12 / 17 / 18).  Expect a steady ~3.3 V.
       - a DMM reading low (~3.0 V) or twitching, or a scope showing sag / ripple /
         collapse synchronized to the ~1 s cadence, confirms a rail fault (bad 3.3 V
         buck, damaged cap, or a downstream short) - no amount of reflashing fixes it.

Only after BOTH are cleared - bytes proven against a good reference AND the 3.3 V rail
proven steady under load - is the fault deeper (off-flash PSP-NV or board damage).
The fastest single test that splits A from B / hardware: write the good full image to
a DIFFERENT or blank SPI128 chip and socket it.  POSTs -> your chip / tooling was the
problem; still dead -> the fault is off-chip (rail or board), chase hardware.

┌─ TRIAGE — NO POST AFTER A "GOOD" REFLASH ────────────────────────────────────────────┐
│  1  Confirm the loop     x86 in reset (no USB enum, no display) + ~1 s NIC/LED       │
│                          heartbeat = PSP bootloop, not an OS / display fault         │
│  2  Trust bytes not tool read back, sha256 / cmp vs a known-good FULL 16 MiB dump    │
│  3  Force the chip ID    flashrom -c the exact MX25L12872F; never auto-detect it     │
│  4  Check image size     exactly 16,777,216 bytes; $PSP magic near 0x8E0000          │
│  5  Measure 3.3 V rail   J4004 pin1->GND / TPMS1 pin9->GND, board ON in the loop;    │
│                          steady 3.3 V?  sag / ripple on the ~1 s cadence = rail bad  │
│  6  Isolation test       good image on a blank chip -> POST = chip/tool, dead = HW   │
└──────────────────────────────────────────────────────────────────────────────────────┘

     See: SMBus / I2C topology — this chapter — Ch 3 · Security & Trust (PSP boot flash)
```

## ASRock Carrier Library — libAsrCore

```
── libAsrCore ──────────────────────────────────────────── v1.70.0 · board management ──

libAsrCore (v1.70.0, Linux x86-64 shared object) is ASRock's proprietary
board-management library, shared across their industrial / embedded line — "Robin"
is the internal codename for this carrier, visible in GetRobinTemperature /
GetRobinMemTiming.  It exports 38 AsrLib* functions (1,336 total): watchdog, GPIO
across four domains, SMBus, hardware monitor, fan control, temperature, CMOS memory
timing, LCD backlight.  On the BC-250 motherboard detection FAILS and must be
shimmed; even then only a subset works, because the NCT6686D is EC-mode-locked.

┌─ LIBRARY ────────────────────────────────────────────────────────────────────────────┐
│  Object           libAsrCore v1.70.0 — Linux x86-64 .so     C++ singletons + vtable  │
│  Exports          38 AsrLib* functions                        1,336 functions total  │
│  Detection        CheckAsrockMotherboard() FAILS on BC-250       needs DllInit shim  │
│  Working set      temperature · CMOS memory timing R/W · WD range query              │
│                   init / uninit / error                                              │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ ERROR CODES ────────────────────────────────────────────────────────────────────────┐
│  0x000    success                                                                    │
│  0x100    library not initialized (InitOk == 0)                                      │
│  0x101    hardware not detected (TAsr constructor failed)                            │
│  0x102    required subsystem unavailable (no SIO / SMBus / ICH)                      │
│  0x105    invalid parameter (null pointer, out of range)                             │
│  0x10B    backlight config file load failed                                          │
│  0xFFFF   SMBus operation failed                                                     │
└──────────────────────────────────────────────────────────────────────────────────────┘

Detection shim — AsrLibDllInit → CheckAsrockMotherboard() fails for two reasons:

  1  Memory scan — it searches physical memory at 0xE0000 for the signature
     $CN1368$, which is absent (only the string "AMD BC-250" is present).
  2  PCI scan — it scans for ASRock subsystem-vendor 0x1849 via legacy CF8/CFC
     I/O; on AMD Zen, CF8/CFC returns the sentinel 0x725B0FEF, so 0x1849 never
     matches.

The bypass computes the library base from the AsrLibDllInit symbol and writes the
non-exported BSS globals directly:

┌─ BSS GLOBALS — OFFSETS FROM LIBRARY BASE ────────────────────────────────────────────┐
│  DllInit (symbol)   base + 0x108B0                                                   │
│  LastErrorCode      base + 0x293370                                                  │
│  asr                base + 0x293B38                                                  │
│  InitOk             base + 0x293B58                                                  │
└──────────────────────────────────────────────────────────────────────────────────────┘

Procedure: read the asr pointer at base + 0x293B38; set asr[8] = 1 (detection
flag); strcpy(asr + 0x14, "BC-250") (platform name); set LastErrorCode = 0,
InitOk = 1.

┌─ CAUTION — GetPlatformName ──────────────────────────────────────────────────────────┐
│  Do NOT call AsrLibGetPlatformName after the shim — it re-creates the TAsr           │
│  instance on every call, overwriting the injected platform name.  Return the         │
│  name string directly instead.                                                       │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ FUNCTION STATUS ON THIS BOARD ──────────────────────────────────────────────────────┐
│  WORKING (with shim)    AsrLibDllInit · AsrLibDllUnInit · AsrLibDllGetLastError      │
│                         AsrLibCheckAvailabled · AsrLibGetRobinTemperature            │
│                         AsrLibGetRobinMemTiming · AsrLibSetRobinMemTiming            │
│                         AsrLibWDGetRange — hardcoded {0,255,1}, no hw access         │
│                                                                                      │
│  FAIL 0x102 (SIO=NULL)  all SuperIO watchdog (AsrLibWD*) · AsrLibReadHWMBank         │
│                         AsrLibGetHardwareMonitor · AsrLibSet/GetFanConfig            │
│                         AsrLibGet/SetGpioValue · AsrLibGet/SetGpioGroup              │
│                                                                                      │
│  UNSAFE                 AsrLibGetPchGpioValue / AsrLibSetPchGpioValue                │
│                         — see the PCH GPIO caution below                             │
│                                                                                      │
│  NOT APPLICABLE         AsrLibGet/SetSocGpioValue — Bay Trail, CPUID 0x30670         │
│                         AsrLibGet/SetMini58GpioValue — MCU not populated             │
│                         AsrLibSetLcdBacklight — 0x10B, no config file                │
│                                                                                      │
│  KNOWN BUG              AsrLibGetIoIndex / AsrLibSetIoIndex — IoIndexReadBlock       │
│                         early-exits when count <= starting_offset (returns           │
│                         success, no data); the memory-timing path avoids it          │
│                         via dedicated CMOS                                           │
└──────────────────────────────────────────────────────────────────────────────────────┘

Watchdog dispatch — AsrLibWDSetConfig dispatches on a board-type string
(TAsr + 0x14) to one of three hardware paths.  This board selects the SuperIO
path — which is locked by EC mode, so the traditional SuperIO watchdog is
permanently unreachable (use the FCH TCO watchdog instead).

┌─ WATCHDOG DISPATCH PATHS ────────────────────────────────────────────────────────────┐
│  SuperIO   NCT6686D / NCT5567D / NCT6116D   0x2E/0x2F index/data   default           │
│  COM       external MCU at SMBus 0xBA       SMBus byte R/W         name has "COM"    │
│  Q212D     external MCU at SMBus 0xBA       SMBus byte R/W         name has "Q212D"  │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ SUPERIO WATCHDOG VTABLE — TNuvotonSio → TWinbondSio → TSioBase ─────────────────────┐
│  +0x00   EnterCfg() — write 0x87 twice to 0x2E                                       │
│  +0x08   ExitCfg() — write 0xAA to 0x2E                                              │
│  +0x20   SelectLDN(ldn) — write LDN to index reg 0x07                                │
│  +0x38   HWMSetBank(bank) — select HWM register bank                                 │
│  +0x40   WatchDogEnable(timeout, unit, kbd_reset) — arm                              │
│  +0x48   WatchDogTrigger() — reload countdown (heartbeat)                            │
│  +0x50   WatchDogDisable() — disarm                                                  │
│  +0x58   WatchDogIsRunning() — 1 if armed                                            │
│  +0x60   WatchDogCounter() — current countdown                                       │
└──────────────────────────────────────────────────────────────────────────────────────┘

WD range is hardcoded {min 0, max 255, step 1 second}.

Mini58 board-management MCU — footprint present, silicon NOT populated.  The
carrier firmware expects a Nuvoton Mini58 (M058LDE) MCU; an SMBus probe at 0xBA
returns no device (NACK).  All Mini58-targeted calls return graceful not-supported
errors; a future board revision could populate it.

┌─ MINI58 — DESIGNED, NOT POPULATED ───────────────────────────────────────────────────┐
│  MCU (designed)   Nuvoton Mini58 (M058LDE) — ARM Cortex-M0                           │
│  Flash / SRAM     32 KB on-chip / 4 KB                                               │
│  Bus              SMBus slave 0x5D (7-bit) / 0xBA (8-bit write)                      │
│  GPIO             8 pins — 0-3 outputs GPO0-3 · 4-7 inputs GPI0-3                    │
│  GPIO registers   'a' (0x61) direction · 'b' (0x62) value                            │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ CAUTION — PCH GPIO PATH IS BROKEN AND UNSAFE ───────────────────────────────────────┐
│  Do NOT call AsrLibGetPchGpioValue / AsrLibSetPchGpioValue on the BC-250.  The       │
│  library's TIch class targets an Intel ICH at PCI 0:1f:0; on the AMD A68H the        │
│  LPC device is at 0:14:3, so the ID reads return 0xFFFFFFFF and gpio_base            │
│  defaults to 0.  Every GPIO register access then aliases onto legacy x86 I/O         │
│  ports:                                                                              │
│                                                                                      │
│  Pins 0-31    base +0x00/+0x04/+0x0C   ports 0x0000/0x0004/0x000C   8237A DMA        │
│  Pins 32-63   base +0x30/+0x34/+0x38   ports 0x0030/0x0034/0x0038   DMA pages        │
│  Pins 64-95   base +0x40/+0x44/+0x48   ports 0x0040/0x0044/0x0048   i8254 PIT        │
│                                                                                      │
│  Writing PCH GPIO pin >= 64 corrupts the system timer frequency (PIT registers       │
│  0x40-0x42) — observed live.                                                         │
└──────────────────────────────────────────────────────────────────────────────────────┘

  See: Super I/O · FCH TCO watchdog — this chapter — Ch 2 → SMU / Power, Clock & Thermal
```

## Board Management ("poor-man's BMC")

```
── BOARD MANAGEMENT ──────────────────────────────────────── no BMC · commodity parts ──

The BC-250 has NO iLO / IPMI / out-of-band console.  Board management is assembled
from commodity interfaces; there is no remote power control.

┌─ CAPABILITIES ───────────────────────────────────────────────────────────────────────┐
│  Auto-reset on hang     FCH TCO watchdog (sp5100_tco) + kicker              working  │
│  Temperatures (9 ch)    NCT6686D HWM — I/O 0x0A20                           working  │
│  Fan curve control      NCT6686D page 11 (writable)                         working  │
│  Voltage monitoring     NCT6686D page 4 — 4 ADC inputs                      working  │
│  Memory timing config   extended CMOS — I/O 0x72/0x73                       working  │
│  Serial console         ttyS1 — NCT6686D UART2 · 0x2F8 IRQ 3                working  │
│                         115200 8N1                                                   │
│  Mini58 MCU GPIO        SMBus 0xBA                                    not populated  │
│  SIO watchdog (LDN 8)   NCT6686D via 0x2E/0x2F                     locked (EC mode)  │
│  Remote power on/off    none — needs a smart plug                     not available  │
│  VRM telemetry          PMBus on I2C_HEADER1                              read-only  │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ RECOVERY PATH BY SCENARIO ──────────────────────────────────────────────────────────┐
│  Ring timeout / device lost   reboot — often via FCH TCO auto-reset                  │
│  Hard hang (unreachable)      physical power cycle — smart plug or manual            │
│  PSP brick (won't POST)       SPI reflash via J4004                                  │
│                               CH341A / RPi 3 / Pico 2 serprog                        │
│  No POST after USB/EFI flash  same - reflash the 16 MiB BIOS_A1 via J4004; the       │
│                               in-band updater only wrote the BIOS chip, not the SIO  │
└──────────────────────────────────────────────────────────────────────────────────────┘

A "no POST after a USB / EFI BIOS updater" is the most common self-inflicted brick,
and it is a PSP-brick by another name: the EFI flasher wrote a bad or interrupted
image to the 16 MiB BIOS_A1 chip, the PSP now rejects it, and in-band reflash is
gone.  It did NOT flash the Super I/O.  Recover exactly like any PSP brick - external
SPI reflash of BIOS_A1 with a known-good 16 MiB dump.  Identify BIOS_A1 by capacity,
not brand (see SPI Flash): if flashrom probes it as a 16384 kB part it is the right
chip whether it prints Winbond or Macronix.  A CMOS battery pull (CLRCMOS1) clears
NVRAM / config only and will NOT fix a bad BIOS image.

              See: FCH TCO watchdog · Super I/O · Debug & Recovery — this chapter — Ch 4
```


# 2. Subsystem Internals

Beyond the security processor (Chapter 3) and the GPU compute surface (Chapter 6), the
Oberon die is a dense cluster of on-die IP blocks: the Zen 2 CPU complex, the RDNA 1.x
graphics core, the unified GDDR6 memory controller, the Data Fabric that ties them
together, and the microcontroller estate — MP1/SMU, SMUIO, THM, FUSE, and the clock
generators — that governs power, clocks, and thermals. This chapter enumerates each
block: what it is, the registers and mailboxes that reach it, the firmware it runs, and
the safety limits that bound access to it.

Every block is enumerated over the internal System Management Network (SMN) and reported
by the driver's IP-discovery tables; the major.minor.revision tuple reported there is
authoritative over documentation.

## APU Identity & IP-Block Catalog

```
── APU IDENTITY ───────────────────────────────────────────── identity · 33 IP blocks ──

A monolithic APU harvested from PlayStation 5 (Oberon) silicon, internally codenamed
Cyan Skillfish. One die: 6C/12T Zen 2, 40-CU RDNA 1.x GPU (gfx1013, 24 active in the
stock harvest), 16 GB unified GDDR6 on one BGA package. Closest to Renoir (model range
40h-4Fh) but uniquely pairs Zen 2 with RDNA 1.x, not Vega. Display, VCN, and two CPU
cores are disabled on this harvest.

┌─ SILICON ────────────────────────────────────────────────────────────────────────────┐
│  PCI ID    1002:13FE rev 00                                                          │
│  CPU       Zen 2 — Family 17h Model 47h, 6C/12T (8 physical, 2 fused)                │
│  GPU       RDNA 1.x — GC 10.1.3, gfx1013, 40 CU on die (24 active stock) / 2,560 SP  │
│  Memory    16 GB GDDR6, 256-bit, unified (UMA)                                       │
│  Package   Monolithic BGA, soldered to carrier                                       │
│  VBIOS     113-AMDRBN-003                                                            │
└──────────────────────────────────────────────────────────────────────────────────────┘

The die enumerates 33 IP blocks over ip_discovery (38 instances counting multi-instance
blocks), plus the CCP as a separate PCI function: 26 active, 1 dead (VCN), 3 display
blocks disabled for headless operation, and a tail of debug and unidentified slots. IP
version 127.127.63 (all bits set) marks an unused or harvested slot.

                        AMD Oberon APU (Family 17h Model 47h)
 ┌───────────────┬────────────────┬───────────────┬─────────────────────┐
 │ COMPUTE / DMA │    MEMORY      │  INTERCONNECT │  POWER / CLK / THERM │
 │ GC  SDMA0/1   │ UMC×2  MMHUB   │ DF  NBIF IOHC │ MP1/SMU  SMUIO       │
 │               │ ATHUB SYSHUB   │ PCIE  PCS     │ THM  FUSE  CLKA/CLKB │
 │               │ HDP  L2IMU     │               │ OSSSYS/IH            │
 ├───────────────┴────────────────┼───────────────┴─────────────────────┤
 │ SECURITY: MP0 / PSP (Cortex-A5)│ MEDIA/DISPLAY/IO: VCN(dead) DMU DIO  │
 │                                │ DAZ  ACP  USB×2  CCP  DBGU  DFX      │
 └────────────────────────────────┴─────────────────────────────────────┘

Full enumeration — hw_id / IP version / instances / status:

Block               hw_id  IP Ver      Inst  Status
──────────────────  ─────  ──────────  ────  ───────────────────────────────────────────
GC (Graphics Core)     11  10.1.3         1  Active — gfx1013, 40 CU on die (24 active
                                             stock harvest), 4 shader arrays, 2 shader
                                             engines
SDMA0                  42  5.0.1          1  Active — primary DMA engine
SDMA1                  43  5.0.1          1  Active — secondary DMA engine
UMC                   150  8.1.1          2  Active — 8 channels, 256-bit GDDR6
MMHUB                  34  2.0.3          1  Active — system aperture (fast) + FB
                                             aperture
ATHUB                  35  2.0.3          1  Active — GART address translation
SYSTEMHUB             128  2.1.0          1  Active — DF-to-MMHUB system-memory bridge
HDP                    41  5.0.1          1  Active — Host Data Path
L2IMU                  28  0.0.0          1  Active — L2 cache invalidation unit
DF (Data Fabric)       46  3.5.0          1  Active — central interconnect
NBIF                  108  2.1.1          1  Active — North Bridge Interface, PCIe
                                             endpoint
IOHC                   24  0.0.0          1  Active — IO Hub Controller
PCIE                   70  4.2.0          1  Active — PCIe Gen 4 controller
PCS                    80  3.6.0          1  Active — PCIe Physical Coding Sublayer
MP0 / PSP             255  11.0.8         1  Active — ARM Cortex-A5 security processor
MP1 / SMU               1  11.0.8         1  Active — Xtensa LX core, 5 message queues
SMUIO                   4  11.0.8         1  Active — power-gate sequencer, GPIO (host
                                             write-protected)
THM                     3  11.0.1         1  Active — on-die thermal monitor
FUSE                    5  11.0.1         1  Active — silicon characterization / harvest
                                             fuses
CLKA                    6  11.0.1         3  Active — primary clock domains GFX/SOC/MEM
CLKB                   47  11.0.1         1  Active — secondary clock domain
OSSSYS / IH            40  5.0.1          1  Active — interrupt handler, IH ring
VCN / UVD              12  2.0.3          1  DEAD — silicon present, power island
                                             inaccessible
DMU                   271  2.0.3          1  Disabled (headless, dc=0)
DIO                   272  127.127.63     1  Disabled (garbage version =
                                             harvested/unused slot)
DAZ                   274  127.127.63     1  Disabled (garbage version =
                                             harvested/unused slot)
ACP                    14  4.0.0          1  Present — audio co-processor (unused,
                                             headless)
USB                   170  4.5.0          2  Active — on-die USB controllers
CCP                     —  —              1  Active — PCI 1022:143E at 01:00.2
                                             (AES/SHA/RSA/RNG)
DBGU_NBIO              36  3.0.0          1  Present — NBIO debug unit
DBGU_IO                45  3.0.0          1  Present — I/O debug unit
DFX                    37  2.0.0          1  Present — design-for-test fabric tap
DFX_DAP                49  2.0.0          1  Present — debug access port (JTAG)
Unknown                 0  11.0.1         2  Unidentified

Each block is managed by a specific amdgpu kernel driver module. GPU clock control uses
the cyan_skillfish (smu_v11_8) power-play table (cyan_skillfish_ppt.c), not Navi10/12.

amdgpu_gfx      GC (10.1.3)
                src: gfx_v10_0.c
amdgpu_sdma     SDMA0, SDMA1 (5.0.1)
                src: sdma_v5_0.c
amdgpu_gmc      UMC, MMHUB, ATHUB, SYSTEMHUB, HDP, L2IMU
                src: gmc_v10_0.c, mmhub_v2_0.c, athub_v2_0.c
amdgpu_ih       OSSSYS/IH (5.0.1)
                src: navi10_ih.c
amdgpu_nbio     NBIF, IOHC, PCIE, PCS
                src: nbio_v2_3.c
amdgpu_df       DF (3.5.0)
                src: df_v3_6.c
amdgpu_psp      MP0/PSP (11.0.8)
                src: psp_v11_0.c
amdgpu_smu      MP1/SMU, SMUIO, THM, FUSE, CLKA, CLKB
                src: smu_v11_0.c, navi10_ppt.c
amdgpu_vcn      VCN/UVD (2.0.3)
                src: vcn_v2_0.c (loads but fails power-on)
amdgpu_display  DMU, DIO, DAZ
                src: dm.c (skipped: dc=0)
amdgpu_hdp      HDP (5.0.1)
                src: hdp_v5_0.c

                      See: CPU — Zen 2 · GPU Silicon Identity · SMU / MP1 · Firmware Map
```

## CPU — Zen 2

```
── CPU ─────────────────────────────────────────────────────────────── Zen 2 · 6C/12T ──

Six-core / twelve-thread Zen 2, harvested from an eight-core die with the fourth core
fused off in each of the two CCXs. Clocking is owned by the SMU; there is no Linux
cpufreq path — the P-states carry the hw_pstate flag and no governor is attached.

┌─ SPECIFICATIONS ─────────────────────────────────────────────────────────────────────┐
│  Cores / threads    6C / 12T (8 physical, 2 fused)                                   │
│  Topology           2 CCX × 4 core slots, 4th slot fused per CCX (core_id 3, 7       │
│                     absent)                                                          │
│  Cache (per core)   32 KB L1D + 32 KB L1I + 512 KB L2                                │
│  Cache (per CCX)    4 MB L3 (8 MB total)                                             │
│  Clock              3.2 GHz base, 4.0 GHz boost — SMU-managed, 100 MHz external ref  │
│  ISA                x86-64, SSE4A, AVX, AVX2, FMA, AES-NI, SHA-NI, CLWB, SEV/SEV-    │
│                     ES; no AVX-512                                                   │
│  Microcode          0x8407007                                                        │
│  CPUID              AuthenticAMD, Family 23 (0x17), Model 71 (0x47), Stepping 0      │
│                     brand 'AMD BC-250', 7nm TSMC, socket BL5                         │
│  Addressing         44-bit physical / 48-bit virtual                                 │
│  Topology (sys)     1 socket / 1 die / 1 package, single NUMA node (cpus 0-11)       │
└──────────────────────────────────────────────────────────────────────────────────────┘

Fuse pattern — each CCX has four physical core slots; the fourth is fused off,
leaving 3+3 = 6 active cores / 12 threads. The die has no chiplet or separate IOD.

Threads   core_id  CCX / core  State
────────  ───────  ──────────  ──────
cpu0/1    0        CCX0 core0  active
cpu2/3    1        CCX0 core1  active
cpu4/5    2        CCX0 core2  active
—         3        CCX0 core3  FUSED
cpu6/7    4        CCX1 core0  active
cpu8/9    5        CCX1 core1  active
cpu10/11  6        CCX1 core2  active
—         7        CCX1 core3  FUSED

Cache hierarchy — L3 is 4 MB per CCX (half of desktop Matisse's 16 MB/CCX). L1D is
VIPT, write-back; L3 is a victim cache — inclusive of L2 for tags, exclusive for data.

Level  Type         Size    Assoc   Line  Sets  Shared With           Inst
─────  ───────────  ──────  ──────  ────  ────  ────────────────────  ────
L1D    Data         32 KB   8-way   64 B  64    2 threads (per core)  6
L1I    Instruction  32 KB   8-way   64 B  64    2 threads (per core)  6
L2     Unified      512 KB  8-way   64 B  1024  2 threads (per core)  6
L3     Unified      4 MB    16-way  64 B  4096  3 cores (per CCX)     2

Register model — the Oberon core (Model 47h) uses architecturally-identical Zen 2
registers per the AMD Family 17h Open-Source Register Reference (Doc #56255, covering
Models 00h-2Fh). Access methods: MSR (rdmsr/wrmsr, ring 0); SMN (PCI cfg 0xB8/0xBC on
0000:00:00.0, requires iomem=relaxed); PCICFG; BAR5 MMIO; APIC. The board has no
BMC/IPMI, so APML and out-of-band management do not apply.

Register           Description
─────────────────  ────────────────────────────────────────
MSRC001_0061       P-state Status (RO)
MSRC001_0062       P-state Control (request transition)
MSRC001_0063       P-state 0 Def (CpuFid, CpuDfsId, CpuVid)
MSRC001_0064-0069  P-state 1-6 Def
MSRC001_0071       COFVID Status (current freq/voltage/DID)
MSRC001_0015       HWCR
MSRC000_0080       EFER
MSRC001_0010       SYSCFG
MSR0000_0277       PAT
MSRC001_001F       NB_CFG

Machine-check banks: 0 MCA::LS, 1 MCA::IF, 2 MCA::L2, 3-6 MCA::DE/EX/FP, 7-10 MCA::L3
(per-CCX slice), 18 MCA::UMC, 19 MCA::NBIO. Selected performance counters (PMCx): 0x076
cycles-not-halted, 0x0C0 retired instructions, 0x0C1 retired uops, 0x043 DC accesses,
0x041 DC misses, 0x064 L2 requests, 0x062 L2 fill-wait cycles.

CPU core-clock and SoC-rail DPM operating points are read from the SMU (queue 3 msgids
0x3B / 0x42), documented under SMU / MP1. CPU overclock/undervolt actuators live on SMU
queue 3.

┌─ CAUTION ────────────────────────────────────────────────────────────────────────────┐
│  CPU curve undervolt is stable to -35 mV; -38 mV crashes under simultaneous          │
│  CPU and GPU load. Stay at or above -35 mV.                                          │
└──────────────────────────────────────────────────────────────────────────────────────┘

                     See: SMU / MP1 (P-state points, CPU OC) · BAPM / CAC (DPM_WAC MSRs)
```

## GPU Silicon Identity (gfx1013)

```
── GPU SILICON ─────────────────────────────────────── RDNA 1.x · gfx1013 · GC 10.1.3 ──

An RDNA 1.x graphics core (GFX10.1.3, Cyan Skillfish, gfx1013) harvested from the Oberon
die. Forty compute units — four shader arrays of ten CU each (twenty WGPs, two shader
engines, 2,560 stream processors); the stock harvest fuses the part to twenty-four
active CU, and a kernel patch liberates all forty. This section is the silicon as a
subsystem; the compute model and dispatch path are in Chapter 6.

┌─ SPECIFICATIONS ─────────────────────────────────────────────────────────────────────┐
│  Architecture     RDNA 1.x (GFX10.1.3)                                               │
│  Compute Units    40 on die (stock harvest 24 active; kernel patch enables fused     │
│                   16)                                                                │
│  Stream Procs     2,560 on die (40 CU × 64); 1,536 active in stock harvest           │
│  Shader Engines   2                                                                  │
│  Shader Arrays    4 (2 per SE, 10 CU per array)                                      │
│  WGPs             20 on die (5 per array, 2 CU each); 12 active in stock harvest     │
│  Base / floor     1000 MHz (driver floor)                                            │
│  OverDrive cap    2000 MHz                                                           │
│  Max (governor)   2350 MHz                                                           │
│  Hardware RT      native IMAGE_BVH_INTERSECT_RAY via FeatureGFX10_AEncoding          │
│  Display          DP 1.4, headless (amdgpu.dc=0)                                     │
│  Firmware         ME 0x63 · PFP 0x94 · CE 0x25 · RLC 0x0D · MEC 0x90                 │
│  Clock domain     GFXCLK, SMN 0x16C00                                                │
└──────────────────────────────────────────────────────────────────────────────────────┘

Shader-array harvest map — stock harvest 24 CU active, 6 per array across the four
10-CU arrays; the remaining 16 CU are fused off, re-enabled by the CU-enable patch.

SE  SH  CUs Active  WGPs Active
──  ──  ──────────  ───────────
0   0   6           3
0   1   6           3
1   0   6           3
1   1   6           3

Cache hierarchy — RDNA 1 adds per-WGP L0 and per-SA L1; no Infinity Cache (L3).
Path: L0 -> L1 -> L2 -> GDDR6.

Level           Size    Scope               Notes
──────────────  ──────  ──────────────────  ───────────────────────────────────
L0 Vector       16 KB   per WGP (2 CUs)     new in RDNA, replaces GCN per-CU L1
L0 Scalar       16 KB   per WGP             scalar data cache
L0 Instruction  32 KB   per WGP             instruction cache
L1              128 KB  per Shader Array    new level (GCN went straight to L2)
L2              2-4 MB  global              write-back, shared
LDS             64 KB   per WGP (32 KB/CU)  32 banks

Shader ISA — gfx1013 is the only RDNA 1 target carrying the BVH-intersect instruction;
Mesa special-cases it (has_image_bvh_intersect_ray = gfx_level >= GFX10_3 || family ==
CHIP_GFX1013). No dedicated RT block, no cooperative-matrix (WMMA), no INT8 dot-product
acceleration.

Property          Value
────────────────  ──────────────────────────────────────────────────────────────────────
Wave width        Wave64 native (default subgroupSize=64); no Wave32 mode
VGPRs per wave    256 addressable (wave64)
SGPRs per wave    106 + VCC (s106-s107) + 16 trap temps
FP16 packed       2:1 via VOP3P (V_PK_FMA_F16)
INT8 dot product  NOT hardware-accelerated (V_DOT4_I32_I8 is RDNA 2 only)
Hardware RT       native IMAGE_BVH_INTERSECT_RAY / IMAGE_BVH64_INTERSECT_RAY (ACO opcode
                  0xe7, non-NSA VADDR packing)

Per-CU occupancy hard caps: 256 VGPRs, 104-106 SGPRs, 64 KB LDS, 20 wavefront slots, 16
work-group slots. gfx1013 carries a silicon-level MEC dispatch limitation that routes
compute through the GFX ring — see Chapter 6.

Chip-capability fuse readouts — CC_* registers in the GFX block expose the die's
fuse-programmed config (read-only). A representative die reports:

Register                        Value       Decode
──────────────────────────────  ──────────  ────────────────────────────────────────────
CC_GC_PRIM_CONFIG               0x00000000  default / all primitive pipes enabled
CC_GC_SHADER_ARRAY_CONFIG_GEN0  0x00FC0001  InactiveCUs field
CC_GC_SHADER_ARRAY_CONFIG       0xFFE00000  shader-array CU-disable mask
CC_GC_SHADER_RATE_CONFIG        0x00000000  —
CC_GC_EDC_CONFIG                0x00000000  GFX error detection disabled by fuse
CC_RB_REDUNDANCY                0x001F1700  5 render backends flagged redundant
CC_RB_BACKEND_DISABLE           0x00000000  all 8 render backends active (no fuse
                                            disables)
CC_RB_DAISY_CHAIN               0x76543210  8 RBs daisy-chained in order
CC_RMI_REDUNDANCY               0x00000010  bit 4 set

Clock-control paths: driver OverDrive (pp_od_clk_voltage, 1000-2000 MHz, DPM levels
0/1/2); SMU governor via queue 3 (1000-2350 MHz, bypasses OD cap); SMU ForceGfxFreq (Q0
0x39 / Q3 0x57, requires a paired voltage). See SMU / MP1 for messaging and Clock
Domains for DPM tables.

                              See: SMU / MP1 · Clock Domains · Chapter 6 · Compute Stack
```

## SDMA — System DMA Engines

```
── SDMA ────────────────────────────────────────────────────── SDMA0 / SDMA1 · v5.0.1 ──

Two System DMA engines move memory and update page tables independently of the graphics
command processor. SDMA0 is primary; SDMA1 is secondary. Both are managed by amdgpu_sdma
(sdma_v5_0.c).

┌─ ENGINES ────────────────────────────────────────────────────────────────────────────┐
│  SDMA0      hw_id 42, IP v5.0.1 — primary DMA (memory copies + page-table updates)   │
│  SDMA1      hw_id 43, IP v5.0.1 — secondary DMA                                      │
│  Firmware   0x34  (feature version 50 / 0x32)                                        │
└──────────────────────────────────────────────────────────────────────────────────────┘

The driver feature_version field and the ip_discovery major.minor.revision tuple are
independent metadata encodings and do not map to one another numerically.

                         See: APU Identity & IP-Block Catalog · Firmware Map · Chapter 6
```

## OSSSYS / IH — Interrupt Handler

```
── OSSSYS / IH ────────────────────────────────────────────── interrupt ring · v5.0.1 ──

The OSSSYS block owns the GPU interrupt ring buffer, aggregating and routing hardware
interrupts from every on-die IP block up to the host. Managed by amdgpu_ih
(navi10_ih.c).

┌─ BLOCK ──────────────────────────────────────────────────────────────────────────────┐
│  Block      OSSSYS / IH — hw_id 40, IP v5.0.1                                        │
│  Function   interrupt ring buffer; aggregates + routes IP-block interrupts           │
│  Driver     amdgpu_ih (navi10_ih.c)                                                  │
└──────────────────────────────────────────────────────────────────────────────────────┘

                                             See: APU Identity & IP-Block Catalog · SDMA
```

## SMU / MP1 — System Management Unit

```
── SMU / MP1 ────────────────────────────────────────────────── SMU · MP1 · Xtensa LX ──

The SMU (MP1 block, hw_id 1, v11.0.8) is a Tensilica Xtensa LX microcontroller that owns
all power, clock, and thermal management. It commands the VRM over SVI2 and enforces the
package-power ceiling. OS-visible sysfs controls (pp_dpm_sclk, pp_dpm_mclk,
pp_od_clk_voltage) never touch silicon directly — every change is a request the firmware
may honor, clamp, or refuse. The firmware is PSP-verified before MP1 leaves reset, so
arbitrary SMU firmware cannot run on a stock board.

┌─ CORE ───────────────────────────────────────────────────────────────────────────────┐
│  Core             Tensilica Xtensa LX (little-endian)                                │
│  IP block         MP1, hw_id 1, v11.0.8                                              │
│  Firmware         PSP directory type 0x08, 262,656 bytes, RSA-2048 signed (key       │
│                   8B8D)                                                              │
│  FW version       88.6.0 (0x00580600) on BIOS P3.00; 88.7.1 (0x00580701) on          │
│                   P5.00-era                                                          │
│  Mailbox queues   5 (Q0-Q4), command/response/argument register triples              │
│  GPU DPM sclk     1000 / 2000 / 2230 MHz                                             │
│  Mem DPM mclk     450 MHz (single state)                                             │
└──────────────────────────────────────────────────────────────────────────────────────┘

The live SMU firmware version tracks the BIOS. Much command-map RE was done against
88.7.1; command semantics may differ slightly on 88.6.0, but the mailbox register
triplets (the C2PMSG offsets) are a kernel-driver contract and identical across
versions.

── SMU — MAILBOX ARCHITECTURE ───────────────────────────────── 5 queues · SMN + BAR5 ──

Five mailbox queues, each a command/response/argument triple on SMN. Access via SMN
indirect (PCI cfg 0xB8/0xBC on 0000:00:00.0) or BAR5 MMIO. The mailbox sits at the MP1
base (0x16000 dwords = 0x58000 bytes from BAR5); BAR5 is physical 0xFE800000, 512 KiB.

  x86 host                         MP1 / Xtensa LX SMU
 ┌─────────┐   write ARG (C2PMSG_82)   ┌────────────────────────┐
 │ driver  │ ────────────────────────► │ arg register           │
 │  or     │   write MSG (C2PMSG_66)   │  ↓ doorbell             │
 │ SMN/BAR5│ ────────────────────────► │ msg_dispatch (0x0DBC)  │
 │ window  │                           │  ↓ index fn-ptr table  │
 │         │   poll RSP (C2PMSG_90)     │  ↓ check capability    │
 │         │ ◄──────────────────────── │ handler → DPM/SMUIO/    │
 └─────────┘                           │           SVI2/PGFSM   │
                                       └────────────────────────┘

Five-queue register map (cmd / rsp / arg over SMN):

Queue  Name                  CMD         RSP         ARG         Hdlrs
─────  ────────────────────  ──────────  ──────────  ──────────  ────────
Q0     PPSMC (Driver)        0x03B10A08  0x03B10A68  0x03B10A48  ~39
Q1     Unknown               0x03B10A00  0x03B10A60  0x03B10A40  untested
Q2     MP1 (RSMU secondary)  0x03B10528  0x03B10564  0x03B10998  ~31
Q3     RSMU primary          0x03B10A20  0x03B10A80  0x03B10A88  ~85
Q4     Freq Ops              0x03B10A24  0x03B10A84  0x03B10A8C  ~16
Q2 name in full: MP1 (RSMU secondary / feature enable).

Q0 is also reachable via MP1 C2PMSG registers over BAR5 MMIO:

Register              SMN         BAR5 offset  SOC15 dword
────────────────────  ──────────  ───────────  ───────────
C2PMSG_66 (command)   0x03B10A08  0x58A08      0x16282
C2PMSG_82 (argument)  0x03B10A48  0x58A48      0x16292
C2PMSG_90 (response)  0x03B10A68  0x58A68      0x1629A

Offset = (MP1_BASE + SOC15_reg) × 4, MP1_BASE = 0x16000. The working SMN-indirect method
via BAR5 writes the SMN address to BAR5 byte 0x38 (PCIE_INDEX2) and reads/writes data at
BAR5 byte 0x3C (PCIE_DATA2).

The host-driver / BAPM command set uses a secondary MP1 mailbox distinct from the RSMU
primary at 0x3B10500+; both coexist on MP1 v11.0.8 silicon.

Register   SMN        Purpose
─────────  ─────────  ──────────────────────────────────────────
C2PMSG_56  0x3B109E0  arg1 (extra, Universal Mode)
C2PMSG_57  0x3B109E4  arg2
C2PMSG_58  0x3B109E8  arg3
C2PMSG_59  0x3B109EC  arg4
C2PMSG_66  0x3B10A08  msgid (write triggers processing)
C2PMSG_81  0x3B10A44  extra arg (CACW weight / second parameter)
C2PMSG_82  0x3B10A48  arg / response value
C2PMSG_90  0x3B10A68  response code / busy

The cyan_skillfish message config declares num_arg_regs=1, so only C2PMSG_82 is in the
formal argument array; C2PMSG_81 sits one register below and is reached directly
(arg_regs[0] - 1) for the CACW extra parameter. C2PMSG_56..59 are argument-bearing only
for the Universal Mode message.

── SMU — COMMAND DISPATCH ───────────────────────────────── doorbell · response codes ──

Protocol: (1) clear the response register, (2) write the argument, (3) write the message
ID — this write is the doorbell that signals the SMU, (4) poll the response until non-
zero, (5) read the argument for the return value. The Xtensa dispatcher (msg_dispatch)
reads the msgid, indexes a runtime fn-ptr table, checks capability flags, and calls the
handler. Handlers may touch DPM tables, SMUIO tile control, an SVI2 voltage command,
clock dividers, or PGFSM state machines.

┌─ RESPONSE CODES ─────────────────────────────────────────────────────────────────────┐
│  0x00  NONE         timeout / mailbox locked                                         │
│  0x01  OK                                                                            │
│  0xFB  DEBUG_END                                                                     │
│  0xFC  BUSY         queue busy / overflow                                            │
│  0xFD  BAD_PREREQ   capability-flag mismatch                                         │
│  0xFE  UNKNOWN      msg ID >= max, or handler NULL (not implemented)                 │
│  0xFF  FAIL         handler exists but policy-rejects                                │
└──────────────────────────────────────────────────────────────────────────────────────┘

EnableSmuFeatures / DisableSmuFeatures are not a working runtime feature toggle on
shipping firmware.

── SMU — QUEUE 0 (DRIVER / PPSMC) ── cmd 0x03B10A08 · arg 0x03B10A48 · rsp 0x03B10A68 ──

Handler address 0x00000000 = no handler (returns 0xFE).

MsgID  Handler     Symbol
─────  ──────────  ──────────────────────────────────────────────
0x01   0x0001B3A8  PPSMC_MSG_TestMessage
0x02   0x0001B3C0  PPSMC_MSG_GetSmuVersion
0x03   0x0001B940  PPSMC_MSG_GetDriverIfVersion
0x04   0x0001B9D0  PPSMC_MSG_SetDriverTableDramAddrHigh
0x05   0x0001BA10  PPSMC_MSG_SetDriverTableDramAddrLow
0x06   0x0001BB1C  PPSMC_MSG_TransferTableSmu2Dram
0x07   0x0001BBC0  PPSMC_MSG_TransferTableDram2Smu
0x0B   0x00022BBC  PPSMC_MSG_RequestCorePstate
0x0C   0x00022C94  PPSMC_MSG_QueryCorePstate
0x0E   0x0002B400  PPSMC_MSG_RequestGfxclk
0x0F   0x0002B4D4  PPSMC_MSG_QueryGfxclk
0x11   0x0002EA74  PPSMC_MSG_QueryVddcrSocClock
0x13   0x0001E988  PPSMC_MSG_QueryDfPstate
0x16   0x00025188  PPSMC_MSG_ConfigureS3PwrOffRegisterAddressHigh
0x17   0x000251B8  PPSMC_MSG_ConfigureS3PwrOffRegisterAddressLow
0x18   0x0002B510  PPSMC_MSG_RequestActiveWgp
0x19   0x0002B5F4  PPSMC_MSG_SetMinDeepSleepGfxclkFreq
0x1A   0x0002B634  PPSMC_MSG_SetMaxDeepSleepDfllGfxDiv
0x1B   0x000270DC  PPSMC_MSG_StartTelemetryReporting
0x1C   0x00027124  PPSMC_MSG_StopTelemetryReporting
0x1D   0x00027154  PPSMC_MSG_ClearTelemetryMax
0x1E   0x0002B690  PPSMC_MSG_QueryActiveWgp
0x2C   0x00030124  PPSMC_MSG_SetCoreEnableMask
0x2F   0x0002CF54  PPSMC_MSG_GfxCacWeightOperation
0x30   0x0002D008  PPSMC_MSG_L3CacWeightOperation
0x31   0x0002D0B4  PPSMC_MSG_PackCoreCacWeight
0x34   0x0001BD9C  PPSMC_MSG_SetDriverTableVMID
0x35   0x000234CC  PPSMC_MSG_SetSoftMinCclk
0x36   0x00023548  PPSMC_MSG_SetSoftMaxCclk
0x37   0x0002B504  PPSMC_MSG_GetGfxFrequency
0x38   0x0002C340  PPSMC_MSG_GetGfxVid
0x39   0x0002B8D4  PPSMC_MSG_ForceGfxFreq
0x3A   0x0002B8F8  PPSMC_MSG_UnForceGfxFreq
0x3B   0x0002C358  PPSMC_MSG_ForceGfxVid
0x3C   0x0002C388  PPSMC_MSG_UnforceGfxVid
0x3D   0x0001DCF4  PPSMC_MSG_GetEnabledSmuFeatures

IDs 0x08-0x0A, 0x0D, 0x10, 0x12, 0x14-0x15, 0x1F-0x2B, 0x2D-0x2E, 0x32-0x33, 0x3E have
no handler. Observed from direct probing: GetSmuVersion returns 0x00580701 (v88.7.1);
GetDriverIfVersion returns 0x08 (interface v8); QueryDfPstate returns 3; PowerUpVcn
(0x0C) returns 0x06 but is a no-op (blocked by the SRAM feature mask at 0xCCB8);
GetGfxVid returns an SVI2 VID code (mV = 1550 - VID×1000/160); GetEnabledSmuFeatures
returns 0xDD602C7D.

── SMU — QUEUE 3 (RSMU PRIMARY) ──── cmd 0x03B10A20 · arg 0x03B10A88 · rsp 0x03B10A80 ──

WRITE handlers mutate silicon; GUARDED = secure-access gated; MISSING FN = present
but undecompilable (treat as a landmine). CPU OC/undervolt + live AVFS live here.

MsgID      Handler     Symbol                           Notes
─────────  ──────────  ───────────────────────────────  ─────────────────────────
0x01       0x0001B3A8  TestMessage                      returns arg+1
0x02       0x0001B3C0  GetSmuVersion
0x04       0x0001E604  q3_0x04                          MISSING FN
0x0B       0x0001BB04  GetDramBaseAddress
0x0C       0x00027AC4  GetTableVersion                  returns 0x00290301
0x0F       0x00027E94  set_cpu_gpu_vid                  WRITE
0x10       0x00027EC8  unforce_cpu_gpu_vid              WRITE
0x1C       0x0002B400  RequestGfxclk
0x1D       0x0002EA08  set_soc_clock_for_index          WRITE
0x1E       0x0001E944  set_PerfProfileIndex             WRITE
0x20       0x000276B4  set_max_temperature              WRITE
0x22       0x0001BB1C  TransferTableSmu2Dram
0x23       0x0001BBC0  TransferTableDram2Smu
0x25       0x00027D50  set_oc_clk                       WRITE
0x26       0x00027DB4  unset_oc_clk                     WRITE
0x27-0x2F  0x000279xx  secure_access                    GUARDED (0x2E MISSING FN)
0x30       0x0002DAFC  return_vid_offset                read
0x35       0x0001DCF4  GetEnabledSmuFeatures            = Q0 0x3D
0x36       0x0002C0C8  get_current_cpu_voltage          read
0x37       0x0002C0EC  get_current_gpu_voltage          read
0x38       0x0001E80C  get_clock_table (FCLK/UCLK DPM)  read
0x39       0x0001E844  get_clock_table (LCLK)           read
0x3A       0x0001E87C  get_clock_table (low-freq)       read
0x3B       0x00022CE8  get_pstate_clock                 read
0x3C       0x0001DC58  enable_smu_features              MISSING FN
0x3D       0x0001DCA4  disable_smu_features             WRITE, DANGEROUS
0x40       0x0002776C  get_cpu_temp_max                 read
0x42       0x0002EB14  return_vddcrsoc_dpm              read
0x43       0x00022D20  get_core_freq                    read
0x49       0x0002DDE4  set_cpu_vid_offset               WRITE
0x4A       0x0002DE10  gfx_vid_offset                   WRITE
0x4B       0x00000000  (no handler)                     would hang
0x4C       0x0002DB90  gfx_droop_cal                    MISSING FN
0x4D       0x00028184  set_cpu_vid_offset_large         WRITE
0x4E       0x000281D4  set_gpu_vid_offset_large         WRITE
0x4F       0x0002810C  q3_0x4F                          MISSING FN
0x50       0x0002814C  scale_vid_curve                  WRITE
0x51       0x00022E78  set_cpu_coeff                    WRITE
0x52       0x00022EBC  cpu_clock_stretch                WRITE
0x53       0x00022F00  ccx_clock_stretch                WRITE
0x57       0x0002B8D4  ForceGfxFreq                     = Q0 0x39
0x5F       0x00027C68  write_cpu_frequency              WRITE
0x62       0x0002B8F8  UnForceGfxFreq                   = Q0 0x3A
0x63       0x00000000  EnableOcMode (DELETED)           gone
0x64       0x00022BBC  RequestCorePstate                = Q0 0x0B
0x77       0x0002F5E8  set_cpu_max_current              WRITE
0x78-0x79  0x00000000  SetOCFreq (DELETED)              gone
0x7A       0x0001B9D0  SetDriverTableDramAddrHigh
0x7B       0x0001BA10  SetDriverTableDramAddrLow
0x86       0x00029280  GetBoostLimitFreq                returns 0
0x87       0x0002937C  IsOverclockable                  returns 0
0x8B       0x000276F8  set_cpu_max_temp                 WRITE
0x8C       0x00027730  set_gpu_max_temp                 WRITE
0x8E       0x0002F6B0  set_vid_limit                    WRITE
0x8F       0x0002344C  set_max_boost_clk                WRITE
0x99       0x00028380  modify_pstate                    WRITE
0x9A       0x0002C310  vid_extra_voltage                WRITE
0x9B       0x0002725C  bilinear_model                   WRITE

Queue spans IDs 0x01-0xA9. EnableOcMode (0x63) and SetOCFreqAllCores (0x79) are deleted
(no handler); ForceGfxFreq/UnForceGfxFreq exist here at 0x57/0x62 (same fn pointers as
Q0 0x39/0x3A). CPU OC/undervolt: VID offsets 0x49/0x4A/0x4D/0x4E, curve scale 0x50,
clock stretch 0x52/0x53.

Queue 3 is also the live AVFS / BAPM actuator surface (subset of ~80 messages):

msgid        Function                              Class
───────────  ────────────────────────────────────  ─────────────────────────────────────
0x0F         set_cpu_gpu_vid(kind, mV)             actuator
0x10         unforce_cpu_gpu_vid                   actuator
0x1E         set_perfprofileindex                  actuator (slots 0..3 valid; 4..7
                                                   rejected)
0x20         set_max_temperature_cpu_gpu           actuator
0x25 / 0x26  set / unset OC clock                  actuator
0x36         get_current_cpu_voltage               getter
0x37         get_current_gpu_voltage               getter (virtualized target, not the
                                                   live rail)
0x3B         get_clk_assigned_to_p_state(0..7)     getter (CPU CCLK P-state table)
0x40         get_cpu_temp_max                      getter (reports TctlMax setpoint;
                                                   settable via 0x8B)
0x42         return_vddcrsoc_dpm_value(idx)        getter (SoC DPM clock table)
0x43         get_core_freq(core)                   getter (per-core CCLK)
0x49         set_cpu_vid_offset                    actuator (+/-1, +/-3 accepted; +/-5
                                                   rejected in nominal AVFS state)
0x4A         set_gfx_vid_offset1                   actuator (same range as 0x49)
0x4C         gfx_droop_calibration                 actuator
0x4D / 0x4E  set large VID offset                  actuator
0x50         scale_f_vid_curve(int16)              actuator (multiplicative AVFS curve
                                                   scaler)
0x52         set_cpu_clock_stretch_coeff(0..1000)  actuator (load-time)
0x6D         force_clock_stretching_vid            actuator
0x77         set_cpu_max_current(mA)               actuator (EDC ceiling)
0x8B / 0x8C  set cpu / gpu max_temperature         actuator (TctlMax)
0x8E         set_vid_main_2_limit(mV)              actuator (does NOT clamp the live
                                                   VDDCR_GFX rail)
0x8F         set_max_cpu_boost_clk                 actuator
0x9A         disable_extra_cpu_gpu_voltage         actuator

The secure-access messages 0x27, 0x2A, 0x2C, 0x2D, 0x2E, 0x2F are gated behind a flag
passed to the SMU at boot and may be unreachable without firmware-side enablement.

── SMU — QUEUES 1, 2, 4 ────────────────────────── sparse · feature-enable · freq ops ──

Queue 1 is sparse (0x03B10A00 / 0x03B10A60 / 0x03B10A40): only TestMessage (0x01),
GetSmuVersion (0x02), q1_0x08 (0x0002C5F8), and q1_0x10 (0x0002C44C) implemented; all
other IDs return 0xFE and direct probing found it non-functional.

Queue 2 (MP1/RSMU secondary, 0x03B10528 / 0x03B10564 / 0x03B10998) — feature
enable/disable and calibration. MISSING FN = non-zero address that failed to decompile.

MsgID  Handler     Symbol                         Status
─────  ──────────  ─────────────────────────────  ───────────
0x01   0x0001B3A8  TestMessage                    HAS HANDLER
0x02   0x0001B3C0  GetSmuVersion                  HAS HANDLER
0x03   0x0001B92C  q2_0x03 (returns constant 23)  HAS HANDLER
0x04   0x0001C814  q2_0x04_get_device_name        HAS HANDLER
0x05   0x0001DC58  q2_0x05_enable_smu_features    MISSING FN
0x06   0x0001DCA4  q2_0x06_disable_smu_features   HAS HANDLER
0x07   0x00028984  q2_0x07                        HAS HANDLER
0x0A   0x00025414  q2_0x0a                        HAS HANDLER
0x0D   0x0001B954  q2_set_some_other_addr_high    HAS HANDLER
0x0E   0x0001B990  q2_set_some_other_addr_low     HAS HANDLER
0x11   0x0001BB1C  TransferTableSmu2Dram          HAS HANDLER
0x12   0x0001BBC0  TransferTableDram2Smu          HAS HANDLER
0x17   0x0002D7A0  q2_0x17_cpu_droop_calibration  HAS HANDLER
0x18   0x0002DB90  gfx_droop_calibration          MISSING FN
0x2A   0x000276F8  set_cpu_max_temperature        HAS HANDLER
0x2B   0x00027730  set_gpu_max_temperature        HAS HANDLER
0x2C   0x0003247C  q2_0x2c_probably_power_limit   HAS HANDLER

Queue 2 spans 0x01-0x31; on one firmware build direct probing found it empty (all 0xFE),
and EnableSmuFeatures is not present — the custom v88.7.1 firmware has no runtime
feature-toggle path.

Queue 4 (Freq Ops, 0x03B10A24 / 0x03B10A84 / 0x03B10A8C):

MsgID  Handler     Symbol
─────  ──────────  ──────────────────────────
0x01   0x0001B3A8  TestMessage
0x02   0x0001B3C0  GetSmuVersion
0x04   0x0001B3F4  q5_0x04
0x05   0x0002548C  q5_0x05
0x06   0x0001E6B0  q5_0x06
0x07   0x0001E720  q5_0x07
0x08   0x0001E7C0  q5_0x08
0x0A   0x0001E7D8  q5_0x0A_freq_op1
0x0C   0x0002E428  pstate_related (= Q3 0x60)
0x0E   0x0001F218  freq_related (= Q3 0x5C)
0x0F   0x0001F2F0  freq_related (= Q3 0x5D)
0x10   0x000300B4  q5_0x10
0x11   0x0002F6C4  q5_0x11
0x14   0x0002A968  (= Q3 0x1A)

Queue 4 spans 0x01-0x15; 0x03, 0x12-0x13, 0x15 have no handler.

── SMU — OPERATING-POINT TABLES ────────────────── q3 0x3B (CCLK) · q3 0x42 (SoC DPM) ──

CPU CCLK P-states (q3 msgid 0x3B) and the SoC-rail DPM clock table (q3 msgid 0x42):

  CPU CCLK P-state          VDDCR_SOC DPM
    P0  3200 MHz              idx 0  1254 MHz
    P1  2550 MHz              idx 1   500 MHz
    P2  2325 MHz              idx 2   762 MHz
    P3  1960 MHz              idx 3   762 MHz
    P4  1820 MHz              idx 4..7 unused
    P5  1600 MHz
    P6  1271 MHz
    P7   800 MHz

The actual per-core clock is read separately via msgid 0x43 get_core_freq(core).

── SMU — FEATURE MASK ──────────────────────────── GetEnabledSmuFeatures = 0xDD602C7D ──

GetEnabledSmuFeatures (Q0 msgid 0x3D) returns 0xDD602C7D — 17 enabled, 15 disabled. This
public/DPM mask is hardcoded in firmware and cannot change at runtime. It is the
externally-readable mask and does NOT gate VCN power-up; that gate is a separate
firewalled internal SRAM mask at 0xCCB8.

Enabled (17): DPM_PREFETCHER (0), DPM_GFX_PACE (2), DPM_UCLK (3), DPM_SOCCLK (4),
DPM_MP0CLK (5), DPM_LINK (6), DS_GFXCLK (10), DS_SOCCLK (11), DS_DCEFCLK (13), USB_PG
(21), RSMU_SMN_CG (22), TDC (24), APCC_PLUS (26), GTHR (27), ACDC (28), VR1HOT (30),
FW_CTF (31).

Disabled (15): DPM_GFXCLK (1, no dynamic GFX clock — use ForceGfxFreq), DPM_DCEFCLK (7),
MEM_VDDCI_SCALING (8), MEM_MVDD_SCALING (9), DS_LCLK (12), DS_UCLK (14), GFX_ULV (15),
FW_DSTATE (16), GFXOFF (17), BACO (18), VCN_PG (19, root cause of VCN being off),
JPEG_PG (20), PPT (23), GFX_EDC (25), VR0HOT (29).

Each disabled feature also has a dedicated AGESA Setup variable in VarStore 0x5000,
contiguous from offset 0x6A5 to 0x6BD, defaulting to Auto (=15 -> Disabled on this
silicon). Selected: DS_GFXCLK 0x6A5, DS_SOCCLK 0x6A6, DS_LCLK 0x6A7, RM 0x6AA, DS_SMNCLK
0x6AC, DS_MP1CLK 0x6AD, DS_MP0CLK 0x6AE, MGCG 0x6AF, DS_FUSE_SRAM 0x6B0, GFX_CKS 0x6B1,
FP_THROTTLING 0x6B2, UMC_THROTTLE 0x6B5, DFLL_BTC_CALIBRATION 0x6BB. Setting a Setup
byte to Enabled does NOT bring the feature online — the bits are fuse-gated below the
IFR level; the working liberation path is direct MMIO register writes.

── SMU — VOLTAGE-FREQUENCY CHAIN ────────────────────────────────── SVI2 · VRM · AVFS ──

GPU clock/voltage is not directly host-programmable; every change passes through the SMU
DPM scheduler (closed-loop on MP1). The host can edit two things: the voltage-frequency
curve table (SRAM, piecewise-linear freq->voltage) and the DPM level mask. The scheduler
refuses a level whose curve voltage is below the AVFS-calibrated minimum — this
interlock prevents undervolt crashes and is why raising the OD curve voltage unlocks
higher DPM levels. AVFS calibration is per-die: on-die ring-oscillator process monitors
sampled at boot set a per-chip voltage floor per frequency; the OD curve sits on top and
cannot go below it.

SVI2 (Serial VID Interface 2) is a two-wire serial link (~20 MHz), point-to-point inside
the package, SMU as sole initiator. VID encoding (cyan_skillfish_ppt.c):

    vid = (1550 - voltage_mV) * 160 / 1000

VRM Part  Role
────────  ────────────────────────────────────────────────────
ISL69247  main controller — VddGfx and primary rails
ISL95712  secondary controller — VddNb / VSoC
ISL99360  smart power stages (integrated FET + driver + sense)

OD command alphabet (amdgpu_pm.c): s (SCLK point), p (CPU clock point), m (MCLK point,
unsupported on gfx1013), r (reset), c (commit), vc (voltage-curve point), vo (global
voltage offset). On gfx1013 the working commands are vc, vo, c; MCLK/UCLK/FCLK have no
host-reachable knob. GPU clock control uses ForceGfxFreq/UnForceGfxFreq (RequestGfxclk
0x0E is rejected 0xFF), or raising the OD curve voltage at the target frequency so the
scheduler stops refusing the higher DPM level.

── SMU — FIRMWARE IMAGE & RE ──────────────────────────────── PMFW 88.6.0 · Xtensa LX ──

The MP1 firmware image (PMFW, version dword 0x00580600 = 88.6.0) is 262,400 bytes; the
88.7.1 image is 262,656 bytes (0x40200) at BIOS offset 0x8FEE00, with a 0x200-byte PSP
header ahead of Xtensa code/data.

┌─ ISA / ABI ──────────────────────────────────────────────────────────────────────────┐
│  ISA / ABI   Xtensa LX, little-endian; call0 windowed ABI (entry aN / retw.n)        │
│              Density option (movi.n, add.n, l32i.n, s32i.n, ...); FP coprocessor     │
│              (f0..f15, b0..b3) used by clock-table + BAPM float math                 │
│  Load base   0x00000000 (confirmed by l32r literal-pool resolution)                  │
│  Functions   ~1280 auto-discovered, no decode errors                                 │
└──────────────────────────────────────────────────────────────────────────────────────┘

Region           Offset        Contents
───────────────  ────────────  ──────────────────────────────────────────
Version dword    0x000         0x00580600
Header           0x000..0x0FF  version + zero padding
Metadata struct  0x100..0x132  0x33-byte in-image struct (lengths / CRCs)
Code entry       0x133         first function prolog (entry a1, 0xN)

Key firmware functions and SRAM data structures (RE of the 88.7.1 image):

Address     Name                  Purpose
──────────  ────────────────────  ─────────────────────────────────────────
0x00000DBC  msg_dispatch          main mailbox dispatcher
0x00000E90  read_msg_id           reads message ID from C2PMSG_66
0x00000EA8  write_response        writes response to C2PMSG_90
0x00000EC0  set_sync_response     sync response (0xFE/0xFD/0xFC)
0x000025B4  enqueue_msg           queues async message
0x0001DC34  check_feature         tests SMU feature N against internal mask
0x0001E348  set_pstate            DPM P-state transition
0x0001E070  write_pgfsm           PGFSM register writer
0x0001E5B0  PowerUpVcn            VCN power-up handler (contains rejection)
0x0001EE90  vcn_power_seq         VCN power sequence (gated by feature 0xB)
0x000240D4  power_gate_tile_up    powers on a SMUIO tile
0x000241DC  power_gate_tile_down  powers off a SMUIO tile
0x0002435C  power_gate_tile       hardware power-gate controller (PGFSM)
0x00002BDC  task_scheduler        main task scheduling loop
0x00002CE8  context_switch        Xtensa context switch / IRQ handler
0x000170EC  DPM state base ptr    -> 0xCEE0
0x000170A0  power_gate_reg        VCN power gate control base
0x000172E0  power_mgmt_lock       power-management lock
0x0001729C  feature_mask_ptr      -> internal feature bitmask (0xCCB8)
0x0000CCB8  feature_mask[0]       internal enabled-features bitmask
0x0001735C  power_tile_table      per-tile power-gate config (0x48 B/tile)

The host-driver C2PMSG_66 msgid dispatch table has not been unambiguously located; the
132-entry pointer run at file offset 0xCCA8 is a feature init/deinit array indexed by
feature ID, not a msgid dispatch table. All msgids at or above 0x40 remain
conservatively classified as suspect — every recognized live-responding msgid falls in
the 0x01..0x3D range.

Universal Mode (msgid 0x22 enter / 0x23 exit) is the only command that uses the
secondary-mailbox extra-argument registers C2PMSG_56..59 as argument slots (clock
floor/ceiling pairs). These are PS5-lineage msgids; on BC-250 PMFW 88.6.0 they return
UNKNOWN_CMD (0xFE) — the handlers appear NOPed.

┌─ CAUTION — SMU MAILBOX HAZARDS ──────────────────────────────────────────────────────┐
│  Q0  0x04 / 0x2E — hang the SMU permanently; recovery is a full power cycle.         │
│  Q3  0x04, 0x2E, 0x3C, 0x4C, 0x4F — valid but UNTESTED handlers, unknown risk.       │
│  Pacing — never send SMU messages faster than one per 100 ms; bursts jam the         │
│           mailbox and every later message times out.                                 │
│  Curve  — bound the q3 AVFS scaler (0x50) to +/-10; a large excursion latches        │
│           the AVFS integrator stuck-high (only a power cycle clears it), after       │
│           which any clock-transition msgid wedges the chip. VID offsets              │
│           (0x49/0x4A) accept only up to +/-3. Bail if idle rail > ~1280 mV or        │
│           < ~700 mV.                                                                 │
│  Compute— NO SMU mailbox traffic during sustained GPU compute; the PMFW lacks a      │
│           preemption fence and mailbox sends wedge the compute pipe. Actuate at      │
│           idle only, serialize sends on the kernel SMN lock, sample telemetry        │
│           via hardware-monitor sensors during the workload.                          │
│  Pairing— always pair ForceGfxVid with ForceGfxFreq; forcing one axis alone          │
│           lands on an untested (voltage, frequency) point.                           │
└──────────────────────────────────────────────────────────────────────────────────────┘

                     See: Interconnect Fabric (SMN) · Clock Domains · VCN · BAPM · SMUIO
```

## VCN — Video Core Next (firmware-disabled)

```
── VCN ───────────────────────────────────────────── media engine · firmware-disabled ──

The BC-250 carries VCN 2.0.3, the Navi 10/14 discrete-class media engine (not the
functional Renoir 2.2 APU variant). Silicon is physically present and un-harvested
(CC_UVD_HARVESTING = 0x00000000) but firmware-disabled: the SMU declines to power the
island. PowerUpVcn returns success without toggling the power gate, the VCN feature bit
in SMU SRAM is clear, and the whole VCN register domain reads 0xFFFFFFFF while the clock
domain is unpowered.

┌─ STATE ──────────────────────────────────────────────────────────────────────────────┐
│  Silicon state    Present (not fused off) — VCN fw ENC 1.24 / DEC 8 / VEP 0 / Rev 9  │
│  Firmware         Not loaded (reports version 0)                                     │
│  Registers        Read 0xFFFFFFFF (power island gated)                               │
│  MMIO segments    seg0 = 0x7800, seg1 = 0x7E00, seg2 = 0x2403000                     │
│  Software paths   Exhausted                                                          │
└──────────────────────────────────────────────────────────────────────────────────────┘

Register          Dword offset          BAR5 byte
────────────────  ────────────────────  ─────────
UVD_PGFSM_CONFIG  seg1+0x0000 = 0x7E00  0x1F800
UVD_PGFSM_STATUS  seg1+0x0001 = 0x7E01  0x1F804
UVD_POWER_STATUS  seg0+0x0004 = 0x7804  0x1E010

Two-gate power-up block — PowerUpVcn (FUN_0001E5B0, 109 bytes at Xtensa 0x1E5B0)
contains two independent gates and returns SUCCESS (resp=0x01) regardless:

Gate 2 — DPM bounds check (secondary, runs first): rejects with 0xFF when
dpm_state[0xF7] (max VCN DPM levels) <= dpm_state[0xF6] (current). The VCN DPM table has
0 entries, so 0 <= 0 rejects. Critical branch at 0x1E5BC is bltu a8,a9,0x1e5c9 (bytes 97
38 09). Bypassable with param=1.

Gate 1 — feature mask (primary, always blocks): vcn_power_seq (FUN_0001EE90) calls
check_feature(0xB) against the INTERNAL feature mask at SMU Xtensa SRAM 0xCCB8 (SRAM
base 0x03C00000, full SMN 0x03C0CCB8). The VCN bit is clear (mask = 0x00000000), so
power_gate_tile(3) and power_gate_tile(4) are never called — a silent no-op even if Gate
1 is bypassed.

Under PowerUpVcn(1) the MMSCH (Multimedia Scheduler) range at dw 0x7DC0-0x7DFF
transitions 0xFFFFFFFF -> 0x00000000 (a partial power domain activates) but the VCN core
at seg1 0x7E00+ stays dead. The SMU SRAM window is firewalled from the host: any x86
read of SMN 0x03C00000+ returns 0xFFFFFFFF and writes are ignored; the 0x00000000 value
is known only from firmware decompilation.

A 3-byte SPI-flash edit neutralizes Gate 2 only:

Property        Value
──────────────  ─────────────────────────────────────────────────────
BIOS offset     0x91D5BC
Xtensa offset   0x1E5BC (within SMU firmware)
Original bytes  97 38 09 (BLTU, conditional branch)
Patched bytes   46 02 00 (J, unconditional jump)
Effect          Always take the power-up path (skip DPM bounds check)

A complete VCN fix additionally requires SRAM 0xCCB8 feature bit 0xB set (Gate 1), which
means patching firmware initialization or writing the bit via PSP code execution before
the SMU starts — and the patch is gated by PSP signature enforcement of the SMU image.
With the VCN DPM table empty, applying P-state index 0 to a 0-entry table also risks an
SMU hang. All remaining enablement paths cross into the Security domain (Chapter 3).

Version lineage and codec capability (block silicon capability, not enabled here):

VCN    Products                   Type
─────  ─────────────────────────  ───────────────────────────────
1.0    Raven Ridge, Picasso       APU
2.0    Navi 10/14 (RX 5700/5500)  Discrete
2.0.3  Oberon (PS5 / BC-250)      Console APU (firmware-disabled)
2.2    Renoir (Ryzen 4000)        APU
3.0    Navi 21/22/23 (RX 6000)    Discrete (adds AV1 decode)
3.1    Van Gogh, Rembrandt        APU
4.0    Navi 31/32/33 (RX 7000)    Discrete (adds AV1 encode)

Codec         Decode  Encode  Max resolution
────────────  ──────  ──────  ──────────────
H.264 / AVC   1.0     1.0     4096×2304
H.265 / HEVC  1.0     1.0     7680×4320 (8K)
VP9           1.0     never   7680×4320 (8K)
AV1           3.0     4.0     7680×4320 (8K)
JPEG          1.0     never   varies

                           See: SMU / MP1 (feature mask, PowerUpVcn) · SMUIO · Chapter 3
```

## UMC / GDDR6 Memory Controller

```
── UMC / GDDR6 ───────────────────────────────────────────── 8-channel GDDR6 · v8.1.1 ──

An 8-channel Unified Memory Controller drives 16 GB of GDDR6 across a 256-bit bus,
shared by CPU and GPU (UMA) — the same physical DRAM at the same bandwidth. Eight
physical 2 GB GDDR6 chips, each with two independent 16-bit sub-channels; 16 sub-
channels × 16-bit = 256-bit. The UMC presents 8 channels (one per chip). amdgpu reports
the memory type as DDR4 (a truncated VBIOS ISI table) and DMI/SMBIOS reports 16 × 1 GB
devices (one per sub-channel) — the true type is GDDR6.

┌─ SPECIFICATIONS ─────────────────────────────────────────────────────────────────────┐
│  IP block    hw_id 150, v8.1.1 (2 instances; presents 8 channels)                    │
│  Capacity    16 GiB on-package GDDR6, 8 × 2 GB chips                                 │
│  Bus         256-bit, 8 channels (16 × 16-bit sub-channels)                          │
│  Apertures   VRAM carve-out (UMA Auto = 256 MiB) + GTT (16 GiB)                      │
└──────────────────────────────────────────────────────────────────────────────────────┘

Memory training is performed by the PSP via UMC firmware during early boot; timings come
from APCB tokens in the BIOS, not SPD. All 8 chips (16 sub-channels) are address-
interleaved by default (~2 KB granules); APCB Type 0x52 configures channel interleaving
= 4, bank interleaving = 4, and 6-way address hashing. Disabling interleaving breaks
GART page-table translation (SDMA page-faults on GTT access), so interleaving must
remain on.

Memory hubs — between the Data Fabric and the GPU/CPU memory paths (amdgpu_gmc):

Block      hw_id  Version  Role
─────────  ─────  ───────  ─────────────────────────────────────────────────────────────
MMHUB         34  2.0.3    System aperture (fast, full bandwidth) + frame-buffer
                           aperture
ATHUB         35  2.0.3    GART address translation for system-memory access
SYSTEMHUB    128  2.1.0    System-memory bridge between Data Fabric and MMHUB
HDP           41  5.0.1    Host Data Path — CPU-to-VRAM data movement

The MMHUB system aperture (MTYPE_CC, cacheable-coherent, snooped) is the fast path;
the frame-buffer aperture (MTYPE_UC, uncached) is the slower path.

Channel register map — address = 0x50000 + (instance << 20) + (channel << 13) +
register_offset. 8 channels (2 UMC instances × 4):

Ch  Inst  SubCh  SMN Base
──  ────  ─────  ────────
0   0     0      0x050000
1   0     1      0x052000
2   1     0      0x150000
3   1     1      0x152000
4   2     0      0x250000
5   2     1      0x252000
6   3     0      0x350000
7   3     1      0x352000

Address config (same across channels): 0x000 UmcCh_Enable = 0x00000001; 0x020 AddrMask =
0x003FFFFE; 0x030 AddrCfg = 0x00124008; 0x040 AddrSel = 0x24008765; 0x050 ColSel =
0xFDC76543; 0x090-0x0A0 Bank/Rank select. DRAM timing registers (same across channels,
format [timing_a:8][timing_b:8][reserved:8][enable:8]):

Offset  Value       Name      Decoded
──────  ──────────  ────────  ─────────────────
0x0C8   0x22220001  DramTmg0  tRAS=34, tRC=34
0x0CC   0x11110001  DramTmg1  tRCD=17, tRP=17
0x0D0   0x08888001  DramTmg2  tCL=8 + compound
0x0D4   0x04444001  DramTmg3  tRRD=4 + compound
0x0F4   0x00000001  DramCtrl  enabled

PHY calibration registers (trained independently per channel): 0xD00 PhyCtrl; 0xD04
PhyAddr; 0xD08 PhyData0; 0xD14 PhyCfg; 0xD20-0xD48 PhyCal0-4; 0xD6C PhyMisc =
0x000000F0; 0xDC4 PhyDll = 0x00011000; 0xDCC PhyTmg = 0x00000019; 0xDD0 PhyStrat =
0x0000020D. Relative to JEDEC GDDR6-2000 minimums, tRCD/tRP (17 vs 14) and tRAS (34 vs
28) carry margin; tCL and tRRD are at JEDEC minimum.

Global registers (SMN 0x01C000, 62 non-zero):

Offset       Value       Description
───────────  ──────────  ──────────────────────────────────────
0x000        0x13F01022  UMC device ID (1022:13F0 = Oberon UMC)
0x044        0x03971277  global timing config
0x080        0x00000281  mode register
0x098        0x28282829  refresh timing [0] (40-cycle base)
0x09C        0x28282828  refresh timing [1]
0x0C0        0x0000D003  power management config
0x0C4        0x0000F028  power management timing
0x300-0x32C  various     address range/decode (3 ranges)

UMC control per instance is at 0x053000 + (inst << 20); CONFIG_MEMSIZE at NBIO
seg2+0xC3 = 0x200 (512 MB VRAM carveout).

CMOS timing override — timing registers are write-locked after training; runtime SMN
writes to 0x0C8/0x0CC silently fail. Persistent changes go through the MemConf_t
structure in CMOS I/O space (ports 0x72 address / 0x73 data), which ABL reads before
training and applies as training targets if the signature and checksum are valid.

CMOS  Field       Size  Range/Notes
────  ──────────  ────  ───────────────────────────────────────────────────
0x90  Signature   4 B   0x42435041 = tool-written; 0x4C424124 = ABL-applied
0x94  Checksum    2 B   16-bit sum of 0x96-0xAB
0x96  ClockSpeed  2 B   450-1750 MHz
0x98  tCL         1 B   8-33
0x99  tRAS        1 B   21-58
0x9A  tRCDRD      1 B   8-27
0x9B  tRCDWR      1 B   8-27
0x9C  tRCAb       1 B   40-90
0x9D  tRCPb       1 B   0-11
0x9E  tRPAb       1 B   8-27
0x9F  tRPPb       1 B   0-11
0xA0  tRRDS       1 B   4-12
0xA1  tRRDL       1 B   4-12
0xA2  tRTP        1 B   0-14
0xA3  tFAW        1 B   4-34
0xA4  tREF        2 B   refresh interval
0xA6  RFCPb       2 B   refresh per bank
0xA8  tRFC        2 B   refresh cycle
0xAA  UMA_SIZE    2 B   UMA in MB

UMA carve-out — VRAM and GTT are software views onto one physical GDDR6 pool (no
PCIe hop, no faster local memory), so both deliver equivalent compute throughput.
The carve-out is fenced at boot by AGESA from an APCB token (4 MB granularity):

APCB value  UMA size  OS RAM remaining
──────────  ────────  ────────────────
0x20        128 MB    ~15.6 GB
0x40        256 MB    ~15.4 GB
0x80        512 MB    ~15.2 GB
0x100       1 GB      ~14.7 GB
0x200       2 GB      ~13.7 GB
0x400       4 GB      ~11.7 GB
0x800       8 GB      ~7.7 GB

The config chain is APCB flash defaults -> CBS overrides in the AmdSetup EFI variable ->
AGESA fences the region -> OS sees the final map (the UMA region is invisible in
/proc/iomem). The '1750 MT/s configured speed' reported by SMBIOS is the UMC controller
clock, not the GDDR6 data rate.

┌─ CAUTION ────────────────────────────────────────────────────────────────────────────┐
│  Never read the MMHUB SMN register range — reads hang the machine; recovery          │
│  requires a power cycle.                                                             │
│  Do not modify the address-config registers (0x000-0x0A0) or the PHY registers.      │
└──────────────────────────────────────────────────────────────────────────────────────┘

               See: Interconnect Fabric · Clock Domains · Chapter 4 (BIOS memory config)
```

## Interconnect Fabric

```
── INTERCONNECT FABRIC ──────────────────────────────────────── Data Fabric · SMN bus ──

The Data Fabric (DF 3.5.0, hw_id 46) is the coherent SoC spine at SMN base 0x00007000,
routing every CPU-GPU-memory-I/O transaction. Attached agents: the two CPU CCXs (via IF
links), the GPU GC block, the UMC memory controllers, and NBIO for root-complex / PCIe /
SMN routing. Coherency means any agent sees any other agent's DRAM writes without
explicit synchronization beyond the consumer's own coherence protocol — this is what
enables zero-copy unified memory. DF is exposed in PCI config space at 00:18.0-7.

  +----------+    +------------+    +----------------+
  |  Zen 2   |<-->| Data Fabric|<-->| RDNA 1.x GPU   |
  |  6 cores |    |  (DF 3.5)  |    |  40 CU         |
  +----------+    +-----+------+    +----------------+
                        |
                 +------+------+
                 |    NBIO     |  root complex + IOMMU + SMN gateway
                 | NBIF/IOHC   |
                 | PCIE/PCS    |
                 +------+------+
                        |
                   PCIe Gen4 x16

NBIO root complex — NBIO is the root complex plus IOMMU plus the SMN gateway between x86
and the internal register plane. NBIF (hw_id 108, v2.1.1) is host-facing; associated
blocks are IOHC (hw_id 24), the PCIE Gen 4 controller (hw_id 70, v4.2.0), and PCS (hw_id
80, v3.6.0). The internal PCIe link to the GPU trains at Gen 4 x16. The NBIO SMN-
indirect control engine at the PCI config-space window lets the host reach any IP
block's SMN-mapped registers, but does not bypass PSP/MP0 authorization — addresses in
the PSP/MP0 SMN window return 0xFFFFFFFF or zero.

SMN bus — the internal SoC register bus, reached from x86 through PCI config space
on device 0000:00:00.0:

    1. write the 32-bit SMN address to offset 0xB8  (index)
    2. read / write 32-bit data at offset 0xBC       (data)
       requires the iomem=relaxed kernel parameter

Works with or without amdgpu loaded; userspace access needs iomem=relaxed.

Landmark SMN addresses:

Address     Name             Value       Notes
──────────  ───────────────  ──────────  ──────────────────────
0x03810004  PSP_BOOT_STATUS  0x40000C25  bit 30 = booted OK
0x03800000  CCP_Q_MASK       0xFFFFFFFF  queues not initialized
0x0380000C  CCP_TRNG         (random)    true RNG output
0x03B10528  MP1_C2PMSG_10    0x02        SMU internal
0x03B1054C  MP1_C2PMSG_19    0x00018B66  bootloader info
0x03B10564  MP1_C2PMSG_25    0x01        SMU internal

PSP / CCP register windows (SMN- and BAR-reachable) — the PSP MP0 register window and
the CCP crypto block share BAR2 at 0xFE700000 (1 MB); the PSP MP0 window sits at
+0x10000..+0x10A6C (24 live registers when accessible). PSP MP0 C2PMSG registers report
platform status: MP0_C2PMSG_1 = 0x80000000 (PSP ready, bit 31), MP0_C2PMSG_26/27 =
0x001C0003 (TOS version 0.1C.0.3), MP0_C2PMSG_37 = 0x1FFED000 (ring buffer address). CCP
is documented below; PSP internals are in Chapter 3.

┌─ CAUTION ────────────────────────────────────────────────────────────────────────────┐
│  Never blind-scan SMN ranges — several regions (MMHUB among them) hang or fault      │
│  the machine on read. Read only documented-safe addresses.                           │
│  Never write FCLK from the OS. FCLK (the Data Fabric clock) is coordinated           │
│  internally by the SMU across CPU caches, GPU TLBs, and UMC training. A runtime      │
│  pp_dpm_fclk write bypasses that sequencing and hangs the entire coherent fabric     │
│  — no OS reboot path exists (the reboot syscall needs a coherent fabric), so a       │
│  physical power cycle is required. Reads are safe; writes are prohibited. No         │
│  persistent damage results (a cold boot re-trains FCLK).                             │
└──────────────────────────────────────────────────────────────────────────────────────┘

                        See: SMU / MP1 (SMN mailbox) · UMC / GDDR6 · Chapter 3 (CCP/PSP)
```

## SMUIO Power Islands

```
── SMUIO POWER ISLANDS ────────────────────────────────────── power islands · 8 tiles ──

SMUIO (hw_id 4, v11.0.8) is the SMU-controlled power-gate sequencer and pin-mux for 8
power-island tiles that gate CPU cores, the GPU, the memory controllers, and the
media/audio/display blocks (VCN, ACP, DCN). Only the SMU's dedicated internal bus can
write the tile-gate registers; every SMUIO register in the 0x5A000-0x5A400 range is
write-protected from all host buses (CPU MMIO, SMN indirect, GFX CP WRITE_DATA, SDMA,
direct BAR5) — writes are silently discarded. The host-visible gate bit is a
request/status flag, not the power switch: clearing it does not activate a rail, and it
auto-restores.

  SMUIO tile array (SMN 0x5A070-0x5A08C, bit 31 = powered on)
  ┌──────┬──────────┬────────────┬───────┬───────────────────┐
  │ Tile │ Address  │ Value      │ State │ Likely assignment │
  ├──────┼──────────┼────────────┼───────┼───────────────────┤
  │  0   │ 0x5A070  │ 0x80000002 │  ON   │ CPU / Fabric      │
  │  1   │ 0x5A074  │ 0x80000002 │  ON   │ CPU / Fabric      │
  │  2   │ 0x5A078  │ 0x80000041 │  ON   │ GPU GC            │
  │  3   │ 0x5A07C  │ 0x80000042 │  ON   │ GPU GC            │
  │  4   │ 0x5A080  │ 0x80000042 │  ON   │ Display / SDMA    │
  │  5   │ 0x5A084  │ 0x00000041 │  OFF  │ VCN (candidate)   │
  │  6   │ 0x5A088  │ 0x00000000 │  OFF  │ CCP queues        │
  │  7   │ 0x5A08C  │ 0x00010000 │  OFF  │ Unknown           │
  └──────┴──────────┴────────────┴───────┴───────────────────┘

Firmware power_gate_tile_up / power_gate_tile_down (Xtensa 0x000240D4 / 0x000241DC)
toggle these; the VCN power sequence would call power_gate_tile(3) and
power_gate_tile(4). VCN sits behind one or more gated tiles.

The wider SMUIO register plane spans two blocks. SMUIO_A (SMN 0x5A000): clock config
0x5A000-0x5A008, PLL/divider 0x5A00C-0x5A010, GPIO 0x5A01C, clock-domain status
0x5A050-0x5A05C, feature/capability masks 0x5A064-0x5A068, the 8 power-island tiles
0x5A070-0x5A08C, pin-mux 0x5A0AC-0x5A0E8 (0x01FF00FF ×4), THM config 0x5A0F0-0x5A0F4.
SMUIO_C (SMN 0x5C000): config 0x5C000, clock dividers 0x5C010-0x5C01C, reference 0x5C028
(0x64 = 100), thermal limits 0x5C034-0x5C038, VRM/power config 0x5C050-0x5C06C, power
delivery 0x5C080-0x5C09C, throttle config 0x5C0D0-0x5C0DC.

Consequence: no host-side path exists to power up a gated island (such as VCN);
enablement must originate inside SMU firmware or from an authenticated SMU-accessible
source such as the APCB.

                                                      See: SMU / MP1 · VCN · THM Thermal
```

## THM — Thermal Monitor

```
── THM ───────────────────────────────────────────────────── on-die thermal · v11.0.1 ──

THM (hw_id 3, v11.0.1, SMN base 0x00059800) is the on-die temperature-sensor aggregator
whose readings feed the SMU's thermal and PPT (package power tracking) policy loops. It
is one of the blocks amdgpu_smu manages alongside MP1/SMU, SMUIO, FUSE, CLKA, and CLKB.
Two SMU feature-mask bits govern the loop it feeds — TDC (Thermal Design Current) and
Thermal control — both enabled on shipping firmware. The CPU/GPU TctlMax setpoint is
reported by the SMU (q3 msgid 0x40) and is runtime-settable via q3 msgid 0x8B/0x8C.

The THM block also exposes control registers that the cyan_skillfish driver init path
leaves unprogrammed (see Unprogrammed Init-Path Registers): THM_TCON_HTC (hardware
TctlMax), THM_TCON_THERM_TRIP (thermal-trip shutdown threshold), THM_THERMAL_INT_CTRL /
_ENA (thermal interrupt routing), THM_GPIO_PROCHOT_CTRL (external PROCHOT pin),
THM_BACO_CNTL (BACO low-power), and THM_CTF_DELAY (critical-temperature-fault delay).

                See: SMU / MP1 (feature mask) · SMUIO · Unprogrammed Init-Path Registers
```

## Clock Domains

```
── CLOCK DOMAINS ────────────────────────────────────────────── CLKA / CLKB · SMU DPM ──

Clock generation is split between two IP blocks driven by the SMU. CLKA (hw_id 6,
v11.0.1) provides the three primary clock domains — GFX, SOC, and MEM — as three
instances; CLKB (hw_id 47, v11.0.1) is a secondary clock domain. Frequency selection is
arbitrated by SMU DPM policy, not directly host-programmable.

Domain  SMN base  Managed by          Notes
──────  ────────  ──────────────────  ──────────────────────────────────
GFXCLK  0x16C00   SMU DPM / governor  GPU core (sclk)
SOCCLK  0x16E00   SMU DPM             SoC / fabric-adjacent rail
MCLK    0x17000   SMU DPM             memory (single 450 MHz state)
LCLK    0x17E00   SMU DPM             PCIe link clock
FCLK    —         SMU DPM (internal)  Data Fabric — OS writes prohibited

DPM tables — the set of allowed operating levels, fixed at SMU firmware build time and
changed only by a firmware-image replacement. Which level is active is governed moment-
to-moment by SMU/governor policy — the DPM scheduler runs on MP1, reads the voltage
curve, and refuses any transition whose curve voltage is below the AVFS-calibrated
silicon minimum.

Rail             DPM table               Notes
───────────────  ──────────────────────  ────────────
sclk (GPU core)  {1000, 2000, 2230} MHz  3 states
mclk (memory)    {450} MHz               single state

The driver OverDrive interface presents a three-level sclk table (1000 / 1500 / 2000
MHz, DPM levels 0/1/2); the SMU governor ceiling is 2350 MHz. Additional DPM rails exist
as sysfs files (pp_dpm_fclk, pp_dpm_socclk, pp_dpm_dcefclk, pp_dpm_pcie); reading them
is safe, but writing pp_dpm_fclk is prohibited.

┌─ CAUTION ────────────────────────────────────────────────────────────────────────────┐
│  Writing pp_dpm_fclk hard-hangs the coherent fabric (see Interconnect Fabric).       │
│  GPU clock changes must go through ForceGfxFreq or the OD voltage curve, never a     │
│  direct clock write.                                                                 │
└──────────────────────────────────────────────────────────────────────────────────────┘

          See: SMU / MP1 (V/F chain) · Interconnect Fabric (FCLK) · GPU Silicon Identity
```

## Display — DMU / DIO / DAZ (headless)

```
── DISPLAY ─────────────────────────────────────────────── DMU / DIO / DAZ · headless ──

The display pipeline is present in silicon but never brought up: the board runs headless
with amdgpu.dc=0, so no display driver attaches. DisplayPort 1.4 output exists on the
board I/O and the DMU silicon is functional, but the display core is skipped at driver
init.

┌─ BLOCKS ─────────────────────────────────────────────────────────────────────────────┐
│  DMU (DCN)   hw_id 271, IP v2.0.3 — display core, DP 1.4; disabled (dc=0)            │
│  DIO         hw_id 272, IP 127.127.63 — display I/O; harvested/unused slot           │
│  DAZ         hw_id 274, IP 127.127.63 — harvested/unused slot                        │
└──────────────────────────────────────────────────────────────────────────────────────┘

IP version 127.127.63 (all bits set) marks an unused or harvested slot. The
amdgpu_display module (dm.c) is skipped entirely when dc=0.

                         See: APU Identity & IP-Block Catalog · Chapter 5 (Driver Stack)
```

## ACP — Audio Co-Processor

```
── ACP ─────────────────────────────────────────────────────────── audio co-processor ──

An on-die audio DSP, present but unused on the headless board — no audio endpoints are
wired on the carrier.

┌─ BLOCK ──────────────────────────────────────────────────────────────────────────────┐
│  Block      ACP — hw_id 14, IP v4.0.0                                                │
│  SMN base   0x02400000                                                               │
│  State      Present, unused (headless)                                               │
└──────────────────────────────────────────────────────────────────────────────────────┘

                                                    See: APU Identity & IP-Block Catalog
```

## USB — On-Die Controllers

```
── USB ────────────────────────────────────────────────── on-die controllers · v4.5.0 ──

Two on-die USB controllers sit on the SoC alongside the discrete FCH's USB stack. Board
USB is served by both the on-die controllers and the A68H FCH (XHCI / EHCI / OHCI) — see
Chapter 1.

┌─ BLOCK ──────────────────────────────────────────────────────────────────────────────┐
│  Block   USB — hw_id 170, IP v4.5.0, 2 instances                                     │
└──────────────────────────────────────────────────────────────────────────────────────┘

                              See: Chapter 1 (FCH USB) · APU Identity & IP-Block Catalog
```

## CCP — Cryptographic Co-Processor

```
── CCP ───────────────────────────────────────────────────── crypto · AES/SHA/RSA/RNG ──

The CCP provides hardware AES, SHA, RSA, and a true random-number generator. It shares
BAR2 with the PSP and appears as its own PCI function; the TRNG is usable independently
of the command-queue block. Full security context is in Chapter 3.

┌─ BLOCK ──────────────────────────────────────────────────────────────────────────────┐
│  PCI           1022:143E at 01:00.2 (AES / SHA / RSA / RNG)                          │
│  BAR2          0xFE700000, 1 MB (shared with the PSP MP0 window)                     │
│  Cmd queues    +0x000..+0xFFF — read 0xFFFFFFFF (queues not initialized)             │
│  TRNG_OUT      +0x00C — 32-bit hardware random output                                │
│  SMN aliases   CCP_Q_MASK 0x03800000 · CCP_TRNG 0x0380000C                           │
└──────────────────────────────────────────────────────────────────────────────────────┘

The CCP TRNG works independently of command-queue initialization.

                 See: Interconnect Fabric (SMN landmarks) · Chapter 3 (Security & Trust)
```

## Debug & Test Blocks — DBGU / DFX / DAP

```
── DEBUG & TEST BLOCKS ───────────────────────────────────────────── DBGU / DFX / DAP ──

Four debug and design-for-test blocks are present on the die but uninvestigated; the
external JTAG landing (J2) is unpopulated on the carrier.

┌─ BLOCKS ─────────────────────────────────────────────────────────────────────────────┐
│  DBGU_NBIO   hw_id 36, IP v3.0.0 — NBIO debug unit                                   │
│  DBGU_IO     hw_id 45, IP v3.0.0 — I/O debug unit                                    │
│  DFX         hw_id 37, IP v2.0.0 — design-for-test fabric tap                        │
│  DFX_DAP     hw_id 49, IP v2.0.0 — debug access port (JTAG)                          │
└──────────────────────────────────────────────────────────────────────────────────────┘

             See: APU Identity & IP-Block Catalog · Chapter 1 (Debug & Recovery Headers)
```

## Firmware Map

```
── FIRMWARE MAP ─────────────────────────────────────────────────── 14 blobs · 6 ISAs ──

The BC-250 loads 14 firmware blobs across 6 ISAs; the PSP loads them at boot (LOAD_IP_FW
= PSP ring command 0x06). Rewritable engines carry a LOAD_IP_FW type; PSP-resident
firmware (TOS, DRIVER_ENTRIES, SEC_GASKET, UMC) loads before the ring is available and
is replaceable only via SPI flash.

Canonical per-IP firmware versions (BIOS P3.00; change on reflash or fw update):

┌─ CANONICAL VERSIONS ─────────────────────────────────────────────────────────────────┐
│  ME / PFP / CE     0x63 / 0x94 / 0x25                                                │
│  RLC               0x0D                                                              │
│  MEC / MEC2        0x90                                                              │
│  SMC (SMU)         0x00580600 = 88.6.0                                               │
│  SDMA0 / SDMA1     0x34   (feature version 50 / 0x32)                                │
│  VCN / UVD / VCE   not loaded                                                        │
│  SOS / ASD / TA    not loaded (no TA firmware on this part)                          │
│  VBIOS             113-AMDRBN-003                                                    │
└──────────────────────────────────────────────────────────────────────────────────────┘

Firmware blob inventory:

Engine                       Size     ISA            Version    LOAD_IP_FW type
───────────────────────────  ───────  ─────────────  ─────────  ───────────────
CP_ME (graphics cmd)         ~257 KB  AMD F-code     0x63       1
CP_PFP (pre-fetch parser)    ~257 KB  AMD F-code     0x94       2
CP_CE (constant engine)      ~257 KB  AMD F-code     0x25       3
CP_MEC (compute dispatch)    ~262 KB  AMD F-code     0x90       4
CP_MEC2 (compute secondary)  ~262 KB  AMD F-code     —          5/6
RLC_G (run list controller)  ~43 KB   RLC microcode  0x0D       8
SDMA0                        ~33 KB   SDMA ISA       0x34       9
SDMA1                        ~33 KB   SDMA ISA       0x34       10
VCN 2.0 (video codec)        ~395 KB  VCN microcode  —          13
SMU (power mgmt)             ~262 KB  Xtensa LX      88.7.1     18
PSP TOS (secure OS)          ~77 KB   ARM Thumb-2    0.1C.0.3   BIOS flash only
PSP DRIVER_ENTRIES (TEE)     ~106 KB  ARM Thumb-2    —          BIOS flash only
PSP SEC_GASKET               ~12 KB   ARM Thumb-2    —          BIOS flash only
UMC (memory controller)      ~64 KB   UMC microcode  81.2.12.0  BIOS flash only

CP microcode jump-table sizes: MEC = 66,752 entries; ME/PFP/CE = 65,536. The VBIOS
(113-AMDRBN-003, 55296 bytes) is not stored decompressed in flash — AGESA generates it
at boot from PSP-provided data and exposes it via the ACPI VFCT table; its hash is byte-
identical across all factory BIOS versions.

SPI flash layout — two SPI NOR chips: BIOS = Winbond W25Q128JVSQ (16 MiB, designator
BIOS_A1, quad-capable); Super I/O = Macronix MX25L4006E (512 KiB, designator SIO1_R,
feeds the NCT6686D — do not flash, no recovery). External programming via the populated
2.54 mm J4004 SPI header.

Region                                 Address   Size     Content
─────────────────────────────────────  ────────  ───────  ──────────────────────────────
PSP region (incl. AmdSetup live data)  0x000000  ~700 KB  PSP firmware, ABL, AmdSetup
                                                          bytes at 0x4CF + 0x261B
PSP Directory 0                        0x8E0000  ~300 KB  SOS, SMU, ABL, keys
BIOS Directory                         0xAB0000  ~16 KB   APCB, APOB, BIOS copy
AMI NVRAM FV                           0xAE0000  3.2 MB   EFI_SYSTEM_NV_DATA_FV_GUID,
                                                          nested compressed FFS, DXE
                                                          drivers
UEFI BIOS Image                        0xE02000  2 MB     main UEFI boot image

PSP directory (16 entries, by flash address):

Type                   Address   Size         Version    Status
─────────────────────  ────────  ───────────  ─────────  ───────────────────────────────
PSP Trusted OS (SOS)   0x8EBA00  78 KB        0.1C.0.3   verified
SMU Firmware           0x8FEE00  262 KB       88.7.1     verified
SEC Debug Public Key   0x93F000  1 KB         1          key_missing
SMU Firmware 2 (copy)  0x93F500  262 KB       88.7.1     verified
Debug Unlock           0x981900  8 KB         29.1C.0.3  verified
Hardware IP Config     0x982000  1.6 KB       0.0.0.1    verified
SEC Gasket             0x984F00  12 KB        B.51.0.16  verified
Driver Entries         0x99F700  107 KB       0.1C.0.3   verified
ABL0                   0x99FD00  1.3->7.9 KB  22.4.6.0   9 funcs, APCB validator
ABL1                   0x9ABE00  49->92 KB    22.4.6.0   150 funcs, APCB token processor
ABL2                   0x9AFF00  16->30 KB    22.4.6.0   108 funcs, secondary init
ABL3                   0x9BA200  42->85 KB    22.4.6.0   170 funcs, DF/DRAM map
ABL4                   0x9C4400  41->92 KB    22.4.6.0   165 funcs, UMC training
FW XHCI                0x9CAB00  0.5 KB       0.0.0.1    verified
TOS Security Policy    0x9CB200  1.7 KB       B.51.1.16  verified
UMC Firmware           0x9DB400  66 KB        81.2.12.0  verified
BL Public Key          0x9DC200  3.5 KB       1          key_missing
TOS Public Key         0x9DC200  1.8 KB       1          key_missing

The BIOS directory (4 entries): APCB at 0xAB1000 (8 KB, unsigned, 235 CBS tokens); APOB
(runtime, dest 0x4000000); BIOS Image at 0xE02000 (2 MB, copies to 0x9E02000); APOB NV
copy at 0xAB3000 (8 KB).

ABL boot-loader stages — all AGESA runs on the PSP (ARM Cortex-A5) before any x86
code executes, loaded as ABL 0-4 (zlib inside $PS1, ARM Thumb-2), 592 funcs, v22.4.6.0.

Stage  Comp    Decomp  Funcs  Role
─────  ──────  ──────  ─────  ──────────────────────────────────────────────────────────
ABL0   1.3 KB  7.9 KB  9      APCB header validator; reads APCB at 0xAB1000
ABL1   49 KB   92 KB   150    APCB token processor; GDDR6 DRAM training (no SPD, config
                              from APCB)
ABL2   17 KB   30 KB   108    memory test, topology discovery
ABL3   42 KB   85 KB   170    DF memory map, UMA/DRAM config; CPU + chipset init
ABL4   41 KB   92 KB   165    UMC training, memory timing; releases x86 reset vector

ABL reads UMC timing overrides from CMOS (0x90-0xAB) before training. EFI variables:
Setup (GUID EC87D643-...) lives in the AMI NVRAM FV, BS-only; AmdSetup (GUID
3A997502-...) is PSP-managed (<= 0x20000), RT-writable via efivarfs. Factory versions:
AGESA V9 RBNBDK-BL5 46.1.2.211126 (live board, P3.00, Nov 2021); a community variant
P5.00 uses AGESA 46.1.2.220426. Platform Secure Boot (PSB) is not enforced — the BIOS
directory is missing PSB entries (0x05, 0x64, 0x65), the APCB is unsigned, and there is
no anti-rollback; SOS and SMU signatures are valid. The VCN 3-byte patch site is BIOS
offset 0x91D5BC (Xtensa 0x1E5BC).

                 See: SMU / MP1 · VCN · Chapter 3 (Security) · Chapter 4 (System Config)
```

## BAPM / CAC Power Model

```
── BAPM / CAC POWER MODEL ────────────────────────────────── power model · CAC / AVFS ──

BAPM (Bidirectional Adaptive Power Management) is the closed-loop power-allocation
algorithm running on the MP1 SMU. It decides in real time how much of the package budget
goes to CPU vs GPU, when to throttle, and when to permit transient overshoot. Per-block
CAC (Cache Access Counter) activity counters are weighted into a dynamic-power estimate,
integrated over a control window, compared against caps, and actuated through AVFS.

  CAC activity counters (per GFX / L3 / CPU block)
     │  × static CACW weights
     ▼
  Σ( counter_i × CACW_i ) + Σ( bapm_param_j × signal_j )
     │  integrate over control window
     ▼
  estimated_dynamic_power
     │  compare vs caps: TDC, EDC, STAPM, fast-PPT / slow-PPT
     ▼
  AVFS actuation (target rail voltage at clock) → throttle / allow

The estimate is workload-dependent: identical silicon at identical clocks reports
different modelled power depending on which counters dominate. BAPM does not itself
change the V/F curve tables and is independent of the slow-PPT / fast-PPT / EDC /
PROCHOT gates, which are separate enforcement paths.

Two halves — a static half and a dynamic half. On this firmware the static-half
tables are loaded once by boot firmware and do not move at runtime (they stay fixed
under sustained load); the only live actuator surface is the q3 AVFS path.

Half     Surface           Members
───────  ────────────────  ─────────────────────────────────────────────────────────────
Static   q0 / host-driver  GFX CACW, L3 CACW, CPU DPM_WAC
Dynamic  q3 mailbox        AVFS curve scaler, VID offsets, droop calibration, perf-
                           profile, current/temperature caps

Model coefficient inputs:

Item         Count     Element                         Channel
───────────  ────────  ──────────────────────────────  ───────────────────────────
GFX CACW     88 slots  u32 packed weight               q0 msgid 0x2F
L3 CACW      80 slots  u32 packed weight               q0 msgid 0x30
BAPM param   176       IEEE-754 float (enable, value)  q0 msgid 0x33
CPU DPM_WAC  21        u64                             MSR 0xC0011076 / 0xC0011077

GFX CACW table (q0 msgid 0x2F) — 88 SRAM slots; only indices 0..63 are serviced. Dual-
mode: write = param (index OR 0x20000 BAPM_INDEX_FLAG) + extra (new weight in
C2PMSG_81), returns resp 0x01; read = param (index, no flag), returns the weight in arg.
The 0x20000 flag marks the param as a CACW index; without it the write is rejected.
Entries are packed multi-byte structures (e.g. 0x0028001E = {hi 0x0028, lo 0x001E}).
Firmware-default non-zero weights (PMFW 88.6.0) — only 15 of 88 indices are non-zero:

idx  value       idx  value
───  ──────────  ───  ──────────
0    0x00000017  23   0x0007019D
2    0x000000A1  24   0x0028001E
5    0x003F0000  28   0x00000035
14   0x01C80000  29   0x0000015D
19   0x00000022  30   0x000001AB
20   0x00000012  31   0x007F0000
35   0x000000C4  49   0x00310000
54   0x00000076

L3 CACW table (q0 msgid 0x30) — 80 slots (0..79), identical dual-mode protocol. A single
firmware-default non-zero slot: idx 71 = 0x3FF80000 (the high half 0x3FF8 is close to
the top word of IEEE-754 double 1.5, suggesting a fixed-point fraction with implicit
scaling).

CACW SRAM allocations are larger than the serviced ranges: GFX is 88 slots but only
0..63 are valid (64); L3 is 80/80. Reads or writes past the valid range return resp 0xFF
(FAIL), surfacing as EIO through a kernel driver — consumers must clamp iteration to the
valid counts, not the allocation lengths.

BAPM coefficient table (q0 msgid 0x33) — a 176-entry array of IEEE-754 single-precision
floats scaling the workload-signal terms, written one entry at a time (param = index,
extra = float reinterpreted as u32; entries with write_flag=0 are skipped, ~140 of 176
written in the reference set). On BC-250 PMFW 88.6.0 this handler is stripped: the
message returns resp 0xFE (UNKNOWN_CMD) with no read-back path, so the coefficient
surface is a reference-lineage-only surface unreachable on this firmware.

CPU DPM_WAC accumulator (MSR) — the CPU complex feeds the model through its own
workload-activity-counter table, addressed via MSRs on each core:

MSR                    Address     Purpose
─────────────────────  ──────────  ─────────────────────────────────────────────────────
MSR_DPM_CFG            0xC0011074  gating register; bit 62 (MSR_ACCESS_DIS) must be
                                   cleared to unlock writes
MSR_DPM_WAC_ACC_INDEX  0xC0011076  selects one of 21 accumulator slots
MSR_DPM_WAC_DATA       0xC0011077  64-bit value for the selected slot (8 packed bytes)

  Programming sequence (per logical CPU):
    rdmsr MSR_DPM_CFG -> cfg
    wrmsr MSR_DPM_CFG = cfg & ~BIT(62)      ; unlock
    for i in 0..20:
        wrmsr MSR_DPM_WAC_ACC_INDEX = i
        wrmsr MSR_DPM_WAC_DATA      = dpm_wac_table[i]
    wrmsr MSR_DPM_CFG = cfg                 ; restore (re-locks)

AVFS f_vid curve scaler (q3 msgid 0x50) — scale_f_vid_curve is the fully characterized
live AVFS lever: a signed 16-bit multiplicative scale on the AVFS activity-vs-rail-
voltage curve, the final stage that maps integrator output to target rail voltage at a
given clock. It scales the activity contribution (deltas from a reference operating
point), so per-unit rail effect is small at idle and much larger under load; gain is
asymmetric between positive and negative scale values because the underlying curve is
non-linear. Large-magnitude excursions latch the AVFS integrator into an elevated state.

The canonical way to read the SMU's live runtime state (integrator, accumulators,
throttle status) is the SetDriverTableDramAddrHigh/Low + TransferTableSmu2Dram sequence
(q0 msgids 0x04/0x05/0x06), which DMAs the metrics table to system memory.

┌─ CAUTION ────────────────────────────────────────────────────────────────────────────┐
│  Bound the q3 AVFS curve scaler (msgid 0x50) to +/-10. A large excursion latches     │
│  the integrator stuck-high; writing scale 0 afterward does not fully relax it —      │
│  only a power cycle resets AVFS state — and in that state any clock-transition       │
│  message wedges the chip.                                                            │
│  The q0 coefficient tables are a static calibration input, not a runtime control;    │
│  the live lever is the q3 AVFS actuator surface, and no SMU mailbox traffic may      │
│  occur while GPU compute is running (see SMU / MP1).                                 │
└──────────────────────────────────────────────────────────────────────────────────────┘

                          See: SMU / MP1 (queue-3 actuators) · CPU — Zen 2 · THM Thermal
```


# 3. Security & Trust

```
── SECURITY & TRUST ───────────────────────────────────────────── PSP · root of trust ──

The Platform Security Processor — an on-die ARM Cortex-A5 that runs ahead of the x86
cores — is the root of the BC-250's firmware trust chain: it owns secure boot, key
management, and firmware loading.  This chapter maps the trust model as it is actually
configured on this silicon.

      reset
        │
        ▼
  ┌──────────────────────────────────┐
  │  PSP MP0 · ARM Cortex-A5         │   on-die ROM decrypts the
  └──────────────────────────────────┘   IKEK-wrapped PSP Boot Loader
        │
        ▼
  ┌──────────────────────────────────┐   AGESA boot stages (run on the PSP):
  │  ABL0 → ABL1 → ABL2 → ABL3 →     │   DRAM init, memory training, APCB
  │  ABL4                            │   parse, DF + UMC-key setup
  └──────────────────────────────────┘
        │
        ▼
  ┌──────────────────────────────────┐
  │  Trusted OS (SOS)                │   RSA-2048 signed, SVC-dispatch kernel
  └──────────────────────────────────┘
        │   x86 release
        ▼
  ┌──────────────────────────────────┐
  │  UEFI (BIOS P3.00)  →  OS        │   x86 cores released last
  └──────────────────────────────────┘

The board is a harvested, fused-non-secure Oberon part: Platform Secure Boot is not
enforced, the SPI flash carries no host-side write protection, and the AGESA
configuration block is unsigned — yet two always-fatal enforcement layers (the $KDB
key stores and the RSA-signed Trusted OS) make naive firmware modification a permanent
brick, and the host-facing Trusted-Application load path is stubbed shut.  What
follows documents each block, what it enforces, and — as importantly — what it does
not.

        See: PSP (MP0) — The Four-Layer Trust Chain — The Trusted OS — Platform Security
                                                                                 Posture
```

## PSP (MP0) — Platform Security Processor

```
── PSP (MP0) ───────────────────────────────────────────── ARM Cortex-A5 · root agent ──

The PSP is an ARM Cortex-A5 (MP0) embedded in the APU, executing before the x86 CPU.
It is the firewall controller for the internal register bus, runs a Trusted OS from
SPI flash, and is the only agent with unrestricted access to per-die fuses and key
material.

┌─ PROCESSOR ──────────────────────────────────────────────────────────────────────────┐
│  Core              ARM Cortex-A5 (ARMv7-A) — Thumb-2 primary                         │
│  IP version        11.0.8                                                            │
│  MP0 SMN base      0x00016800                                                        │
│  MP0 IP identity   SMN 0x00017800 = 0x30110704 (IP major version 11)                 │
│  Linux driver      psp_v11_0_8 (amdgpu) — stripped-down, ring-only                   │
│  Firmware at rest  Trusted OS (SOS) + ABL stages 0-4, in the $PSP directory          │
└──────────────────────────────────────────────────────────────────────────────────────┘

The Linux psp_v11_0_8 variant binds only when IP_VERSION(11,0,8) and apu_flags &
AMD_APU_IS_CYAN_SKILLFISH2, and implements just 5 of the 16 PSP driver functions —
ring_create, ring_stop, ring_destroy, ring_get_wptr, ring_set_wptr.  The eleven
firmware-loading routines of the full v11.0 driver (init_microcode,
bootloader_load_kdb/spl/sysdrv/sos, mem_training, mode1_reset, load_usbc_pd_fw,
read_usbc_pd_fw, wait_for_bootloader) are absent, because the pre-boot AGESA + PSP
chain bootstraps the processor before the OS loads.  No Trusted Application is loaded
through amdgpu on this platform: a Trusted-App invoke can reach the ring, but the
prerequisite SOS + KDB + TA-image loader chain is gone.  ring_create uses
C2PMSG_64..71; ring_stop uses C2PMSG_64; ring_get_wptr/ring_set_wptr use C2PMSG_67.

A live PSP is observable read-only from the host through the amdgpu register BAR5 (512
KiB).  The MP0/MP1 C2PMSG block carries the boot-ready bit, firmware versions, ring
state, a boot-stage magic, and a heartbeat.

  MP0_C2PMSG_n : dword (0x16000 + 0x0040 + n) → byte offset ×4
  MP1_C2PMSG_n : dword (0x16000 + 0x0240 + n) → byte offset ×4

Reg        Value                    Meaning
─────────  ───────────────────────  ────────────────────────────────────────────────────
MP0 33     0x80000000               PSP boot-ready bit
MP0 58     0x001c0002               FW version (mirrors TOS header +0x60)
MP0 64     0x80020000               ring-type: ack bit OR host ring-kind
MP0 67/68  0x000000b0               ring-wptr
MP0 69/70  0xfffe0000 / 0x000000f5  ring-addr (0x000000f5_fffe0000)
MP0 71     0x00001000               ring-size (4096 B)
MP0 73     0xd007be11               PSP-published boot-stage magic
MP0 81     increments (~100 Hz)     heartbeat / liveness
MP0 91     0x0b510016               last PSP status word
MP1 38     0x00580600               SMU firmware version

When the host reads MP0_C2PMSG_73 == 0xd007be11, TOS has reached its publish point — a
precise boot-timeline anchor.  A freeze of the MP0_C2PMSG_81 heartbeat signals a hung
PSP, an amdgpu-independent watchdog.  This is a read-only observability channel; it
does not perturb the PSP.

The C2PMSG_46 register reports firmware and security flags after boot (a historical
read returned 0x000100FD), read via GPU BAR5 rather than SMN indirect.  Bit 3
PSP_SECURE_BOOT_EN records only that the secure-boot fuse is programmed, not that
enforcement is armed — booting an unsigned Boot Loader cleanly proves enforcement does
not fire on non-secure parts.

Bit  Flag                     Meaning
───  ───────────────────────  ──────────────────────────────────────────────────────────
0    PSP_FW_LOADED            firmware loaded and running
2    PSP_TEE_READY            trusted execution environment initialized
3    PSP_SECURE_BOOT_EN       fuse programmed — enforcement NOT firing here
4    PSP_FW_VERIFIED          firmware verified during boot
5    PSP_ANTI_ROLLBACK_EN     anti-rollback counters
6    PSP_DEBUG_LOCKED         debug-unlock fuse blown
7    PSP_PLATFORM_KEYS_FUSED  platform key hash programmed
16   PSP_SECURE_BOOT_PLAT     platform secure-boot status

A second host-visible window, the MP9 SMN region 0xC000-0xCFFF, mirrors PSP-internal
state (feature mask, TA loaded-state markers, digest/measurement clusters, TMR
allocations).  Anchors: 0xC180 = 0xFEDCBADF (DEBUG_MAGIC), 0xC71C = 0x07FFFFFF
(FEATURE_MASK, 27 TA-feature bits), and five 0x40000000 TMR allocations (1 GiB each).
The published TA registry shows 13 of those 27 feature TAs loaded; the digest cluster
(0xC480..0xC4B8) drifts between reads, so it holds runtime measurements, not stable
identity hashes.  The region is host-writable but scratch-only: writes are accepted at
the SMN bus and produce no observable cascade in PSP behavior.  It is a forensic
readout, not an actuation surface — the real TMR bases live in PSP private memory,
reached only through the MP0 mailbox.

┌─ CAUTION ────────────────────────────────────────────────────────────────────────────┐
│  The PSP configures an SMN bus firewall during boot that gates x86 access to         │
│  selected control registers; the PSP is itself the firewall controller and it        │
│  cannot be removed from the host side.                                               │
└──────────────────────────────────────────────────────────────────────────────────────┘

    See: The Four-Layer Trust Chain — The Trusted OS — PSP Directory & Firmware Layout —
                                                   Platform Fuses — SMU (MP1), Chapter 2
```

## The Four-Layer Trust Chain

```
── FOUR-LAYER TRUST CHAIN ────────────────────────── L1 inert · L2/L3 fatal · L4 stub ──

The PSP boot chain is built from four independent validation mechanisms.  This model
is not documented in available AMD references; it was characterized empirically.  On
this fused-non-secure silicon, Layer 1 does not fire and Layer 4 is stubbed — the two
layers that remain fatal are $KDB integrity (Layer 2) and $PS1 firmware signatures
(Layer 3).

┌─ L1  Boot ROM Root Key — DOES NOT FIRE ──────────────────────────────────────────────┐
│  On-die ROM validates BL_PUBLIC_KEY (0x50) against a silicon-fused root key.  An     │
│  unsigned, plaintext Boot Loader boots cleanly here.                                 │
└──────────────────────────────────────────────────────────────────────────────────────┘
                   │  inert on non-secure silicon
┌─ L2  $KDB Key-Store Integrity — ALWAYS FATAL ────────────────────────────────────────┐
│  Validates all $KDB blobs (dir 0x50, 0x51), independent of PSB and $PS1.  Any        │
│  modification is non-recoverable in software.                                        │
└──────────────────────────────────────────────────────────────────────────────────────┘
                   │
┌─ L3  $PS1 Firmware Signatures — FATAL FOR TOS ───────────────────────────────────────┐
│  RSA-2048 + SHA-256 on blobs mapped SPI → PSP SRAM.  Enforced fatally for TOS        │
│  (proven).                                                                           │
└──────────────────────────────────────────────────────────────────────────────────────┘
                   │
┌─ L4  DRIVER_ENTRIES Ring Commands — STUBBED ─────────────────────────────────────────┐
│  Would validate ring-loaded TAs/ASDs.  LOAD_TA and LOAD_ASD return an error          │
│  constant; the RSA verification code is unreachable.                                 │
└──────────────────────────────────────────────────────────────────────────────────────┘

Two empirical facts anchor the "non-secure" posture.  First, an AMD-internal Boot
Loader with signed_flag=0, an all-zero SHA-256 header field, no RSA tail, and a
plaintext body boots through POST, UEFI, and Linux — proving the Boot ROM performs no
BL signature or encryption check on this part.  BL encryption/signing is an OEM
packaging step, not a code difference: the BL code bytes are byte-identical between
internal (plaintext, entropy 6.66) and public (AES, entropy 8.00) builds, as is every
other non-BL firmware component.  Second, Platform Secure Boot is not enforced: no OEM
key is fused and the PSB-critical directory entries are absent.  Even so, the Boot
Loader itself still RSA-2048-verifies TOS, SMU, the ABLs, and the OEM trustlet before
loading them — the "non-secure" property is confined to the ROM→BL edge.

      See: PSP (MP0) — The Trusted OS — Trust-Chain Keys & the $KDB Key Store — Platform
                                                                                   Fuses
```

## The Boot Chain & ABL Stages

```
── BOOT CHAIN & ABL STAGES ─────────────────────────── on-die ROM → BL → ABL0-4 → TOS ──

On reset the on-die ROM decrypts the IKEK-wrapped PSP Boot Loader, which drives the
five AGESA Boot Loader stages, returns to itself, then loads the signed Trusted OS
that hosts Trusted Applications reachable from the host GFX ring.

  on-die ROM → PSP_BL (IKEK-encrypted, self-decrypts in ROM)
    → ABL0 (dispatcher) → ABL1 → ABL2 → ABL3 → ABL4 → ABL0 → PSP_BL
    → TOS (PSP_SECURE_OS, RSA-2048 signed, unencrypted at rest)
    → TA[0..N] loaded via LOAD_TA sig verifier (RSA-2048 + SHA-256)
    → reachable from host x86 via GFX ring buffer

The PSP Boot Loader (dir type 0x01) is ARM Cortex-A5 (ARM-mode init then Thumb-2),
~247 functions.  Its reset handler configures the MMU, caches, and exception vectors,
then switches to a Thumb-2 C runtime.  It derives seven key types from the hardware-
fused Platform Derivation Secret and exposes two signature-verification entry points:
an internal $PS1 verifier at 0x5C90 for its own loads, and a separate export at
0x11B04 that TOS calls.  PSP_BL is the unique directory blob with no $PS1 envelope —
it is encrypted-then-signed rather than signed-only, so offline static analysis is
infeasible without the IKEK.

┌─ PSP BOOT LOADER ────────────────────────────────────────────────────────────────────┐
│  Directory type   0x01                                                               │
│  Architecture     ARM Cortex-A5 — ARM-mode init, then Thumb-2                        │
│  Size (public)    43,008 B (0xA800) — AES-encrypted $PS1 wrapper, entropy 8.00       │
│  Size (internal)  39,616 B (0x9AC0) — plaintext                                      │
│  Function count   247 Thumb-2 + ARM-mode init                                        │
│  Encryption       IKEK-derived (AMD on-die fuse); no offline analysis                │
└──────────────────────────────────────────────────────────────────────────────────────┘

The BL retail path performs five distinct writes/checks, of which only one is a true
security check.  Gate 3 (fn 0x944) is a hardware-fingerprint validator that panics on
mismatch: it reads 0x03200048 (must equal 0xBC0B02A0, a 32-bit silicon fingerprint
appearing exactly once in the firmware set) plus bit-field checks on SRAM 0x0005D7AC
(bits 10-13 = 0b0111) and 0x0005D5A4 ((value>>6) == 0x5E).  Gates 1/2/4/5 prepare the
HDT mailbox rather than enforcing security.

The five AGESA Boot Loader stages run on the PSP in ARM Thumb-2, before TOS, on the
PSP-bootloader SVC ABI (0x00-0x7f range, sharing only platform SVCs 0xf0 and 0xf3 with
the Trusted OS).  ABL3 hosts the APCB parser.

Stage  Dir   Size (live)  Fns  Role
─────  ────  ───────────  ───  ─────────────────────────────────────────────────────────
ABL0   0x30  1,088 B      9    Minimal bootstrap / dispatcher
ABL1   0x31  48,848 B     150  Early memory init, APCB ingestion
ABL2   0x32  16,240 B     108  Memory training / DRAM setup, UMC LM32 microcode
ABL3   0x33  41,312 B     170  APCB parser + config-block processing, DF init, APOB
                               handoff
ABL4   0x34  40,928 B     165  Final init, UMC encryption keys, DF security widget, x86
                               handoff

  All stages share a uniform, minimal hardening profile:

┌─ HARDENING PROFILE ──────────────────────────────────────────────────────────────────┐
│  Architecture     ARM Thumb-2 (Cortex-A5)                                            │
│  Stack canary     Static 0xDEAD5555 — NOT randomized                                 │
│  NX (no-execute)  NOT enabled (stack executable)                                     │
│  ASLR             NOT enabled                                                        │
│  Bounds check     Compiler-inserted halt ("ABL3 StackCheck failed!")                 │
│  Version code     0x21112600 (2021-11-26) on ABL0-4                                  │
└──────────────────────────────────────────────────────────────────────────────────────┘

ABL bodies are AES-CBC encrypted; the signer fingerprint 663ca522... covers plaintext-
header + encrypted-body integrity.  ABL3's APCB parser is the subject of
CVE-2021-26344; in this AGESA the classic overflow primitive was not found because
copy callers bound their destinations with compiler-inserted checks.  ABL3's top
helper (0x01be4, 433 callers) is a variadic ASSERT/log_panic — 4x the call density of
the busiest TOS helper, indicating ABL3 is defensively coded.  Cross-stage calls land
in the PSP boot ROM aperture at SMN 0xffb00000, which exposes shared helpers (memcpy,
crypto primitives, key derivation).

ABL4 is the security-relevant final stage.  It programs UMC memory-encryption keys via
Svc_ProgramUmcKeys (SVC 0x21 — the only SVC in the BIOS with a verifiable name,
recovered by log-string back-walk); the AddrTweakEn flag indicates AMD SME-style XEX-
mode encryption with address-derived tweaks.  ABL4 also configures the Data Fabric
security widget (TrustZone enforcement, memory protection regions, APCB-driven), the
DRAM scramble key, APOB/memory-map HMAC generation, and the single-die APOB handoff to
PSP_BL.  The ABL stages run with full PSP privilege and are each $PS1-signed (verified
by PSP_BL before load), so they are not a softer target than TOS: code injection would
require signature forgery, a TOCTOU race against the sig check, or an ABL logic bug.

The PSP_BL SVC namespace (inferred from ABL caller sites; PSP_BL handler bodies are
encrypted at rest) spans 25 distinct SVCs across ABL1-ABL4 (ABL0 issues none): notably
0x26/0x27 (73 sites each, likely begin/lock and end/unlock), 0x1b (28 sites, memcpy or
SMN R/W), 0x0a (21, malloc/log), 0x05/0x06 (16 each, paired), and 0x21 in ABL4 only.

┌─ CAUTION — BL swap bricks the board ─────────────────────────────────────────────────┐
│  The Boot Loader is NOT a drop-in component across BIOS images.  Even though the BL  │
│  code bytes are byte-identical between internal and public builds, swapping an       │
│  internal plaintext BL into a stock public BIOS (zero-padding the size difference)   │
│  permanently bricks the board (no POST).  A given BL works only within its own       │
│  complete BIOS image; never transplant a BL between builds.                          │
│  Recovery requires an external SPI programmer with a known-good backup.              │
└──────────────────────────────────────────────────────────────────────────────────────┘

 See: The Trusted OS — APCB — Memory Encryption — CVEs & Public Research — PSP Directory
                                                                       & Firmware Layout
```

## The Trusted OS (TOS) & SVC Dispatch

```
── TRUSTED OS · SVC DISPATCH ──────────────────────────── dir 0x02 · ~82 KB · Thumb-2 ──

The PSP's Trusted OS (dir type 0x02, ~82 KB, ~396 functions) is an SVC-dispatch kernel
in ARM Thumb-2.  It contains no RSA implementation of its own — no 0x10001 exponent,
no modular-exponentiation or bignum routines — and delegates signature verification
back to the Boot Loader.  Its $PS1 parser at +0x12644 reads the header and SHA-256;
the verify site at +0x12684 branches to the BL export at 0x11B04.  Because that is a
different export than the BL-internal 0x5C90 verifier, patching 0x5C90 alone does not
bypass TOS-context verification.

┌─ TRUSTED OS ─────────────────────────────────────────────────────────────────────────┐
│  Directory type    0x02                                                              │
│  Size (this BIOS)  82,768 B (0x14350)                                                │
│  Architecture      ARM Thumb-2 (LE, v7), Cortex-A5                                   │
│  Function count    396 (327 PUSH-with-LR prologues, ~32% coverage)                   │
│  Kernel stack      0xB000                                                            │
│  SVC vector        +0x108 → handler +0x0240 → dispatcher +0x4544                     │
└──────────────────────────────────────────────────────────────────────────────────────┘

The SVC handler at +0x0240 saves user context, switches to the kernel stack, sets DACR
= 0x55555555, extracts the SVC immediate, and calls the Thumb dispatcher, which routes
through a three-way range gate:

  SVC# 0x00-0x50   → no handler; error handler +0x501A returns 9
  SVC# 0x51-0xD0   → mid-range 128-entry PC-relative jump table +0x46E0
  SVC# 0xE0-0xEF   → perm-bit-gated handler +0x45B8
  SVC# 0xF0-0xF5   → high-range TBB table +0x506A

Of the 128 mid-range slots, 95 point to unique handlers and 33 route to the shared
error handler.  Named SVCs include the SPI/FCH mapping and kernel-SMN primitives that
dominate the trust surface:

SVC   Handler     Name / role
────  ──────────  ──────────────────────────────────────────────────────────────────────
0x60  0x49D6      SVC_GET_SPI_INFO
0x61  0x49CC      SVC_MAP_SPIROM_DEVICE
0x62  0x49E8      SVC_UNMAP_SPIROM_DEVICE
0x63  0x4A90      SVC_MAP_FCH_IO_DEVICE
0x64  0x4ACC      SVC_UNMAP_FCH_IO_DEVICE
0x65  0x4AFC      SVC_UPDATE_PSP_BIOS_DIR
0x66  0x4B1A      SVC_COPY_DATA_FROM_UAPP
0x6B  via 0x4CC8  memory-map from SPI flash (triggers $KDB/$PS1 validation)
0x75  0x4D9A      restricted-policy-only SVC
0x7B  0x4E1E      kernel SMN read (4 B)
0x7C  0x4E32      kernel SMN write (4 B)
0xA6  0x4FCC      kernel masked SMN write
0xF2  via 0x5050  trustlet SMN write
0xF3  via 0x5050  trustlet SMN read

Privilege policy is the low nibble of perm_table[priv*8] (table at +0x69B0, populated
at runtime): values 5 or 6 = restricted (only SVC 0x75 allowed, all else denied with
error 0xE); any other value = unrestricted.  Two structural findings define the
runtime posture.  SVC 0x1F is unhandled: because 0x1F < 0x51, the bounds check
underflows and routes to the error handler returning 9 — no low-range dispatcher
exists (this is what makes firmware debug unlock inert).  SVC 0xF3 is inert: on this
TOS it loads constant 0x00F00100 and returns, performing no MMIO — so the SVC-mediated
MMIO access control seen in newer AMD firmware is absent, and ABL stages retain
unrestricted direct MMIO access.

The extended (0xf0-0xf5, dispatcher FUN_4F48, 6 entries) and restricted (0xe0-0xe9,
dispatcher FUN_44B8, 10 entries) tables form a TA-self-management API: a TA may read
its own descriptor slots {0x18,0x1c} and write {0x14,0x01,kernel_state[0x4d]}, but
field_0 (the class/valid flag) is excluded by design.  SVC 0xf2's sub-dispatch table
at body+0x8430 is all zeros (dead) — which cascades to render 14 of the 18 host ring
commands inert (their shims all call SVC 0xf2).  All seven Thumb TBB dispatcher sites
in the body:

TBB addr  Function       Entries  Role
────────  ─────────────  ───────  ──────────────────────────────────────────────────────
0x44d0    FUN_44B8       10       restricted SVC dispatcher
0x4f5e    FUN_4F48       6        extended SVC dispatcher
0xe4e2    FUN_E38A       16       crypto-algo sub-cmd selector (sig verifier)
0x1057e   FUN_1043C      37       host ring-buffer command dispatcher
0x1286c   FUN_1281E      8        TLV/algo tag processor
0x12844   FUN_1281E sib  16       sub-cmd routing
0x12eee   FUN_12EBC      11       LOAD_TA verifier state machine

  User-pointer validators — 93 catalogued runtime SVCs (dispatcher VA 0x2045a0):

Validator   N   SVCs sharing / role
──────────  ──  ────────────────────────────────────────────────────────────────────────
FUN_0x158c  17  0x67,0x69,0x6d,0x6f,0x71,0x7b,0x7f,0x80,0x82,0x84,0x89,0x8f,0x90,0x9f,0x
                a7,0xa9,0xd0 — user-pointer range validator
FUN_0x15c0  12  0x51,0x52,0x6b,0x72,0x77,0x78,0x8b,0x98,0x9b,0x9e,0xa4,0xab —
                validation-then-dispatch helper
inline      17  pure accessors

FUN_158c looks up the per-PID permission word at *(PTR + pid*8); if mode==4 and the
pointer is inside the TOS-internal windows [0x200000,0x2fffff] or [0xe00000,0xeffffff]
it DENIES, else if the pointer is in [0x200000, 0x10200000) (256 MB DRAM) it ALLOWS,
else DENIES — via a 32-bit unsigned-subtraction wraparound test (param_2 - 0x200000U <
0xfe00001).  It validates only the start address, not the buffer length.  A
complementary kernel-VA check FUN_0x2cb8 runs a branch-predictor barrier then checks
the page table at 1 MB granularity.  The per-PID permission table (0x69b0, 32 x 8 B)
carries a mode nibble in word0 and a sign-bit "blocked" flag in word1.  SVC 0x89
(FUN_2ac8) is a phys-region mapping primitive that validates only its R1 output
pointer while consuming an unvalidated R0 options struct — but a secondary check
(FUN_5810) constrains unprivileged callers to page-aligned windows.

The only contact the runtime SVC ABI has with hardware state is a paired
interrupt/event-mask SET (FUN_05858) / CLEAR (FUN_031f8) API over a 256-bit mask at
SMN 0x03200200-0x032002fc.  A write-isolation property holds architecturally: 0 of 11
MP0_C2PMSG publish sites are reachable from either runtime entry surface — all 11 live
in boot/init code, so the C2PMSG mailbox is a boot-only publish channel and runtime
access is read-only observation.

Debug unlock is a dead path.  SVC 0x1F (Svc_GetDebugUnlockInfo) — the first call the
DEBUG_UNLOCK trustlet issues — is unhandled, so the firmware debug-unlock path never
runs; the parallel kernel-mode unlock routine in DRIVER_ENTRIES has zero references
anywhere in the firmware set.  Both are fully scaffolded with no live invocation edge.

In-band TA-identity forgery is closed.  The caller_id byte (kernel_state[0x4b]) has
zero writers in the entire TOS body via any tested idiom, and TA_descriptor field_0 is
set only by the LOAD_TA chain (post-signature-check) and the boot allocator — both
outside the SVC entry path.  An audit of 15 candidate writer functions and every USR-
reachable write primitive found all gate USR-controlled destinations through the
FUN_158c/FUN_15c0 range validators, which exclude PSP kernel RAM.  No in-band path
defeats the kernel-identity check; the remaining avenues are out-of-band (PSP_BL
static analysis blocked by IKEK, AGESA pre-boot, hardware TOCTOU, side-channel, SPI
boundary substitution).

The host ring-command surface is 4 live of 18.  Only LOAD_TA (0x01), UNLOAD_TA (0x02),
INVOKE_CMD (0x03), and TMR_PROG (0x23) are functionally live; the other 14 are dead
shims that call the inert SVC 0xf2.  LOAD_TA runs a real RSA-2048 + SHA-256 verifier
(FUN_11A04); INVOKE_CMD is blocked behind that sig-gate.  The TOS_SECURITY_POLICY blob
(type 0x45, signed key 96EA, ~1.7 KB) holds an 8-byte-entry register access whitelist
governing the PROG_REG ring command (0x0B) — and empirically all 256 PROG_REG register
IDs return 0xFFFF0009 (access-denied) because PROG_REG requires a loaded ASD, and the
ring-only driver never loads one (a cross-signed ASD load returns firmware validation
error 0x0000000F).

    See: The Boot Chain & ABL Stages — Debug Interface Exposure — Trust-Chain Keys & the
                                $KDB Key Store — DRIVER_ENTRIES — CVEs & Public Research
```

## Debug Interface Exposure (JTAG / HDT)

```
── DEBUG INTERFACE EXPOSURE ───────────────────────────────── JTAG / HDT · dead paths ──

The board exposes an unpopulated JTAG / HDT+ header and a signed firmware debug-unlock
trustlet, but neither yields a working debug session on shipped silicon: hardware
debug is gated behind the PSP, and every firmware route to open it is either unhandled
or dead code.  Firmware-only debug unlock is architecturally impossible on this
platform.

┌─ DEBUG SURFACE ──────────────────────────────────────────────────────────────────────┐
│  JTAG / HDT+   J2 — 20-pin, 1.27 mm, UNPOPULATED (Chapter 1)                         │
│  Debug-lock    C2PMSG_46 bit 6 PSP_DEBUG_LOCKED (fuse-programmed flag)               │
│  DEBUG_UNLOCK  dir 0x13, 8,512 B, signed key 9F0D, PSP SRAM 0x54000                  │
│  HDT mailbox   SMN 0x03200070 ctrl / 0x03200074 data / 0x0320046C clock              │
│  Global HDT    SMN 0x03200010 — per-IP permission bits                               │
│  Commit regs   0x03A10064 SecureUnlock · 0x03010058 ASIC-lock · 0x5E004 HDT          │
└──────────────────────────────────────────────────────────────────────────────────────┘

DEBUG_UNLOCK (type 0x13) is a signed Cortex-A5 Thumb-2 trustlet, byte-identical
between stock and internal builds.  Its container is a $PS1 header (256 B) at
file+0x00, an 8,000 B body at +0x100, and a signature trailer at +0x2040, header
signer key-ID 9F0D3830 9C0F4509 993B7781 CF643EF3.  main() (body+0xE84) issues svc
#0x1F (Svc_GetDebugUnlockInfo) to obtain an UnlockMode, then dispatches through a TBB
table at body+0xEE6.  SecureUnlock_Commit (body+0xBF8) would engage HDT via register
0x5E004 (bits 7/8), run the IP-block unlock scan, stage a token at 0x032001D4, then
set bit 31 on both 0x03A10064 and 0x03010058.  The IP-block whitelist (body+0x1C52)
admits nine IDs {0x0C,0x0E,0x0F,0x10,0x12,0x22,0x23,0x28,0x29}; the HDT-state
permission map (body+0x1C2C) further restricts these so at most four
(0x0C,0x0F,0x10,0x22) can ever unlock, and only when HDT has set the matching bit in
0x03200010.  The internal UnlockNegotiation stub (body+0x1C82) is two instructions
returning 0 — there is no in-trustlet challenge-response.

  Two independent facts make the path dead:

  - TOS has no SVC 0x1F handler.  Because 0x1F < 0x51, the dispatcher bounds-check
underflows and routes to the error handler (returns 9), so main() always takes its
FAIL exit before any UnlockMode runs.

  - The DRIVER_ENTRIES kernel mirror is unreferenced.  SecureUnlock_Commit_Kernel
(body+0x666C) and IP_Block_Scan_Kernel (body+0x11008) use live kernel SVCs
(0x7B/0x7C/0xA6) against the real commit registers, but an exhaustive 4-byte-aligned
pointer search across every extracted PSP blob and the full BIOS finds zero
references — compiled but never invoked.

The Mode 2/3 HDT challenge-response mailbox (fn body+0x1804) is fully decoded but
never executes: a 5-step validation chain over three stack buffers, a per-round
primitive at body+0x167C (2-second timeout 0x77359400), and an RSA-2048 crypto core
(body+0x1A94) that signs an 8-byte nonce.  Forging a valid response would require the
PSP device RSA-2048 private key — not software-feasible.  All debug primitives are
present (signed module, kernel SVCs, SMN commit targets, IP whitelists) but the
invocation edges are absent.

┌─ CAUTION ────────────────────────────────────────────────────────────────────────────┐
│  The J2 header is unpopulated, and even fitted the HDT path is PSP-gated and inert   │
│  on shipped firmware; no host-side action opens a debug session.  The DACR =         │
│  0x55555555 set in the TOS SVC prologue grants the kernel full domain access — a     │
│  runtime property, not a debug entry point.                                          │
└──────────────────────────────────────────────────────────────────────────────────────┘

   See: The Trusted OS (SVC 0x1F) — DRIVER_ENTRIES — Debug & Recovery Headers, Chapter 1
```

## DRIVER_ENTRIES (PSP Ring-Command Processor)

```
── DRIVER_ENTRIES ────────────────────────────────────── dir 0x28 · Layer 4 · stubbed ──

DRIVER_ENTRIES (dir type 0x28, 108,400 B, ~649 functions, load base 0xE00000, signed
with key 5A35 — the same signer as TOS) is the PSP's ring-command processor and Layer
4 of the trust chain.  Its two most security-relevant handlers are hardcoded stubs,
which is why in-band Trusted-App loading is unreachable.

┌─ DRIVER_ENTRIES ─────────────────────────────────────────────────────────────────────┐
│  Directory type    0x28                                                              │
│  Size (this BIOS)  108,400 B (0x1A770)                                               │
│  Architecture      ARM Thumb-2 (LE, v7)                                              │
│  Function count    649                                                               │
│  Load base         0xE00000                                                          │
└──────────────────────────────────────────────────────────────────────────────────────┘

Cmd                Handler       Status
─────────────────  ────────────  ───────────────────────────────────────────────────────
0x01 LOAD_TA       FUN_00e0e450  STUB — returns 0xFFFF000A (4-byte LDR, no verify)
0x04 LOAD_ASD      FUN_00e0e29c  STUB — returns 0xFFFF000A
0x05 SETUP_TMR     —             implemented
0x06 LOAD_IP_FW    —             WORKING — the only usable ring-path FW loader
0x08 SAVE_RESTORE  —             implemented
0x0B PROG_REG      —             implemented (requires ASD context)

Both stub handlers are 4-byte functions (a single load from a constant pool) — no
branch, no signature check, no parameter parsing.  A complete RSA/PKCS verification
apparatus exists in the binary but is unreachable from the ring interface: 0xE10374
(RSA PKCS#1 v1.5), 0xE10704 (RSA-PSS), 0xE044C0 (modular exponentiation), 0xE0A0F0
(key lookup), 0xE0AD74 (constant-time memcmp), 0xE0B298 (RSA key setup, exponent
0x10001).  Consequently, defeating Layer 4 in software would require modifying
DRIVER_ENTRIES itself — which triggers Layer 3 $PS1 verification against the AMD-held
5A35 key.

      See: The Trusted OS — The Four-Layer Trust Chain — PSP Directory & Firmware Layout
```

## Trust-Chain Keys & the $KDB Key Store

```
── TRUST-CHAIN KEYS · $KDB ────────────────────── $PS1 containers · always-fatal keys ──

Every PSP firmware blob is wrapped in a $PS1 signed/encrypted container.  Verification
keys live in two $KDB key-store blobs that are always-fatal to modify.

Off      Sz   $PS1 header field (0x100 bytes)
───────  ───  ──────────────────────────────────────────────────────────────────────────
0x00     16   Pre-header (zeros in unsigned/debug builds)
0x10     4    "$PS1" magic (24 50 53 31)
0x14     4    signed_size / total size
0x18     4    Encryption flag (0 = plaintext)
0x20     4    signed_flag (0 = unsigned, 1 = signed)
0x30     16   Signing key ID / fingerprint
0x40     16   IV (if encrypted)
0x48     4    Body size
0xD0     32   SHA-256 of body (over [0x100 .. 0x100+body_size])
0x100    N    Body (firmware code)
0x100+N  256  RSA-2048 signature (if signed, PKCS#1 v1.5)

The hash at header +0xD0 is not a plain SHA-256 — the Boot Loader's verify_module
routine validates it as a CCP HMAC-SHA-256 using a hardware-derived key.  This is why
patching a compressed ABL and re-hashing with a standard tool fails to boot: a
partially-modified original is rejected, while an entirely different full-replacement
body passes only because the RSA gate is bypassed on non-secure silicon.  Faithful
modification of a signed blob is not possible without the hardware key or the AMD
signing key.

  The two $KDB containers hold the RSA public keys for downstream verify:

Blob            Dir   Size     Contents
──────────────  ────  ───────  ─────────────────────────────────────────────────────────
BL_PUBLIC_KEY   0x50  3,536 B  11 RSA-2048 keys
TOS_PUBLIC_KEY  0x51  1,856 B  5 RSA-2048 keys

Each key carries a 16-byte key-ID fingerprint referenced by the $PS1 signing-key-ID
field at header 0x30.  RSA public exponent throughout is 0x10001.  The signer-to-
firmware-class map:

Key-ID  Signs
──────  ────────────────────────────────────────────────────────────────────────────────
5A35    PSP Trusted OS (0x02), DRIVER_ENTRIES (0x28)
8B8D    SMU firmware (0x08, 0x12)
9F0D    DEBUG_UNLOCK trustlet (0x13)
537B    Hardware IP Config (0x20)
96EA    SEC Gasket, TOS Security Policy (0x24 / 0x45)
9ACE    FW_XHCI (0x44)
39BF    UMC firmware (0x4F)
663C    ABL0-4 (0x30-0x34)
FFC8    key-store blobs / SEC_DBG_PUBLIC_KEY (validated by $KDB, not $PS1)

Full fingerprints: TOS/DRIVER_ENTRIES signer 5A3587B5 9F4A4537 A4C4C877 DB0EF505;
DEBUG_UNLOCK signer 9F0D3830 9C0F4509 993B7781 CF643EF3; silicon-fused AMD root
FFC8ACD45BB748E0... (there is no dir entry 0x00 — the root pubkey is silicon-fused
only).

  The BL derives seven usage-tagged keys from the fused Platform
  Derivation Secret (PDS); only AES-128/192/256 sizes are accepted:

ID  Size     Purpose
──  ───────  ───────────────────────────────────────────────────────────────────────────
0   AES-256  Platform Derivation Secret (PDS base)
1   AES-256  HMAC key for PSP data in DRAM
2   AES-256  Encryption key for PSP data in DRAM
3   AES-256  NV Storage encryption key (off-chip)
4   AES-256  Inline AES key-wrapper key
5   AES-256  HMAC key for APOB data
6   AES-128  iKEK (special-case AES-128)

The TOS_PUBLIC_KEY entry (0x51) is itself a $PS1-signed $KDB store at flash 0x9DBB00
(0x740 B), holding 3 TOS key slots of 0x150 B each (moduli at 0x9DBCA0 / 0x9DBDF0 /
0x9DBF40, 256 B LE).  Only TOS[1] (9ace85d9b8084270af98d5483e11d800) backs a static
signature chain (the FW_XHCI USB-PHY chain); TOS[0] and TOS[2] have no static consumer
but are re-verified at PSP runtime.  That runtime re-verification is the operative
gate: modifying a TOS key-slot modulus in place does not stop PSP_BL (the outer $PS1
signature is a container-level check at the BL phase, not a boot-stop), but when TOS
goes to consume any key from the $KDB it re-verifies the envelope hash — so any
modification to any key body breaks the hash and every subsequent key lookup fails.
Flash-time pubkey-body replacement is not viable without re-signing the envelope
against the FFC8 silicon-fused root, for which no private key exists.

┌─ CAUTION — $KDB / TOS modification is hardware-fatal ────────────────────────────────┐
│  Any modification to a $KDB key-store blob (dir type 0x50 or 0x51) is hardware-      │
│  fatal and non-recoverable in software — even a single bit flip with the $PS1        │
│  SHA-256 correctly recalculated, because the Layer-2 integrity mechanism is          │
│  independent of that field.  Two boards were destroyed this way (one by modifying    │
│  BL_PUBLIC_KEY, one by flipping a single modulus bit in TOS_PUBLIC_KEY).             │
│  Tooling such as psptool produces a valid-looking image that passes its own re-      │
│  verification but still bricks real hardware — tool-level verification is necessary  │
│  but not sufficient.  Modifying the TOS body is equally fatal: the BL                │
│  RSA-2048-verifies the TOS $PS1 container, so SHA-256 recalculation cannot rescue a  │
│  modified TOS.  Recovery requires an external SPI programmer with a known-good       │
│  backup.                                                                             │
└──────────────────────────────────────────────────────────────────────────────────────┘

     See: PSP Directory & Firmware Layout — The Trusted OS — SPI Flash Security & Write-
                                                                                 Protect
```

## PSP Directory & Firmware Layout

```
── PSP DIRECTORY & FIRMWARE ──────────────────────────── $PSP @ 0x8E0000 · 19 entries ──

The Oberon SPI image carries a single $PSP directory at flash offset 0x8E0000 with 19
entries, each a 16-byte record (type, size, in-flash offset).  All firmware blobs are
$PS1 signed containers; key-store blobs (0x50/0x51) are $KDB containers and are
always-fatal to modify.  There is no L2 directory in this BIOS.

Type  Name                  Size           Version     Notes
────  ────────────────────  ─────────────  ──────────  ─────────────────────────────────
0x01  PSP_FW_BOOT_LOADER    39,616/43,008  0.1C.1.2    Unsigned+plaintext internal;
                                                       AES+signed public
0x02  PSP_FW_TRUSTED_OS     82,768         0.1C.0.2    Signed key 5A35
0x08  SMU_OFFCHIP_FW        262,656        0.58.6.0    Signed key 8B8D
0x09  AMD_SEC_DBG_PUBKEY    1,088          1           Silicon-validated debug key
                                                       (fuse-gated)
0x12  SMU_OFFCHIP_FW_2      262,656        0.58.6.0    Backup SMU (same FW, not a
                                                       revision)
0x13  DEBUG_UNLOCK          8,512          29.1C.0.x   Signed key 9F0D
0x20  HARDWARE_IP_CONFIG    1,616          0.0.0.1     IP-block config
0x24  SEC_GASKET / SEC_POL  11,856         B.51.0.16   Security gasket
0x28  DRIVER_ENTRIES        108,400        0.1C.0.2    Signed key 5A35
0x30  ABL0                  1,088          21.11.26.0  AGESA boot stage 0
0x31  ABL1                  48,848         21.11.26.0  Memory init
0x32  ABL2                  16,240         21.11.26.0  DRAM training
0x33  ABL3                  41,312         21.11.26.0  APCB parser host
0x34  ABL4                  40,928         21.11.26.0  x86 handoff
0x44  FW_XHCI / MP2_CFG     ~0.5-26 KB     0.0.0.1     Unified USB PHY FW (Synopsys DWH
                                                       C10G; not a trustlet)
0x45  TOS_SECURITY_POLICY   1,760-1,856    B.51.1.16   Signed key 96EA
0x4F  UMC_FW / OEM_TL       65,536-66,048  81.2.12.0   Memory-controller FW (signer
                                                       39BF); opaque body
0x50  BL_PUBLIC_KEY         3,536          1           $KDB key store — NEVER MODIFY
0x51  TOS_PUBLIC_KEY        1,856          1           $KDB key store — NEVER MODIFY

Version-coupling: ABL0-4 all carry 0x21112600 (2021-11-26, matching AGESA
46.1.2.211126); SMU sections 1+2 both 0x00580600; key tables 0x50+0x51 both
0x301c0001; TOS 0x02 and DRIVER_ENTRIES 0x28 both 0x001c0002.  Entries absent versus
stock AMD AGESA: 0x10 SEV firmware, 0x14 SEV-PSP variant, 0x39 SEV app (SEV is dead-
on-arrival on stock BC-250 BIOS), 0x3C Unified DRTM (no DRTM measurement chain),
0x36/0x38 PSP-OEM data / signing key.

  BIOS Header Directory (BHD, base 0xAB0000) — 4 entries:

Type  Name              Flash off          Size   Notes
────  ────────────────  ─────────────────  ─────  ──────────────────────────────────────
0x60  APCB              0xAB1000           8,192  Unsigned, 1-byte checksum
0x63  APOB_NV_COPY      0xAB3000           8,192  Non-volatile AGESA output
—     APOB              runtime 0x4000000  —      RAM destination
—     BIOS (x86 reset)  0xE02000           ~2 MB  Copied to 0x9E02000

Two build variants exist (stock P3.00 and AMD-internal P3.00.AMD); in-flash offsets
differ per variant while the type/name map is stable.  The psp_v11_0_8 driver reserves
a 4 MiB PSP Trusted Memory Region at MC (GMC) address 0xf40f_8000_0000 — a GMC-mapped
GDDR6 carve-out fenced by TMR enforcement, not host-physical RAM (host RAM ends at
0x4_5fff_ffff); the host x86 cannot reach it through normal page tables.

The host talks to the already-running PSP through the amdgpu GPCOM ring buffer (in
VRAM) and the C2PMSG mailbox.  Ring commands (psp_gfx_if.h): LOAD_TA 0x01, UNLOAD_TA
0x02, INVOKE_CMD 0x03, LOAD_ASD 0x04, SETUP_TMR 0x05, LOAD_IP_FW 0x06, DESTROY_TMR
0x07, SAVE_RESTORE 0x08, PROG_REG 0x0B, GET_FW_ATTESTATION 0x0F, LOAD_TOC 0x20,
AUTOLOAD_RLC 0x21, BOOT_CFG 0x22.  Control commands via C2PMSG_64: INIT_RBI_RING
0x00010000, INIT_GPCOM_RING 0x00020000, DESTROY_RINGS 0x00030000, CAN_INIT_RINGS
0x00040000, ENABLE_INT 0x00050000, DISABLE_INT 0x00060000, MODE1_RST 0x00070000.  PSP
ring commands require GART-mapped GPU virtual addresses (the PSP accesses memory
through the GPU memory controller / VMID-0 page tables, not raw physical DMA).  On
this firmware, GET_FW_ATTESTATION returns success while LOAD_TA and BOOT_CFG return
not-supported/stub status.

      See: Trust-Chain Keys & the $KDB Key Store — DRIVER_ENTRIES — SPI Flash Security &
                                                                    Write-Protect — APCB
```

## Platform Fuses

```
── PLATFORM FUSES ─────────────────────────────── SMN via PCI 00:00.0 · un-programmed ──

Per-die identity and security fuses live in SMN space, addressed via PCI 00:00.0
config-space indirect (write the address to offset 0xB8, read data from 0xBC).  On
this cut-down non-secure silicon the identity/security ranges read all-zero because
the bits are un-programmed, while other SMN control registers return real values.

SMN range        N   Live value              Purpose (Family 17h reference)
───────────────  ──  ──────────────────────  ───────────────────────────────────────────
0x5D000-0x5D034  14  all 0x00000000          SOC fuses; 0x5D000 = secure state,
                                             0x5D004-0x5D00B = 64-bit chip serial
0x5D048-0x5D058  5   all 0x00000000          Security/PSB fuses (incl. PSB_STATUS at
                                             0x5D04C)
0x5D100-0x5D12C  12  all 0x00000000          Identity fuses (layout unverified)
0x59800-0x5980C  4   real (e.g. 0x717b0fef)  THM (thermal) area

PSB_STATUS at SMN 0x5D04C — the OEM public-key / Platform Secure Boot status fuse —
reads 0x00000000, indicating no OEM key is fused.  This is the foundational posture
behind "PSB not enforced": the BIOS directory is missing every PSB-critical entry
(0x05 BIOS public key, 0x64 BIOS RTM signature, 0x65 OEM signing key), so no vendor
signature is required for the image to boot.

An earlier "firewalled fuses" narrative is falsified on this part.  x86 with
CAP_SYS_RAWIO reads non-zero fuse/boot-control registers directly — e.g. SMN[0x5D800]
= 0xb08007c2 and SMN[0x59800] = 0x717b0fef.  The documented identity range reads zero
because it is un-programmed, not because it is firewalled.  The 0x72FB0FEF value
documented elsewhere as a fuse or firewall sentinel is the SMN bus access-denied
response for a firewalled register (the PSP control aperture 0x03xxxxxx returns it),
not a fuse value; the superficially similar 0x717b0fef at 0x59800 is a legitimate live
THM register.

Security implication: any hardware-bound encryption scheme on this silicon cannot rely
on "fuse data unextractable from x86" — a key derived from any non-zero readable fuse
is extractable from Linux userspace via the PCI config-space indirect path.  Genuine
per-die identity secrets (secure state, chip serial) remain inside PSP-only fuse
ranges that the PSP firewalls from the host; only PSP-side code can read them.

                  See: PSP (MP0) — The Four-Layer Trust Chain — Memory Encryption — APCB
```

## Memory Encryption — SEV / SEV-ES / SME

```
── MEMORY ENCRYPTION ──────────────────────────────── SEV DOA · SME-style DRAM active ──

The Zen 2 cores enumerate SEV and SEV-ES in CPUID, but the platform ships no SEV
firmware: the PSP directory is missing every SEV blob, so guest memory encryption is
dead on arrival on the stock BIOS.  What is active is AMD SME-style transparent DRAM
encryption, established by ABL4 before the x86 cores are released.

┌─ ENCRYPTION STATE ───────────────────────────────────────────────────────────────────┐
│  CPU capability   SEV, SEV-ES enumerated in CPUID                                    │
│  SEV firmware     Absent — dir 0x10 / 0x14 / 0x39 not present (SEV DOA)              │
│  DRTM             Absent — dir 0x3C not present (no DRTM measurement chain)          │
│  DRAM encryption  ABL4 Svc_ProgramUmcKeys (SVC 0x21) → UMC keys                      │
│  Mode             SME-style XEX (XOR-Encrypt-XOR); AddrTweakEn = address tweak       │
│  Scramble key     DRAM scramble (anti-snoop / anti-row-hammer XOR)                   │
│  Key custody      Derived + programmed PSP-side; not host-visible                    │
└──────────────────────────────────────────────────────────────────────────────────────┘

ABL4 also generates APOB / memory-map HMACs and installs the Data Fabric security
widget (TrustZone enforcement, memory-protection regions, all APCB-driven).  The UMC
key source is the highest-value structural surface in the boot chain — a race between
key derivation and the Svc_ProgramUmcKeys call would install known keys — but the ABL
stages run with full PSP privilege and are $PS1-signed, so this is a conclusion about
the trust boundary, not a reachable path on stock firmware.

Note that on the live board the kernel command line carries amd_iommu=off and
mitigations=off: without IOMMU DMA isolation, a GPU or other DMA master can reach any
host-physical page.  This is a deployment-configuration observation, not a memory-
encryption property.

┌─ CAUTION ────────────────────────────────────────────────────────────────────────────┐
│  DRAM encryption keys are established before x86 release and are never exposed to    │
│  the host; there is no host-side path to read or set them.                           │
└──────────────────────────────────────────────────────────────────────────────────────┘

       See: The Boot Chain & ABL Stages (ABL4) — Platform Fuses — CPU — Zen 2, Chapter 2
```

## APCB — AMD PSP Configuration Block

```
── APCB ────────────────────────────────────────── @ 0xAB1000 · unsigned · 235 tokens ──

The APCB is the primary configuration data the ABL stages parse before x86 reset.  It
is parsed by ABL3 on the PSP and is unsigned — guarded only by a trivial single-byte
checksum — which makes it the most accessible configuration surface on the platform.

┌─ APCB ───────────────────────────────────────────────────────────────────────────────┐
│  Location     BIOS offset 0xAB1000 (BHD type 0x60)                                   │
│  Magic        "APCB" (0x41504342), version 0x20                                      │
│  Allocated    8,192 bytes (0x2000)                                                   │
│  Used size    2,920 bytes (header-encoded 0xB68)                                     │
│  Slack space  5,272 bytes of trailing 0xFF                                           │
│  Format       V2                                                                     │
│  Signature    Not required                                                           │
│  Integrity    1-byte checksum at header offset 0x10                                  │
└──────────────────────────────────────────────────────────────────────────────────────┘

APCB size and slack are BIOS-version-specific.  A token word is 32-bit little-endian:
bits 20:8 = token_id (13-bit), bits 23:21 = size field (1-8 value bytes), low byte(s)
= value; terminator token_id = 0x1FFF.  The same parser appears in ABL1 and ABL3 (one
AGESA source).  Tokens are organized in six groups (235 descriptors in the CBS
Extended group):

Group         Group ID  Type        Content
────────────  ────────  ──────────  ────────────────────────────────────────────────────
DF Fabric     0x1703    0x05/0x06   Data Fabric config (0x0301-0x0308)
FCH/Chipset   0x1706    0x0B/0x0C   FCH (0x1C01 / 0x1C02)
CBS Debug     0x1707    0x0D        CBS debug simple tokens
CBS Extended  0x1707    0x0F        235 descriptors incl. gate tokens
Memory        0x1704    0x07/0x08   27 memory-controller tokens (0x0701-0x071B)
PSP Config    0x1701    0x02, 0x60  PSP config + instance

Gated CBS debug values (0x15/0x17/0x19) are only read by ABL3 if the paired gate token
(0x14/0x16/0x18) is set to 0x00000001 (gate offsets 0x14 → 0xAB1A90, 0x16 → 0xAB1A98,
0x18 → 0xAB1AA0).  On this BIOS all gates read 0x00000000 (CLOSED), so the gated
values never apply; the CBS_DBG_MEM path is disabled because its gate token 0x76 is
absent (inverted logic).  Named direct tokens recovered from ABL debug strings include
0x1B Bank Group Swap, 0x1C Address Hash Bank, 0x47 Power Down Enable, 0x4E DRAM Phy
Power Saving, 0x50 DRAM All Clocks On, 0x51 DRAM All CKEs, 0x52 DRAM All CSs.

Because the block requires no signature and is protected only by the 1-byte checksum,
a modification within its slack that corrects the checksum is accepted and the board
boots cleanly.  This is why the ABL3 APCB parser (CVE-2021-26344) is of interest as a
research target — although the classic overflow primitive was not found in this AGESA
(see CVEs).

 See: The Boot Chain & ABL Stages — CVEs & Public Research — SPI Flash Security & Write-
                                 Protect — System Configuration (APCB tokens), Chapter 4
```

## CCP — Cryptographic Co-Processor

```
── CCP ─────────────────────────────────────────────── PCI 01:00.2 · gated from Linux ──

The AMD CCP is present at PCI 01:00.2 (device 0x143e) but is unusable from Linux for
three independent reasons: its device ID is PSP-specific and not in the upstream ccp
driver table, the endpoint is PCI-disabled by firmware, and its queues are never
initialized.

┌─ CCP ────────────────────────────────────────────────────────────────────────────────┐
│  PCI identity    Vendor 0x1022 (AMD), device 0x143e, at PCI 01:00.2                  │
│  BAR layout      BAR2 1 MiB @ 0xfe700000; BAR5 8 KiB @ 0xfe884000                    │
│  Driver binding  None — 0x143e absent from ccp PCI ID table                          │
│  PCI enable      Disabled (enable=0) by firmware                                     │
│  Queue state     Uninitialized — PSP firmware sets up no CCP queues                  │
└──────────────────────────────────────────────────────────────────────────────────────┘

The CCP also target-aborts x86 MMIO reads even with memory space enabled — the
endpoint enumerates and links (16 GT/s x16, D0) but its x86-facing MMIO decoder is
gated.  A separate TRNG path is functional and exposes PSP-internal randomness via
/dev/hwrng, orthogonal to the main CCP block.

Internally, the PSP programs its own CCP by MMIO writes from PSP SRAM during boot
(distinct from the OS-visible BAR2 interface).  Control registers live in the
0x3002000 range:

Address    Register         Function
─────────  ───────────────  ────────────────────────────────────────────────────────────
0x3002000  CCP_CTRL_STATUS  0x17 = single-descriptor DMA; 0x73 = chained
0x3002004  CCP_TAIL         descriptor chain tail
0x3002008  CCP_HEAD         descriptor chain head
0x3002100  CCP_ERR          error status

Completion is (CCP_CTRL_STATUS & 3) == 2.  The 32-byte DMA descriptor (8 x 32-bit
words) carries cmd/len/src_lo/src_hi/dst_lo/dst_hi/key_lo/key_hi; the memory-type
field in src_hi/dst_hi selects 0x000000 SYSTEM (x86 DRAM), 0x010000 LSB (CCP key
slots), or 0x020000 PSP SRAM.  Because the CCP has its own bus-master path independent
of the ARM MMU and privilege level, it is the structural basis of the published
EL0→EL1 escalation technique — documented here as a trust-boundary property only, with
no delivery detail.  This internal CCP is also what verifies the $PS1 body hash (an
HMAC-SHA-256 with a hardware-derived key).

           See: PSP (MP0) — Trust-Chain Keys & the $KDB Key Store — CCP block, Chapter 2
```

## SPI Flash Security & Write-Protect

```
── SPI FLASH · WRITE-PROTECT ─────────────────────────────── 16 MiB · no host-side WP ──

The BIOS lives in a 16 MiB Winbond W25Q128JV SOIC-8 SPI flash.  There is no host-side
write protection: the part is readable and writable from Linux with flashrom.

┌─ SPI FLASH ──────────────────────────────────────────────────────────────────────────┐
│  Chip         Winbond W25Q128JVSQ (flashrom: W25Q128JV), SOIC-8                      │
│  Capacity     16 MiB (16,777,216 bytes)                                              │
│  Memory map   physical address 0xff000000                                            │
│  Host WP      None (flashrom write+verify succeeds)                                  │
│  Linux read   flashrom -p internal:laptop=this_is_not_a_laptop -r bios.bin           │
│  External     CH341A programmer on the J4004 SPI header                              │
│  $PSP dir     0x8E0000 (19 entries) · APCB 0xAB1000                                  │
└──────────────────────────────────────────────────────────────────────────────────────┘

The laptop=this_is_not_a_laptop override is required because the carrier board does
not populate SMBIOS tables, which otherwise triggers flashrom's laptop safety mode.  A
non-destructive read establishes the known-good recovery baseline: read the flash,
record the image hash, archive it, and optionally diff two independent reads (expect
differing bytes only in NVRAM counter regions).  Always hold a verified backup before
touching any flash region.

The BIOS reaches flash through the AMD FCH SPI100 controller at MMIO 0xFEC10000.  No
DXE/SMM module programs the FCH SPI write-protect registers (SpiBlockProtect +0x44,
RangeProtect +0x50..0x60, PSP SoftLock +0xAA, RestrictedCmd Lock +0xFC) — only three
modules embed the base constant (FlashDriverSmm, SmiFlash, FlashDriver) and none write
a WP register; a string hunt for SpiLock/BiosLock/BLE/RangeProtect yields zero hits.
The AMD FlashAccess protocol writer performs no bounds check at any layer, so any
reachable write can target the entire 16 MiB shadow.

A kernel-to-flash write chain is gated only PSP-side.  Eight OS-callable SW SMI
handlers are registered (reached via outb(0xB2)); the high-value ones are SmiFlash
(BIOS flash R/W, cmd 0x00..0x25), NvmeSmm (NVMe LBA R/W, cmd 0x42), and SmbiosDmiEdit
(0x50..0x53, with an unguarded MTRR-unlock on cmd 0x52).  All OS-callable handlers
funnel through the AMI in-SMM validator (GUID DA473D7F-4B31-4D63-92B7-3D905EF84B84),
whose whitelist admits any address inside an MMIO/Reserved/NonExistent GCD entry —
which includes the entire BIOS flash shadow 0xFF000000-0xFFFFFFFF (GCD type 3).
System DRAM (type 2) is excluded.  The layer-by-layer enforcement result:

Layer                    Gate                        Status
───────────────────────  ──────────────────────────  ───────────────────────────────────
OS→SMM transit (0xB2)    vendor SwSmi                OPEN (SmiFlash registered)
OS buffer (Method 0)     outside-SMRAM blacklist     PASSES for normal DRAM
Flash target (Method 1)  blacklist + MMIO whitelist  PASSES for any byte in the BIOS
                                                     shadow
AMD FlashAccess writer   none                        no bounds check at any layer
FCH SPI WP register      not programmed by host      negative finding
PSP-side SPI WP          live-test only              the ultimate gate

A parallel BIOS-side PSP mailbox surface exposes 5 host→PSP (C2P) and 6 PSP→host (P2C)
commands; the P2C set includes SPI flash read/write/erase (0x83-0x86).  Its
CheckMboxValidity gate is not a security boundary — it uses only an 8-bit additive
checksum with a sender-controlled SKIP_CHECKSUM flag (bit 8) and a 6-entry command
enum check, with no signature, HMAC, nonce, or sequence counter; a parallel
gBS->SmmCommunicate path skips even the checksum.  It defends against memory-
corruption noise, not against a sender who knows the message format.  Both the P2C
mailbox path and the SmiFlash SW SMI path funnel through the same AMI validation and
the same unbounded writer, so the effective scope of a reachable write is the entire
16 MiB ROM, modulo hardware write protect.

Interpretation: the host BIOS enforces no SPI write-protect at the FCH controller.
Any actual WP must come from PSP-side enforcement, an AGESA pre-DXE PEIM (a separate
firmware volume not scanned), SMN-based ROM-Protect registers, or be absent.  If the
PSP enforces WP the chain is a kernel→SMM→read-only-flash primitive; if not, it is a
full kernel→SMM→flash-R/W primitive equivalent to the OEM flash-update path.  Because
PSB is not enforced and there is no host-side flash protection, the flash is freely
rewritable — subject to the fatal $KDB and TOS enforcement layers on what may be
modified.

┌─ CAUTION ────────────────────────────────────────────────────────────────────────────┐
│  Never modify a $KDB key-store blob (dir 0x50 / 0x51) or the TOS body — both are     │
│  hardware-fatal and non-recoverable in software.  Recovery from a bricked board      │
│  requires a CH341A programmer on the J4004 header with a known-good backup.          │
└──────────────────────────────────────────────────────────────────────────────────────┘

      See: PSP Directory & Firmware Layout — Trust-Chain Keys & the $KDB Key Store — SPI
                                           Flash and Debug & Recovery Headers, Chapter 1
```

## CVEs & Public Research

```
── CVEs & PUBLIC RESEARCH ──────────────────────────── AGESA 46.1.2.211126 · Nov 2021 ──

The board runs AGESA V9 RBNBDK-BL5 46.1.2.211126 (November 2021).  CVEs whose fixes
postdate 2021-11 are generally unpatched here.  The following are provided as a
platform-security reference — a catalogue, not exploitation procedures.  Track fixes
via AMD product-security bulletins.

  Tier 1 — direct PSP code execution (software):

CVE             Sev       Status     Fix                    Component
──────────────  ────────  ─────────  ─────────────────────  ────────────────────────────
CVE-2022-23817  HIGH 7.0  UNPATCHED  AMD-SB-4004 Aug24      TOS — buffer overflow
                                                            processing a malicious TA
CVE-2025-29951  HIGH 7.3  UNPATCHED  AMD-SB-4013 Feb26      PSP BL — stack buffer
                                                            overflow
CVE-2021-26391  HIGH 7.8  UNPATCHED  AMD-SB-4001 May23      TOS TA loader — $PS1
                                                            size_signed vs body mismatch
CVE-2022-23815  HIGH      Unclear    pre-2022 AGESA         APCB firmware — OOB write
                                                            (overlaps CVE-2021-26344)
CVE-2025-48515  MEDIUM    UNPATCHED  2025 fix               PSP BL — SPIROM integer
                                                            overflow
CVE-2021-26344  HIGH      Disputed   ComboAM4v2PI 1.2.0.8+  ABL3 APCB parser overflow —
                                                            classic primitive not found

  Tier 2 — stepping stones (x86 SMM, not direct PSP):

CVE             Sev  Component           Status
──────────────  ───  ──────────────────  ───────────────────────────────────────────────
CVE-2024-36311  MED  SMM TOCTOU race     Support-path only
CVE-2025-29950  MED  SMM stack overflow  Support-path only

  Relevant external research (references, not procedures):

Research                      Venue  Year     Relevance
────────────────────────────  ─────  ───────  ──────────────────────────────────────────
faulTPM (Buhren / Werling)    —      2023     PSP fault-injection; introduces the TOCTOU
                                              SPI-interposer approach for AMD
pAMDora (Buhren et al.)       39C3   2022     TOCTOU attack targeting the BC-250 / 4700S
                                              family
Positive Tech. BC-250 TOCTOU  —      2023-24  Reproduction of pAMDora against the BC-250
Google Project Zero           —      2021     CVE-2021-26344 discovery (APCB parser
                                              overflow)

The TOCTOU class relies on an SPI interposer presenting different flash contents to
the signature check versus the actual load; it requires physical flash isolation and a
spare board.  psptool (the community PSP directory / $PS1 tool) is used for extraction
but cannot verify RSA signatures on compressed ABLs and reports false verification
failures for valid signatures.

    See: The Boot Chain & ABL Stages — The Trusted OS — APCB — Platform Security Posture
```

## Platform Security Posture (Summary)

```
── SECURITY POSTURE ─────────────────────────────── fused non-secure · fatal $KDB/TOS ──

The BC-250 is fused non-secure with an open SPI flash and an unsigned APCB, but the
$KDB key-store and TOS signature layers make naive firmware patching hardware-fatal,
and the ring-command TA-loading path is stubbed.  Systematic investigation of the PSP-
facing surfaces concluded:

Surface                              Verdict
───────────────────────────────────  ───────────────────────────────────────────────────
Firmware-only debug unlock           Ruled out — DEBUG_UNLOCK negotiation is a stub;
                                     DRIVER_ENTRIES kernel unlock routines have zero
                                     references (dead code)
CCP DMA from Linux                   Ruled out — CCP target-aborts x86 MMIO; no driver
                                     binds its PSP-specific device ID
BL internal-verifier patch (0x5C90)  Non-fatal but ineffective — TOS uses a different BL
                                     export (0x11B04)
TOS body / key-store patch           Fatal — RSA-2048 ($PS1) plus the always-fatal $KDB
                                     integrity layer
amdgpu PSP mailbox (BAR5 C2PMSG)     Confirmed live x86-to-PSP channel; some registers
                                     target-abort (active per-register access control)
APCB modification                    Accepted — unsigned, 1-byte checksum; changes the
                                     config surface only
SEV / SEV-ES guest encryption        Dead on arrival — no SEV firmware in the PSP
                                     directory
Host-side SPI write-protect          Not enforced by the BIOS; PSP-side WP is the only
                                     remaining gate
CVE-2021-26344 APCB overflow         Disputed — the classic overflow primitive was not
                                     found in this AGESA

These are characterization conclusions, not methods.  In-band TOS TA-identity forgery
is exhaustively closed; the remaining avenues are out-of-band (PSP_BL static analysis
blocked by IKEK, AGESA pre-boot init, hardware TOCTOU on multi-core silicon, side-
channel against the signature verifier, and SPI-flash boundary substitution).

 See: The Four-Layer Trust Chain — The Trusted OS — Debug Interface Exposure — SPI Flash
                                       Security & Write-Protect — CVEs & Public Research
```


# 4. System Configuration

```
┌──────────────────────────────────────────────────────────────────────────────────────┐
│                                                                                      │
│      S Y S T E M   C O N F I G U R A T I O N                             CHAPTER  4  │
│      APCB flash tokens · EFI variable stores · kernel command line                   │
│                                                                                      │
└──────────────────────────────────────────────────────────────────────────────────────┘

The BC-250 is configured through a layered model.  Compile-time AGESA Parameter
Block (APCB) tokens in the PSP-managed flash region are applied before the x86
cores start; two UEFI variables — the boot-service Setup store and the runtime
AmdSetup store — carry the OEM platform and AGESA settings; and the kernel
command line tunes the running system.  The layers differ in when they are
consumed, how they are changed, and — importantly on this harvested die —
whether they have any runtime effect at all.

   SPI FLASH · compile time      BOOT SERVICES · UEFI          RUNTIME · OS
  ┌───────────────────────┐     ┌───────────────────────┐     ┌───────────────────────┐
  │  APCB tokens          │     │  Setup — BS-only      │     │  kernel command line  │
  │   @ 0xAB1000          │     │   AMI NVRAM FV        │     │   (GRUB)              │
  │   235 CBS tokens,     │ ──→ │   @ 0xAE0000          │ ──→ │  module options       │
  │   applied by the PSP  │     │  AmdSetup — NV+BS+RT  │     │  driver params        │
  │   before CPU init     │     │   PSP-managed region  │     │           │           │
  └───────────────────────┘     └───────────────────────┘     └───────────┼───────────┘
                                                                          ▼
             boot chain:  PSP → ABL0–4 → UEFI → OS                 running system

This chapter documents each layer: the APCB token set, the IFR-declared BIOS
variable store, which settings are live versus decorative on this hardware,
memory configuration, the clock/power configuration surface, and the kernel
parameters that make the board usable.  Ground truth: BIOS P3.00 · CachyOS
(BORE) kernel · the live production command line.

── IN THIS CHAPTER ────────────────────────────────────────────────── read outside-in ──

  Configuration Layers               Live vs Decorative Settings
  APCB Token Configuration           Memory Configuration
  IFR & the BIOS Variable Store      Clock & Power Configuration Surface
  AmdSetup EFI Variable              Kernel & Module Parameters
  Setup EFI Variable
```

## Configuration Layers

```
── CONFIGURATION LAYERS ───────────────────────────── four layers by consumption time ──

The configuration surface is organized into four layers, ordered by consumption
time.  Layers 1 and 1.5 live in the firmware image and require an SPI re-flash
to change; Layer 2 is writable from a running OS but is decorative for runtime
overrides on this hardware; Layer 3 is the safe, reversible surface.

┌─ THE FOUR LAYERS ────────────────────────────────────────────────────────────────────┐
│  Layer 1    APCB flash tokens   SPI @ 0xAB1000 (PSP-managed), unsigned CBS-token     │
│                                 series, applied by the PSP before CPU init.          │
│                                 Recovery: SPI programmer only.                       │
│                                                                                      │
│  Layer 1.5  Setup variable      boot-service-only EFI variable in the AMI NVRAM      │
│                                 firmware volume; live at the silicon, no runtime     │
│                                 efivars entry.  Change requires a BIOS re-flash.     │
│                                 Recovery: SPI programmer only.                       │
│                                                                                      │
│  Layer 2    AmdSetup variable   runtime EFI variable (NV+BS+RT) in the PSP-managed   │
│                                 region; writable via efivarfs, decorative for        │
│                                 runtime overrides.  Recovery: efivar -d if the       │
│                                 system boots; else re-flash.                         │
│                                                                                      │
│  Layer 3    Kernel boot params  GRUB command line + module options; reversible on    │
│                                 the next boot.  Recovery: edit the GRUB entry or     │
│                                 boot a snapshot.                                     │
└──────────────────────────────────────────────────────────────────────────────────────┘

The 16 MB SPI image (Winbond W25Q128JV) splits into an AMD-PSP-signed region and
an unsigned UEFI firmware volume, and this boundary governs what can be changed
safely:

  0x000000  ┌──────────────────────────────────────────────────────────────────┐
            │  AMD-SIGNED REGION              PSP Boot ROM verifies against    │
            │   PSP directory (0x8E0000)      the fused root key.              │
            │   PSP firmware blobs            Modify → BRICK.                  │
            │   ABL0–ABL4 (memory training)                                    │
            │   $PS1, $KDB key stores                                          │
            │   APCB blocks (0xAB1000)                                         │
  0xAE0000  ├──────────────────────────────────────────────────────────────────┤
            │  UEFI BIOS FIRMWARE VOLUME      UNSIGNED — AMI internal CRC32    │
            │   Setup, CbsSetupDxe, …         only.  Safe to reflash + revert. │
            └──────────────────────────────────────────────────────────────────┘
              Winbond W25Q128JV — 16 MB SPI flash

┌─ MODIFICATION SAFETY ────────────────────────────────────────────────────────────────┐
│  $KDB key-store edits              FATAL — bricks the board                          │
│    BL_PUBLIC_KEY (type 0x50) · TOS_PUBLIC_KEY (type 0x51)                            │
│  BIOS-volume reflashes             SAFE — flash and revert verified clean            │
│    Setup forms · CbsSetupDxe · AmdNbioSmuV10Dxe                                      │
│  flashrom -p internal -w           WORKS — full 16 MB image write, boots clean       │
└──────────────────────────────────────────────────────────────────────────────────────┘

Platform Secure Boot (PSB) OEM key entries (types 0x05, 0x64, 0x65) are absent
from the BC-250 PSP directory, so the UEFI volume is reflashable — but this does
not make the whole flash safe: the key-store entries and the PSP/APCB region are
always fatal to modify.

┌─ CAUTION ────────────────────────────────────────────────────────────────────────────┐
│  Never touch Layer 1 or Layer 1.5 without an SPI-programmer brick-recovery plan      │
│  ready (header J4004, Chapter 1).  Firmware-signature enforcement on the signed      │
│  region is not something to test empirically.                                        │
└──────────────────────────────────────────────────────────────────────────────────────┘

 See: APCB Token Configuration — Setup EFI Variable — Chapter 3 → Firmware map, PSP/APCB
```

## APCB Token Configuration

```
── APCB TOKEN CONFIGURATION ─────────────────────────── compile time · 235 CBS tokens ──

The AGESA Parameter Block holds 235 CBS configuration tokens as a binary token
series in SPI flash at offset 0xAB1000, loaded by the PSP before CPU init.  The
tokens are unsigned — no signature verification — but they reside in the
PSP-managed region and can only be changed by re-flashing the BIOS image: this
is the compile-time configuration layer.

┌─ SPECIFICATIONS ─────────────────────────────────────────────────────────────────────┐
│  Location          SPI flash @ 0xAB1000 (PSP-managed region)                         │
│  Format            binary CBS-token series, 235 tokens                               │
│  Applied by        PSP, before CPU init                                              │
│  Coverage          memory/UMA · CPU · NBIO/PCIe · PSP · FCH · GPU/display            │
│  Editing           BIOS image edit + re-flash; corruption → SPI programmer           │
└──────────────────────────────────────────────────────────────────────────────────────┘

  MEMORY / UMA TOKENS

  TOKEN        FIELD                 DEFAULT        FUNCTION
  0xAB1328     UMA Size              Auto (256 MB)  GDDR6 carved out for the GPU
                                                    framebuffer
  CBS 0x006C   UMA Mode              Enabled        whether the UMA frame buffer is
                                                    allocated
  CBS 0x006E   Memory Interleaving   Enabled        interleave accesses across channels
  CBS 0x0070   Channel Interleaving  Enabled        GDDR6 channel interleaving (256-bit
                                                    effective bus)
  CBS 0x0074   Memory Power Down     Enabled        GDDR6 low-power state when idle
  CBS 0x0075   Address Hash Bank     Enabled        bank address hashing
  0xAB1338     Total UMA + Overhead  0x84 (528 MB)  UMA size + 16 MB page-table overhead
  0xAB133C     Remaining Memory      0x3A98 (MB)    GDDR6 left for system use

  CPU / NBIO / PCIE / FCH / GPU TOKENS

  TOKEN        FIELD                    DEFAULT    FUNCTION
  CBS 0x0002   SMT                      Enabled    6 cores → 12 threads
  CBS 0x0005   Core Performance Boost   Enabled    CPU boost (SMU-managed)
  CBS 0x0006   Global C-state           Enabled    CPU power states (C6 deep sleep when
                                                   idle)
  CBS 0x000E   L1/L2 Prefetch           Enabled    hardware prefetcher
  CBS 0x0003   Downcore                 Default    at the full 6 cores
  CBS 0x0101   IOMMU                    Present    AMD-Vi; disabled at runtime by
                                                   amd_iommu=off on the production
                                                   command line
  CBS 0x0102   PCIe Speed               Gen 3      SoC ↔ chipset link
  CBS 0x0103   PCIe Width               x16        full width to the A68H chipset
  CBS 0x0601   PSP fTPM                 Default    firmware TPM (unused headless)
  CBS 0x0501   XHCI Enable              Enabled    USB 3.0 controller
  CBS 0x0301   SATA Mode                AHCI       not RAID
  CBS 0x0201   Integrated Graphics      Enabled    the GPU is the compute device
  CBS 0x0203   Display Output           Disabled   headless; paired with amdgpu.dc=0

UMA Auto (256 MB) with the stock driver routes GPU allocations through GTT
correctly.  Forcing a larger UMA (e.g. token value 0x80 for 512 MB at 0xAB1328)
requires a driver-side GTT-preference patch (the apu_prefer_gtt route) to avoid
the slower framebuffer-aperture path.

┌─ CAUTION ────────────────────────────────────────────────────────────────────────────┐
│  The APCB blocks sit inside the AMD-signed PSP region — corruption bricks the board  │
│  and requires an SPI programmer to recover.                                          │
└──────────────────────────────────────────────────────────────────────────────────────┘

     See: Configuration Layers — Memory Configuration — Chapter 3 → PSP/APCB — Chapter 5
```

## IFR & the BIOS Variable Store

```
── IFR & THE BIOS VARIABLE STORE ────────────────────── 1357 questions · 42 VarStores ──

The BIOS declares its settings UI and the variables behind it in IFR (Internal
Form Representation).  Every question — OneOf, Numeric, CheckBox — maps back to
a VarStore name, GUID, and byte offset.  Seven UEFI drivers publish IFR forms;
the two that carry the platform configuration are AmdCBS (backing the runtime
AmdSetup variable) and MainSetup (backing the boot-service Setup variable).
Strict question count across the image: 1357 (1372 counting VarStore-less
display rows), across 42 VarStores.

  DRIVER              GUID       STORES  QUESTIONS  FORMS  SUPPRESSIF  PRIMARY VARIABLE
  AmdCBS              f639d37e        1       1134     71         651  AmdSetup (2229 B)
  MainSetup           899407d7       30        218     39         138  Setup (467 B)
  RealtekRealManage   eb53fcad        1         12      7           4  small
  RecoveryFlash       70e1a818        3          5      2           9  small
  HttpBoot            ecebcb00        1          3      1           0  small
  SioConfiguration    1830a6dd        6          0      2           0  display-only
  NvmeInfo            668706b2        0          0      2           0  display-only
  TOTAL                              42       1372    124         802

Defaults are injected by the standard EDK2/AMI HII mechanism.  Each question may
carry an IFR Default opcode (0x5B; DefaultId 0 = standard, 1 = manufacturing);
at boot, AMI's HiiDataBaseDxe walks these opcodes and, if the target variable is
missing or the IFR signature changed, writes the declared defaults into the
variable.  Other DXE modules then gRT->GetVariable(...) and read bytes at known
offsets.

   IFR Default opcodes          AMI HiiDataBaseDxe            NVRAM
   (LZMA-compressed section     (at boot, if the variable     (variable record
    of the driver PE)       →    is missing or the IFR     →   initialized)
                                 signature changed)

Every question ships SuppressIf-hidden in stock firmware — the BIOS Setup TUI
appears empty on stock boot.  The community clv mod neutralizes 656 SuppressIf
opcodes in the AMD CBS driver to surface the hidden forms; visibility is
independent of whether a field has a live DXE consumer.  Forms are decoded
offline with ifrextractor (v1.6.1, LongSoft), where a trailing comma in the
options column marks the default value.

┌─ BIOS VERSION PARITY — P2.00 · P3.00 · P5.00 ────────────────────────────────────────┐
│  The settable IFR surface is identical across factory BIOS versions — the same       │
│  question counts per driver and the same Above 4G / IOMMU / ASPM defaults.  The      │
│  only inter-version code change with platform relevance is a +608-byte SMM handler   │
│  (SmuSmmAccessEventNotify) added to AmdNbioSmuV10Dxe in P5 — a security/audit        │
│  handler (it reads MSRs 0xC0010112/0xC0010113 and logs an SMM-table walk), not a     │
│  Setup→SMU write filter.  The VBIOS is not stored decompressed in flash: AGESA       │
│  generates it at boot from PSP hardware-config data and exposes it via the ACPI      │
│  VFCT table, and the runtime VBIOS is byte-identical across factory versions.  BIOS  │
│  version choice is therefore neutral for configuration purposes.                     │
└──────────────────────────────────────────────────────────────────────────────────────┘

           See: AmdSetup EFI Variable — Setup EFI Variable — Live vs Decorative Settings
```

## AmdSetup EFI Variable

```
── AMDSETUP EFI VARIABLE ────────────────────────────── runtime-writable · decorative ──

AmdSetup is the runtime-writable AGESA tunable store declared by the AmdCBS
driver.  It is reachable from a running OS through efivarfs, and its bytes
persist across reboot — but on this hardware runtime writes are decorative
(see Live vs Decorative Settings).

┌─ SPECIFICATIONS ─────────────────────────────────────────────────────────────────────┐
│  Name          AmdSetup                                                              │
│  GUID          3A997502-647A-4C82-998E-52EF9486A247                                  │
│  Size          2229 bytes (0x8B5)                                                    │
│  Attributes    NV + BootService + Runtime (0x07)                                     │
│  IFR fields    1132, declared by AmdCBS (GUID prefix f639d37e)                       │
│  Storage       PSP-managed flash region — live data at offsets 0x4CF and             │
│                0x261B, below the 0xAE0000 NVRAM FV                                   │
│  Runtime path  /sys/firmware/efi/efivars/                                            │
│                  AmdSetup-3a997502-647a-4c82-998e-52ef9486a247                       │
│  Defaults      IFR Default opcodes in the AmdCBS PE                                  │
└──────────────────────────────────────────────────────────────────────────────────────┘

The efivarfs file prepends a 4-byte EFI attribute header before the 2229-byte
payload; IFR-declared offsets refer to the payload (add 4 for an efivarfs byte
offset).  Because the live copy is also persisted in the PSP-managed region,
clearing the variable does not necessarily revert to IFR defaults — it may
revert to whatever the PSP wrote at last init.

── BYTE LAYOUT ─────────────────────────────────────

  0x0000     4 B       EFI attributes: 0x07 (NV + BS + RT)
  0x0004     2225 B    settings payload

  payload    0x00         0x01     BIOS version marker
             0x01         0xFF     Setup revision (auto)
             0x02–0x03    0x03     Setup mode
             0x05         0x03     CBS submenu mode
             0x08         0x01     CBS Enable — master switch for AMD CBS settings
             0x09         0x0E     CBS Debug Level (14)
             0x0D–0xF7    stride   repeating 9-byte stride 01 00 00 00 00 00 00 00 00
                                   — a 0x01 boolean enable + 8 padding bytes; ~28 CBS
                                   feature-group booleans follow this pattern, all
                                   Enabled

The full IFR-declared surface (1132 fields) is far wider than these visible
boolean rows.

── IFR OFFSET MAPS ─────────────────────────────────

Payload offsets from the AmdCBS IFR decode, grouped by domain.  Auto defaults
encode as 255 (single byte) or 65535 (16-bit).

  MEMORY — 96 entries, 55 with defaults · MEMORY TRAINING — 10 entries

  OFFSET       FORM                       FIELD                           STD DEFAULT
  0x21C        Memory Hole                Memory Hole on Die 0            0
  0x25E        Common Options             UMA Mode                        Auto
  0x25F        Common Options             UMA FB Size                     Auto (u32)
  0x316        Common Options             Memory Clock Speed              Auto
  0x6F2        SMU Debug                  AllocateDramBuffer              0
  0x609–0x618  UMCCONFIG Control          DFE RX Value UMC0..7 /          0
                                          Channel0..1
  0x32F        GDDR6 DRAM Timing Cfg      DRAM Timing User Controls       Auto — Manual
                                                                          unlocks all
                                                                          timing fields
  0x348        GDDR6 DRAM Controller Cfg  On DIMM Temperature Sensor      Auto
                                          Enable
  0x349        GDDR6 DRAM Controller Cfg  CmdThrottleMode Control         Auto
  0x34A        GDDR6 DRAM Controller Cfg  CmdThrottleMode                 8
  0x37D        training                   APUDFE Training Control         Auto

UMC controls (UclkDiv1 M0–M3 near 0x330, UMCCTRL_MISC0–9 mask/value pairs near
0x37E) allow raw UMC controller register modification.

  PCIE LANE EQUALIZATION — 75 entries, 34 with defaults · SPREAD SPECTRUM

  OFFSET       FORM                       FIELD                           STD DEFAULT
  0x2A0–0x2A8  TXEQ                       txX_eq_pre/main/post[5:0]       255 (Auto)
                                          GEN1/2/3
  0x2AF–0x2B1  RXEQ                       rxX_eq_att_lvl[2:0] GEN1/2/3    255 (Auto)
  0x2B2–0x2BD  RXEQ                       rxX_eq_vga / ctle               255 (Auto)
  0x2BE–0x2C2  RXEQ                       rxX_eq_dfe_tap1[7:0] GEN1/2/3   65535 (Auto)
  0x2C5–0x2C7  LOS BOOST                  tx_vboost_lvl[2:0] GEN1/2/3     255 (Auto)
  0x288        Display interface SS       Display Spread Spectrum         0
                                          Percentage
  0x747        G6 Mem SS configuration    Uclk Spread Percentage          0

  C-STATE / DPM — 119 entries, 88 with defaults · FABRIC CLOCK — 24 entries

  OFFSET       FORM                       FIELD                           STD DEFAULT
  0x004        Vh Common Options          Core Performance Boost          Auto
  0x005        Vh Common Options          Global C-state Control          Auto
  0x6A8        SMU Features               CORE_CSTATES                    Auto
  0x6C1        SMU Features               CSTATE_BOOST                    Auto
  0x6A3        SMU Features               FCLK_DPM                        Auto
  0x6B7        SMU Features               DS_MP3FCLK                      Auto
  0x703        SMU Debug                  DF Clock Override Control       Auto
  0x704        SMU Debug                  DF Clock Override Value         1000
  0x735        DFLL freq tuning           GfxDfllFreqMeasInRefClkCycles   0

  CLOCK GATING — 11 entries · SMT / CORE — 21 entries

  OFFSET       FORM                       FIELD                           STD DEFAULT
  0x228        DF Debug Options           Medium grain clock gating       Auto
  0x247        DF Debug Options           DF clock gating control         Auto
  0x2E5        PMM - General              ATHUB Clock Gating              Auto
  0x2E6        PMM - General              IOHC LCLK Clock Gating          Auto
  0x2E7        PMM - General              IOMMU L1 Clock Gating           Auto
  0x2E8        PMM - General              IOMMU L2 Clock Gating           Auto
  0x007        Core/Thread Enablement     Downcore control                —
  0x008        Core/Thread Enablement     SMTEN                           Auto — board
                                                                          boots with SMT
                                                                          on
  0x009        Core/Thread Enablement     NumEncryptedGuests              14
  0x6D4        SMU Debug                  Core Dldo Psm Margin            0

  POWER MANAGEMENT — 29 · THERMAL / FAN — 28 · VOLTAGE OFFSET — 28 · GPU/GFX — 27

  OFFSET       FORM                       FIELD                           STD DEFAULT
  0x364        DRAM Power Options         Power Down Delay                1
  0x367        DRAM Power Options         Aggressive Power Down Delay     1
  0x718        SMU Debug                  VRM_CURRENT_LIMIT               65000
  0x71C        SMU Debug                  VRM_MAXIMUM_CURRENT_LIMIT       95000
  0x6A1        SMU Features               THERMAL                         Auto
  0x6C2        SMU Debug                  Thermal Control                 Auto
  0x6C9        SMU Debug                  TctlMax_Gfx                     100
  0x6F0        SMU Debug                  ThermTrip                       Auto
  0x6E5        SMU Debug                  VDDCR_VDD Voltage Offset        Auto
                                          Control
  0x6E6        SMU Debug                  VDDCR_VDD Voltage Offset        0
  0x6EA        SMU Debug                  VDDCR_GFX Voltage Offset        Auto
                                          Control
  0x6EB        SMU Debug                  VDDCR_GFX Voltage Offset        0
  0x6F7        SMU Debug                  CPU DC BTC Set voltage          0
  0x6D8        SMU Debug                  Gfx Stretch Thresh              0
  0x6DA        SMU Debug                  Gfx Stretch Amount              0
  0x6FB–0x6FE  SMU Debug                  GFX DC BTC (Bias Tracking)      0

The IOMMU category (10 entries) is mostly redundant declarations covering clock
gating (0x2E7/0x2E8) and SVM debug; the operational IOMMU enable lives in Setup
byte 0xD2, not AmdSetup.  SEV / SME / TSME and MCA-mask fields exist but are
diagnostic/security fields, out of tuning scope.

── READING AND WRITING FROM LINUX ──────────────────

  # full dump
  sudo cat \
    /sys/firmware/efi/efivars/AmdSetup-3a997502-647a-4c82-998e-52ef9486a247 | xxd

  # single payload byte at offset 0x316 (add 4 for the attribute header)
  sudo dd \
    if=/sys/firmware/efi/efivars/AmdSetup-3a997502-647a-4c82-998e-52ef9486a247 \
    bs=1 skip=$((4 + 0x316)) count=1 status=none | xxd

After the first successful write in a session, the kernel sets the i (immutable)
attribute on the efivars file; subsequent writes fail silently with "Operation
not permitted" (a recent-mainline default).  Clear it before every new write
campaign:

  sudo chattr -i \
    /sys/firmware/efi/efivars/AmdSetup-3a997502-647a-4c82-998e-52ef9486a247
  sudo dd if=new_bytes.bin bs=1 conv=notrunc status=none \
    of=/sys/firmware/efi/efivars/AmdSetup-3a997502-647a-4c82-998e-52ef9486a247

┌─ CAUTION ────────────────────────────────────────────────────────────────────────────┐
│  Always back up the full variable before writing.  A worst-case bad write can        │
│  prevent the system from reaching userspace (where efivar -d lives), leaving only    │
│  an SPI re-flash to recover.  Runtime writes are decorative for AGESA-controlled     │
│  behavior regardless (see Live vs Decorative Settings).                              │
└──────────────────────────────────────────────────────────────────────────────────────┘

 See: IFR & the BIOS Variable Store — Live vs Decorative Settings — Chapter 3 → PSP/APCB
```

## Setup EFI Variable

```
── SETUP EFI VARIABLE ────────────────────────────────────── boot-service only · live ──

Setup is the AMD/OEM platform settings variable declared by the MainSetup
driver.  Unlike AmdSetup it is live — its bytes reach the silicon — but it is
boot-service-only, so it cannot be written from a running OS; changing a
default requires patching the BIOS image and reflashing.

┌─ SPECIFICATIONS ─────────────────────────────────────────────────────────────────────┐
│  Name          Setup                                                                 │
│  GUID          EC87D643-EBA4-4BB5-A1E5-3F3E36B20DA9                                  │
│  Size          467 bytes                                                             │
│  Attributes    boot-service only — no /sys/firmware/efi/efivars/ entry               │
│  IFR offsets   129 mapped (of 467 bytes; the rest is SetupUtility state,             │
│                VarStore padding, reserved)                                           │
│  Declared by   MainSetup (GUID prefix 899407d7)                                      │
│  Storage       AMI NVRAM firmware volume @ 0xAE0000                                  │
└──────────────────────────────────────────────────────────────────────────────────────┘

The MainSetup driver declares 30 VarStores; only Setup carries the AMD platform
settings — the rest are AMI/UEFI bookkeeping (language, boot manager, driver
health, USB topology).  Six stores share the Setup GUID and follow the same
boot-service-only lifecycle:

  ID     NAME               SIZE    ROLE
  0x1    Setup              467 B   AMD platform settings
  0x15   SetupCpuFeatures   7 B     CPU-feature scratch/state
  0x1A   UsbMassDevNum      2 B     USB topology
  0x1B   UsbMassDevValid    16 B    USB topology
  0x1C   UsbControllerNum   4 B     USB topology
  0x1D   UsbSupport         33 B    USB configuration

── IFR OFFSET MAPS ─────────────────────────────────

Fields are single bytes unless noted (16-bit / 32-bit widths called out in
NOTES).

  BOOT (form 0x2715) · HARDWARE MONITOR / FAN (form 0x2856)

  OFFSET        QUESTION                        STD          OPTIONS / RANGE
  0x000         Bootup NumLock State            1            0=Off, 1=On
  0x001         Fast Boot                       0            0=Disabled, 1=Enabled
  0x002         SATA Support (boot)             0            0=Last Boot HDD Only, 1=All
                                                             SATA
  0x003         VGA Support (boot)              1            0=Auto, 1=EFI Driver
  0x004         USB Support (boot)              1            0=Disabled, 1=Full Initial,
                                                             2=Partial
  0x005         PS2 Devices Support             1            0=Disabled, 1=Enabled
  0x006         Redirection Support             0            0=Disabled, 1=Enabled
  0x007         NetWork Stack Driver Support    1            0=Disabled, 1=Enabled
  0x015–0x018   Temperature 1–4                 60/70/80/80  [0x14–0x64]
  0x019         Critical Temperature            85           [0x3C–0x64]
  0x01A–0x01D   Fan Speed for Temperature 1–4   70/80/90/90  [0x0–0x64]

  IMC / ACPI FAN CONTROL (form 0x28DA)

  OFFSET        QUESTION                        STD          OPTIONS / RANGE
  0x1AD         IMC Fan Control                 0            0=By Jumper, 1=Enabled,
                                                             2=Disabled
  0x1AE         CPU Thermal Zone 1              53           [0x0–0xFF]
  0x1AF         CPU Thermal Zone 2              14           [0x0–0xFF]
  0x1B0         CPU Hysteresis                  84           [0x0–0xFF]
  0x1B1         CPU Fan PWM Stepping %          1            [0x0–0x64]
  0x1B2         CPU Fan PWM Ramping Rate        0            [0x0–0x64]
  0x1B3–0x1BA   CPU _AC0.._AC7                  70/60/0..    [0x0–0x64]
  0x1BB         CPU _CRT                        105          [0x0–0xFF]
  0x1BC–0x1C3   CPU _AL0.._AL7                  100/0..      [0x0–0x64]

  CPU CONFIGURATION (forms 0x2858 · 0x285A · 0x28DB)

  OFFSET        QUESTION                        STD      NOTES
  0x0AC         PSS Support                     1        P-state selection
  0x0AD         PSTATE Adjustment               0        0=PState 0 … 4=PState 4
  0x0AF         PPC Adjustment                  0        Performance State Cap
  0x0B0         SVM Mode                        0        AMD-V; 3 form copies flip the
                                                         same byte; live (5 DXE
                                                         consumers)
  0x0B1         NX Mode                         1        No-Execute
  0x0B2         C6 Mode                         1        deep CPU sleep; suspected brick
                                                         if disabled in firmware
  0x0B3         CPB Mode                        1        Core Performance Boost
  0x0D2         IOMMU                           0        proven live consumer (2 form
                                                         copies, 10 hits)
  0x1C4         Cooler and Quieter operation    72000    32-bit; 88000=Disabled,
                                                         72000=Enabled; live (4 DXE
                                                         consumers)

  MEMORY CONFIGURATION (form 0x285E) · PCI SUBSYSTEM (form 0x2860)

  OFFSET        QUESTION                        STD      NOTES
  0x0D5         Bank Interleaving               1        empirically inert (GDDR6
                                                         manages internal interleaving)
  0x0D6         Channel Interleaving            1        empirically inert
  0x0D7         Memory Clock                    0        empirically inert; VBIOS
                                                         PowerPlay table owns clock
                                                         policy
  0x0D8         Memory Clear                    0        boot-time security feature;
                                                         adds boot time
  0x0DA         Above 4G Decoding               0        proven live consumer (3 hits);
                                                         prerequisite for Resizable BAR
  0x0DB         SR-IOV Support                  0        non-functional without a GPU
                                                         firmware unlock
  0x0DC         PCI Latency Timer               —        32 / 64 / 96 PCI Bus Clocks
  0x0DD         VGA Palette Snoop               0
  0x0DE         PERR# Generation                0        PCI parity error reporting
  0x0DF         SERR# Generation                0        PCI system error reporting
  0x0ED         Don't Reset VC-TC Mapping       0        Virtual Channel / Traffic Class

  PCI EXPRESS (form 0x2861) · SB GPP CHIPSET PCIE (form 0x28C6)

  OFFSET        QUESTION                        STD      NOTES
  0x0E1         Relaxed Ordering                1        37 DXE consumers — default
                                                         already optimal
  0x0E2         Extended Tag                    0        8-bit tag space (vs 5-bit) — up
                                                         to 256 outstanding non-posted
                                                         requests
  0x0E3         No Snoop                        1
  0x0E4         Maximum Payload                 55       55=Auto / 128 / 256 / 512 /
                                                         1024 / 2048 / 4096 B
  0x0E5         Maximum Read Request            55       55=Auto / same options
  0x0E6         ASPM Support                    0        proven live consumer (4 DXE);
                                                         0=Disabled, 55=Auto, 1=Force
                                                         L0s
  0x0E7         Extended Synch                  0
  0x0E8         Link Training Retry             5        0=Disabled, 2, 3, 5
  0x0E9         Link Training Timeout (us)      1000     16-bit; [10–10000]
  0x0EB         Unpopulated Links               0        0=Keep Link ON, 1=Disabled
  0x0EC         Restore PCIE Registers          0        0=Disabled, 255=Enabled
  0x13E         SB GPP Function                 1
  0x13F         GPP Link Configuration          3        0=x4, 2=2:2, 3=2:1:1, 4=1:1:1:1
  0x141         GPP Link ASPM                   3        0=Disabled, 1=L0s, 2=L1,
                                                         3=L0s+L1; live (4 DXE
                                                         consumers)
  0x142         GPP Gen2                        1
  0x143         UMI Gen2                        1
  0x146/0x147   UMI / SB GPP PHY PLL Power      1
                Down

  USB / CHIPSET DEVICE ENABLES

  OFFSET        QUESTION                        STD      NOTES
  0x1C8         USB mode (form 0x28DC)          1        0=GEN1, 1=Auto
  0x118         OnChip SATA Channel             1        Enabled
  0x119         OnChip SATA Type                2        0=Native IDE, 1=RAID, 2=AHCI,
                                                         3=Legacy IDE
  0x11D/0x11E   XHCI Controller 0/1             1        Enabled
  0x126–0x137   USB PORT 0/1/2…                 1        per-port enables
  0x14E         HD Audio Azalia Device          2        0=Auto, 1=Disabled, 2=Enabled

── CHANGING A SETUP DEFAULT — BIOS IMAGE PATCH ─────

Because Setup has no efivarfs entry, a default change must live in the BIOS
image: locate the IFR Default opcode, patch the PE inside the nested compressed
FFS section, recompress, reseat checksums, reflash.

  ifrextractor      →   find the IFR Default opcode bytes in the decoded text
  patcher script    →   find the PE32 in the nested compressed FFS section
  python lzma       →   decompress the section, modify the IFR Default opcode
                        bytes, recompress with the same parameters
  reseat FFS        →   recompute FFS header + section checksums
  flashrom -w       →   write the full 16 MB SPI image

The Setup driver PE lives in a nested LZMA-compressed FFS section; a single
default flip is typically a few byte changes plus a recomputed FFS checksum.
These edits target the unsigned BIOS volume and are safe to flash and revert.

┌─ CAUTION ────────────────────────────────────────────────────────────────────────────┐
│  C6 Mode (0xB2) disabled in firmware — as opposed to at OS level — is a known        │
│  AGESA/SMU init hazard on Zen 2 and the prime suspect for a NO-POST brick observed   │
│  after a multi-patch flash; leave it Enabled in any patch design.  Validate every    │
│  BIOS patch as a single-feature proof-of-concept before any multi-patch flash, and   │
│  flash incrementally, one new feature at a time.                                     │
└──────────────────────────────────────────────────────────────────────────────────────┘

 See: IFR & the BIOS Variable Store — Live vs Decorative Settings — Configuration Layers
```

## Live vs Decorative Settings

```
── LIVE VS DECORATIVE SETTINGS ──────────────────── what actually reaches the silicon ──

A large fraction of the 1357 IFR-declared questions do nothing on this specific
hardware, even when a DXE module reads the byte.  The BC-250 inherits the
generic AGESA settings surface built for socketed AM4/AM5 platforms with
discrete DDR4 DIMMs, where the BIOS owns memory training; on a harvested Oberon
die with on-package GDDR6, training, interleaving, frequency, and voltage are
decided by the AGESA-generated VBIOS PowerPlay table at boot.  Three independent
mechanisms make a field inert:

  1  AGESA reference-design inheritance — the field exists for socketed DDR4
     platforms; on the BC-250 the AGESA-generated VBIOS PowerPlay table owns
     the behavior.
  2  GDDR6 manages its own interleaving — bank/channel interleave live in the
     GDDR6 controller/PHY, not in address-map software; nothing for a BIOS
     toggle to flip.
  3  Voltage VALUE bytes have no DXE consumer — the VRM control plane is
     SVI2-only from the SMU; only loadline (impedance) bytes are consumed.

┌─ FINDING — AMDSETUP RUNTIME WRITES ARE DECORATIVE ───────────────────────────────────┐
│  Runtime writes to AmdSetup round-trip correctly through NVRAM and persist across    │
│  reboot — and produce no silicon effect.  Verified per-offset: 12 distinct field     │
│  writes across 11 unique offsets, spanning power management, clock gating, fabric    │
│  DPM, CPU boost, and memory/fabric clocks — all persisted, none took effect.  The    │
│  runtime AmdSetup path is not a productive tuning surface; a persistent change must  │
│  go through a Setup-variable BIOS image patch, a PSP-region defaults edit            │
│  (brick-class), or AGESA reverse engineering.                                        │
└──────────────────────────────────────────────────────────────────────────────────────┘

  OFFSET    FIELD                        DEFAULT → WRITTEN   PERSISTED  RUNTIME EFFECT
  0x004     Core Performance Boost       Auto → Disabled     yes        none
  0x247     DF clock gating control      Auto → Disabled     yes        none
  0x2E5     ATHUB Clock Gating           Auto → Disabled     yes        none
  0x2E6     IOHC LCLK Clock Gating       Auto → Disabled     yes        none
  0x2E7     IOMMU L1 Clock Gating        Auto → Disabled     yes        none
  0x2E8     IOMMU L2 Clock Gating        Auto → Disabled     yes        none
  0x316     Memory Clock Speed           Auto → 0x23         yes        none
  0x6A3     FCLK_DPM                     Auto → Disabled     yes        none
  0x6B7     DS_MP3FCLK                   Auto → Disabled     yes        none
  0x703     DF Clock Override Control    Auto → Enable       yes        none
  0x704     DF Clock Override Value      1000 → 0x06D6       yes        none

Root cause: a GUID search finds the AmdSetup VarStore referenced only in
CbsSetupDxe and CbsBaseDxe — both declare the form, neither programs the SMU.
AGESA acts on the PSP-region copy laid down at init, not the runtime efivars
copy.  The remaining 1120 fields are untested but expected to behave the same.

  VOLTAGE VALUE BYTES DECORATIVE — LOADLINE BYTES LIVE

A byte-level DXE consumer scan across all 13 PE files referencing the Setup
GUID (Voltage Configuration form 0x28EA):

  OFFSET    QUESTION              STD    DXE HITS   VERDICT
  0x1C9     VCORE Fix Voltage     0      0          decorative
  0x1CA     VCORE Voltage (mV)    900    0          decorative
  0x1CC     VCORE Loadline        2      1          live (VRM impedance)
  0x1CD     GFX Fix Voltage       0      0          decorative
  0x1CE     GFX Voltage (mV)      900    0          decorative
  0x1D0     GFX Loadline          2      4          live (VRM impedance)
  0x1D1     MEMIO Fix Voltage     0      0          decorative
  0x1D2     MEMIO Voltage         30     0          decorative

There is no BIOS path to user-controlled voltage-curve modification on this
hardware; the VRM control plane is SVI2-only from the SMU (Chapter 2).

  SETTINGS THAT DO TAKE EFFECT

Six Setup fields are validated live, each confirmed by direct runtime
measurement after reboot:

  FIELD              BYTE   RECOMMENDED       EFFECT / VERIFICATION
  Above 4G Decoding  0xDA   Enabled           BARs placed above 4 GB; prerequisite
                                              for Resizable BAR
  IOMMU              0xD2   Enabled           /sys/class/iommu/ populates; AMD-Vi
                                              available
  SVM Mode           0xB0   Enabled           svm flag appears on all 12 threads
  Cool'n'Quiet       0x1C4  Disabled (88000)  pins the CPU at full performance state
  GPP Link ASPM      0x141  Disabled          lower NVMe / chipset PCIe latency
                                              under load
  Extended Tag       0xE2   Enabled           8-bit PCIe transaction tag (vs 5-bit)

Caveat on Above 4G: with it enabled, BAR0 remains 256 MB — Resizable BAR does
not auto-activate even though the GPU advertises it; activating the resize
requires a separate mechanism (module parameter, VBIOS switch, or bridge
configuration).  Defaults already optimal (do not flip): Bank/Channel
Interleaving (0xD5/0xD6), Maximum Payload / Read Request (0xE4/0xE5, Auto),
Relaxed Ordering (0xE1), GPP Gen2 (0x142), CPB Mode (0xB3), NX Mode (0xB1).

A live consumer is a real x86_64 [reg + disp32] memory-access instruction in a
DXE PE whose displacement matches the Setup byte offset.  The most-scrutinized
live bytes:

  FFS GUID   SETUP BYTE  INSTRUCTION ENCODING  DECODED
  580dd900   0xDA        89 81 DA 00 00 00     mov [rcx+0xDA], eax
  d7e6abc1   0xDA        88 83 DA 00 00 00     mov [rbx+0xDA], al
  d7e6abc1   0xD2        88 83 D2 00 00 00     mov [rbx+0xD2], al
  d7e6abc1   0xE6        88 83 E6 00 00 00     mov [rbx+0xE6], al
  510df6a1   0xD2        8A 80 D2 00 00 00     mov al, [rax+0xD2]

d7e6abc1 is the primary consumer, copying Setup bytes into a structure that
feeds AGESA's NBIO and IOMMU DXEs; the same source appears compiled into
PEI/DXE/SMM triplets (510df6a1 / b7d19491 / d7770c0b).  A byte-level scan
across 126 Setup IFR offsets finds 104 (83%) with at least one confirmed DXE
consumer; the 22 with none cluster around the Hardware Monitor temperatures and
the voltage VALUE bytes.

┌─ CAUTION ────────────────────────────────────────────────────────────────────────────┐
│  The loadline bytes (0x1CC VCORE, 0x1D0 GFX) are live and modify VRM impedance —     │
│  wrong values risk VRM instability and silicon damage; do not modify without         │
│  electrical instrumentation.  The BIOS layer is not the surface for                  │
│  memory-bandwidth tuning; the ceiling is hardware-bound in the GDDR6 signaling.      │
└──────────────────────────────────────────────────────────────────────────────────────┘

               See: AmdSetup EFI Variable — Setup EFI Variable — Chapter 2 → SMU / Power
```

## Memory Configuration

```
── MEMORY CONFIGURATION ──────────────────────── UMA carve-out · CMOS timing override ──

CPU and GPU share the 16 GB GDDR6 pool; the BIOS carves a framebuffer region at
boot and the remainder is a shared system pool.  Timings are set from APCB
tokens at boot and are overridable via extended CMOS — the one working software
memory-timing surface.

┌─ SPECIFICATIONS ─────────────────────────────────────────────────────────────────────┐
│  Capacity      16 GB GDDR6 (8 chips × 2 GB) · 256-bit · 8 channels · UMC 8.1.1 ×2    │
│  Apertures     VRAM carve-out (BIOS UMA, default 256 MB; this system runs            │
│                512 MB) + shared system pool                                          │
│  UMA tokens    APCB 0xAB1328 (size) / CBS 0x006C (mode) — see APCB Token             │
│                Configuration                                                         │
│  Timings       set from APCB tokens at boot; overridable via extended CMOS           │
└──────────────────────────────────────────────────────────────────────────────────────┘

GDDR6 timings are staged in a 28-byte block in extended CMOS at offset 0x90,
written through the legacy I/O ports 0x72 (index) / 0x73 (data).  The firmware
(ABL) trains the staged timings on the next boot and stamps a result signature.

┌─ EXTENDED-CMOS TIMING OVERRIDE ──────────────────────────────────────────────────────┐
│  Ports          0x72 (index) / 0x73 (data)                                           │
│  Timing block   28 bytes @ extended-CMOS offset 0x90                                 │
│  Applied by     ABL at next boot (trains, stamps a result signature)                 │
│  Recovery       a timing set that fails to train is cleared with a CMOS              │
│                 clear (CLRCMOS1 jumper, Chapter 1)                                   │
└──────────────────────────────────────────────────────────────────────────────────────┘

The runtime memory clock lives in three places, all out of software reach: the
SMU PowerPlay table inside the runtime VBIOS (AGESA-generated at boot, not
stored decompressed in flash); the APCB blocks in the PSP region (brick-class
to modify); and the signed SMU firmware.  The GDDR6 clock is trained to its
hardware-determined operating point at boot; the BIOS memory-clock and
interleaving fields are inert:

  Setup      0xD5           Bank Interleaving             inert
  Setup      0xD6           Channel Interleaving          inert
  Setup      0xD7           Memory Clock                  inert (UCLK unchanged)
  AmdSetup   0x316          Memory Clock Speed            inert (UCLK unchanged)
  AmdSetup   0x703/0x704    DF Clock Override Ctrl/Val    inert (FCLK unchanged)

┌─ CAUTION ────────────────────────────────────────────────────────────────────────────┐
│  The APCB blocks are in the AMD-signed PSP region — corrupting them bricks the       │
│  board.  The CMOS timing path is the only reversible software memory-timing          │
│  surface.                                                                            │
└──────────────────────────────────────────────────────────────────────────────────────┘

  See: APCB Token Configuration — Live vs Decorative Settings — Chapter 2 → UMC / Memory
```

## Clock & Power Configuration Surface

```
── CLOCK & POWER CONFIGURATION ────────────────── the SMU owns; the layers set policy ──

Clock and voltage are owned by the SMU; the configuration layers only set
policy around it.  The four clock domains and their configuration surfaces:

┌─ CLOCK DOMAINS ──────────────────────────────────────────────────────────────────────┐
│  GFXCLK   SMN 0x16C00   SMU DPM; runtime control via amdgpu OverDrive (requires      │
│                         amdgpu.ppfeaturemask=0xffffffff)                             │
│  SOCCLK   SMN 0x16E00   SMU-owned; no configuration surface                          │
│  MCLK     SMN 0x17000   trained at boot from the AGESA-generated PowerPlay table;    │
│                         BIOS fields inert (see Memory Configuration)                 │
│  LCLK     SMN 0x17E00   SMU-owned; chipset-link ASPM policy via Setup 0x141          │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ CPU · GPU VOLTAGE · VRM ────────────────────────────────────────────────────────────┐
│  CPU clock      3.2 GHz base · 4.0 GHz boost — SMU-managed; no Linux cpufreq path.   │
│                 Config surface: APCB CBS 0x0005 (boost) · CBS 0x0006 (C-states) ·    │
│                 Setup 0x1C4 (Cool'n'Quiet) · Setup 0xB3 (CPB).                       │
│  GPU voltage    SVI2-only from the SMU.  The only working voltage-curve mechanism    │
│                 is the runtime OverDrive vc command via pp_od_clk_voltage (Chapter   │
│                 5) — no BIOS field reaches it.                                       │
│  VRM            BIOS loadline bytes (Setup 0x1CC / 0x1D0) are the sole live          │
│                 BIOS-side VRM controls — impedance only.                             │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ CAUTION ────────────────────────────────────────────────────────────────────────────┐
│  SMU mailbox discipline applies to any tool driving these domains — never send Q0    │
│  0x04 / 0x2E, or untested Q3 handlers (hang; power-cycle needed); never exceed one   │
│  mailbox message per 100 ms (Chapter 2).                                             │
└──────────────────────────────────────────────────────────────────────────────────────┘

 See: Live vs Decorative Settings — Chapter 2 → SMU / Power, Clock & Thermal — Chapter 5
```

## Kernel & Module Parameters

```
── KERNEL & MODULE PARAMETERS ────────────────────────────────── the reversible layer ──

The kernel command line is the reversible configuration layer and what makes
the board usable for compute.  Several parameters are load-bearing: without
them, large GPU allocations deadlock or the SMU registers are unreachable from
userspace.  The live production command line (CachyOS/BORE kernel, BIOS P3.00):

┌─ PRODUCTION COMMAND LINE ────────────────────────────────────────────────────────────┐
│  mitigations=off amd_iommu=off                                                       │
│  zswap.enabled=1 zswap.zpool=zsmalloc zswap.compressor=zstd                          │
│  ttm.pages_limit=4194304                                                             │
│  amdgpu.ppfeaturemask=0xffffffff amdgpu.noretry=0 amdgpu.gartsize=16384 amdgpu.dc=0  │
│  amdgpu.bc250_cc_write_mode=3 amdgpu.mtype_local=2 amdgpu.sched_policy=2             │
│  amdgpu.lockup_timeout=2000,2000,100,2000 amdgpu.num_kcq=4 amdgpu.cg_mask=0          │
│  pci=realloc,assign-busses iomem=relaxed console=tty0 console=ttyS1,115200           │
└──────────────────────────────────────────────────────────────────────────────────────┘

  PARAMETER                   VALUE                  WHY
  ttm.pages_limit             4194304                LOAD-BEARING — large GPU
                                                     allocations can deadlock without
                                                     it
  iomem                       relaxed                LOAD-BEARING — required for SMU
                                                     register access from userspace
                                                     tools
  amdgpu.ppfeaturemask        0xffffffff             LOAD-BEARING — enables
                                                     OverDrive/DPM/power management;
                                                     without it the GPU is locked to
                                                     base clock
  amdgpu.gartsize             16384                  16 GB GTT zero-copy pool; large
                                                     buffers map here
  amdgpu.noretry              0                      GPU page-fault retry enabled —
                                                     required for UMA coherency
  amdgpu.dc                   0                      display controller off
                                                     (headless); saves init time,
                                                     avoids DCN errors
  amdgpu.bc250_cc_write_mode  3                      gfx1013 liberation-patch runtime
                                                     control: cache-coherency write
                                                     mode
  amdgpu.mtype_local          2                      cache-coherent MTYPE for
                                                     local-memory allocations (UMA
                                                     coherency)
  amdgpu.sched_policy         2                      compute queues without the
                                                     hardware scheduler (works around
                                                     the harvested-die MEC)
  amdgpu.lockup_timeout       2000,2000,100,2000     per-ring lockup timeout, ms (gfx,
                                                     compute, sdma, video)
  amdgpu.num_kcq              4                      kernel compute queues (down from
                                                     the default 8)
  amdgpu.cg_mask              0                      driver-side clockgating features
                                                     all off
  amd_iommu                   off                    IOMMU fully disabled — no
                                                     translation overhead on the
                                                     DMA/GPU path
  pci                         realloc,assign-busses  PCI resource reallocation for BAR
                                                     sizing
  mitigations                 off                    speculative-execution mitigations
                                                     off on a dedicated compute node
  zswap.*                     1 / zsmalloc / zstd    compressed swap cache; reduces
                                                     NVMe wear under memory pressure
                                                     (max_pool_percent at the kernel
                                                     default, 20)
  console                     tty0 · ttyS1,115200    serial console for panic capture

Variants in use on sibling configurations: amdgpu.rebar=1, and a long
compute-ring lockup timeout (amdgpu.lockup_timeout=2000,60000,2000,2000) for
extended dispatches.

── WATCHDOG MODULE OPTIONS ─────────────────────────

On a headless board with no out-of-band management, auto-recovery on hang uses
the FCH TCO hardware watchdog, configured as static module options:

┌─ WATCHDOG — SP5100_TCO ──────────────────────────────────────────────────────────────┐
│  Module        sp5100_tco (FCH TCO timer)                                            │
│  Persistence   /etc/modules-load.d/sp5100_tco.conf                                   │
│  Timeout       heartbeat=30 in /etc/modprobe.d/sp5100_tco.conf; the device           │
│                reports an effective 60 s timeout at runtime                          │
│  Kicker        a userspace daemon opens /dev/watchdog0 and writes every 5 s          │
│                (12x margin against the effective 60 s timeout)                       │
│  Arming        the TCO timer counts only once something opens the device             │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ CAUTION ────────────────────────────────────────────────────────────────────────────┐
│  The TCO timer is disarmed until the watchdog device is opened — if the kicker       │
│  service is absent or stopped, there is no auto-recovery on hang.  Regression rule:  │
│  do not re-add a previously removed command-line parameter without recording why —   │
│  some debug overrides that work on stock RDNA hang the gfx1013-patched amdgpu.       │
└──────────────────────────────────────────────────────────────────────────────────────┘

     See: Configuration Layers — Chapter 5 · Driver Stack — Chapter 1 → Debug & Recovery
```


# 5. Driver Stack

```
┌──────────────────────────────────────────────────────────────────────────────────────┐
│                                                                                      │
│      D R I V E R   S T A C K                                       CHAPTER  5        │
│      Patched amdgpu kernel · Mesa userspace · LLVM backend · ROCm toolchain          │
│                                                                                      │
├──────────────────────────────────────────────────────────────────────────────────────┤
│                                                                                      │
│      KERNEL     amdgpu — 6-patch gfx1013 series      in-tree module · GCC-built      │
│      SMU        cyan_skillfish_ppt.c — PowerPlay     SMU fw 88.6.0 (0x00580600)      │
│      MESA       RADV (ACO) · radeonsi · rusticl      Vulkan · OpenGL · OpenCL 3.0    │
│      COMPILER   ACO (RADV) · LLVM AMDGPU             -mcpu=gfx1013 every other path  │
│      COMPUTE    all production via the GFX ring      MEC bypassed — silicon defect   │
│      ROCM       hipcc / HSACO · rocBLAS retarget     HSA_OVERRIDE_GFX_VERSION=10.1.0 │
│      PLATFORM   r8169 · ahci · xhci/ehci/ohci · i2c-piix4 (FCH) · k10temp (CPU die)  │
│                 nct6683 (Super I/O) available but not loaded                         │
│                                                                                      │
└──────────────────────────────────────────────────────────────────────────────────────┘

The driver stack brings the Oberon GPU up under Linux: the patched amdgpu kernel
module and its PowerPlay/SMU layer below; the Mesa userspace (RADV/ACO, radeonsi,
rusticl), the LLVM AMDGPU backend, and the ROCm toolchain above.  On gfx1013 the
stock stack does not work — the silicon requires a six-patch kernel series, a
cache-coherent memory model, and a compute routing that avoids a defective command
processor.  The carrier's platform devices — network, FCH SATA/USB/SMBus, Super
I/O — bind stock Linux drivers and close the chapter.

     HIP app             Vulkan app            OpenGL / OpenCL app
        │                    │                          │
   libamdhip64           RADV (ACO)         radeonsi / rusticl (LLVM)
        │                    │                          │
   ROCr / ROCt             libdrm ──────────────────── libdrm
        │                    │                          │
     /dev/kfd         /dev/dri/card1 (DRM/GEM ioctls)   │
        │                    │                          │
        └────────────────────┼──────────────────────────┘
                             │
                   amdgpu kernel module
                             │
             cyan_skillfish_ppt → SMU mailbox (MP1)
                             │
              GFX ring (ME)        [ MEC: defective — disabled ]
                             │
                        gfx1013 GPU

── SECTIONS ────────────────────────────────────────── kernel → userspace → toolchain ──

  amdgpu kernel module              Mesa Userspace — RADV / radeonsi / rusticl
  Applied Patch Ledger              LLVM AMDGPU Backend and Target Features
  PowerPlay / SMU Driver Layer      ROCm Toolchain
  GPUVM Memory Model                Kernel Boot Parameters
  KFD / HSA Compute Interface       Platform Drivers
                                    Firmware and VBIOS Identifiers

        See: Chapter 2 · Subsystem Internals (GPU, SMU, UMC) — Chapter 6 · Compute Stack
                     Chapter 4 · System Configuration (UMA carveout, boot configuration)
```

## amdgpu kernel module

```
── AMDGPU KERNEL MODULE ───────────────────────────────── patched in-tree · GCC-built ──

The amdgpu module is the foundation of all GPU access: it binds the PCI device
(1002:13FE), enumerates the IP blocks, and owns the ring, buffer-object, and fence
infrastructure plus the sysfs control plane.  On gfx1013 it must be built from the
local kernel source and patched — upstream assumes a discrete-GPU memory model.
GC 10.1.3 brings up the shader core: 40 CU on die, of which the stock harvest
activates 24; a kernel patch enables the fused 16 (Chapter 2 → GPU).

┌─ BUILD & INSTALL ────────────────────────────────────────────────────────────────────┐
│  PCI device       1002:13FE → /dev/dri/card1                                         │
│  Compute node     /dev/kfd (CONFIG_HSA_AMD=y)                                        │
│  Build            make M=drivers/gpu/drm/amd/amdgpu modules — the running module     │
│                   is GCC-built (GCC 16.1.1, matching the kernel; no clang build      │
│                   strings)                                                           │
│  Install          in place over /usr/lib/modules/$KVER/kernel/drivers/gpu/drm/       │
│                   amd/amdgpu/ (no updates/ overlay on the running kernel), then      │
│                   mkinitcpio -P                                                      │
│  Source tree      /usr/lib/modules/$(uname -r)/build/ — never a mismatched           │
│                   upstream tree                                                      │
│  Re-patching      pacman hook 90-amdgpu-bc250.hook re-applies the series and         │
│                   rebuilds on kernel upgrade; the kernel packages are                │
│                   additionally held via IgnorePkg                                    │
│  Firmware         SMU 88.6.0 ships in the BIOS — the APU loads no separate GPU       │
│                   microcode                                                          │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ IP BLOCKS ──────────────────────────────────────────────────────────────────────────┐
│  GC        10.1.3   shader execution — 40 CU on die, stock 24 active        working  │
│  SDMA      5.0.1    system DMA, page-table updates                          working  │
│  GMC       10.0     GPUVM, page tables, apertures                           patched  │
│  SMU       11.0.8   power, clocks, voltage, thermals                        patched  │
│  VCN       2.0      video decode/encode                  firmware-locked (disabled)  │
│  DCN/DMU   2.0.3    display output (headless; amdgpu.dc=0)                  working  │
│  gfxhub    2.1      GFX address-translation TLB                             patched  │
│  mmhub     2.3      system address-translation TLB                          patched  │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ BOOT SEQUENCE — PCI probe → early_init / sw_init / hw_init / late_init per block ───┐
│  GMC   configures GPUVM, GART, and the apertures                                     │
│  SMU   checks firmware, sets the AUTO performance level                              │
│  GC    enables the compute units                                                     │
│  VCN   hw_init fails non-fatally (power island firmware-locked) → disabled           │
│  KFD   exposes /dev/kfd                                                              │
│  DRM   registers /dev/dri/card1                                                      │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ MODULE PARAMETERS — function-critical · boot-line placement → Boot Parameters ──────┐
│  amdgpu.gartsize             16384        16 GB GTT pool — without it, only the      │
│                                           small carveout                             │
│  amdgpu.ppfeaturemask        0xffffffff   unlocks SMU power management (clock        │
│                                           control)                                   │
│  amdgpu.noretry              0            enables page-fault retry                   │
│  amdgpu.dc                   0            display core off — headless                │
│  amdgpu.sched_policy         2            round-robin GPU scheduler for compute      │
│  amdgpu.num_kcq              4            kernel compute queues (set on the boot     │
│                                           line; unset default 8)                     │
│  amdgpu.cg_mask              0            clock gating disabled (boot line)          │
│  amdgpu.mtype_local          2            MTYPE_CC for local memory (boot line)      │
│  amdgpu.lockup_timeout       2000,2000,100,2000                                      │
│                                           per-ring hang detection ms: GFX,           │
│                                           compute, SDMA, video                       │
│  amdgpu.bc250_cc_write_mode  3            BC-250 cache-coherent write mode —         │
│                                           3 = full-40CU-liberation                   │
│  ttm.pages_limit             4194304      16M-page TTM pool — too small              │
│                                           deadlocks large allocations                │
└──────────────────────────────────────────────────────────────────────────────────────┘

num_kcq=4, cg_mask=0, and mtype_local=2 are all set explicitly on the boot line —
cg_mask=0 and mtype_local=2 are not driver defaults.  amdgpu.gttsize is superseded
by gartsize (left at -1); amdgpu.pg_mask is not set (the power-gating default
0xffffffff is correct).

┌─ VERIFICATION — the initramfs loads modules before root mounts ──────────────────────┐
│  srcversion   cat /sys/module/amdgpu/srcversion vs modinfo   built module is         │
│                                                              the running one         │
│  clock path   dmesg | grep -E 'ForceGfx|UnForce'             ForceGfxFreq path       │
│                                                              (not RequestGfxclk)     │
│  DPM levels   cat /sys/class/drm/card1/device/pp_dpm_sclk    three levels incl.      │
│                                                              2230 MHz listed         │
│  hook ran     journalctl -b | grep amdgpu                    rebuild hook ran on     │
│                                                              kernel install          │
└──────────────────────────────────────────────────────────────────────────────────────┘

Recovery: reinstall the kernel package (restoring the stock in-tree module) or
restore a .stock/.bak backup where one is kept alongside the module — older
patched trees carry them; the running tree keeps none — then depmod -a,
mkinitcpio -P, reboot.  A previous kernel can also be booted from GRUB (several
module trees remain installed).

Earlier out-of-tree builds showed sporadic ring resets attributed to GCC codegen
differences in atomic-ordering and branch-hint paths and were built clang-only;
the current production module is GCC 16.1.1-built (in-tree rebuild) and stable —
the compiler sensitivity applied to the old out-of-tree flow.

┌─ CAUTION ────────────────────────────────────────────────────────────────────────────┐
│  Never read any amdgpu debugfs file — reading the debugfs tree (notably              │
│  amdgpu_regs) wedges the GPU instantly and requires a power-cycle.                   │
│  rmmod amdgpu cannot unload cleanly (module in use); PCI unbind/rebind causes a      │
│  kernel null-pointer crash; halting the MEC (CP_MEC_CNTL=0x11) crashes the           │
│  system (KIQ serves display).                                                        │
│  If the SMU mailbox is jammed, blacklist amdgpu before rebooting — driver init       │
│  against a jammed SMU hangs the next boot; a full power cycle clears the jam.        │
└──────────────────────────────────────────────────────────────────────────────────────┘

        See: Applied Patch Ledger — Chapter 2 → GPU — Chapter 6 → compute dispatch paths
```

## Applied Patch Ledger

```
── APPLIED PATCH LEDGER ──────────────────────────────────── six patches · four files ──

Six patches across four source files bring stock amdgpu to gfx1013 working state.
They are stored in a local patch repository and re-applied automatically on kernel
upgrade by a pacman hook.

┌─ PATCH SERIES ───────────────────────────────────────────────────────────────────────┐
│  1  Clock + voltage    cyan_skillfish_ppt.c   replaces broken RequestGfxclk with     │
│                                               ForceGfxFreq / ForceGfxVid; adds       │
│                                               V/F curve (1000-2230 MHz), 3 DPM       │
│                                               levels, voltage coordination           │
│  2  MTYPE=CC default   gmc_v10_0.c            compute BOs default to MTYPE=CC —      │
│                                               the stock NC default ring-resets       │
│                                               on compute dispatch (MANDATORY)        │
│  3  APU prefer GTT     amdgpu_ttm.c           routes UMA allocations to GTT when     │
│                                               a fixed UMA carveout is forced;        │
│                                               adds SNOOPED+SYSTEM PTE flags to       │
│                                               VRAM (conditional — not needed         │
│                                               with BIOS UMA=Auto)                    │
│  4  gfxhub L1 TLB      gfxhub_v2_1.c          MTYPE_CC for coherent GFX address      │
│                                               translation                            │
│  5  mmhub L1 TLB       mmhub_v2_3.c           MTYPE_CC for coherent system           │
│                                               (SDMA/VCN/display) address             │
│                                               translation                            │
│  6  VCN fast-skip      vcn_v2_0.c             early return in vcn_v2_0_hw_init()     │
│                                               — skips ring/IB tests the              │
│                                               firmware-locked power island can       │
│                                               never pass                             │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ SUPPLEMENTARY — gmc_v10_0.c (introduced with the BORE kernel rebuild) ──────────────┐
│  TLB KIQ bypass    gmc_v10_0_flush_gpu_tlb() skips the KIQ path, uses direct         │
│                    MMIO — the part hangs on the KIQ TLB-invalidation path            │
│  MTYPE CC default  default MTYPE set to CC; override remains NC for RADV-safe        │
│                    BOs                                                               │
│  GART CC+SNOOP     GART PTEs set MTYPE_CC + SNOOPED (stock: uncached, no-snoop)      │
└──────────────────────────────────────────────────────────────────────────────────────┘

The MTYPE patch is surgical: it keys on the buffer-object coherency flag, so BOs
created with the coherent flag map to CC and everything else remains NC — this is
how coherent (KFD) and non-coherent (RADV) consumers coexist on one module.  The
patch also restores the TLB invalidation an earlier broad patch had skipped, and
retains invalidation engine 14 (engine 17 is dead on this silicon).

┌─ DELIBERATELY NOT APPLIED ───────────────────────────────────────────────────────────┐
│  vram_base_offset   broke the rusticl path; redundant once apu_prefer_gtt was        │
│                     in place                                                         │
│  VCN power-gate     the boot-time SDMA fence timeouts it addresses are cosmetic      │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ CAUTION ────────────────────────────────────────────────────────────────────────────┐
│  In the patch-1 OD path, voltage MUST be set before frequency — reverse              │
│  ordering violates SVI2 sequencing and can crash the SMU (see PowerPlay).            │
└──────────────────────────────────────────────────────────────────────────────────────┘

        See: GPUVM Memory Model (PTE flag semantics) — PowerPlay / SMU Layer (patch 1)
```

## PowerPlay / SMU Driver Layer (cyan_skillfish_ppt)

```
── POWERPLAY / SMU DRIVER LAYER ──────────────────────────────── cyan_skillfish_ppt.c ──

cyan_skillfish_ppt.c is the ASIC-specific PowerPlay layer: it translates sysfs
DPM and voltage requests into SMU mailbox commands.  It sits below the generic
dispatch and above the generation-common layer and the mailbox transport.

     sysfs (amdgpu_pm.c)          control plane
             │
     amdgpu_smu.c                 lifecycle / dispatch
             │
     cyan_skillfish_ppt.c         ASIC-specific callbacks
             │
     smu_v11_0.c                  generation-common
             │
     smu_cmn.c                    mailbox transport
             │
     SMU firmware 88.6.0          MP1 / SMU 11.0.8

┌─ CALLBACK ACCOUNTING ────────────────────────────────────────────────────────────────┐
│  Implemented (11)   force_clk_levels · set_performance_level · od_edit_dpm_table     │
│                     · read_sensor · get_gpu_metrics · print_clk_levels ·             │
│                     is_dpm_running · dpm_set_vcn_enable · get_dpm_ultimate_freq      │
│                     · get_enabled_mask · init_smc_tables                             │
│  Inherited (9)      check_fw_status · check_fw_version · init_power · fini_power     │
│                     · fini_smc_tables · register_irq_handler ·                       │
│                     notify_memory_pool_location · set_driver_table_location ·        │
│                     interrupt_work                                                   │
│  NULL (~30+)        no power limits, fan control, workload profiles, thermal         │
│                     alerts, PCIe DPM, BACO, reset, I2C, or microcode load —          │
│                     correct, not bugs: the silicon does not expose these             │
│                     features                                                         │
└──────────────────────────────────────────────────────────────────────────────────────┘

get_enabled_mask returns all-ones (faked).  Only 15 of the 38 defined SMU
messages are mapped.  DPM tables carry three sclk levels (1000 / 2000 / 2230 MHz)
and one mclk level (450 MHz).

┌─ MAILBOX PROTOCOL — set by smu_v11_0_set_smu_mailbox_registers ──────────────────────┐
│  MSG      MP1_SMN_C2PMSG_66     write message ID — triggers execution                │
│  PARAM    MP1_SMN_C2PMSG_82     write parameter                                      │
│  RESP     MP1_SMN_C2PMSG_90     read response                                        │
├──────────────────────────────────────────────────────────────────────────────────────┤
│  SEND     mutex_lock → poll RESP until non-zero (previous done) → write RESP=0       │
│           → write PARAM → write MSG (triggers the SMU) → poll RESP until             │
│           non-zero (completion) → read RESP for status and PARAM for the             │
│           return value → mutex_unlock                                                │
│  CODES    0x01 OK · 0xFF Failed · 0xFE Unknown · 0xFD BadPrereq · 0xFC Busy          │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ MESSAGE MAP — mapped by the driver (15 of 38) ──────────────────────────────────────┐
│  0x01  TestMessage                  echo test                                        │
│  0x02  GetSmuVersion                read firmware version                            │
│  0x03  GetDriverIfVersion           interface compatibility                          │
│  0x04  SetDriverTableDramAddrHigh   metrics DMA setup                                │
│  0x05  SetDriverTableDramAddrLow    metrics DMA setup                                │
│  0x06  TransferTableSmu2Dram        pull metrics from SMU                            │
│  0x07  TransferTableDram2Smu        push config to SMU                               │
│  0x0B  PowerDownVcn                 VCN power control                                │
│  0x0C  PowerUpVcn                   VCN power control                                │
│  0x0E  RequestGfxclk                soft clock request                               │
│  0x39  ForceGfxFreq                 lock GPU clock                                   │
│  0x3A  UnForceGfxFreq               release clock lock                               │
│  0x3B  ForceGfxVid                  lock GPU voltage (SVI2 VID)                      │
│  0x3C  UnforceGfxVid                release voltage lock                             │
│  0x3D  GetEnabledSmuFeatures        query feature mask                               │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ DEFINED BUT UNUSED (selection) ─────────────────────────────────────────────────────┐
│  0x0F  QueryGfxclk                    0x1C  StopTelemetryReporting                   │
│  0x11  QueryVddcrSocClock             0x1D  ClearTelemetryMax                        │
│  0x13  QueryDfPstate                  0x1E  QueryActiveWgp                           │
│  0x18  RequestActiveWgp               0x2C  SetCoreEnableMask                        │
│  0x19  SetMinDeepSleepGfxclkFreq      0x2E  InitiateGcRsmuSoftReset                  │
│  0x1A  SetMaxDeepSleepDfllGfxDiv      0x35  SetSoftMinCclk                           │
│  0x1B  StartTelemetryReporting        0x36  SetSoftMaxCclk                           │
│                                       0x37  GetGfxFrequency                          │
│                                       0x38  GetGfxVid                                │
└──────────────────────────────────────────────────────────────────────────────────────┘

The patched driver carries a conservative piecewise voltage/frequency curve
(cyan_skillfish_freq_to_mv()) keyed on GFXCLK, and encodes voltage on the SVI2
bus as a VID code.  The encoding is fixed:

┌─ SVI2 VID ENCODING ──────────────────────────────────────────────────────────────────┐
│  vid = (1550 - voltage_mV) * 160 / 1000                                              │
│                                                                                      │
│  magic argument 0x13FE (5118) = "let the SMU manage voltage" — on receipt the        │
│  driver sends UnforceGfxVid rather than a forced code                                │
└──────────────────────────────────────────────────────────────────────────────────────┘

(The tuned per-die voltage points along the curve are dynamic and belong to the
companion tuning tools, not this reference.)

┌─ PATCH-1 KEY FUNCTIONS ──────────────────────────────────────────────────────────────┐
│  cyan_skillfish_freq_to_mv(freq_mhz)              piecewise V/F lookup               │
│  cyan_skillfish_force_voltage(smu, mv)            mV → SVI2 VID → ForceGfxVid        │
│                                                   (0 or magic value unforces)        │
│  cyan_skillfish_force_gfxclk(smu, min, max)       voltage BEFORE frequency,          │
│                                                   then ForceGfxFreq;                 │
│                                                   min==SCLK_MIN && max==             │
│                                                   SCLK_MAX unforces both             │
│  cyan_skillfish_force_clk_levels(smu, type, mask) backs pp_dpm_sclk; levels          │
│                                                   0=1000, 1=2000, 2=2230 MHz         │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ SYSFS CONTROL PLANE — /sys/class/drm/card1/device/ ─────────────────────────────────┐
│  power_dpm_force_performance_level   manual / auto — must be manual before           │
│                                      forcing                                         │
│  pp_dpm_sclk                         level bitmask → force_clk_levels →              │
│                                      force_gfxclk → ForceGfxVid then                 │
│                                      ForceGfxFreq                                    │
│  pp_od_clk_voltage                   vc <pt> <MHz> <mV>  stage a voltage-curve       │
│                                      point                                           │
│                                      s <pt> <MHz> · v <pt> <mV>  stage freq /        │
│                                      voltage                                         │
│                                      c  commit: force_voltage then force_gfxclk      │
└──────────────────────────────────────────────────────────────────────────────────────┘

A pp_dpm_sclk write returns "Invalid argument" unless
power_dpm_force_performance_level is manual first.  Passing the magic value on a
voltage edit returns voltage management to the SMU V/F curve.  During
smu_late_init the driver calls set_performance_level(AUTO) and unforces both
voltage and frequency, leaving the SMU in DPM-managed mode.

┌─ PINNING THE CLOCK — both methods coordinate voltage automatically ──────────────────┐
│  # Method 1 — DPM level (level 2 = 2230 MHz)                                         │
│  echo manual | sudo tee /sys/class/drm/card1/device/power_dpm_force_performance_level│
│  echo 2      | sudo tee /sys/class/drm/card1/device/pp_dpm_sclk                      │
│  echo auto   | sudo tee /sys/class/drm/card1/device/power_dpm_force_performance_level│
│                                                                                      │
│  # Method 2 — OD interface, auto voltage (magic 5118 = V/F curve decides)            │
│  echo 's 0 2230' | sudo tee /sys/class/drm/card1/device/pp_od_clk_voltage            │
│  echo 'v 0 5118' | sudo tee /sys/class/drm/card1/device/pp_od_clk_voltage            │
│  echo 'c'        | sudo tee /sys/class/drm/card1/device/pp_od_clk_voltage            │
└──────────────────────────────────────────────────────────────────────────────────────┘

Raising a curve-point voltage at a target frequency (vc <point> <MHz> <mV>; c) is
what unblocks the SMU DPM scheduler from refusing higher levels; the in-driver
V/F curve supplies the matching voltage.

┌─ TELEMETRY — SmuMetrics_t, table ID 6 — DMA via TransferTableSmu2Dram ───────────────┐
│  CoreFrequency[6] · CorePower[6] · CoreTemperature[6]      per-core                  │
│  L3Frequency[2] · L3Temperature[2]                         per-L3-slice              │
│  GfxclkFrequency · GfxTemperature                          GPU                       │
│  SocclkFrequency · VclkFrequency · DclkFrequency ·         SOC / media / memory      │
│  MemclkFrequency                                           clocks                    │
│  Voltage[2] · Current[2] · Power[2]                        [SOC, GFX]                │
│  CurrentSocketPower · ThrottlerStatus                      package / throttle        │
│                                                            bitmap                    │
└──────────────────────────────────────────────────────────────────────────────────────┘

get_gpu_metrics fills a gpu_metrics_v2_2 structure from these fields;
read_sensor maps individual PP sensors onto them.

┌─ CAUTION ────────────────────────────────────────────────────────────────────────────┐
│  Voltage before frequency, ALWAYS — raising the clock at the default low             │
│  voltage causes voltage sag and a crash within milliseconds; the force_gfxclk        │
│  helper enforces the ordering in-kernel.                                             │
│  Never send ForceGfxVid with arg=0 — it crashes the SMU; release voltage with        │
│  the magic value 5118 (0x13FE) / UnforceGfxVid instead.                              │
│  At AUTO performance level (as during smu_late_init) the SMU accepts                 │
│  UnforceGfxVid / UnForceGfxFreq but NOT ForceGfxVid — adding ForceGfxVid to          │
│  the AUTO path hangs the SMU.                                                        │
│  An SMU hang generally requires a full power-cycle; the SMU re-initializes           │
│  from PSP firmware on cold boot with no persistent damage.                           │
└──────────────────────────────────────────────────────────────────────────────────────┘

   See: Chapter 2 → SMU (mailbox queues, SMUIO) — Chapter 1 → Power Delivery (SVI2, VRM)
```

## GPUVM Memory Model

```
── GPUVM MEMORY MODEL ────────────────────────────────────── one DRAM · two apertures ──

GPUVM maps GPU virtual addresses to physical GDDR6 through page tables in memory,
with routing decided by address range at the MMHUB.  On this unified-memory part
the VRAM/GTT split is a software abstraction over one physical DRAM — the same
die layout as the console original (Garlic = VRAM/WC, Onion = GTT/WB).

┌─ MEMORY POOLS ───────────────────────────────────────────────────────────────────────┐
│  VRAM pool   BIOS carveout at 0xF400000000 (256 MB with UMA=Auto; 512 MB when a      │
│              fixed UMA size is forced) — default MTYPE_UC, reached via BAR0          │
│  GTT pool    16,384 MB (set by amdgpu.gartsize=16384) — reached via GART             │
│              through the system aperture                                             │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ APERTURE ROUTING (MMHUB) ───────────────────────────────────────────────────────────┐
│  System aperture   full system RAM → Data Fabric → memory controller → GDDR6         │
│                    used by Mesa paths, GTT, host-mapped allocations                  │
│  FB aperture       0xF400000000 via BAR0 → frame buffer (256 MB visible) → GDDR6     │
│                    backs the default discrete-VRAM allocation path                   │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ PTE FLAGS & MTYPE ENCODINGS ────────────────────────────────────────────────────────┐
│  SNOOPED         CPU cache-coherency participation — required for                    │
│                  system-aperture routing                                             │
│  SYSTEM          route through the system aperture — required for the                │
│                  full-fabric path                                                    │
│  MTYPE_NC (0)    non-coherent caching — GPU-cached, no CPU coherency                 │
│  MTYPE_WC (1)    write-combining — writes gathered before commit                     │
│  MTYPE_CC (2)    cache-coherent — GPU snoops the CPU cache                           │
│  MTYPE_UC (3)    uncached — every access hits DRAM                                   │
│  READABLE / WRITEABLE / EXECUTABLE    standard; EXECUTABLE required for shader       │
│                                       code                                           │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ CORRECT FLAG SETS ON UMA ───────────────────────────────────────────────────────────┐
│  VRAM (patched)   SYSTEM | SNOOPED | MTYPE_CC | READABLE | WRITEABLE | EXECUTABLE    │
│  GTT              SNOOPED | MTYPE_CC | READABLE | WRITEABLE                          │
│  GART (stock)     MTYPE_UC | EXECUTABLE — patched to MTYPE_CC + SNOOPED              │
└──────────────────────────────────────────────────────────────────────────────────────┘

Stock VRAM PTEs default to MTYPE_UC with SNOOPED missing, which forces the slow
frame-buffer aperture; the TTM patch corrects this.  The stock MTYPE_NC compute
default ring-resets the part on essentially every compute dispatch — the MTYPE=CC
default is mandatory for functional compute.

  per-CU L0 TLB ──→ gfxhub L1 TLB (GFX: compute, graphics) ──┐
                                                             ├─→ L2 TLB → page-
                    mmhub  L1 TLB (SDMA, VCN, display) ──────┘   table walk in
                                                                 GDDR6

Both L1 TLBs carry MTYPE configuration registers, patched to CC on this silicon.
Without the correct MTYPE the coherent path serves stale entries that corrupt
compute buffers and mimic shader bugs.  TLB invalidation between dispatches is
mandatory — RADV remaps constantly, and skipping invalidation produces
stale-entry corruption.

Practical rule: keep the VRAM carveout minimal, route everything through the
GTT/system aperture, set apu_prefer_gtt=true only when the BIOS forces a fixed
UMA size (unnecessary with UMA=Auto), and never skip TLB invalidation.

┌─ CAUTION ────────────────────────────────────────────────────────────────────────────┐
│  Never set vm_block_size=11 (R-Mode) — the gfx1013 page-table walker                 │
│  mishandles this block size and faults during translation.                           │
│  TLB flushes must use the direct-MMIO path, not KIQ — the part hangs on KIQ          │
│  invalidation (see the patch ledger).                                                │
└──────────────────────────────────────────────────────────────────────────────────────┘

     See: Chapter 2 → UMC / Memory, MMHUB — Chapter 4 → BIOS UMA — Applied Patch Ledger
```

## KFD / HSA Compute Interface

```
── KFD / HSA COMPUTE INTERFACE ──────────────────────────────── /dev/kfd · AQL queues ──

KFD (Kernel Fusion Driver) is the kernel interface for AMD GPU compute; HSA is
the userspace runtime above it.  KFD provides per-process GPU virtual address
spaces, buffer-object allocation, AQL queues, HSA signals, and events.

  HIP app → libamdhip64.so → libhsa-runtime64.so (ROCr) → libhsakmt.so
  (ROCt/thunk) → /dev/kfd → amdgpu

┌─ INTERFACE ──────────────────────────────────────────────────────────────────────────┐
│  Device     /dev/kfd (single character device, CONFIG_HSA_AMD=y)                     │
│  Topology   /sys/class/kfd/kfd/topology/nodes/*/properties                           │
│  Queues     AQL — 64-byte packets, ring buffer in system memory,                     │
│             doorbell-signalled, no kernel involvement per dispatch                   │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ KEY IOCTLS ─────────────────────────────────────────────────────────────────────────┐
│  KFD_IOC_CREATE_QUEUE             create AQL command queue                           │
│  KFD_IOC_DESTROY_QUEUE            destroy queue                                      │
│  KFD_IOC_ALLOC_MEMORY_OF_GPU      allocate GPU BO                                    │
│  KFD_IOC_FREE_MEMORY_OF_GPU       free BO                                            │
│  KFD_IOC_MAP_MEMORY_TO_GPU        map BO into the GPU address space                  │
│  KFD_IOC_UNMAP_MEMORY_FROM_GPU    unmap BO                                           │
│  KFD_IOC_GET_PROCESS_APERTURES    query aperture info                                │
│  KFD_IOC_SET_SCRATCH_BACKING_VA   set scratch backing for private variables          │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ BO COHERENCY FLAG → MTYPE — the surgical MTYPE patch keys on these ─────────────────┐
│  AMDGPU_GEM_CREATE_COHERENT       CC   CPU-GPU coherent                              │
│  AMDGPU_GEM_CREATE_EXT_COHERENT   CC   extended coherency                            │
│  AMDGPU_GEM_CREATE_UNCACHED       UC   no caching                                    │
│  (no flag)                        NC   stock default                                 │
└──────────────────────────────────────────────────────────────────────────────────────┘

This mapping is how ROCm and RADV coexist on one module.

gfx1013 specifics: XNACK (page-fault retry) is disabled on this silicon — GPU
page faults are fatal, all memory must be pre-mapped before launch, and there is
no demand paging.  Four kernel compute queues are enabled (num_kcq=4, set on the
boot line).  HSA_OVERRIDE_GFX_VERSION=10.1.0 makes the HSA runtime load gfx1010
code objects, since no native gfx1013 target exists.

┌─ CAUTION ────────────────────────────────────────────────────────────────────────────┐
│  KFD compute queues live on the MEC, which carries a silicon-level store-hang        │
│  defect — never rely on non-atomic global stores through the MEC.  The               │
│  production compute paths (Vulkan, rusticl, raw PM4) bypass KFD entirely and         │
│  run on the GFX ring.                                                                │
└──────────────────────────────────────────────────────────────────────────────────────┘

          See: Mesa Userspace (the GFX-ring bypass) — Chapter 6 → compute dispatch paths
```

## Mesa Userspace — RADV / radeonsi / rusticl

```
── MESA USERSPACE ───────────────────────────────────────── RADV · radeonsi · rusticl ──

The BC-250 uses the fully open-source Linux graphics stack; there is no
proprietary driver.  Mesa provides three consumer paths over libdrm and the
amdgpu module — and, critically, detects gfx1013 and disables MEC compute queues
(info->ip[AMD_IP_COMPUTE].num_queues = 0 in ac_gpu_info.c), routing all compute
through AMDGPU_HW_IP_GFX.  This is what makes compute work on gfx1013 at all.

┌─ CONSUMER PATHS ─────────────────────────────────────────────────────────────────────┐
│  Vulkan       RADV       compiler ACO    GFX ring   production compute path          │
│  OpenGL       radeonsi   compiler LLVM   GFX ring                                    │
│  OpenCL 3.0   rusticl    compiler LLVM   GFX ring   via radeonsi (NIR → ACO          │
│                                                     backend)                         │
└──────────────────────────────────────────────────────────────────────────────────────┘

Mesa tracks the rolling repo (26.0.3 at verification; it is not held in
IgnorePkg — only the patched kernel packages are pinned there).  A Mesa upgrade
once introduced a UMA-buffer correctness regression, so upgrades are gated
behind a correctness regression test.  AMDVLK exists as an alternative Vulkan
driver but RADV+ACO is preferred.

── THE MEC DEFECT AND THE GFX-RING BYPASS ───────────────────── silicon, not firmware ──

The MEC (compute command processor) cannot complete non-atomic global VMEM
stores on this silicon; atomic operations complete normally, and the ME on the
GFX ring handles the same compute operations correctly.  A firmware swap (stock
cyan_skillfish2_mec.bin ucode v144 replaced with navi10_mec.bin v156, both IP
version 10.1) produced identical hangs, confirming a silicon rather than
firmware defect.  The hang occurs with every combination of GLC/DLC/SLC
coherency flags.

                      MEC dispatch
                     /            \
            atomic store        non-atomic store
                 │                     │
            GL2 atomic          GL1 write-through
            processing          → GL2 write buffer
                 │                     │
             completes          coalesced write-back → HANGS

┌─ WORKING COMPUTE PATHS — all via the GFX ring ───────────────────────────────────────┐
│  Raw PM4 dispatch   libdrm → GFX ring                    verified                    │
│  Vulkan compute     RADV → GFX ring                      production                  │
│  OpenCL 3.0         rusticl → radeonsi → GFX ring        standard API                │
│  Atomic-only HIP    ROCm → KFD → MEC                     functional but limited      │
│                                                          (legacy)                    │
└──────────────────────────────────────────────────────────────────────────────────────┘

An earlier mitigation primed undocumented SPI registers that the GFX-ring
firmware programs on first dispatch but the MEC firmware leaves uninitialised
(leaving MEC pipes stalled on VMEM completion).  Documented for reference; the
approach was superseded by the GFX-ring routing:

┌─ SUPERSEDED MITIGATION — SPI register priming (reference only) ──────────────────────┐
│  0x8784-0x8787   boot 0x01050105 → 0x01850185   SPI shader config (x4 SE)            │
│  0x8788-0x878b   boot 0x06000600 → 0x05400540   SPI shader config (x4 SE)            │
│  0x8794-0x8797   boot 0x01540154 → 0x00060006   SPI shader config (x4 SE)            │
│  0x9114          boot 0x00000003 → 0x00000000   MEC pipe stall control               │
└──────────────────────────────────────────────────────────────────────────────────────┘

0x9114 is writable via SMN (PCI config 0xB8/0xBC indirect); the 0x8784-0x8797
range is read-only via SMN but writable via kernel MMIO (WREG32).  Raw register
writes bypass command-processor pipeline synchronisation and can leave the
hardware state machine inconsistent — PM4 packets via the GFX ring are the
correct mechanism.

── RADV DRIVER INTERNALS ──────────────────────────────────────────── src/amd/vulkan/ ──

RADV translates Vulkan calls into PM4 packets for the amdgpu module.

┌─ RADV INTERNALS ─────────────────────────────────────────────────────────────────────┐
│  Submission    command buffers of PM4 packets, chained as indirect buffers           │
│                (IBs); vkQueueSubmit → kernel ioctl → ring-buffer write →             │
│                doorbell                                                              │
│  Memory        DRM/GEM ioctls (no KFD/HSA dependency)                                │
│  UMA           device->uma set for integrated GPUs: CPU memcpy instead of            │
│                DMA-engine transfers, no staging copies, prefer_host_memory           │
│                for HOST_VISIBLE zero-copy allocation                                 │
│  Descriptors   allocate/update/bind/dispatch; or vkCmdPushDescriptorSetKHR +         │
│                dispatch                                                              │
│  Cache         on-disk shader cache ~/.cache/mesa_shader_cache/ keyed on             │
│                SPIR-V hash + pipeline state (RADV_DEBUG=nocache disables)            │
│  Queues        single-queue model — compute submits over the GFX ring                │
│  gfx1013       has_image_bvh_intersect_ray (gfx_level >= GFX10_3 ||                  │
│                CHIP_GFX1013), GFX10_A encoding flag, NSA address packing for         │
│                BVH operands                                                          │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ DEBUG INTERFACES — comma-separated environment-variable flag sets ──────────────────┐
│  RADV_DEBUG      shaders · asm · cs · nir · ir · preoptir · shaderstats ·            │
│                  spirv · hang · nocache · llvm (debug-build-only backend) ·          │
│                  info / startup                                                      │
│  ACO_DEBUG       validateir · validatera · noopt · nosched · perfinfo ·              │
│                  liveinfo · force-waitcnt                                            │
│  RADV_PERFTEST   cswave32 · sam / nosam                                              │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ RADV_DEBUG=hang — dump directory ───────────────────────────────────────────────────┐
│  pipeline.log     shader IR + wave PCs                                               │
│  trace.log        annotated command stream with hang location                        │
│  umr_waves.log    active waves + registers                                           │
│  bo_history.log   allocation timeline                                                │
│  registers.log    GPU state                                                          │
│  gpu_info.log     hardware config                                                    │
└──────────────────────────────────────────────────────────────────────────────────────┘

── ACO — THE RADV SHADER BACKEND ────────────────────────────────── src/amd/compiler/ ──

ACO was written specifically for RDNA and understands RDNA register pressure and
instruction scheduling.  Its allocator respects the 256-VGPR hard ceiling — a
key reason RADV is stable where LLVM-based codegen can overflow it; it compiles
substantially faster than LLVM and produces better bandwidth-bound compute code
on gfx1013.  Wave64 is native on both backends.

── OPENCL: RUSTICL VS THE ROCM ICD ──────────────────────── /etc/OpenCL/vendors/*.icd ──

OpenCL dispatches through an ICD loader (libOpenCL.so) reading
/etc/OpenCL/vendors/*.icd.  Two implementations target gfx1013:

┌─ ICD COMPARISON ─────────────────────────────────────────────────────────────────────┐
│                       rusticl (Mesa)           ROCm OpenCL (amdocl64)                │
│  Platform name        rusticl                  AMD Accelerated Parallel              │
│                                                Processing                            │
│  Library              libMesaOpenCL.so         libamdocl64.so                        │
│  Compiler             NIR → ACO                LLVM AMDGPU                           │
│  Memory path          DRM/GEM (radeonsi)       KFD/HSA                               │
│  OpenCL C version     1.2                      2.0                                   │
│  SVM                  none                     coarse-grained                        │
│  Dispatch path        GFX ring                 KFD/MEC                               │
└──────────────────────────────────────────────────────────────────────────────────────┘

rusticl works on gfx1013 (standard OpenCL 3.0 API, 1024-thread workgroups).  The
ROCm ICD dispatches through KFD/MEC, does not honor Mesa's num_queues=0, and
initializes MEC queues inside clGetPlatformIDs() — hanging the entire ICD loader
before any user code runs.

┌─ CAUTION ────────────────────────────────────────────────────────────────────────────┐
│  Disable the ROCm OpenCL ICD before running any OpenCL on this part:                 │
│    mv /etc/OpenCL/vendors/amdocl64.icd \                                             │
│       /etc/OpenCL/vendors/amdocl64.icd.disabled                                      │
│  Otherwise any OpenCL call, including device enumeration, hangs the stack.           │
└──────────────────────────────────────────────────────────────────────────────────────┘

                           See: LLVM AMDGPU Backend — Chapter 6 → Vulkan compute
```

## LLVM AMDGPU Backend and Target Features

```
── LLVM AMDGPU BACKEND ────────────────────────────── -mcpu=gfx1013 · target features ──

Every GPU code path except RADV flows through LLVM:

  source (HIP C++ / OpenCL C / GLSL) → frontend (clang / glslang) → LLVM IR
  → AMDGPU backend (-mcpu=gfx1013) → native ISA (.hsaco or shader binary)

┌─ ADDRESS SPACES ─────────────────────────────────────────────────────────────────────┐
│  AS 0   flat / generic      runtime-resolved                                         │
│  AS 1   global              GDDR6 via GPUVM                                          │
│  AS 3   LDS (local)         per-WGP shared, 64 KB                                    │
│  AS 4   constant            read-only global                                         │
│  AS 5   private (scratch)   per-thread; VGPRs spill to the scratch buffer            │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ REGISTER-FILE LIMITS (hard ceilings) ───────────────────────────────────────────────┐
│  VGPRs per wave   256 (hard ceiling)                                                 │
│  SGPRs per wave   106, plus VCC and trap temps                                       │
│  Wave mode        Wave64 native                                                      │
└──────────────────────────────────────────────────────────────────────────────────────┘

More VGPRs per wave means fewer concurrent waves and lower occupancy.  The
generic LLVM allocator can target register counts beyond the 256-VGPR ceiling
for large kernels — the mechanism behind failures of high-register-pressure
kernels on this part.  ACO's RDNA-aware allocator respects the ceiling.

┌─ RDNA 1 ISA FAMILY — gfx101x ────────────────────────────────────────────────────────┐
│  Feature                       gfx1010   gfx1011   gfx1012   gfx1013 (BC-250)        │
├──────────────────────────────────────────────────────────────────────────────────────┤
│  Base ISA (VALU, LDS, VMEM)    yes       yes       yes       yes                     │
│  V_MAC_F32                     yes       yes       yes       yes                     │
│  V_FMAC_F32                    yes       yes       yes       yes                     │
│  V_DOT2_F32_F16 (FP16 dot)     no        yes       yes       no                      │
│  V_DOT4_I32_I8 (INT8 dot)      no        yes       yes       no                      │
│  IMAGE_BVH_INTERSECT_RAY       no        no        no        yes                     │
│  Wave64 native                 yes       yes       yes       yes                     │
│  256 VGPRs max                 yes       yes       yes       yes                     │
└──────────────────────────────────────────────────────────────────────────────────────┘

gfx1013 matches gfx1010's instruction capabilities — no dot products; the only
addition is BVH ray intersection, which is additive.  Hence
HSA_OVERRIDE_GFX_VERSION=10.1.0 is the only safe override: gfx1010 is classified
"no dot product" in every ROCm library, so no V_DOT2/V_DOT4 is ever emitted, and
gfx1010-compiled code never emits BVH instructions.  A gfx1012 override would
emit V_DOT4_I32_I8 and crash.

┌─ ROCM LIBRARY CLASSIFICATION — Composable-Kernel macros (track gfx1010) ─────────────┐
│  CK_USE_AMD_V_MAC_F32        yes                                                     │
│  CK_USE_AMD_V_FMAC_F32       yes                                                     │
│  CK_USE_AMD_V_DOT2_F32_F16   no                                                      │
│  CK_USE_AMD_V_DOT4_I32_I8    no                                                      │
│  Buffer resource 3rd DWORD   0x31014000 — RDNA1/RDNA2 format (GCN uses               │
│                              0x00020000)                                             │
│  BatchNorm path              generic (non-AMDGCN inline ASM), same as RDNA2          │
│  Buffer load/store           disabled for gfx101x/gfx103x (known RDNA                │
│                              workaround)                                             │
│  buffer_wbinvl1_vol          disabled — GCN instruction; RDNA invalidates            │
│                              through the cache hierarchy                             │
└──────────────────────────────────────────────────────────────────────────────────────┘

Libraries that gate on the device-name string (MIOpen, MIGraphX, rocRAND) need
an explicit gfx1013 entry to accept the part natively; those that check ISA
capability are handled by the override or the ELF retarget (next section).

                                     See: ROCm Toolchain — Chapter 6 → ISA reference
```

## ROCm Toolchain

```
── ROCM TOOLCHAIN ───────────────────────────────────────────────────── hipcc → HSACO ──

The ROCm compilation toolchain is built on the LLVM AMDGPU backend.  hipcc (a
clang wrapper) compiles HIP C++ into HSACO (HSA Code Object) ELF binaries
containing native gfx1013 ISA.  Runtime compilation is supported with
content-hash caching — kernels rebuilt on source change, cached objects reloaded
thereafter.

┌─ BUILD ──────────────────────────────────────────────────────────────────────────────┐
│  Build         hipcc --offload-arch=gfx1013 -O3 -o kernel kernel.hip                 │
│  Flags         -save-temps (keep LLVM IR + ISA) · -v (show clang invocation)         │
│  Disassemble   llvm-objdump -d --mcpu=gfx1013 kernel.hsaco                           │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ HSACO ELF LAYOUT ───────────────────────────────────────────────────────────────────┐
│  ELF header   Machine EM_AMDGPU · Flags EF_AMDGPU_MACH_AMDGCN_GFX1013                │
│  .text        GPU ISA instructions                                                   │
│  .rodata      constant data                                                          │
│  .note.amd    metadata — kernel args, workgroup size, VGPR count                     │
│  .symtab      kernel entry points                                                    │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ ENVIRONMENT VARIABLES ──────────────────────────────────────────────────────────────┐
│  HSA_OVERRIDE_GFX_VERSION    10.1.0   force the gfx1010 code path — required         │
│  HSA_FORCE_FINE_GRAIN_PCIE   1        redirect discrete-VRAM allocation to the       │
│                                       system aperture — causes page faults on        │
│                                       large allocations                              │
│  GPU_ENABLE_WAVE32_MODE      -        force wave32 — untested                        │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ ROCBLAS ELF RETARGET — Tensile kernels ship built for gfx1012 ──────────────────────┐
│  Scope        54 .hsaco files                                                        │
│  Edit         offset 0x30 — EF_AMDGPU_MACH machine flag, gfx1012 → gfx1013           │
│  FP32 GEMM    works after retarget                                                   │
│  FP16 GEMM    not covered — needs custom kernels (dot instructions absent)           │
└──────────────────────────────────────────────────────────────────────────────────────┘

A single-byte edit to the ELF machine flag retargets each binary to gfx1013 (the
base RDNA1 ISA is shared).  The retarget is applied by a small patcher script
and is required for rocBLAS to find kernels for the part — the ISA-capability
route, as opposed to device-name gating.

┌─ TOOLS ──────────────────────────────────────────────────────────────────────────────┐
│  rocgdb         GPU-aware debugger (set amdgpu precise-memory on)                    │
│  rocprof        GPU performance profiling                                            │
│  RGA            offline ISA analysis, no GPU needed —                                │
│                 rga -s vulkan --isa out.isa -c gfx1013 shader.spv                    │
│  llvm-objdump   HSACO disassembly (--mcpu=gfx1013)                                   │
└──────────────────────────────────────────────────────────────────────────────────────┘

RGA targets gfx1013 directly for register-usage and occupancy analysis.  CodeXL
is archived, replaced by RGA/RGP/uProf.

┌─ CAUTION ────────────────────────────────────────────────────────────────────────────┐
│  HIP dispatch reaches the GPU through KFD/MEC — restrict HIP kernels to the          │
│  atomic-store-safe subset or use the GFX-ring paths (see Mesa Userspace).            │
│  Allocate with hipHostMalloc(..., hipHostMallocMapped) +                             │
│  hipHostGetDevicePointer() for zero-copy system-aperture access; plain               │
│  hipMalloc routes through the BAR0-limited frame-buffer aperture unless              │
│  apu_prefer_gtt is in effect.                                                        │
└──────────────────────────────────────────────────────────────────────────────────────┘

              See: KFD / HSA Compute Interface — GPUVM Memory Model (aperture routing)
```

## Kernel Boot Parameters

```
── KERNEL BOOT PARAMETERS ───────────────────────────────────────── /etc/default/grub ──

Driver-relevant boot parameters are set in /etc/default/grub and applied with
grub-mkconfig -o /boot/grub/grub.cfg plus a reboot.

┌─ CRITICAL — the system does not work correctly without these ────────────────────────┐
│  amdgpu.gartsize=16384            16 GB GTT pool — without it only ~256 MB is        │
│                                   usable                                             │
│  amdgpu.ppfeaturemask=0xffffffff  unlocks all SMU power management — without         │
│                                   it the GPU is stuck at 1500 MHz                    │
│  amdgpu.noretry=0                 enables page-fault retry — without it every        │
│                                   GPU page fault is fatal                            │
│  ttm.pages_limit=4194304          16M-page TTM pool — without it large GPU           │
│                                   allocations can deadlock                           │
│  pci=realloc,assign-busses        fixes the BAR0 256 MB window mismatch — GART       │
│                                   writes beyond the BAR window corrupted             │
│                                   memory                                             │
│  iomem=relaxed                    enables userspace MMIO for SMU tooling             │
│  amd_iommu=off                    IOMMU disabled; iommu=pt was removed (a            │
│                                   no-op under amd_iommu=off, and it caused           │
│                                   DeviceLost regressions)                            │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ PERFORMANCE / MEMORY / SCHEDULING ──────────────────────────────────────────────────┐
│  mitigations=off                                                                     │
│  zswap.enabled=1  zswap.zpool=zsmalloc  zswap.compressor=zstd                        │
│  amdgpu.dc=0                      headless — all compute paths including RADV        │
│                                   work identically with dc=0                         │
│  amdgpu.sched_policy=2            round-robin GPU scheduler                          │
│  amdgpu.lockup_timeout=2000,2000,100,2000                                            │
│                                   per-ring hang detection ms — GFX 2 s,              │
│                                   compute 2 s, SDMA 100 ms, video 2 s                │
│  amdgpu.num_kcq=4                 kernel compute queues                              │
│  amdgpu.cg_mask=0                 clock gating disabled                              │
│  amdgpu.mtype_local=2             MTYPE_CC for local memory                          │
│  amdgpu.bc250_cc_write_mode=3     BC-250 CC write mode — full-40CU-liberation        │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ NOT ON THE BOOT LINE — the driver defaults are correct ─────────────────────────────┐
│  amdgpu.gttsize   superseded by gartsize — left at -1                                │
│  amdgpu.pg_mask   power-gating default 0xffffffff                                    │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ CAUTION ────────────────────────────────────────────────────────────────────────────┐
│  Never set amdgpu.vm_block_size=11 (R-Mode) — GPUVM page-table walker fault.         │
│  Do not re-add removed debug overrides without recording why.                        │
└──────────────────────────────────────────────────────────────────────────────────────┘

  See: Chapter 4 · System Configuration (BIOS-side settings feeding the same memory map)
```

## Platform Drivers

```
── PLATFORM DRIVERS ─────────────────────────────────────── FCH · Super I/O · network ──

The board's non-GPU peripherals bind standard Linux drivers.  The A68H FCH
functions arrive over the transparent UMI link and enumerate as ordinary
root-bus PCI devices; the Super I/O sits behind the FCH's LPC bridge.  There is
no CPU-frequency driver on this part — the SMU owns CPU clocking.

┌─ DRIVER BINDINGS ────────────────────────────────────────────────────────────────────┐
│  Network      r8169       Realtek RTL8111H GbE (10ec:8168) — FCH PCIe Gen1 x1        │
│  SATA         ahci        FCH SATA III (1022:7801) — M.2 SATA mode via the NXP       │
│                           mux                                                        │
│  USB 3.0      xhci        FCH XHCI (1022:7814)                                       │
│  USB 2.0      ehci        FCH EHCI ×2 (1022:7808)                                    │
│  USB 1.1      ohci        FCH OHCI ×3 (1022:7807/7809)                               │
│  SMBus        i2c-piix4   FCH SMBus (1022:780b) — VRM, sensors, SPD sub-buses        │
│  Super I/O    nct6683     Nuvoton NCT6686D hwmon over LPC — temps, voltage           │
│                           ADCs, fan PWM, watchdog.  NOT auto-loaded on the           │
│                           live system: module present on disk, no nct6683            │
│                           hwmon node                                                 │
│  CPU clocks   (none)      no Linux cpufreq path — SMU-managed (Zen 2                 │
│                           microcode 0x8407007)                                       │
└──────────────────────────────────────────────────────────────────────────────────────┘

The live hwmon surfaces are the amdgpu driver's SMU-fed sensors (read_sensor /
gpu_metrics, see the PowerPlay section) for the GPU die and k10temp for the CPU
die; the nct6683 driver for board-level temperatures, rails, and fans is
available but not currently bound.

     See: Chapter 1 · Carrier Board (FCH, network, storage, Super I/O) — Chapter 2 → SMU
```

## Firmware and VBIOS Identifiers

```
── FIRMWARE & VBIOS IDENTIFIERS ─────────────────────────────── what the driver meets ──

Firmware components the driver interacts with, and their version identifiers.
The MEC firmware and the amdgpu kernel module are independent systems, both
cached in the initramfs; changing one does not affect the other.

┌─ IDENTIFIERS ────────────────────────────────────────────────────────────────────────┐
│  SMU firmware          88.6.0 (0x00580600)    MP1 / SMU 11.0.8 — resides in          │
│                                               the BIOS, no runtime loading on        │
│                                               this APU                               │
│  PSP (MP0)             11.0.8                 resides in the SPI flash               │
│                                               (Chapter 3)                            │
│  VBIOS                 113-AMDRBN-003         board VBIOS string                     │
│  MEC ucode (stock)     v144 · feature v32     cyan_skillfish2_mec.bin — loaded       │
│                                               by the kernel at GPU init              │
│  MEC ucode (swap)      v156 · feature v35     navi10_mec.bin — same IP version       │
│                                               10.1; behaviour unchanged              │
│                                               (defect is silicon-level)              │
│  GPU core firmware     ME 0x63 · PFP 0x94 · CE 0x25 · RLC 0x0D · MEC 0x90            │
│  SDMA0/1 firmware      5.0.1 (fw 0x34)                                               │
└──────────────────────────────────────────────────────────────────────────────────────┘

The SMU interface version is negotiated at boot via GetSmuVersion /
GetDriverIfVersion.

    See: Chapter 3 → Firmware map (SPI layout, PSP directory) — Chapter 2 → GPU firmware
```


# 6. Compute Stack

```
── COMPUTE STACK ──────────────────────────────── gfx1013 · GFX ring is the only path ──

Compute on the BC-250 runs entirely on the graphics command processor.  The
MEC async-compute engine is defective at the silicon level, so every production
dispatch — Vulkan and OpenCL alike — is a PM4 packet stream submitted to the GFX
ring.  This chapter documents the gfx1013 compute hardware: CU topology, ISA,
register files, memory hierarchy, DMA engines, and the programming model the
silicon enforces.

            gfx1013 die — 2 Shader Engines · 4 Shader Arrays · 40 CU
 ┌──────────────────────────────────────────────────────────────────────────┐
 │           Command Processor  ME · PFP · CE  ·  RLC sequencer             │
 ├─────────────────────────────────────┬────────────────────────────────────┤
 │           Shader Engine 0           │           Shader Engine 1          │
 │  ┌───────────────┐ ┌──────────────┐ │ ┌──────────────┐ ┌───────────────┐ │
 │  │     SA 0      │ │     SA 1     │ │ │     SA 2     │ │     SA 3      │ │
 │  │ WGP WGP WGP   │ │ WGP WGP WGP  │ │ │ WGP WGP WGP  │ │ WGP WGP WGP   │ │
 │  │ WGP WGP       │ │ WGP WGP      │ │ │ WGP WGP      │ │ WGP WGP       │ │
 │  │ 5 WGP = 10 CU │ │ 5 WGP =10 CU │ │ │ 5 WGP =10 CU │ │ 5 WGP = 10 CU │ │
 │  │ L1  128 KB    │ │ L1  128 KB   │ │ │ L1  128 KB   │ │ L1  128 KB    │ │
 │  └───────────────┘ └──────────────┘ │ └──────────────┘ └───────────────┘ │
 ├─────────────────────────────────────┴────────────────────────────────────┤
 │                          L2 — 4 MB, GPU-shared                           │
 ├──────────────────────────────────────────────────────────────────────────┤
 │              GDDR6 — 16 GB unified · 256-bit · 8 channels                │
 └──────────────────────────────────────────────────────────────────────────┘
   40 CU on die = 4 SA × 10 CU = 20 WGP = 2,560 SP · Wave64 native
   stock harvest: 24 CU active — kernel patch enables the fused 16

                     See: Chapter 2 · GPU block — Chapter 5 · amdgpu driver
```

## CU Topology & the Harvest Model

```
── CU TOPOLOGY ─────────────────────────────────── 40 CU · 20 WGP · 2 SE · harvest 24 ──

Forty compute units on die: four shader arrays of ten CU each — twenty Work-Group
Processors (five per array) across two shader engines, 2,560 stream processors in
total.  The stock BC-250 harvest fuses the part down to twenty-four active CU
(twelve populated WGPs); a kernel patch liberates all forty.  Live topology
confirms the full geometry.

┌─ GEOMETRY ───────────────────────────────────────────────────────────────────────────┐
│  GPU               gfx1013 — RDNA 1.x, GC 10.1.3 (custom part, not Navi)             │
│  CU complement     40 on die — stock harvest 24 active; patch frees the fused 16     │
│  Shader layout     4 shader arrays × 10 CU · 20 WGP (5/array) · 2 SE                 │
│  Stream procs      2,560 SP full die; 24 CU × 2 SIMD × 32 = 1,536 lanes stock        │
│  SIMD              2× SIMD32 per CU — 80 SIMD full die                               │
│  Clock domain      GFXCLK — SMN 0x16C00 (SMU-managed)                                │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ LIVE TOPOLOGY (gfx_target_version 100103) ──────────────────────────────────────────┐
│  simd_count 80           simd_per_cu 2                                               │
│  array_count 4           cu_per_simd_array 10                                        │
└──────────────────────────────────────────────────────────────────────────────────────┘

WGP internals — two CUs sharing one 64 KB LDS:

   ┌──────────────────── Work-Group Processor (WGP) ─────────────────────┐
   │   ┌──────── CU 0 ────────┐        ┌──────── CU 1 ────────┐          │
   │   │ SIMD0  32 lanes      │        │ SIMD0  32 lanes      │          │
   │   │ SIMD1  32 lanes      │        │ SIMD1  32 lanes      │          │
   │   │ 256 VGPR / SIMD      │        │ 256 VGPR / SIMD      │          │
   │   │ scalar unit (SGPRs)  │        │ scalar unit (SGPRs)  │          │
   │   │ 16 KB vector L0 (ro) │        │ 16 KB vector L0 (ro) │          │
   │   └──────────────────────┘        └──────────────────────┘          │
   │   64 KB shared LDS (32 banks)  ·  16 KB scalar/const K$ (both CUs)  │
   └─────────────────────────────────────────────────────────────────────┘
        L1 128 KB per shader array, shared by that array's WGPs

┌─ CAUTION ────────────────────────────────────────────────────────────────────────────┐
│  Under compute, never gate off a whole shader array — an empty array HANGS           │
│  and wedges the box.  Safe CU shapes populate all four arrays; when routing          │
│  or masking CUs keep at least one active WGP in every shader array.                  │
└──────────────────────────────────────────────────────────────────────────────────────┘

        See: Chapter 2 → GPU block — Chapter 5 → CU-liberation patch, config
```

## Wavefront & Execution Model

```
── WAVEFRONT ────────────────────────────────────────────── Wave64 native · no Wave32 ──

The native wavefront is Wave64 — there is no Wave32 mode on this part.  Each
wave64 vector instruction issues across a SIMD32 as two halves (double-pumped).

┌─ EXECUTION ──────────────────────────────────────────────────────────────────────────┐
│  Wavefront         Wave64 native — no Wave32 mode                                    │
│  Issue             wave64 double-pumped across SIMD32 (two halves / instr)           │
│  Work-group max    hardware 1024 work-items w/ S_BARRIER; rusticl reports 256        │
│  Sync scope        S_BARRIER + LDS sharing within the WGP;                           │
│                    no cross-work-group sync inside a dispatch                        │
└──────────────────────────────────────────────────────────────────────────────────────┘

API execution model → hardware mapping:

 API model        Hardware unit        Scale / limit
 ───────────────  ───────────────────  ─────────────────────────────────────
 Work-item        SIMD lane            1,536 lanes stock / 2,560 full die
 Work-group       one CU               up to 1024 work-items (hardware)
 Wavefront        wave64 unit          64 work-items (sub-group)
 Local memory     LDS                  64 KB/WGP (32 KB/CU), 32 banks
 Private memory   VGPRs                256 per SIMD; spills to scratch
 Constant memory  scalar cache (K$)    16 KB per WGP (shared by 2 CUs)
 Global memory    GDDR6 via MMHUB      16 GB unified, 256-bit, 8 channels

        See: Register Files & Occupancy — Local Data Share
```

## Register Files & Occupancy

```
── REGISTER FILES ──────────────────────────────────── 256 VGPR hard ceiling · wave64 ──

Each wavefront owns a fixed scalar register set and up to 256 vector registers.
The 256-VGPR ceiling is a hard silicon limit — toolchains whose codegen assumes
the 512-VGPR budgets of later gfx10.x parts produce kernels that cannot launch.

┌─ REGISTER FILE ──────────────────────────────────────────────────────────────────────┐
│  SGPRs             106 general (s0–s105)                                             │
│  VCC               s106–s107 (64-bit for wave64)                                     │
│  Trap temporaries  16 SGPRs                                                          │
│  Alignment         64-bit scalar ops require even register alignment                 │
│  Inline consts     integers -16..64; floats ±0.5, ±1.0, ±2.0, ±4.0                   │
│  VGPRs             256 addressable per wavefront (hard ceiling)                      │
│  VGPR alloc        blocks of 8 (wave64 granularity; RDNA 2 doubles to 16)            │
│  Spill target      scratch (global) memory when VGPR budget exceeded                 │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ SPECIAL REGISTERS ──────────────────────────────────────────────────────────────────┐
│  EXEC      64-bit execute mask (wave64)                                              │
│  VCC       64-bit vector condition code                                              │
│  SCC       scalar condition code                                                     │
│  M0        LDS addressing, sendmsg control                                           │
│  PC        64-bit program counter                                                    │
│  STATUS    read-only hardware status                                                 │
│  MODE      FP rounding, denormal, exception enables                                  │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ PER-CU RESIDENCY LIMITS ────────────────────────────────────────────────────────────┐
│  VGPRs             256 per SIMD                                                      │
│  SGPRs             ~104–106                                                          │
│  LDS               64 KB per WGP (32 KB per CU)                                      │
│  Wavefront slots   20 per CU                                                         │
│  Work-group slots  16 per CU                                                         │
└──────────────────────────────────────────────────────────────────────────────────────┘

Occupancy — concurrent wave64 per SIMD as a function of VGPR use:

 VGPRs used   ≤24   ≤28   ≤32   ≤36   ≤42   ≤51   ≤64   ≤84   ≤128   ≤256
 ──────────  ────  ────  ────  ────  ────  ────  ────  ────  ─────  ─────
 Waves/SIMD    10     9     8     7     6     5     4     3      2      1

Higher occupancy hides memory latency; ≥8 waves/SIMD is the target for
bandwidth-bound kernels.

┌─ CAUTION ────────────────────────────────────────────────────────────────────────────┐
│  Upstream LLVM AMDGCN targets gfx10xx assuming 512+ VGPRs and over-allocates         │
│  for gfx1013 — register-heavy kernels fail at dispatch with                          │
│  HSA_STATUS_ERROR_OUT_OF_REGISTERS (0x2d).  Mesa's ACO allocates against the         │
│  real 256-VGPR wave64 envelope and compiles the same shaders — the root reason       │
│  the Vulkan/rusticl paths work where ROCm/HIP struggles.                             │
└──────────────────────────────────────────────────────────────────────────────────────┘

        See: Shader ISA — Compute API Paths (ACO compiler)
```

## Local Data Share

```
── LOCAL DATA SHARE ───────────────────────────────────── 64 KB/WGP · 32 banks · AS 3 ──

Each WGP carries one 64 KB LDS, shared by its two CUs and banked 32 ways.  LDS
is the work-group-scope scratchpad: programmer-managed, addressed through address
space 3, synchronized with S_BARRIER.

┌─ LDS ────────────────────────────────────────────────────────────────────────────────┐
│  Capacity          64 KB per WGP (32 KB per CU)                                      │
│  Banks             32                                                                │
│  Address space     AS 3 (Local), 32-bit; null pointer 0xFFFFFFFF                     │
│  Scope             work-group; no cross-work-group sharing in a dispatch             │
│  Addressing        M0 register participates in LDS addressing                        │
│  Known hazard      LDSMisalignedBug — misaligned access may return wrong             │
│                    data (compiler-mitigated)                                         │
└──────────────────────────────────────────────────────────────────────────────────────┘

        See: Wavefront & Execution Model — Shader ISA → hazards
```

## Shader ISA

```
── SHADER ISA ─────────────────────────────────────────────── gfx1013 = gfx1010 + BVH ──

The gfx10.1 family is defined in the LLVM AMDGPU TableGen source as one shared
base (ISAVersion10_1_Common) plus per-chip feature deltas.  gfx1013's delta is
exactly one feature: GFX10_AEncoding, the hardware BVH ray-intersection encoding.
In one line: gfx1013 = gfx1010 + BVH.  It carries no dot-product family of any
kind.

┌─ ISA IDENTITY ───────────────────────────────────────────────────────────────────────┐
│  Family base       ISAVersion10_1_Common — ScalarStores, ScalarAtomics,              │
│                    ScalarFlatScratchInsts, GetWaveIdInst, MadMacF32Insts,            │
│                    DsSrc2Insts, SupportsXNACK, LDSMisalignedBug,                     │
│                    VcmpxPermlaneHazard, VMEMtoScalarWriteHazard,                     │
│                    SMEMtoVectorWriteHazard, InstFwdPrefetchBug,                      │
│                    VcmpxExecWARHazard                                                │
│  gfx1013 delta     GFX10_AEncoding (only)                                            │
│  Processor model   ProcessorModel<"gfx1013", GFX10SpeedModel,                        │
│                    FeatureISAVersion10_1_3.Features>  (GCNProcessors.td)             │
│  XNACK             disabled (xnack-) — GPU page faults are fatal                     │
└──────────────────────────────────────────────────────────────────────────────────────┘

Feature availability across the gfx10.1 family (no rows dropped):

 Feature                 1010 1011 1012 1013  Instructions
 ──────────────────────  ──── ──── ──── ────  ───────────────────────────────
 Base ISA (10_1_Common)  yes  yes  yes  yes   all VALU, LDS, VMEM, SMEM
 MadMacF32Insts          yes  yes  yes  yes   V_MAC_F32, V_MAD_F32
 Dot1Insts               no   yes  yes  no    V_DOT2_I32_I16, V_DOT2_U32_U16
 Dot2Insts               no   yes  yes  no    V_DOT2_F32_F16, V_DOT4_U32_U8
 Dot5Insts               no   yes  yes  no    V_DOT8_I32_I4, V_DOT8_U32_U4
 Dot6Insts               no   yes  yes  no    V_DOT4_I32_IU8
 Dot7Insts               no   yes  yes  no    V_DOT2_F32_BF16, V_DOT4_I32_I8
 Dot10Insts              no   yes  yes  no    V_DOT2_F16_F16
 GFX10_AEncoding         no   no   no   yes   IMAGE_BVH_INTERSECT_RAY,
                                             IMAGE_BVH64_INTERSECT_RAY

GFX10_AEncoding otherwise appears only on gfx1030+ (RDNA 2), GFX11 and GFX12;
gfx1013 is the sole gfx10.1 part carrying it.  The LLVM AMDGPUUsage prose
incorrectly states BVH is unavailable on gfx1013 — the TableGen source and the
AMDGPUAsmGFX1013 assembler reference both list the opcodes as valid.  BVH is
unavailable on gfx10-1-generic (the target covering 1010/1011/1012/1013 with no
Dot and no BVH features).
```

### Instruction encoding format families

```
Scalar ALU (SALU) and Vector ALU (VALU) encoding families:

 SALU                                  VALU
 ────                                  ────
 SOP2   S_ADD_I32, S_AND_B64           VOP2   V_ADD_F32, V_MUL_F32,
        S_LSHL_B32                             V_CNDMASK_B32, V_MAC_F32
 SOP1   S_MOV_B32, S_BCNT1_I32_B64     VOP1   V_CVT_F32_I32, V_RCP_F32,
 SOPK   S_MOVK_I32, S_ADDK_I32,               V_SQRT_F32, V_SIN_F32
        S_GETREG_B32                   VOP3   V_FMA_F32, V_MAD_F32,
 SOPC   S_CMP_EQ_I32, S_BITCMP0_B32           V_MED3_F32, V_READLANE_B32
 SOPP   S_BRANCH, S_BARRIER,           VOPC   V_CMP_GT_F32, V_CMPX_EQ_U32
        S_WAITCNT, S_ENDPGM            VOP3P  V_PK_FMA_F16, V_PK_ADD_F16,
                                              V_PK_MUL_F16  (packed FP16)

Cross-lane / sub-dword modifiers, all present on gfx1013 (same set as RDNA 2):

 DPP16   row shift, row mirror, row broadcast, quad permute across 16 lanes
 DPP8    arbitrary permute within groups of 8 lanes
 SDWA    sub-dword addressing (byte/word select from a 32-bit VGPR)
```

### Delta vs RDNA 2 (gfx103x)

```
Present on gfx1013, removed or replaced on RDNA 2:

 V_MAC_F32, V_MADMK_F32, V_MADAK_F32    RDNA 2 → FMA forms
 V_MAC_LEGACY_F32, V_MAD_LEGACY_F32     RDNA 2 → V_FMA_LEGACY_F32
 S_MEMTIME                              RDNA 2 → s_getreg_b32 SHADER_CYCLES

RDNA 2 additions absent on gfx1013:

 All dot-product ALU        V_DOT2_F32_F16, V_DOT2C_F32_F16, V_DOT2_I32_I16,
                            V_DOT2_U32_U16, V_DOT4_I32_I8, V_DOT4C_I32_I8,
                            V_DOT4_U32_U8, V_DOT8_I32_I4, V_DOT8_U32_U4
 IMAGE_LOAD_MSAA            multisample image load
 Add-TID global loads       global memory loads with Add-TID
 Clamped atomic subtract    buffer and global forms
 Doubled alloc units        RDNA 2 doubles VGPR and LDS allocation-unit size
 Wave-shared VGPRs          cross-half exchange for wave64
 Infinity Cache             128 MB L3 (gfx1013 has none)
 Automatic L2 coherency     cross-dispatch invalidation (manual on gfx1013)

Memory-instruction classes: SMEM (S_LOAD_DWORD[X2..X16],
S_BUFFER_LOAD_DWORD[X2..X16], S_DCACHE_INV, S_GL1_INV) identical to RDNA 2;
MUBUF/MTBUF identical; MIMG identical except IMAGE_BVH present and IMAGE_LOAD_MSAA
absent; FLAT/GLOBAL/SCRATCH identical except no Add-TID and no clamped atomic
subtract; DS (LDS/GDS) and EXP identical.

Packed FP16 (VOP3P) is present and full-rate, but there is no packed integer or
float dot instruction of any kind: integer dot workloads must unpack and lower to
V_FMA_F32 or V_PK_FMA_F16.  RDNA 1 predates WMMA (RDNA 3) and MFMA (CDNA), so no
cooperative-matrix silicon exists on this die and VK_KHR_cooperative_matrix is not
advertised.
```

### Hardware hazards, address spaces, S_WAITCNT

```
All gfx10.1 chips share hardware hazards the compiler mitigates with NOPs / waits:

 VcmpxPermlaneHazard       VCMPX followed by permlane requires NOPs
 VMEMtoScalarWriteHazard   VMEM followed by a scalar write needs a wait
 SMEMtoVectorWriteHazard   SMEM followed by a vector write needs a wait
 InstFwdPrefetchBug        forward instruction prefetch may fault
 VcmpxExecWARHazard        VCMPX write-after-read on EXEC
 LDSMisalignedBug          misaligned LDS access may return wrong data

AMDGPU address spaces:

 AS 0   Generic (flat)     64-bit   runtime-resolved
 AS 1   Global             64-bit   GDDR6 via GPUVM
 AS 3   Local (LDS)        32-bit   64 KB per WGP
 AS 4   Constant           64-bit   read-only global
 AS 5   Private (scratch)  32-bit   VGPRs, spill buffer
 ─────────────────────────────────────────────────────────────────────────────
 Null pointers:  Global 0x0  ·  Local 0xFFFFFFFF  ·  Private 0xFFFFFFFF

S_WAITCNT dependency counters (same as RDNA 2):

 vmcnt     vector-memory loads in flight    MUBUF, MTBUF, MIMG, FLAT loads
 lgkmcnt   LDS, GDS, scalar-memory ops      DS, SMEM, FLAT-to-LDS
 expcnt    exports and GDS writes           EXP, GDS stores

RDNA 2 adds split variants (S_WAITCNT_VMCNT / _LGKMCNT / _EXPCNT); whether these
exist on gfx1013 is untested.
```

### Binary compatibility within the family

```
gfx1010, gfx1012 and gfx1013 share the same RDNA 1 base ISA — same VALU set, LDS
ops, wave64 model, and 256-VGPR/106-SGPR register file.  This makes two things
possible:

┌─ GFX VERSION OVERRIDE ───────────────────────────────────────────────────────────────┐
│  gfx1013 is a strict superset of the 10_1_Common base, so                            │
│  HSA_OVERRIDE_GFX_VERSION=10.1.0 (gfx1010) is safe: that target emits neither        │
│  dot nor BVH instructions, and every other ISA property is identical.                │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ ELF RETARGETING ────────────────────────────────────────────────────────────────────┐
│  Precompiled gfx1012 code objects retarget to gfx1013 by changing one byte per       │
│  file — the EF_AMDGPU_MACH target-arch field at ELF offset 0x30.  rocBLAS ships      │
│  no gfx1013 Tensile kernels; patching its 54 gfx1012 .hsaco files this way makes     │
│  them load and run.  FP32 GEMM works; FP16 and INT8 GEMM do not (their kernels       │
│  use V_DOT4_I32_I8, which gfx1013 lacks).                                            │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ CAUTION ────────────────────────────────────────────────────────────────────────────┐
│  Never override to 10.1.1 or 10.1.2 — those targets emit V_DOT4_I32_I8 and other     │
│  dot-product opcodes that do not exist on gfx1013.  XNACK is disabled (xnack-);      │
│  a GPU page fault is fatal, not retried.                                             │
└──────────────────────────────────────────────────────────────────────────────────────┘

        See: IMAGE_BVH reference — MIMG / Texture Pipeline — Register Files
```

## IMAGE_BVH_INTERSECT_RAY — Instruction Reference

```
── IMAGE_BVH ──────────────────────────────────── native ray intersection · MIMG path ──

gfx1013 is the only RDNA 1.x part with native BVH ray intersection, added via
FeatureGFX10_AEncoding.  The instructions execute on the texture (MIMG) path;
there is no dedicated ray-tracing block.

┌─ OPCODES ────────────────────────────────────────────────────────────────────────────┐
│  IMAGE_BVH_INTERSECT_RAY       opcode 0xe6 — 32-bit node pointers                    │
│  IMAGE_BVH64_INTERSECT_RAY     opcode 0xe7 — 64-bit node pointers                    │
└──────────────────────────────────────────────────────────────────────────────────────┘

 image_bvh_intersect_ray   vdst[4], vaddr[11], srsrc[4]        ; 32-bit node ptrs
 image_bvh_intersect_ray   vdst[4], vaddr[8],  srsrc[4]  a16   ; float16 directions
 image_bvh64_intersect_ray vdst[4], vaddr[12], srsrc[4]        ; 64-bit node ptrs

Input operands (default, non-NSA layout):

 VADDR[0]      uint32       BVH node pointer (offset into accel structure)
 VADDR[1]      float32      ray extent (max search distance)
 VADDR[2:4]    float32 ×3   ray origin (x, y, z)
 VADDR[5:7]    float32 ×3   ray direction (x, y, z)
 VADDR[8:10]   float32 ×3   inverse ray direction (1/x, 1/y, 1/z)
 SRSRC[0:3]    uint32  ×4   128-bit resource descriptor (BVH memory location)

A16 mode packs ray direction and inverse direction as float16 (two per VGPR),
reducing address VGPRs from 11 to 8.  NSA (Non-Sequential Address) mode lets the
five operand groups occupy non-contiguous VGPRs, e.g.
v[4:7], [v50, v46, v[20:22], v[40:42], v[47:49]], s[12:15]; gfx1013 uses the
non-NSA VADDR packing (GFX10 register layout).

Output is 4 VGPRs (v4i32), interpreted by node type.  A box (internal) node
returns 4 child node pointers, hardware-sorted by intersection distance (nearest
first) — the ray is tested against all four child boxes simultaneously.  A
triangle (leaf) node returns intersection distance (t), triangle ID, and
barycentric coordinates.

BVH node formats (64-byte aligned; node type in low 3 bits of the pointer):

 Box16 node (64 bytes)      [0:15]   4× uint32 child pointers
                            [16:63]  4× AABB float16 (min_xyz, max_xyz / child)
 Box32 node (128 bytes)     [0:15]   4× uint32 child pointers
                            [16:111] 4× AABB float32 (6 floats per AABB)
                            [112:127] reserved
 Triangle node (64 bytes)   [0:35]   3× float3 vertex positions (V0, V1, V2)
                            [48:51]  triangle_id (uint32)
                            [52:55]  geometry_id_and_flags (uint32)

Exposed to Vulkan compute as rayQueryEXT via VK_KHR_ray_query +
VK_KHR_acceleration_structure; RADV special-cases enablement as
has_image_bvh_intersect_ray = gfx_level >= GFX10_3 || family == CHIP_GFX1013.

        See: MIMG / Texture Pipeline → Ray Accelerators — Shader ISA → matrix
```

## MIMG / Texture Pipeline & Ray Accelerators

```
── MIMG / TEXTURE ──────────────────────────────────── TMU + Ray Accel · off the VALU ──

MIMG (Memory Image) instructions execute on the Texture Mapping Units and Ray
Accelerators — a fixed-function path physically separate from the vector ALU.  The
texture L0 is distinct from the vector L0, so texture traffic never evicts VALU
data, and during pure-compute the samplers are otherwise idle — usable as a
parallel data-fetch channel (prefetch, BCn-compressed data, lookup tables).

┌─ TEXTURE PATH ───────────────────────────────────────────────────────────────────────┐
│  Execution units   TMUs + Ray Accelerators (independent of VALU)                     │
│  Texture L0        16 KB per WGP, 4-way (separate from vector L0)                    │
│  L1                128 KB per shader array                                           │
│  L2                4 MB, shared with the vector path across all CUs                  │
│  Cache line        64 bytes (BCn blocks 8–16 bytes; several per line)                │
│  ASTC              not supported (RDNA 1)                                            │
└──────────────────────────────────────────────────────────────────────────────────────┘

        See: IMAGE_BVH reference — Cache Hierarchy & Coherency
```

### Instruction families

```
 IMAGE_SAMPLE      filtered reads — 1D/2D/3D/Cube/Array
                   modifiers: _LZ (level 0) _L (explicit LOD) _B (bias)
                              _CL (clamp) _O (offset)
 IMAGE_SAMPLE_C    comparison / shadow variants (same modifiers)
 IMAGE_SAMPLE_D    explicit-derivative sampling (2D/3D)
 IMAGE_LOAD/STORE  raw unfiltered access, bypasses the sampler
                   (also _MIP forms; IMAGE_LOAD_MSAA absent — RDNA 2+ only)
 IMAGE_GATHER4     2×2 texel neighborhood, no interpolation (_CL _O _CL_O)
 IMAGE_ATOMIC      ADD, SUB, SMIN/SMAX, UMIN/UMAX, AND/OR/XOR, INC/DEC,
                   SWAP, CMPSWAP
 IMAGE_BVH         IMAGE_BVH_INTERSECT_RAY / IMAGE_BVH64_INTERSECT_RAY
                   (GFX10_AEncoding — unique to gfx1013 in RDNA 1)

IMAGE_SAMPLE register-operand layout:

 Resource descriptor (srsrc)   4 SGPRs      Coordinates (vaddr)   1–4 VGPRs
 Sampler state (ssamp)         4 SGPRs      Output (vdata)        1–4 VGPRs

The fixed-function filtering pipeline runs at zero ALU cost, in six stages:

 1. Address generation    coordinate → texel position
 2. Fetch                 4 texels (bilinear) or 8 (trilinear)
 3. BCn decompression     if the format is compressed
 4. Weight calculation    fractional position → blend weights
 5. Interpolation         weighted sum
 6. Format conversion     texture format → float32 output

Compute-shader restrictions: no implicit LOD selection — use _L/_LZ (GLSL
textureLod(...,0.0) compiles to IMAGE_SAMPLE_LZ); no implicit derivatives (no
dFdx/dFdy); unnormalized coordinates need a specific image-view configuration;
imageLoad(...) compiles to IMAGE_LOAD.
```

### BCn fixed-function decompression

```
The TMU decompresses all BCn block-compression formats in hardware during texture
fetch, between the cache read and delivery to the shader — latency hidden inside
the normal fetch pipeline.

 Format        Block  Bits/texel  Data type                       Ratio
 ───────────   ─────  ──────────  ─────────────────────────────   ─────
 BC1  (DXT1)   4×4         4       RGB + 1-bit alpha               8:1
 BC2  (DXT3)   4×4         8       RGBA (explicit alpha)           4:1
 BC3  (DXT5)   4×4         8       RGBA (interpolated alpha)       4:1
 BC4  (RGTC1)  4×4         4       single channel                  2:1
 BC5  (RGTC2)  4×4         8       two channels                    2:1
 BC6H          4×4         8       HDR float16 RGB                 6:1
 BC7           4×4         8       high-quality RGBA               4:1

BC6H decodes a 16-byte block to a 4×4 tile of float16 values, then format-converts
to float32 in hardware.

MIMG cache-control modifiers: GLC=1 bypasses L0 and reads from L2 (globally
coherent); SLC=1 is non-temporal, skipping L2 (streaming); DLC=1 bypasses L1
(device-level coherent).  For read-only resident data, GLC=0/SLC=0/DLC=0 (maximum
caching) is optimal.
```

### Ray Accelerator hardware

```
The Ray Accelerators execute the BVH intersection instructions, integrated into
the TMU pipeline: node data (64 bytes) is fetched through the texture cache
hierarchy and the intersection math is pipelined behind the memory access.  The
MIMG path is independent of the VALU, so BVH traversal and vector compute run
concurrently within a CU without contending.

┌─ RAY ACCELERATORS ───────────────────────────────────────────────────────────────────┐
│  Ray Accelerators  2 per WGP — 24 in the stock 12-WGP configuration                  │
│  Box tests         4 per cycle per RA                                                │
│  Triangle tests    1 per cycle per RA                                                │
│  BVH node size     64 B                                                              │
│  Cache             L2 (shared with compute)                                          │
│  Vulkan exposure   rayQueryEXT in compute shaders, via                               │
│                    VK_KHR_ray_query + VK_KHR_acceleration_structure                  │
└──────────────────────────────────────────────────────────────────────────────────────┘

        See: IMAGE_BVH reference — Cache Hierarchy & Coherency
```

## Cache Hierarchy & Coherency

```
── CACHE HIERARCHY ───────────────────────────────────── 3 levels → GDDR6 · manual L2 ──

The compute memory path runs three cache levels straight into GDDR6 — no Infinity
Cache on this die — and, unlike RDNA 2, the hardware does not invalidate L2 across
dispatches.  Coherency on gfx1013 is managed manually at two layers: cache-control
bits in the memory instructions, and MTYPE bits in the GPU page-table entries.

     VALU  ─→  L0            ─→  L1              ─→  L2          ─→  GDDR6
             per-CU 16 KB       per-SA 128 KB       GPU 4 MB        16 GB
             read-only vec      shared              shared          unified

┌─ CACHE FACTS ────────────────────────────────────────────────────────────────────────┐
│  Cache line        64 bytes                                                          │
│  Infinity Cache    none (RDNA 2 adds 128 MB L3; gfx1013 goes L2 → GDDR6)             │
│  Texture L0        separate from the vector L0                                       │
│  Scalar cache      16 KB per WGP (K$, constant path; shared by 2 CUs)                │
│  L2 coherency      MANUAL across dispatches (automatic on RDNA 2+)                   │
└──────────────────────────────────────────────────────────────────────────────────────┘

Cache-control bits on loads and stores:

          loads                              stores
 ──────   ────────────────────────────────  ──────────────────────────────
 GLC=0    may read from L0                   write-combine through L2
 GLC=1    bypass L0, read from L2            write directly to L2 (coherent)
 SLC=0    normal L2 caching                  normal L2 caching
 SLC=1    non-temporal, skip L2 (stream)     non-temporal store
 DLC=0    normal                             normal
 DLC=1    bypass L1 (device coherent)        bypass L1
```

### Manual L2 coherency and scalar-L2 aliasing

```
The most operationally significant RDNA 1.x difference from RDNA 2: a buffer bound
readonly in a compute shader lowers to the SMEM (scalar L2) load path.  If the
host updates that buffer between dispatches, the scalar L2 returns stale data —
wrong output, and in the worst case a GFX-ring timeout on the next dispatch.  Two
remedies:

 1. Remove the readonly qualifier from persistent buffers, so the compiler
    lowers to the VMEM vector-load path and bypasses the scalar-L2 aliasing.
 2. Insert explicit cache flush/invalidate barriers between dispatches; the
    scalar invalidation opcodes are S_DCACHE_INV and S_GL1_INV.

This aliasing is gfx1013-specific behavior, not a bug on RDNA 2+.
```

### MTYPE page-table coherency

```
gfx1013 is a UMA part, but the stock amdgpu driver treats its VRAM PTEs as
discrete-GPU memory, which breaks the compute memory path.  MTYPE modes are UC
(uncached), NC (non-coherent), CC (cache-coherent), WC (write-combined).  The
compute-buffer path requires MTYPE=CC: NC bypasses coherency (writes hit L2
directly) and can leave the next dispatch reading stale data or trigger a ring
reset; the stock driver's MTYPE_UC for VRAM PTEs is correct but non-cached.

Six driver locations decide PTE MTYPE (five patched on production, sixth inherits):

 # Source location                    Sets                     Status
 ─ ─────────────────────────────────  ───────────────────────  ──────────────────
 1 gmc_v10_0.c (GART PTE flags)        MTYPE_CC + SNOOPED       patched
 2 amdgpu_ttm.c (VRAM TTM)             SNOOPED + SYSTEM         patched
 3 gmc_v10_0.c (get_vm_pte DEFAULT)    MTYPE_CC                 patched
 4 gfxhub_v2_1.c (L1 TLB MTYPE)        MTYPE_CC                 patched
 5 mmhub_v2_3.c (L1 TLB MTYPE)         MTYPE_CC                 patched
 6 amdgpu_amdkfd_gpuvm.c (KFD)         MTYPE_DEFAULT            inherits CC (layer 3)

Correct PTE flag set for the compute path:
SYSTEM | SNOOPED | MTYPE_CC | READABLE | WRITEABLE | EXECUTABLE.

┌─ CAUTION ────────────────────────────────────────────────────────────────────────────┐
│  MTYPE=CC is mandatory for compute and Vulkan buffers — NC mappings trigger a        │
│  GFX ring reset on dispatch (reconfigure to CC; reboot if the ring is stuck          │
│  resetting).  A blanket MTYPE_CC patch that flips EVERY VRAM PTE hard-hangs RADV     │
│  compute deterministically ("GPU reset begin! ... guilty of a hard recovery") —      │
│  apply the coherency change narrowly to the compute-buffer path.  HOST_COHERENT      │
│  Vulkan allocations hang GPU compute for the same MTYPE reason.                      │
└──────────────────────────────────────────────────────────────────────────────────────┘

        See: Shader ISA → memory classes — Compute Memory Path — Ch.5 amdgpu
```

## Compute Memory Path

```
── COMPUTE MEMORY PATH ────────────────────────── UMA · GDDR6 via UMC · two apertures ──

Compute reaches memory through the unified controllers: 16 GB of GDDR6 on a
256-bit bus across eight channels, driven by two UMC 8.1.1 instances and shared
with the CPU over the Data Fabric.  Two hardware apertures reach the same physical
DRAM by different routes.

┌─ MEMORY ─────────────────────────────────────────────────────────────────────────────┐
│  Memory            16 GB GDDR6 on-package — 256-bit, 8 channels                      │
│  Controller        UMC 8.1.1 ×2                                                      │
│  Hub path          MMHUB (GPU-side address translation)                              │
│  Memory clock      single DPM state (450 MHz); no multiple mclk levels               │
└──────────────────────────────────────────────────────────────────────────────────────┘

 System aperture   via Data Fabric    GTT, RADV production allocations
                                      (the full-bandwidth path)
 FB aperture       via MMHUB (BAR0)   VRAM-default allocations
                                      (the reduced path)

Logical apertures visible to the compute APIs (BIOS UMA=Auto):

 VRAM          256 MiB dedicated carve-out at system-memory start
 GTT           16 GiB system RAM visible to the GPU via the host aperture
 System heap   16 GiB total, shared — no dedicated GPU memory (UMA)

RADV exposes two relevant Vulkan memory types, both backed by the same GDDR6:

 Type 0x0001   DEVICE_LOCAL                                  PTE MTYPE NC
 Type 0x0007   DEVICE_LOCAL | HOST_VISIBLE | HOST_COHERENT   PTE MTYPE CC

RADV allocates DEVICE_LOCAL for compute buffers; a HOST_COHERENT allocation forces
MTYPE_CC for that buffer object.  On UMA the staging copy is a CPU memcpy rather
than a DMA transfer and staging buffers are skipped; prefer_host_memory=true
allocations bypass the GTT copy path entirely — host-allocated buffers the CPU
writes directly and the GPU reads at full bandwidth (zero-copy).  A Dynamic UMA
Pool (DUP) kernel patch adds console-style garlic/onion runtime allocation on top.
With UMA=Auto the stock driver routes GPU allocations through GTT correctly, so
the apu_prefer_gtt patch (needed only when UMA is forced to 512 MB) is not
required on this board.

┌─ CAUTION ────────────────────────────────────────────────────────────────────────────┐
│  Mesa buffer-import changes have previously corrupted host-allocated GPU-visible     │
│  (zero-copy / DUP) buffers — pin the Mesa version and run a correctness              │
│  regression test before any Mesa upgrade.                                            │
└──────────────────────────────────────────────────────────────────────────────────────┘

        See: Ch.2 → UMC / Memory, MMHUB — Ch.4 → BIOS memory (UMA)
```

## Command Processor & Microengines

```
── COMMAND PROCESSOR ────────────────────────────────── GFX ring front-end · MEC dead ──

The graphics command processor — microengine (ME), prefetch parser (PFP), and
constant engine (CE), sequenced by the RLC — is the sole working dispatch
front-end on gfx1013.  The MEC async-compute microengine is present and loaded but
defective at the silicon level, so all production compute routes through the GFX
ring.

┌─ MICROENGINES ───────────────────────────────────────────────────────────────────────┐
│  ME    microengine — executes PM4 packets                             firmware 0x63  │
│  PFP   prefetch parser — parses ring ahead of ME                      firmware 0x94  │
│  CE    constant engine                                                firmware 0x25  │
│  RLC   run-list controller — power/state sequencing                   firmware 0x0D  │
│  MEC   async-compute microengine (defective)                          firmware 0x90  │
└──────────────────────────────────────────────────────────────────────────────────────┘

┌─ RINGS & LOAD ───────────────────────────────────────────────────────────────────────┐
│  Rings exposed     GFX · COMPUTE (MEC-backed, unusable) · SDMA0 · SDMA1 · VCN        │
│  Dispatch slots    4096/4096 exposed on the GFX ring (correct encoding)              │
│  Firmware load     PSP → SMU → MEC → GFX → SDMA → RLC (PSP-verified)                 │
└──────────────────────────────────────────────────────────────────────────────────────┘

        See: PM4 Dispatch — Ch.2 → GPU block — Ch.5 → amdgpu ring mgmt
```

### MEC — non-functional async compute

```
The MEC is the designed async-compute engine: four pipes of eight queues each, the
path ROCm/HIP and the KFD scheduler normally use.

┌─ MEC DEFECT ─────────────────────────────────────────────────────────────────────────┐
│  Design        4 pipes × 8 queues (async compute rings)                              │
│  Defect        non-atomic global/flat ops from the compute queue hang                │
│                the GPU; MEC queue init hangs the ring                                │
│  Works         atomic ops only (global_atomic_swap/_add)                             │
│  Fails         global_load/store_dword, flat_load/store_dword                        │
│  FW fix        ruled out — alternate MEC revisions also hang;                        │
│                the defect is silicon-level                                           │
│  Recovery      reboot (module reload may not clear ring state)                       │
└──────────────────────────────────────────────────────────────────────────────────────┘

    designed path (BROKEN)                 working path
  app → ROCm/HIP → KFD → MEC          app → RADV / rusticl → ACO
                    │                            │
           4 pipes × 8 queues             PM4 IB → amdgpu CS ioctl
                    │                            │
          non-atomic store →                GFX ring (PFP / ME)
          RING HANG (silicon)                    │
                                             CUs (wave64)

Consequences and the bypass:

 - ROCm/HIP's KFD path creates an MEC compute queue at dispatch → hang.  The ROCm
   OpenCL runtime inits an MEC queue during platform discovery → hang.  ROCm is
   research-tier only (out-of-tree patches, no upstream), blocked independently by
   the MEC hang (dispatch) and the LLVM VGPR over-allocation (compile).
 - Mesa forces the bypass in ac_gpu_info.c by setting num_queues = 0 for
   AMD_IP_COMPUTE on CHIP_GFX1013, so RADV exposes zero compute-queue families and
   every Vulkan/rusticl dispatch submits to AMDGPU_HW_IP_GFX.
 - Non-atomic global stores work on the GFX ring, confirming the defect is
   MEC-specific, not a general compute failure.

┌─ CAUTION ────────────────────────────────────────────────────────────────────────────┐
│  Never rely on non-atomic global_store on the MEC path — silent corruption or a      │
│  crash with no error signaled.  Disable the ROCm OpenCL ICD before any OpenCL        │
│  tool runs — any process calling clGetPlatformIDs() while amdocl64 is registered     │
│  triggers MEC queue init and hangs the whole GPU (all Vulkan, rusticl, amdgpu-       │
│  fence clients; reboot-only recovery):                                               │
└──────────────────────────────────────────────────────────────────────────────────────┘

 $ sudo mv /etc/OpenCL/vendors/amdocl64.icd \
           /etc/OpenCL/vendors/amdocl64.icd.disabled

This disables only the ICD registration; the library stays on disk for ROCm/HIP
callers.  Tooling should refuse to run if the live amdocl64.icd file is present.

        See: PM4 Dispatch — Ch.2 → GPU block — Ch.5 → amdgpu ring mgmt
```

## SDMA Copy Engines

```
── SDMA ──────────────────────────────────────────────── two DMA engines · SDMA 5.0.1 ──

Two System DMA engines handle buffer copies, fills, and page-table updates
independently of the shader core.  They are the board's GPU copy engines; on UMA
their role in the compute path is reduced, since host-to-device staging collapses
to a CPU memcpy.

┌─ SDMA ───────────────────────────────────────────────────────────────────────────────┐
│  Engines           SDMA0 · SDMA1                                                     │
│  IP version        SDMA 5.0.1                                                        │
│  Firmware          0x34 (both engines)                                               │
│  Rings             SDMA0, SDMA1 — exposed alongside GFX, COMPUTE, VCN                │
│  Role              buffer copy / fill / PTE update, async to compute                 │
└──────────────────────────────────────────────────────────────────────────────────────┘

        See: Ch.2 → GPU block (IP enum) — Compute Memory Path (UMA staging)
```

## PM4 Dispatch & Programming Model

```
── PM4 DISPATCH ──────────────────────────────────── PKT3 on the GFX ring · RADV-only ──

Compute is dispatched by writing SH-space registers with SET_SH_REG, then issuing
DISPATCH_DIRECT — all as PM4 type-3 (PKT3) packets on the GFX ring.  With correct
encoding the ring exposes 4096/4096 dispatch slots; with incorrect encoding it
hangs, which is why production compute never hand-rolls packets.

       compiled shader ISA (ACO output)
                  │
    RADV builds a PM4 indirect buffer:
    ACQUIRE_MEM · SET_SH_REG · DISPATCH_DIRECT
                  │
       DRM_IOCTL_AMDGPU_CS  (amdgpu ioctl)
                  │
    kernel wraps the IB:  COND_EXEC · CONTEXT_CONTROL · FRAME_CONTROL ·
    WAIT_REG_MEM · INDIRECT_BUFFER · FRAME_CONTROL · RELEASE_MEM (fence)
                  │
         GFX ring — PFP parses, ME executes  →  CUs execute wave64 fronts
```

### PKT3 header

```
 #define PACKET3(op, n)  ((3 << 30) | ((op) << 8) | ((n) << 16))

 Type     bits 31:30   always 3 (type-3 packet)
 Count    bits 29:16   dwords following the header, encoded as N-1
 Opcode   bits 15:8    PM4 opcode

The count field is bits 29:16, NOT 13:0.  A NOP with count 0 is insensitive to
this (all-zero shifts), which historically masked a mis-placed count field.

Per-opcode support from a userspace IB on the GFX ring:

 NOP                0x10   pass (PFP-local)
 DISPATCH_DIRECT    0x15   works (compute dispatch)
 DISPATCH_INDIRECT  0x16   works
 CONTEXT_CONTROL    0x28   context save/restore
 WRITE_DATA         0x37   works
 ACQUIRE_MEM        0x58   pass
 SET_SH_REG         0x76   pass
 SET_UCONFIG_REG    0x79   user-config write
```

### Compute SH register map

```
SH-offset convention A (GC-base relative):
SH_offset = (GC_BASE + mmREG) - 0x2C00, GC_BASE = 0x1260, window 0x2C00–0x3400.

 mmREG    SH offset   Register
 ──────   ─────────   ─────────────────────────────────────────────
 0x2E07   0x6007      COMPUTE_NUM_THREAD_X
 0x2E08   0x6008      COMPUTE_NUM_THREAD_Y
 0x2E09   0x6009      COMPUTE_NUM_THREAD_Z
 0x2E0C   0x600C      COMPUTE_PGM_LO           (shader VA low 32)
 0x2E0D   0x600D      COMPUTE_PGM_HI           (shader VA high)
 0x2E12   0x6012      COMPUTE_PGM_RSRC1        (VGPR/SGPR alloc)
 0x2E13   0x6013      COMPUTE_PGM_RSRC2        (user-SGPR count, scratch)
 0x2E14   0x6014      COMPUTE_PGM_RSRC3        (wave limit, shared VGPR count)
 0x2E15   0x6015      COMPUTE_RESOURCE_LIMITS  (CU/SE limits)
 0x2E18   0x6018      COMPUTE_TMPRING_SIZE     (scratch ring)
 0x2E40   0x6040      COMPUTE_USER_DATA_0
 0x2E41   0x6041      COMPUTE_USER_DATA_1
 0x2E42   0x6042      COMPUTE_USER_DATA_2

Representative RADV-matched values: RSRC1 = 0x002C0040, RSRC2 = 0x00000098,
RESOURCE_LIMITS = 0x00000000, TMPRING_SIZE = 0x00000000, NUM_THREAD = (64,1,1).

SH-offset convention B (SI_SH_REG_OFFSET relative), used when emitting SET_SH_REG
directly against the compute block: offset = (register_byte_addr - 0xB000) / 4.
Using 0xB800 as the base is a known encoding error that leaves compute state
uninitialized.

 Register                          Byte addr   Offset
 ────────────────────────────────  ─────────   ──────
 COMPUTE_PGM_LO                    0xB830      0x20C
 COMPUTE_PGM_HI                    0xB834      0x20D
 COMPUTE_PGM_RSRC1                 0xB848      0x212
 COMPUTE_PGM_RSRC2                 0xB84C      0x213
 COMPUTE_RESOURCE_LIMITS           0xB854      0x215
 COMPUTE_STATIC_THREAD_MGMT_SE0    0xB858      0x216
 COMPUTE_TMPRING_SIZE              0xB860      0x218

The RADV compute preamble (gfx10_init_compute_preamble_state) writes, before any
dispatch: CP_COHER_START_DELAY; TA_CS_BC_BASE_ADDR / _HI; COMPUTE_PGM_HI;
COMPUTE_STATIC_THREAD_MGMT_SE0..SE3; COMPUTE_USER_ACCUM_0..3.
```

### dispatch_initiator and compute select

```
DISPATCH_DIRECT dispatch_initiator bit layout (example value 0xA045):

 bit 0    COMPUTE_SHADER_EN    enable compute shader
 bit 2    FORCE_START_AT_000   start at threadgroup (0,0,0)
 bit 6    ORDER_MODE           ordered mode
 bit 13   TUNNEL_ENABLE        tunnel enable
 bit 15   CS_W32_EN            wave32 mode select

The compute-vs-graphics selection is NOT in dispatch_initiator: it is the
SHADER_TYPE_S bit (bit 1) of the DISPATCH_DIRECT PKT3 header word, which tells the
ME to treat the dispatch as compute.
```

### Captured packet sequences

```
RADV IB sequence (via RADV_DEBUG=dumpibs):

 Preamble IB   ACQUIRE_MEM (full cache invalidation)
               EVENT_WRITE (PIPELINESTAT_START)
 Main IB       WRITE_DATA
               SET_SH_REG (PGM_LO)
               SET_SH_REG (RSRC1 + RSRC2)
               SET_SH_REG (RSRC3)
               SET_SH_REG (RESOURCE_LIMITS)
               SET_SH_REG (NUM_THREAD = 64,1,1)
               SET_SH_REG (USER_DATA_2)
               DISPATCH_DIRECT (1,1,1, initiator = 0xA045)
               DMA_DATA

Kernel ring wrapping (decoded from gfx_v10_0_ring_emit_*): COND_EXEC;
CONTEXT_CONTROL (0x81018003 = full context load, LOAD_EN + SHADOW_EN on all
context blocks); FRAME_CONTROL (DE_FRAME_START); WAIT_REG_MEM; INDIRECT_BUFFER
(jump to the user IB); FRAME_CONTROL (DE_FRAME_END); RELEASE_MEM (completion
fence).
```

### The three masking encoding bugs

```
Three independent encoding errors together masked working GFX-ring dispatch; all
must be correct simultaneously:

 1. SET_SH_REG base — use 0xB000 (SI_SH_REG_OFFSET), not 0xB800; the wrong base
    runs the dispatch with uninitialized compute state.
 2. PKT3 count off-by-one — count is (dwords − 1); a value one too high makes each
    packet consume a dword from the next, cascading misalignment through multi-
    packet IBs (often a garbage shader address → ring stall).
 3. Shader memory SEG field — global_store_dword needs SEG=10 (GLOBAL, address =
    SADDR+VADDR), not SEG=00 (FLAT, address from VADDR only), which writes to 0.

A wrong register offset writes an SH register to a reserved index → ring reset;
register offsets are gfx1013-specific (it is not Navi 10).

┌─ CAUTION ────────────────────────────────────────────────────────────────────────────┐
│  Production compute is RADV-only — never hand-roll PKT3 encoding from the spec;      │
│  start any low-level PM4 work from RADV's exact macros, and keep experimental        │
│  raw-PM4 paths in dedicated, clearly warned test tools.  Pin the GPU clock at or     │
│  above 1000 MHz before dispatching (a dispatch at lower clock stalls the command     │
│  processor — a deterministic hang; reboot to recover).  Wrap every GPU test in a     │
│  3-second timeout, run tests synchronously in the foreground, and scale dispatch     │
│  sizes up from a 4×4×4 grid, stopping at the first hang.                             │
└──────────────────────────────────────────────────────────────────────────────────────┘

        See: Command Processor — Ch.2 → SMU clocks — Ch.5 → amdgpu driver
```

## Vulkan Compute — RADV

```
── VULKAN — RADV ─────────────────────────────── production surface · ACO · GFX queue ──

RADV, Mesa's AMD Vulkan driver, is the production compute surface on gfx1013.  It
compiles shaders through the ACO back-end, emits PM4 directly, and submits
everything to the GFX queue — the flow never touches MEC/KFD, which is why it is
stable where ROCm hangs.

┌─ RADV ───────────────────────────────────────────────────────────────────────────────┐
│  API version       Vulkan 1.4 (RADV)                                                 │
│  Shader compiler   ACO (Mesa src/amd/compiler/; RADV src/amd/vulkan/)                │
│  Command stream    PM4 — RADV builds indirect buffers directly                       │
│  Compute queue     GFX queue (graphics ring; zero MEC families exposed)              │
│  Submission        amdgpu ioctl DRM_IOCTL_AMDGPU_CS                                  │
└──────────────────────────────────────────────────────────────────────────────────────┘

Dispatch flow: GLSL/HLSL → glslc/dxc → SPIR-V → RADV → NIR (Mesa's
target-independent IR: constant folding, DCE, algebraic simplification) → ACO →
gfx1013 ISA → shader cache; RADV then builds the PM4 IB (CONTEXT_CONTROL,
SET_SH_REG, DISPATCH_DIRECT) and submits via the CS ioctl to the GFX ring.
rusticl feeds the same NIR/ACO back-end from a CL-C front end.

Why ACO and not LLVM: the LLVM AMDGPU allocator targets gfx103x budgets (512+
VGPRs) and over-allocates for gfx1013; ACO's linear-scan allocator with
backtracking is tuned to the real 256-VGPR wave64 envelope and compiles shaders
LLVM rejects.  LLVM remains the source of truth for what gfx1013 is (AMDGPU.td);
ACO generates the working code.  gfx1013 quirks the compiler handles:

 Wave size      wave64 native (no Wave32 mode; wave32 is double-pumped)
 VGPRs          256 max (some targets allow 512)
 BVH            GFX10_AEncoding, unique to gfx1013 in RDNA 1
 NSA addressing 5 register groups in non-contiguous VGPRs for BVH
 INT8 dot       no V_DOT4_I32_I8 — integer dots lower to V_FMA_F32 / V_PK_FMA_F16

Inspection / debug environment:

 RADV_DEBUG=shaders    dump compiled ISA for all shaders
 RADV_DEBUG=dumpibs    dump submitted PM4 IBs
 ACO_DEBUG=noopt       disable ACO optimizations
 NIR_DEBUG=noopt       disable NIR optimizations

External tools: spirv-dis, spirv-opt, spirv-val, RGA (Radeon GPU Analyzer), umr
(read GPU registers / inspect shader state).
```

### Extension and property matrix

```
Supported and relevant to compute:

 VK_KHR_push_descriptor            eliminate descriptor-set allocation
 VK_KHR_ray_query                  BVH spatial search in compute shaders
 VK_KHR_acceleration_structure     build BVH trees
 VK_KHR_16bit_storage              FP16 storage access in shaders
 VK_KHR_shader_float16_int8        FP16/INT8 arithmetic
 VK_KHR_buffer_device_address      pointer-like buffer access
 VK_EXT_subgroup_size_control      subgroup-size selection
 VK_EXT_device_generated_commands  GPU-side command generation (RADV, Mesa 26.x)

Missing / not supported:

 VK_KHR_cooperative_matrix                 RDNA 2+ only (no WMMA/MFMA silicon)
 VK_NV_device_generated_commands           NVIDIA only
 VK_EXT_device_generated_commands_compute  superseded by the non-_compute ext
 VK_EXT_mesh_shader                        not applicable to compute-only use

Device properties:

 Subgroups   subgroupSize 64; minSubgroupSize 32, maxSubgroupSize 64
             (no native Wave32 — a wave32 subgroup is double-pumped emulation)
 Integer dot every integerDotProduct*Accelerated field false —
             INT8 dot products are emulated, not hardware
 Ray tracing rayQueryEXT in compute; hardware BVH traversal;
             has_image_bvh_intersect_ray = GFX10_3+ || CHIP_GFX1013
 Memory      maxMemoryAllocationSize 16 GB; deviceLocal 16 GB;
             hostVisible 16 GB (UMA — all memory is both)
```

### Dispatch recording

```
Each compute dispatch records this command sequence:

 1. vkCmdPushDescriptorSetKHR   bind input/output buffer addresses
 2. vkCmdPushConstants          pass dimensions, offsets, shader parameters
 3. vkCmdBindPipeline           select the compiled compute pipeline
 4. vkCmdDispatch               launch the workgroups
 5. vkCmdPipelineBarrier        COMPUTE_SHADER stage, SHADER_WRITE →
                                SHADER_READ — order dependent dispatches

VK_KHR_push_descriptor collapses the four-call legacy path
(vkAllocateDescriptorSets → vkUpdateDescriptorSets → vkCmdBindDescriptorSets →
vkCmdDispatch) into two calls, removing the descriptor pool, its lifetime
tracking, and the pool-exhaustion bug class; when absent the legacy path is the
fallback.

Command-buffer strategy: one command buffer per submission carries the full
dispatch chain — one kernel ioctl, not one per dispatch.  Pipeline barriers enforce
inter-dispatch dependencies and the hardware scheduler chains dispatches back-to-
back as barriers clear; CPU recording of dispatch N+1 overlaps GPU execution of
dispatch N.  On gfx1013 the barrier additionally forces L2 writeback +
invalidation for read-write buffers — required because L2 coherency across
dispatches is manual.

┌─ CAUTION ────────────────────────────────────────────────────────────────────────────┐
│  Never allocate compute buffers HOST_COHERENT (MTYPE hang — see Cache Hierarchy &    │
│  Coherency); pin the Mesa version and regression-test buffer-import correctness      │
│  before upgrades.                                                                    │
└──────────────────────────────────────────────────────────────────────────────────────┘

        See: PM4 Dispatch — Cache Hierarchy & Coherency — Ch.5 → Mesa / RADV
```

## OpenCL — rusticl

```
── OPENCL — RUSTICL ──────────────────────────────── OpenCL 3.0 · same ACO / GFX path ──

rusticl is Mesa's Rust-based OpenCL 3.0 runtime and the only working OpenCL on
gfx1013.  Its dispatch is structurally identical to the Vulkan path — CL-C → clang
→ SPIR-V into the same NIR/ACO back-end, PM4 onto the GFX ring via amdgpu —
differing only in the front end.  The ROCm OpenCL ICD (amdocl64) dispatches via
KFD/MEC and hangs during platform enumeration; there is no ROCm-OpenCL workaround,
and the ICD must be disabled before any OpenCL tool runs (see Command Processor).

┌─ RUSTICL ────────────────────────────────────────────────────────────────────────────┐
│  Runtime           Mesa rusticl (radeonsi / Gallium)                                 │
│  OpenCL version    3.0 (OpenCL C 1.2)                                                │
│  Compile pipeline  CL-C → SPIR-V (clang) → ACO (shared with RADV)                    │
│  Dispatch path     GFX ring via amdgpu                                               │
└──────────────────────────────────────────────────────────────────────────────────────┘

Static device capabilities (as enumerated in the stock 24-CU configuration):

 Compute units          24 (stock harvest; full die 40 with the CU-liberation
                        patch — see Chapter 5)
 Max work-group size    256
 Work-item dimensions   3 (max sizes 256 × 256 × 256)
 Preferred WG multiple  64 (wavefront size)
 Global memory          16 GB GDDR6, 256-bit bus
 Local memory (LDS)     32,768 bytes per CU (dedicated)
 Max constant buffer    64 KB
 Max buffer allocation  ~75% of global
 Image support          yes; max 2D image 16384 × 16384
 FP64 / FP16            supported (cl_khr_fp64, cl_khr_fp16)
 FP32 denormals         supported

Address-space mapping to hardware:

 __global     GDDR6 via MMHUB — all work-items, all CUs
 __local      LDS, 64 KB/WGP (32 KB/CU), 32 banks — work-group scope
 __private    VGPRs (default); spills to scratch (global)
 __constant   scalar cache (K$), 16 KB/WGP (2 CUs) — read-only, broadcast

Synchronization is barrier(CLK_LOCAL_MEM_FENCE) / barrier(CLK_GLOBAL_MEM_FENCE)
within a work-group; no cross-work-group sync exists inside a dispatch.  SVM:
rusticl exposes OpenCL C 1.2 with no SVM of any granularity (coarse buffer / fine
buffer / fine system all unsupported) and routes host/device sharing through the
system aperture; on a UMA part fine-grained system SVM would eliminate buffer
copies entirely, but it is not exposed.
```

### Extension matrix

```
 Precision      cl_khr_fp64, cl_khr_fp16, cl_khr_int64_base_atomics,
                cl_khr_int64_extended_atomics
 Sub-group      cl_khr_subgroups, cl_khr_subgroup_extended_types,
                cl_khr_subgroup_non_uniform_vote, cl_khr_subgroup_ballot,
                cl_khr_subgroup_shuffle (64 lanes on wave64; hardware lane
                shuffles, no barrier)
 Memory/atomic  cl_khr_global_int32_base_atomics,
                cl_khr_global_int32_extended_atomics,
                cl_khr_local_int32_base_atomics,
                cl_khr_local_int32_extended_atomics,
                cl_khr_byte_addressable_store, cl_khr_3d_image_writes
 Media/bit ops  cl_amd_media_ops (SAD, lerp, byte pack/unpack),
                cl_amd_media_ops2 (msad, amd_bfe, amd_bfm,
                amd_max3/amd_min3/amd_median3), cl_amd_popcnt
                — map directly to RDNA 1.x VALU instructions
 Device query   cl_amd_device_attribute_query, cl_khr_device_uuid
 Ingest/misc    cl_khr_il_program (SPIR-V), cl_khr_spir, cl_khr_icd,
                cl_khr_gl_sharing, cl_amd_printf, cl_amd_compiler_options

cl_amd_device_attribute_query hardware constants — applications combine the GFXIP
pair with the PCIe ID to detect the exact architecture and select a per-arch
kernel variant:

 CL_DEVICE_GFXIP_MAJOR_AMD             10 (RDNA 1)
 CL_DEVICE_GFXIP_MINOR_AMD             1 (gfx101x)
 CL_DEVICE_SIMD_PER_COMPUTE_UNIT_AMD   2
 CL_DEVICE_WAVEFRONT_WIDTH_AMD         64
 CL_DEVICE_LOCAL_MEM_BANKS_AMD         32 (LDS banks)
 CL_DEVICE_GLOBAL_MEM_CHANNELS_AMD     8 (256-bit bus)
 CL_DEVICE_PCIE_ID_AMD                 0x13FE
 CL_DEVICE_BOARD_NAME_AMD              marketing name string
 CL_DEVICE_TOPOLOGY_AMD                PCI bus/device/function

┌─ CAUTION ────────────────────────────────────────────────────────────────────────────┐
│  Disable the ROCm OpenCL ICD before any OpenCL tool loads (see Command Processor     │
│  — the clGetPlatformIDs() hang); after disabling, only rusticl is enumerated.        │
└──────────────────────────────────────────────────────────────────────────────────────┘

        See: Vulkan Compute (shared ACO) — Wavefront & Execution Model — Ch.5
```


