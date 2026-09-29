## prebuilt/

No prebuilt `.ko` files included. The kernel module is built from upstream
source via `build-and-install.sh build` — this ensures the `.ko` is compiled
against the exact running kernel on the target host.

The previous `nct6687-7.0.9-1-cachyos.ko` was removed because kernel module
ABI is tied to the exact kernel version string; a CachyOS-built `.ko` won't
load on Alpine, Debian, or any other distro with a different kernel flavor.

To get a working `.ko`:

```sh
# On an x86-64-v4 build host:
./build-and-install.sh build
# Output: /tmp/nct6687d/nct6687.ko

# Copy to board, then install:
./build-and-install.sh install /path/to/nct6687.ko
```
