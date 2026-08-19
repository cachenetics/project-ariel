# arieltune-core.md — BC-250 8-Core CPU Unlock: Design Handoff

Status: **LIVE AND VERIFIED** — blade 115 runs 8C/16T (mask 0xFF, forced from
an abnormal 0xD7), 0 MCE, two clean per-core stress sweeps, boot unit enabled.
Full validation + ops log: `docs/ops/blade115-validation-log.md` (internal
record, not part of upstream PRs).

Scope: the `aputune cores` feature, kernel patch 28, and the fleet rollout
plan. All conclusions below come from the 2026-08-18 investigation of the
community 8-core unlock forks, plus the live validation on blade 115.

## 0. At a glance

- **The primitive, once:** core-presence mask SMN `0x5A870` (SMU space
  `0x115A870`). Stock `0x77` = 6C/12T (cores 3 and 7 masked), `0xFF` = 8C/16T.
  Only writable through the SMU **queue-3 msg `0x98`**, which does a raw
  SMN-window write of a hardcoded `0xFF` to the address in arg0 —
  all-or-nothing, no value argument, no address validation.
- **Semantics:** a warm reboot re-enumerates the topology and preserves the
  mask; a cold boot (power removed) reverts it. Nothing is written to flash.
- **Surface:** `arieltune apu cores …` CLI verbs + a TUI Core Map panel +
  `aputune-cores.service` boot unit + kernel patch 28 (8-core telemetry).
- **Safety invariants:** no automatic reboots, ever · refuse unknown masks on
  every *safe* path · post-write readback verification · advisory-only `verify`
  (cloned-image lineage makes persisted verdicts untrustworthy).
- **Escape hatch (deliberate, explicit):** boards with an abnormal live mask
  (we met 0xD7 on blade 115) are refused by every safe path, but
  `cores apply --force-abnormal` (CLI) and `[F]` in the Core Map panel (TUI,
  warning popup + confirm) force the same 0x98 write. No force path exists at
  boot (the unit never forces).

## 1. Decision log — what was investigated and why we chose what we chose

### 1.1 The forks reviewed

| Repo | Method | Verdict |
|---|---|---|
| `rw-r-r-0644/bc250-core-unlock` | Runtime: Python sends SMU queue-3 msg `0x98` → writes `0xFF` to core-presence mask SMN `0x5A870`. Refuses non-`0x77` masks by default. | Reference writeup. The primitive. |
| `GabriWar/bc250-core-cu-unlock` | Runtime script + systemd persistence + kernel telemetry patch `0001` + ACPI fix + P5.00_clv BIOS ROM with "Unlock CPU Cores" Setup toggle (default OFF). | Best integrated package; adopted for Phase 1. Documents a real bootloop lesson. |
| `RescueMei/BC250-DXE-SMU-Core-Unlock` (v1) | DXE driver in BIOS performing the same q3 `0x98` exploit pre-OS, auto warm-reset. | Superseded by V2. |
| `RescueMei/BC250-DXEv2-BIOSMOD` | V2: core-unlock + ACPI-autoinject + cold-boot DXE drivers, xdelta against `BC250_3.00_CHIPSETMENU.ROM`, Setup toggle. | Phase 2 option for P3 boards only. |
| `keyboardspecialist` metrics patch | Assumes a *uniformly expanded* 8-wide metrics table. | **Wrong** — contradicted by measurement. Rejected. |
| `rw-r-r-0644/bc250-smu-unlock` | q2 msg `0x23` exploit → full arbitrary SMN read/write + code exec; firmware patch `metrics-8core.s`; arbitrary core masks via `set_core_mask.py`. BIOS-3 offsets only. | Research-only. **The experimental roadmap** (section 10). |
| `mikael2df-ux/bc250-40cu-unlock` | GPU CU-only content. | Not relevant to cores. |

### 1.2 The primitive, once

