#!/bin/bash
# cu_map.sh — Read and display CU bitmap from DRM ioctl via libdrm

python3 << 'PYEOF'
import ctypes, struct, os

libdrm = ctypes.CDLL("libdrm_amdgpu.so.1")
fd = os.open("/dev/dri/renderD128", os.O_RDWR)
dev = ctypes.c_void_p()
maj, min_ = ctypes.c_uint32(), ctypes.c_uint32()
libdrm.amdgpu_device_initialize(fd, ctypes.byref(maj), ctypes.byref(min_), ctypes.byref(dev))

buf = (ctypes.c_uint8 * 1024)()
libdrm.amdgpu_query_info(dev, 0x16, 1024, ctypes.byref(buf))
raw = bytes(buf)

num_se = struct.unpack_from('<I', raw, 20)[0]
num_sh = struct.unpack_from('<I', raw, 24)[0]
cu_active = struct.unpack_from('<I', raw, 48)[0]

total = 0
print()
for se in range(num_se):
    for sh in range(num_sh):
        bm = struct.unpack_from('<I', raw, 56 + (se * 4 + sh) * 4)[0]
        n = bin(bm).count('1')
        total += n
        bar = ''.join('■' if bm & (1 << i) else '□' for i in range(10))
        print(f"  SE{se} SH{sh}: {bar} ({n}/10)")

possible = num_se * num_sh * 10
harvested = possible - total
print(f"\n  {total}/{possible} CUs active, {harvested} harvested")

libdrm.amdgpu_device_deinitialize(dev)
os.close(fd)
PYEOF
