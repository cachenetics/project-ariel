# Project Ariel

**Liberating the ASRock BC-250.**

Project Ariel is [Cachenetics'](https://cachenetics.com) open-source effort to reverse
engineer and liberate the ASRock BC-250 - a Zen 2 + RDNA accelerator on a harvested AMD
console-class APU (codename *Ariel*, PCI `1002:13FE`), built by the tens of thousands for a
crypto workload that evaporated and now sold as e-waste: 16 GB of unified memory and a real
GPU, locked down and underclocked as shipped.

We tear down the firmware and the silicon - mapping SMU mailboxes, freeing the full GPU,
retraining memory, and lifting the factory clock and power limits - to unlock hardware the
vendor left crippled. The result is a board that punches well above its throwaway price:
console-class compute for the cost of e-waste. All open, all reproducible, all documented.

## Tools

- **[arieltune](arieltune/)** - the unified BC-250 tuning suite. One tabbed TUI + CLI
  (WIKI / BIOS / APU / MEM): browse the firmware surface, tune GDDR6 memory timings, unlock
  and tune the APU (40-CU liberation, CPU/GPU/CU control), and read the hardware-verified
  BC-250 manual. Built on reverse-engineered SMU mailboxes and a liberated amdgpu.

## Get it running

Runs on a **BC-250** (or an Ariel-APU board) under Linux x86-64. You need a Rust
toolchain to build ([rustup.rs](https://rustup.rs)) and `root` for any actuation.

```sh
git clone https://github.com/cachenetics/project-ariel.git
cd project-ariel
./install.sh                 # build (release) + install to /usr/local/bin
arieltune                    # launch the TUI (opens on WIKI)
```

`install.sh` installs one binary (`arieltune`), a short `at` alias, and
`aputune`/`memtune`/`biostune`/`wikitune` compat symlinks. Jump straight to a tab with
`arieltune apu` (or `bios` / `mem` / `wiki`); everything is also scriptable from the CLI.
See [`arieltune/README.md`](arieltune/README.md) for the full build/usage guide.

## Safety

These tools **write hardware** - SMU registers, CMOS/NVRAM, SPI flash, memory training. A
bad value can fail to POST and need a power-cycle or CMOS-clear. Read each tool's
`SECURITY.md` and its inline safety notes first; firmware and memory tuning are done at your
own risk, on hardware you own.

## Licensing

**GPL-2.0-only** for the entire project. The kernel-derived parts (the amdgpu liberation
patch series and kernel modules, under `arieltune/`) were always GPL-2.0, inheriting the
Linux kernel's license; the rest of the tree now matches. See `LICENSE` and the per-subtree
NOTICE files under `arieltune/crates/` for upstream provenance and attribution.