- Core presence mask: SMN `0x5A870` (SMU space `0x115A870`). Stock `0x77` =
  6C/12T (cores 3 and 7 masked), `0xFF` = 8C/16T.
- Only writable through the SMU: **queue-3 msg `0x98`** does a raw SMN-window
  write of a **hardcoded `0xFF`** to the address passed as arg0 — all-or-nothing,
  no value argument, no address validation.
- Warm reboot re-enumerates the topology and preserves the mask; a cold boot
  (power removed) reverts to `0x77`. Nothing is written to flash.

### 1.3 Key decisions and reasons

1. **Runtime unlock via q3 `0x98` in arieltune** (not DXE, not BIOS) — same
   primitive, zero flashing, BIOS-version-agnostic (works on P3 and P5).
   arieltune already owns the exact machinery: `ariel_hal::SmnAperture`
   (PCI-config 0xB8/0xBC index/data) and `ariel_smu::ocq3::OcQ3` (the queue-3
   mailbox with the exact terminal status codes the DXE driver uses). This is a
   thin typed wrapper, not a port of shell script.
2. **No automatic reboots, ever.** GabriWar's earlier systemd version rebooted
   after applying and bootlooped a real board (the reset didn't preserve the
   mask; `/var` state wasn't durable early enough; `systemctl reboot` can't run
   before D-Bus). Our unit applies the mask and stops; cores appear on the next
   *operator-chosen* reboot.
3. **Refuse abnormal masks — with one explicit escape hatch.** `cores apply`/
   `boot` proceed only when the live mask reads `0x77` (or already `0xFF`).
   Anything else is refused, same rule as GabriWar, rw-r-r, and the DXE driver
   ("avoid lockout on abnormal masks"). Because blade 115 shipped with a
   genuine abnormal mask (0xD7 — stock 0x77 with bit 6 set), the refusal is
   bypassable **only** through `apply --force-abnormal` or the TUI `[F]`
   confirmation popup: the same 0x98 write, the same 0xFF readback, an
   explicit warning, and never any auto-reboot. `boot` (the unit path) has no
   force.
4. **No verdict cache / no hardware identity.** The fleet clones OS images
   across blades, so any persisted "this blade passed verification" state is
   untrustworthy lineage. The only trusted checks are live reads of the current
   hardware. `cores verify` is therefore an **on-demand advisory tool**, never a
   gate. `cores apply` gates only on live facts: BC-250 silicon present, live
   mask == `0x77` (or forced), post-write live mask == `0xFF`.
5. **Granularity = live OS offlining, not firmware masks.** Firmware-level
   arbitrary masks exist but require the q2 `0x23` exploit (SMU code-exec class,
   BIOS-3-validated offsets) — see the roadmap (section 10). Every practical
   core count (2C test runs, skip a defective core) is achieved by `0xFF`
   unlock + `/sys/devices/system/cpu/cpuN/online` offlining, which is proven,
   instant, and reversible. Offlining keeps the SMU-visible topology at the
   exact 8-core shape all community fixes were validated against.
6. **Patch 28 (8-core telemetry) ships with the feature.** After unlock,
   `pp_dpm_sclk`'s starred value, OD_SCLK, `gpu_metrics.current_gfxclk`, and
   hwmon `freq1_input` all read a residency counter (the firmware redistributed
   the 116-byte metrics table; `GfxclkFrequency`'s 0x44 slot is now
   `C0Residency[6]`). Patch 28 is the kernel-side fix, ported to use our
   battle-tested `SMU_MSG_QueryGfxclk` (section 6). filippor's userspace
   bind-overlay fix was reviewed and rejected for us: it only patches hwmon,
   needs a mount dance, and we ship the kernel.
7. **GabriWar's `0002` (rocm-vm-flush) is redundant here** — our applied patch
   14(e) already forces `flush_pasid_uses_kiq = false` for gfx10.1.x (same line
   `0002` touches). Not staged; documented only.
