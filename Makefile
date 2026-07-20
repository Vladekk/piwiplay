# Convenience shortcuts for piwiplay.
#
#   make install          build (release) and put `piwiplay` on your PATH (~/.cargo/bin)
#   make install-toolbox  same, but run the build inside the `wiremix-build`
#                         toolbox (for immutable/atomic OSes without pipewire-devel
#                         on the host); override the box with BOX=<name>
#   make run ARGS=...     run without installing (e.g. make run ARGS=~/Music)
#   make test             run the test suite
#   make uninstall        remove the installed binary

BOX ?= wiremix-build
ROOT := $(shell pwd)

.PHONY: install install-toolbox run test uninstall

install:
	cargo install --path crates/tui --force

install-toolbox:
	podman exec $(BOX) bash -lc 'cd $(ROOT) && cargo install --path crates/tui --force'

run:
	cargo run -p piwiplay -- $(ARGS)

test:
	cargo test

uninstall:
	cargo uninstall piwiplay
