# nct6687-bc250 - writable fan control for the BC-250 carrier

The ASRock BC-250 carrier board's fans hang off a Nuvoton **NCT6686D** Super-I/O
running in **EC-firmware mode** (custom ASRock EC firmware, "NCT6686D EC firmware
version 1.0 build 07/28/21"). The in-kernel `nct6683` driver reads its sensors
but is **read-only** - the EC ignores PWM writes made through nct6683's
FAN_CFG-REQ/DONE handshake. Fan speed is left to the EC's built-in temperature
curve.

Fred78290's out-of-tree **`nct6687`** driver uses a different register protocol
(page/index/data EC window, `reg_pwm_write` per channel) that the BC-250 EC
**does** honour - verified: writing pwm2 = 255 drove the cooler 3200 -> 4395 rpm,
pwm2 = 180 -> 3636 rpm, and restoring pwm_enable = 2 returned it to the EC auto
curve. But stock `nct6687d force=true` refuses to attach to the BC-250. This
patch fixes three EC-firmware-mode blockers.

## The patch (`0001-nct6687-bc250-ec-firmware-attach.patch`)

Against upstream `Fred78290/nct6687d` @ `cd735225`:

1. **Chip-ID guard.** In EC mode the SIO chip-ID register (`SIO_REG_DEVID`)
   reads back `0xffff` (open bus) even though the EC data window and the full
   NCT6686D register map are present and correct. Upstream's `force=1` guard
   rejects anything outside `0xD000-0xDFFF`. Accept `0xffff` **only on the
   primary SIO port `0x2e`** (where the BC-250 EC lives) and attach as an
   `nct6686`. Gating to `0x2e` is essential: a `0xffff` on the secondary port
   `0x4e` is genuine "nothing present" - accepting it too makes both ports
   resolve to the same EC base `0x0a20` and collide on platform-device
   registration (`nct6687.2592`, `-EEXIST`).

2. **EC base address.** The SIO logical-device base-address register (`0x60`)
   is likewise open-bus on cold access, so the driver can't discover the EC I/O
   window. Hard-code the BIOS-assigned base `0x0a20` (what the in-kernel
   nct6683 reports as `0x2e:0xa20`) when attached via the EC-mode path.

3. (Consequence of 1) the dual-port `-EEXIST` self-collision, fixed by the
   `0x2e` gate above.

Readings were validated against nct6683's known-good values (SoC temp, rail
voltages, fan RPM all matched) before any PWM write, confirming the register
map is correct.

## Build + install

The BC-250 is x86-64-**v3**; CachyOS host build tools are x86-64-**v4**, so the
module cannot be built on the board (`fixdep: CPU ISA level is lower than
required`). Build on a v4 host against the board's kernel build tree, copy the
`.ko` over, install:

```
# on a v4 host (a modern x86-64 build host), against the board's rsync'd /lib/modules/<ver>/build:
./build-and-install.sh build /path/to/kbuild-tree
# copy the resulting nct6687.ko to the board, then on the board:
./build-and-install.sh install nct6687.ko
```

`install` drops the module in `/lib/modules/<ver>/updates/`, blacklists
`nct6683`, and autoloads `nct6687 force=true` at boot. It attaches as hwmon
name `nct6686` with `driver=nct6687` (the chip name, not the driver name - don't
match on `nct6687` when looking for the node).

## Consumed by aputune

`aputune`'s `telemetry::ensure_carrier_sensors()` loads this driver; `fan_writable()`
reports whether the writable driver is bound; `set_fan_duty(channel, pct)` drives
a channel (or restores the EC auto curve with `None`). Channel 2 = the Pump-Fan
header that carries the main BC-250 cooler.