8. **ACPI:** stock CST/PST tables stop at C00B (12 threads); after unlock,
   threads 12–15 get zero C-states. The 8-core fix (`mendesrr/
   bc250-acpi-fix-updated-8c` SSDT-CST + SSDT-PST) extends to C00F and
   over-covers every mask 0–16 threads, so it is installed once when unlocking.
   Over-coverage is harmless; under-coverage is the only failure mode.
9. **Governor safety verified:** arieltune's fence-rate governor never reads the
   metrics table for its control loop (fence counters + `temperature_gfx` at
   offset 4, which keeps its 0x46 slot in the hybrid layout). Only TUI/status
   display reads were exposed. Patch 28 still fixes them; the governor is
   immune regardless.
10. **Phase 2 (BIOS persistence) revised for fleet reality:** the hive runs a
    custom P5 BIOS (UMA memory options); the dev blade is P3. GabriWar's
    `BC250_P5clv_8core_v2.ROM` is P5.00_clv-based — a same-lineage upgrade for
    the hive. Rollout: dev blade stays runtime/P3 for development; later
    crossgrade dev to stock custom P5, validate, then flash GabriWar's ROM on
    one hive blade (backup first, sha256 pin, CMOS-clear escape — the ROM
    defaults cores OFF so CMOS clear is always recovery), then the rest. A
    full-image flash path in `arieltune bios` (in-system `flashrom -p internal`
    already exists there) is a candidate but not required now.

---

## 2. State machine and gates

```
                 cold boot (mask reverts)
LOCKED ─────────────────────────────────────────┐
 0x77 mask, 12 threads                           │
   │ apply/boot (q3 0x98, verified 0xFF)         │
   ▼                                             │
PENDING-REBOOT ── warm reboot ──▶ UNLOCKED ──────┘
 0xFF mask, 12 threads          0xFF mask, 16 threads
   │
   └─ any mask ∉ {0x77, 0xFF} ⇒ ABNORMAL (safe writes refuse;
      only the explicit force hatch writes)
```

- `LOCKED`: mask `0x77`, 12 visible threads.
- `PENDING-REBOOT`: mask `0xFF`, still 12 visible threads → "warm reboot needed".
- `UNLOCKED`: mask `0xFF`, 16 visible threads.
- `ABNORMAL`: any other mask → status explains; every *safe* mutating verb
  refuses. The force hatch (`--force-abnormal` / TUI `[F]`) is the only writer
  and always asks first.

Hard gates on every mutating verb, all live reads:
1. `ariel_apu_present()` — PCI `1002:13fe` present (it really is a BC-250).
2. `SmnAperture` open + short-transfer sanity.
3. Live mask read; proceed only from `0x77` (or no-op on `0xFF`).
4. Post-write live mask verify == `0xFF`; on failure report SMU status and stop.

Soft output (advisory, never blocking):
- "MCE entries in dmesg: N" in `status`.
- Post-apply warning: the SoC power/thermal envelope changes with 2 extra
  cores; existing CPU OC/UV and GPU VDDC settings must be re-validated.
- "cold boot reverts this; the service re-applies the mask but cores appear
  only after the next reboot."

---

## 3. CLI surface

```
aputune cores status                        # mask, state, visible cores, MCE count
aputune cores apply [--reboot] [--force-abnormal]  # unlock now; force bypasses the 0x77 gate
aputune cores boot                          # idempotent boot path for the unit (never forces)
aputune cores install                       # install /usr/local/bin + systemd unit, enable
aputune cores uninstall                     # remove unit + binary
aputune cores verify [seconds_per_core]     # on-demand stress-ng --verify sweep (advisory)
aputune cores acpi {status|install|revert}  # 8-core SSDT-CST/PST initcpio override
aputune cores offline {core|all} [..]       # live OS-layer per-core toggle (instant)
aputune cores online  {core|all} [..]
```

