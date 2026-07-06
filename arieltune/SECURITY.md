# Security and responsible use

arieltune is a hardware-tuning suite for the AMD BC-250 (Cyan Skillfish /
gfx1013 "Ariel" APU). It talks directly to low-level parts of a board you own:

- **SMU mailboxes** - GPU/CPU clocks, voltages, CU routing, telemetry.
- **CMOS / NVRAM** - memory timings and platform settings.
- **SPI flash via SMM** - OEM `Setup`/CBS settings, through the platform
  firmware's own SW-SMI update path (the `smiflash` kernel module).

## Intended use

These features are for **hardware you own or are authorized to modify**. The
BIOS/SMM and flash features in particular are powerful: they use the board's own
intended firmware-update mechanisms (the same paths a vendor BIOS updater uses),
not novel exploits, but a bad write can leave a board that fails to POST.

There is a legitimate defensive angle too: inspecting and testing your own
board's SMM and SPI-flash surface is exactly what open firmware-security and
firmware tooling exists for. Prior art in this space includes
[CHIPSEC](https://github.com/chipsec/chipsec) (platform security assessment),
[coreboot](https://www.coreboot.org/) (open firmware), and
[flashrom](https://www.flashrom.org/) (SPI-flash read/write). arieltune's
smiflash module is a board-specific instance of the same class of tool.

## Risk / recovery

- Actuation requires **root** and a real BC-250 (or Ariel-APU) board.
- A bad CMOS/NVRAM or flash write can fail to POST and may require a
  **power-cycle or CMOS clear** to recover.
- **Use at your own risk.** There is no warranty (see `LICENSE`).

The suite enforces guards where it can - BC-250 detection before any hardware
write, an APCB-slot write guard on the flash path, and voltage/brick guards on
the SMU path - but these are best-effort protections on inherently dangerous
operations.

## Reporting an issue

If you find a security-relevant defect (for example, a guard that can be
bypassed into an unsafe hardware write), please report it privately to
Cachenetics rather than opening a public issue with a working exploit path.
Include the affected component, the board/BIOS version, and steps to reproduce.
