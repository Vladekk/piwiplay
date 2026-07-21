# AGENTS.md

Guidance for AI coding agents working in this repo. Keep it short; the details
live in [README.md](README.md).

## Start here
- **[README.md](README.md)** — what piwiplay is, install, keys, output modes,
  architecture. Read it first.
- **[SPEC.md](SPEC.md)** — v1 design (native DSD over PipeWire).
- **[SPEC-v2.md](SPEC-v2.md)** — v2 design + implementation status (ffmpeg/PCM,
  transcode, DoP). Check its status section before assuming a feature exists.

## Layout
- `crates/engine` (`piwiplay-engine`) — headless: decode, PipeWire sink, ffmpeg
  PCM path, playlist, waveform. Driven only by the `Command`/`Event` API in
  `player.rs`. **No UI dependencies.**
- `crates/tui` (`piwiplay`) — ratatui frontend (lib + thin `main.rs`).
- `crates/dst` (`piwiplay-dst`) — DST→DSD decoder, **LGPL-2.1** (port of ffmpeg's
  `dstdec.c`), kept isolated so `engine`/`tui` remain MIT.
- `spike/` — the Milestone-0 DSD proof; see `spike/RESULTS.md`.

## Build & test
- `cargo test` — run the suite (engine + TUI, incl. `TestBackend` render tests).
- `cargo build` / `make install` — see README for the toolbox note on
  immutable-OS hosts (build needs `pipewire` dev headers; runtime needs
  `ffmpeg` for non-DSD/transcode).

## Conventions
- Keep the engine UI-agnostic: features flow through `Command`/`Event`, never
  reach into ratatui from the engine.
- Add tests with changes (unit for pure logic; `crates/tui/tests/render.rs` for
  UI). Keep `cargo build` warning-free.
- When adding a keybinding, update the in-app help (`ui.rs`) **and** README.
