# smiflash - the SMM SPI-flash driver behind OEM `Setup` editing

## What this is (responsible use)

`smiflash` is a firmware-research / owner-control tool for hardware you own. It
lets the OS read and write the board's own SPI flash by invoking the platform
firmware's OWN intended SMM update mechanism -- the same SW-SMI path a vendor
BIOS updater (AMI AFU) uses -- not a novel exploit. It is board-specific:
BC-250 / AMI Aptio V, with the SW-SMI command port read from the FADT `SMI_CMD`
field.

It has a defensive purpose too: understanding and testing your own board's SMM
and SPI-flash attack surface is exactly what open firmware-security tools
(CHIPSEC, coreboot, flashrom) exist for. The caller (arieltune) enforces the
safety gates -- BC-250 detection before any SMI, and an APCB-slot write guard.
Powerful: a bad write can require a CMOS clear (or worse) to recover. Use it
only on hardware you own, at your own risk.

arieltune's BIOS tab changes OEM `Setup` settings (Above 4G, IOMMU, SVM, …) with
**no external flash programmer** by driving AMI's `SmiFlash` SW-SMI handler from a
tiny kernel module. This directory is that module plus the packaging that lets a
BC-250 build a kernel-matched copy **on the board itself**.

`smiflash.c` is the reusable form of the research probe that proved the chain
live on a BC-250 (BIOS P3.00). It is a dumb single-SMI firer; all safety lives in
arieltune. See the header comment for the interface, and the note that the SW-SMI
command port is the FADT `SMI_CMD` field (**0xB0** on the BC-250, not the
conventional 0xB2).

## Install (recommended: DKMS - auto-rebuilds on kernel upgrades)

```sh
./install-dkms.sh                # dkms add/build/install for the running kernel
sudo arieltune bios driver load  # load it (auto-detects the SW-SMI port from the FADT)
```

`arieltune bios driver build` runs this for you when `dkms` is present.

## Why a build step at all (and why it works on the board)

A kernel module must match the running kernel's `vermagic` exactly, so it has to
be built for *your* kernel. That is normally trivial, but two things make the
ASRock BC-250 + CachyOS combination hard, both handled by `prepare.sh` (the DKMS
`PRE_BUILD` hook):

1. **v4 host tools on a v3 CPU.** CachyOS compiles the kernel's host tools
   (`fixdep`/`modpost`/`objtool`/…) with `-march=x86-64-v4`, so their ELF carries
   an "x86 ISA needed: v4" note and glibc refuses to start them on the Zen2
   BC-250 (no AVX-512): *"CPU ISA level is lower than required"*. But they only
   *use* baseline instructions - `prepare.sh` strips the advisory
   `.note.gnu.property` and they run unchanged. (Only done when the CPU lacks
   AVX-512, so it never touches a machine whose tools already run.)
2. **Incomplete headers.** The package omits `include/generated/autoconf.h` and
   strips some Kconfig sources. `prepare.sh` stubs the missing sources and runs
   `syncconfig` to generate it.

On a normal distro with complete headers, `prepare.sh` is a no-op and the module
builds the ordinary way.

## Manual build (no DKMS)

```sh
sudo ./prepare.sh            # no-op on a complete tree
make                        # -> smiflash.ko for the running kernel
sudo insmod smiflash.ko smi_port=0xB0
```
