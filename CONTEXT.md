The newline split survives
persist.rs:238 and :243 still embed \n\t mid-command, so the GENERATED script reads:
start() {
    start-stop-daemon --start --pidfile "$pidfile"
    --background --exec /usr/local/bin/arieltune -- apu gpu apply-boot
}

Shell runs those as TWO commands: line 1 is start-stop-daemon with no --exec (usage error), line 2 tries to run --background as a program
start() fails on every OpenRC card the moment this merges.
Fix: one line, or end line 1 with a backslash.

Two smaller ones while you are in there
.unwrap_or_default() gives an EMPTY exec_start when ExecStart is missing -> --exec  with nothing; the old .unwrap_or("exit 0") was safer, keep trim too
the one-shot modes question is still open: manual/released apply-boot exits 0 immediately, so --background + pidfile reads died-immediately and the TUI shows dead after a successful pin

One more push and I think you are done with persist.rs.