Root-gated by the existing `ariel_hal::require_root()` in `run_cli`. All verbs
print a final state line parseable as `state=LOCKED|PENDING-REBOOT|UNLOCKED|
ABNORMAL`.

Details:
- `apply`: sets `/sys/kernel/reboot/mode` to `warm` only with `--reboot`, then
  `systemctl reboot`. Default: no reboot, print the PENDING-REBOOT reminder.
  `--force-abnormal` skips the 0x77 gate (prints the starting mask + an
  EXPERIMENTAL warning first); the write and readback are identical to the safe
  path.
- `boot`: same safe write path, never reboots, exit 0 on already-unlocked and on
  refused (unknown mask) with a journal note — designed to be safe in a unit.
- `verify`: per-core `stress-ng --cpu 1 --cpu-method all --verify` sweep pinned
  to one thread per physical core, then an all-core sustained run, then a dmesg
  MCE grep. Prints bogo-ops, verify pass/fail, deviation-from-median table, and
  flags cores 3/7 as NEW. Writes a timestamped report to
  `/var/lib/aputune/cores-verify-<date>.txt` for records — **nothing reads it
  back for decisions** (cloned-image lineage problem).
- `acpi install`: fetches/stages the two 8-core AML tables into
  `/etc/initcpio/acpi_override/` (backups kept), appends the
  `acpi_override` hook to `/etc/mkinitcpio.conf` if absent, rebuilds the
  initramfs. `status` counts `C0xx` processor objects vs live thread count;
  `revert` restores the backup.
- `offline/online`: writes `/sys/devices/system/cpu/cpuN/online` (0/1), skips
  cpu0 (never offlinable), reports resulting thread count. Instant, no reboot.

---

## 4. TUI — the Core Map panel (CPU section)

The APU screen keeps its panel layout; the Core Map is its own panel between
the CPU panel and the GPU/CU row (the GPU/CU row shrank to make room), and it
is its **own Tab focus target** — focus order: system → CPU → Core Map → GPU →
CU. Only the focused panel lights and takes keys. The grid cells are rendered
by a fixed-width builder (padding computed from the styled parts), so rows can
never drift against the border; glyphs are `██`/`··` only, with reverse video
on the selected cell.

```
╭ Core Map ──────────────────────────────────────────────────────────────╮
│ UNLOCKED  mask 0xFF · 8C/16T · 0 offline                               │
│  ╭─────────┬─────────┬─────────┬─────────┬─────────┬─────────┬─────────┬─────────╮
│  │  core 0 │  core 1 │  core 2 │  core 3 │  core 4 │  core 5 │  core 6 │  core 7 │
│  │  fw  ██ │  fw  ██ │  fw  ██ │  fw  ██ │  fw  ██ │  fw  ██ │  fw  ██ │  fw  ██ │
│  │  os  ██ │  os  ██ │  os  ██ │  os  ██ │  os  ██ │  os  ██ │  os  ·· │  os  ██ │
│  ╰─────────┴─────────┴─────────┴─────────┴─────────┴─────────┴─────────┴─────────╯
│ keys: [space] toggle · [[]/[]] select · [o]/[O] all off/on · [u] unlock · [F] force-unlock · [i] unit · [v] verify │
╰─────────────────────────────────────────────────────────────────────────╯
```

Key bindings and rules:
- The OS-layer toggles are **draft-then-apply**: `[space]` (toggle core), `[o]`
  (all except 0 offline) and `[O]` (all online) only edit the draft — nothing
  touches hardware until `[a]` apply. `[esc]` discards the draft, `[r]` resets
  (onlines every CPU immediately — the recovery route). The title shows
  `· draft [a] apply [esc] cancel` and pending cells render yellow.
- `[←]` / `[→]` (and `[[]` / `[]]`) move the selected cell; `[u]` performs the
  firmware unlock (same gates as `cores apply`); the fw row re-renders from a
  live mask read and `PENDING-REBOOT` shows with a reminder after success.
