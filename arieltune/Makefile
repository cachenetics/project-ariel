# arieltune -- unified BC-250 tuning suite (WIKI | BIOS | APU | MEM)
BIN     := arieltune
PREFIX  ?= /usr/local
BINDIR  := $(DESTDIR)$(PREFIX)/bin
COMPAT  := aputune memtune biostune wikitune

.PHONY: build release install install-units install-driver uninstall test check fmt clean

build release:
	cargo build --release

# Install the one binary + the `at` alias + the four compat symlinks.
# Use `sudo make install` (writes to $(PREFIX)/bin).
install: release
	install -d $(BINDIR)
	install -m755 target/release/$(BIN) $(BINDIR)/$(BIN)
	ln -sf $(BIN) $(BINDIR)/at
	for n in $(COMPAT); do ln -sf $(BIN) $(BINDIR)/$$n; done
	@echo "installed $(BINDIR)/$(BIN) (+ at + compat: $(COMPAT))"

# Lay + enable the APU GPU power unit. The mode command writes the unit itself,
# so this just points you at it (the unit name is now arieltune-gpu.service).
install-units: install
	@echo "pick the APU power mode to lay + enable the unit:"
	@echo "  sudo $(BIN) apu gpu governor-on | autosleep-on | force <mhz>"
	@echo "service routing (optional): sudo $(BIN) apu cu route-save && systemctl enable arieltune-route"

# Build + install the BIOS smiflash DKMS driver (needs a board + kernel headers).
install-driver: install
	$(BIN) bios driver build

uninstall:
	$(BIN) uninstall || rm -f $(BINDIR)/$(BIN) $(BINDIR)/at $(foreach n,$(COMPAT),$(BINDIR)/$(n))

test:
	cargo test --workspace

# Full local gate: format, lint, test.
check: fmt
	cargo clippy --workspace --all-targets -- -D warnings
	cargo test --workspace

fmt:
	cargo fmt --check

clean:
	cargo clean
