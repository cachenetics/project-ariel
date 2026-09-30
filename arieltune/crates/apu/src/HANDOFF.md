Bug 1: enabled() is never true on OpenRC
It checks /etc/runlevel/arieltune-gpu
rc-update add <name> default creates /etc/runlevel/default/<name> - the runlevel dir is missing from the path
Consequence: any_legacy_gpu_unit_enabled() always returns false on Alpine - the double-writer guard silently passes, and that guard is the whole ONLY SMU clock writer invariant
Fix: check /etc/runlevel/default/{rc} - the same runlevel install_enable adds to

Bug 2: the generated init script is untrackable
start() { <ExecStart> & } - no pidfile, no stop()
Governor mode: bare & is invisible to rc-service status, and restart cannot kill the old daemon - two governors racing is exactly the wedge class this file exists to prevent
Manual/released: the one-shot exits, service reads stopped, TUI shows dead even though the pin applied
Fix: pidfile= + start-stop-daemon --background --pidfile in start() - OpenRC then gives you stop and status for free