- `[F]` is the **forced** unlock for abnormal masks: a modal warning popup
  shows the current mask, states that the 0x98 write is all-or-nothing (0xFF),
  and lists warm-reboot/cold-revert semantics; `[y]` writes, `[esc]` cancels.
  The popup owns every key until answered, and the TUI still never reboots.
- Offlined CPUs lose their `topology/` subtree on this kernel, so the map's
  grouping falls back core_id -> online SMT sibling -> adjacent-pair index, and
  `online` (all) writes every hotpluggable CPU dir with no topology dependency
  — offlined cores can always be recovered.
- `[o]`/`[O]` offline/online all cores (except 0) — the preset shapes (2C/4T
  etc.) are reached by offlining, the presets menu itself is deferred (`[p]`
  is the global patch popup).
- `[v]` runs the sweep in a worker (the keys line is replaced by the verdict
  while a report exists); `[i]` installs the boot unit ("unit not installed"
  badge clears).
- The fw row shows the SMU mask bits (green `██` = present), the os row shows
  live per-thread online state (`██` both, `█·` mixed, `··` none/masked).
- `ABNORMAL` renders red with `· writes refused` on every safe key; `[F]`
  remains the only writer.

---

## 5. Persistence (boot service)

Unit `aputune-cores.service` (written by `cores install`):

```ini
[Unit]
Description=BC-250 8-core unlock (SMU msg 0x98)
Documentation=file:///usr/share/doc/aputune/arieltune-core.md
After=multi-user.target
ConditionPathExists=/sys/bus/pci/devices/0000:00:00.0/config

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/usr/local/bin/arieltune apu cores boot

[Install]
WantedBy=multi-user.target
```

Rules that MUST survive any edit:
- The unit **never reboots** (bootloop lesson, section 1.3).
- `boot` is idempotent: `0xFF` → exit 0 silently; `0x77` → apply and exit 0;
  anything else → journal the refusal, exit 0 (not a service failure, no
  restart storm). **The unit has no force path**, so a board with an abnormal
  mask is only re-unlocked by an operator running `apply --force-abnormal`.
