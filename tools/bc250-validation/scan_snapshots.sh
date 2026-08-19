#!/bin/bash
# Report, for each snapshot initramfs, whether the embedded amdgpu.ko has the TLB param.
shopt -s nullglob
cd /var/tmp || exit 1
rm -rf snapscan && mkdir snapscan && cd snapscan || exit 1

for img in /boot/initramfs-snap-*.img; do
    id=$(basename "$img" .img | sed 's/initramfs-//')
    vm="/boot/vmlinuz-$id"
    [ -f "$vm" ] && vmok="vmlinuz:OK" || vmok="vmlinuz:MISSING"
    sz=$(du -m "$img" | cut -f1)

    rm -rf x && mkdir x && cd x || continue
    zstdcat "$img" 2>/dev/null | cpio -idm --quiet 2>/dev/null
    ko=$(find . -name 'amdgpu.ko*' 2>/dev/null | head -1)
    if [ -n "$ko" ]; then
        src=$(modinfo "$ko" 2>/dev/null | awk '/^srcversion/{print $2}')
        if modinfo "$ko" 2>/dev/null | grep -q 'bc250_flush_by_runlist'; then
            tlb="TLB-PATCH:YES"
        else
            tlb="TLB-PATCH:no"
        fi
    else
        src="-"; tlb="NO-MODULE-IN-INITRAMFS"
    fi
    cd ..
    printf '%-18s %4sMB  %-14s  %-24s %s\n' "$id" "$sz" "$vmok" "$src" "$tlb"
done
