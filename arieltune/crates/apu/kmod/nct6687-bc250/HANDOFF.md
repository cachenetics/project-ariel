# nct6687-bc250 → Alpine Linux HANDOFF

## Status

Issues 1-3 are **resolved**. Issue 4 (README.md Alpine notes) remains.

---

## Resolved issues

### Issue 1 — Prebuilt `.ko` is CachyOS-versioned ✅ DONE

**What changed:** Build output now goes to a fixed path `build-output/nct6687.ko`.
The `install` command no longer takes a `.ko` path argument — it reads from
`build-output/` automatically. The old versioned `.ko` in `prebuilt/` was
removed. This implements Option B from the original handoff: version-agnostic
naming via a consistent output directory.

**Files changed:** `build-and-install.sh` (BUILD_OUT, do_build, do_install,
install case), `prebuilt/` (`.ko` removed).

### Issue 2 — Alpine `linux-headers` package name ✅ DONE

**What changed:** Alpine branch of `install_headers()` now detects kernel flavor
from `uname -r` (`*-lts*` → `linux-lts-headers`, `*-zen*` → `linux-zen-headers`,
else `linux-headers`) and installs the correct headers package.

### Issue 3 — `apk add ... bash` unnecessary ✅ DONE

**What changed:** Removed `bash` from Alpine apk list. Alpine's `ash` is
POSIX-compliant and sufficient for this script.

---

## Remaining work

### Issue 4 — README.md Alpine build notes

The README.md needs an Alpine-specific build section describing:
- How to build on an Alpine v4 host against the board's kernel build tree
- The new `build-output/` workflow (build → copy → install, no `.ko` argument)
- Alpine-specific commands (`apk add`, `doas` instead of `sudo`)

---

## Working as-is

- ✅ `#!/usr/bin/env sh` — ash is POSIX-compliant
- ✅ `doas` detection — falls back correctly when `sudo` isn't present
- ✅ `apk` detection — found automatically
- ✅ `tee`, `grep -q`, `rm -f`, `lsmod` — all busybox-compatible
- ✅ Modprobe paths (`/etc/modprobe.d/`, `/etc/modules-load.d/`) — valid on Alpine
- ✅ Cross-compile guard (v3 board → v4 host) — distro-agnostic
