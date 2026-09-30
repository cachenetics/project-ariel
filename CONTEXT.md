Bug A: the oneshot command lands OUTSIDE start()
The is_oneshot arm pushes just \t{exec_start}\n at TOP LEVEL - openrc-run SOURCES these scripts for every verb (status, stop, shutdown)
So the SMU write fires whenever OpenRC merely asks a question, and there is no start() for the oneshot units at all
Wrap it: start() { <exec>; }

Bug B: is_oneshot never matches the GPU unit
GPU_UNIT is Type=simple + RemainAfterExit=yes - not Type=oneshot
Only the cpu-oc and route units are oneshot, and they just got Bug A
The manual/released-exits-0 behavior you are solving for is RUNTIME state (what apply-boot reads from power.json), not unit type
Suggested shape, mirrors systemd exactly:
start() {
    if grep -q "force_mhz" /var/lib/aputune/power.json; then
        /usr/local/bin/arieltune apu gpu apply-boot   # sync, rc tells the story
    else
        start-stop-daemon --start --pidfile "$pidfile" --background --exec ... 
    fi
}

Also worth a unit test: assert every generated script contains start() { - that alone would have caught Bug A for all three unit kinds.
