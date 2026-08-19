#!/bin/bash
# Launch ComfyUI with the BC-250 ROCm environment. Run on the blade.
set -u

pkill -9 -f "main.py --listen" 2>/dev/null
sleep 3
rm -f /opt/ComfyUI/user/comfyui.db.lock

# shellcheck disable=SC1091
source /etc/profile.d/bc250-rocm.sh

export BC250_CONV_FIX=1
# warmup left off by default: it hangs if the HSA env is not applied
export BC250_WARMUP="${BC250_WARMUP:-0}"

echo "HSA_OVERRIDE_GFX_VERSION=${HSA_OVERRIDE_GFX_VERSION:-UNSET}"
echo "BC250_WARMUP=${BC250_WARMUP}"

cd /opt/ComfyUI || exit 1
# Bind loopback only — the API has no auth; remote access can be added with
# --listen 0.0.0.0 explicitly when needed.
nohup python3 main.py --listen 127.0.0.1 --port 8188 --fp16-vae \
    > /var/tmp/comfy.log 2>&1 &
echo "pid=$!"

for i in $(seq 1 60); do
    code=$(curl -s -o /dev/null -w "%{http_code}" --max-time 4 \
        http://127.0.0.1:8188/system_stats)
    if [ "$code" = "200" ]; then
        echo "READY after $((i * 3))s"
        exit 0
    fi
    sleep 3
done

echo "TIMEOUT - last http=$code"
tail -15 /var/tmp/comfy.log
exit 1