- Cold-boot UX: after a cold boot the mask is re-set within seconds; the cores
  appear on the next reboot. `status` makes this legible ("mask set, cores
  pending next reboot").

---

## 6. Kernel patch 28 — 8-core telemetry (port spec)

Source: GabriWar `0001-bc250-8core-telemetry.patch`. Ported onto the
`01–27`-patched tree; new file
`arieltune/crates/apu/patches/bc250-cachyos-7.0.9/28-bc250-8core-telemetry.patch`.

Content (both files below):
- `smu11_driver_if_cyan_skillfish.h`: `SmuMetricsTable_hybrid_t` (empirical
  116-byte layout: `CoreFrequency[8]` @0x00, `CorePower[7]` @0x10,
  `CoreTemperature[2]` @0x2C, L3 @0x30/0x34, `C0Residency[7]` @0x38,
  `GfxTemperature` @0x46 — the 0x44 gfxclk slot is absorbed) +
  `static_assert(sizeof == 116)`.
- `cyan_skillfish_ppt.c`:
  - `#include <linux/topology.h>` + `<linux/cpumask.h>`
  - module param `cs_eight_core_map` (bool, 0644) — force hybrid layout for
    debugging; auto-detect is the default.
  - `cyan_skillfish_core_count()` — count physical cores via topology sibling
    groups (credited FilippoR).
  - `cyan_skillfish_get_smu_metrics_data` `METRICS_CURR_GFXCLK` case → direct
    SMU query with metrics-table fallback. **Port divergence:** uses
    `SMU_MSG_QueryGfxclk` (0x0F), not GabriWar's `GetGfxclkFrequency`
    (msgid-0 map entry). Reason: `QueryGfxclk` is already mapped by our patch
    02 and battle-tested live on our blades by patch 05's paths; msgid 0 is not
    proven on our firmware. Both serialize under `msg_ctl.lock` identically.
    This also means the port **does not touch the message map**.
  - `cyan_skillfish_get_gpu_metrics`: gfxclk via the same SMU query (fallback
    to table), and when `cs_eight_core_map || core_count >= 8`, reinterpret the
    Current table as the hybrid layout for `current_coreclk[8]`,
    `average_core_power[1..7]`, `temperature_core[4..5]` (exactly GabriWar's
    field mapping); else the stock 6-wide loop.

Registration: `SERIES` entry `28`, tell = `Tell::ModParam("cs_eight_core_map")`,
touches `cyan_skillfish_ppt.c` + `smu11_driver_if_cyan_skillfish.h`. Safe to
always apply: auto-detect keeps 6-core blades on the stock layout. SERIES.md
gets rows for 28 and a note that GabriWar `0002` is superseded by applied
`14(e)` and deliberately not staged.

Verification before commit: fresh pristine `cachyos-7.0.9-1` + `patch -p1
--forward --fuzz=0` for 01–28 in order must apply cleanly, and the result must
byte-match the port source for the touched files.

---

## 7. Repo map — files to create/modify

| File | Change |
|---|---|
| `arieltune-core.md` | this document |
| `crates/ariel-smu/src/ocq3.rs` | add `q3::WRITE_SMN = 0x98`; public typed `unlock_cores()` + `unlock_cores_any()` on `OcQ3` (mask read/guard/write/verify); unit tests |
| `crates/apu/src/cores.rs` | new: state machine, status/apply/boot (safe + forced), presets, offlining, verify, acpi, unit install |
| `crates/apu/src/cli.rs` | `Cmd::Cores { action: CoreCmd }` + dispatch; `apply` gains `--force-abnormal` |
| `crates/apu/src/lib.rs` | `mod cores;` |
| `crates/apu/src/screen.rs` | Core Map panel + `[F]` force-confirm popup (section 4) |
| `crates/apu/src/cpu.rs` | read-only awareness: 8-core-safe core iteration (topology-derived, no hardcoded 6) |
| `crates/apu/src/telemetry.rs` | `gfxclk_mhz()`: prefer direct SMU query, fall back to `pp_dpm_sclk` with a <350 MHz sanity floor; use in TUI/status |
| `crates/apu/patches/bc250-cachyos-7.0.9/28-bc250-8core-telemetry.patch` | new (section 6) |
| `crates/apu/patches/bc250-cachyos-7.0.9/SERIES.md` | rows for 28 + 0002-redundancy note |
| `crates/apu/src/patches.rs` | SERIES entry 28 |
| `crates/apu/systemd/…` | reference copy of `aputune-cores.service` |
| tests | `apu` unit tests: state machine, unlock guard paths (safe + forced), offline parse; full `cargo test -p apu` green (37/37) |

Not implemented now (roadmap, section 10): arbitrary firmware masks, q2 `0x23`
exploit port, DXE/BIOS flashing tooling, full-image `bios` flash path.

---

## 8. Runtime verification checklist (blade)

On a P3 dev blade, after building and installing:
1. `aputune cores status` → `LOCKED`, mask 0x77, 12 threads.
2. `aputune cores verify` → clean sweep (or note defects).
3. `aputune cores apply` → PENDING-REBOOT; `sudo reboot` (warm).
4. After boot: `UNLOCKED`, 16 threads; `lscpu` 8C/16T.
5. TUI gfxclk matches `SMU_MSG_QueryGfxclk` reading; `gfx_temp_c` sane.
6. `patches` tell for 28 (ModParam `cs_eight_core_map`) present; series fully
   patched.
7. `cores acpi install` + reboot → all 16 threads have C-states.
8. Cold power-cycle → boots 6C, unit re-applies mask, status shows
   PENDING-REBOOT; next reboot → 16 threads.
9. Governor behaves normally (fence-driven) through all of this.

Status on blade 115 (2026-08-18): items 1–6 and 9 executed and passed (from an
abnormal 0xD7 start via the force hatch); item 7 (ACPI override) intentionally
not installed; item 8 confirmed in part — the mask survived even a
`reboot/mode=cold` reset on this board. See
`docs/ops/blade115-validation-log.md`.

---

## 9. Fleet rollout (Phase 2, unchanged until dev proves Phase 1)

1. Dev blade (P3): Phase 1 runtime unlock only. No flashing.
2. When proven: crossgrade dev blade to the stock custom P5 (UMA options) to
   match hive lineage; re-run the checklist.
3. One hive blade: backup current ROM, flash `BC250_P5clv_8core_v2.ROM`
   (sha256 `e7347f3a…`), enable `Advanced → Advanced CPU Settings → Unlock CPU
   Cores`, re-apply UMA settings (the ROM ships `/clrcfg` defaults), verify.
4. Roll across hive one blade at a time. CMOS clear is always the recovery
   path (ROM defaults cores OFF). SPI programmer on hand before the first
   boot-block write.

---

## 10. Experimental roadmap — arbitrary firmware core masks

**What it is:** writing any 8-bit mask (e.g. `0x03` = 2C, `0x7F` = skip core 7)
to SMN `0x5A870` via the q2 msg `0x23` SMU exploit (`bc250-smu-unlock`:
ring-subqueue overflow → fake transfer-table pointer → arbitrary SMU write +
code exec), then a warm reboot. Proven only as a PoC on **BIOS 3**;
`set_core_mask.py` demonstrates arbitrary values but no production deployment
exists anywhere in the community.

**What it would enable:**
- Firmware-level core counts: the SMU, AGESA, and ACPI enumerate fewer cores
  for real — smaller power budget, fewer C-state slots needed, and no
  offlining housekeeping. The right tool if you want a permanent 2C low-power
  blade or to permanently exclude a defective core.
- Permanent exclusion of defective harvested silicon (not all BC-250s are
  golden) without the unlock+offline dance.

**Why it's not shipped now:**
- The exploit is memory-corruption class (SMU code exec) with BIOS-3-only
  validated offsets; the hive is P5. Unexplored blast radius.
- **Telemetry cannot be trusted:** the firmware's 116-byte metrics layout has
  only been mapped for `0x77` (6-wide) and `0xFF` (hybrid). For any other mask
  the layout is unknown — the firmware could key off "mask == stock", off bit
  7, or off the enabled count; rw-r-r's `metrics-8core.s` shows literal 6-wide
  store loops in firmware code, so it is not clearly data-driven. Patch 28's
  auto-detect (`core_count >= 8`) would mis-decode e.g. a 7-core system.
- ACPI is static per boot; over-coverage with the 8-core tables *should* work
  for any mask ≤ 16 threads (unmatched processor objects are ignored), but
  this is inference, never verified on a non-standard mask.

**What must happen before telemetry can be trusted for a given mask M:**
1. Port the q2 `0x23` unlock exploit to P5 firmware offsets and re-validate it
   (BIOS-3-only today).
2. Re-run GabriWar's differential core-offline probe with mask M live: sweep
   cores offline one at a time and record which metric fields move, to map the
   firmware's per-mask layout empirically.
3. Extend patch 28's auto-detect from a boolean into a mask-keyed layout table
   (and decide the `GetGfxclkFrequency`-style fallback stays the gfxclk
   source, since direct SMU queries are layout-independent).
4. Verify ACPI over-coverage on mask M (C-states present for all enumerated
   threads).
5. Only then consider an experimental `cores mask <0xNN>` verb, hidden behind
   an explicit `--experimental` flag with the exploit's risk stated.

**Verdict:** not proven anywhere; parked as research. Everything the fleet
needs today is covered by the proven `0xFF` + OS-offline combination.
