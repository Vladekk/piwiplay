# piwiplay

A console (TUI) audio player for Linux that plays **DSD** files natively through
**PipeWire**, with a colorful, resizable, unicode-rich interface.

- Native DSD (DSD64/128/256/512) via `.dsf` and `.dff`/`.dsdiff`
- Bit-perfect passthrough to a DSD-capable PipeWire sink — no resampling
- File/folder browser, simple playlists (M3U), transport, seek, volume
- Braille waveform, eighth-block sub-cell seek/volume bars, level meters
- Resizable layout, truecolor/256/16-color with `NO_COLOR` support
- Installs into your user profile — no root, no system files

See [`SPEC.md`](SPEC.md) for the v1 design and [`SPEC-v2.md`](SPEC-v2.md) for the
planned all-formats/ffmpeg/DoP release. The `spike/` directory documents the
Milestone 0 proof that native DSD works through `pipewire-rs`
([`spike/RESULTS.md`](spike/RESULTS.md)).

## Requirements

**Runtime**
- A running PipeWire session (≥ 0.3.60; developed against 1.6).
- `libpipewire-0.3` shared library.
- A DAC that accepts **native DSD** for sound. v1 has no DoP fallback (that is
  v2); on a sink whose active profile exposes no DSD format, playback fails with
  a clear message rather than converting.

**Build**
- A Rust toolchain (see `rust-version` in `Cargo.toml`).
- `pkg-config`, `clang` (for bindgen), and PipeWire development headers.

Per-distro dev packages:

| Distro | Packages |
|---|---|
| Fedora | `pipewire-devel clang pkgconf-pkg-config` |
| Debian/Ubuntu | `libpipewire-0.3-dev clang pkg-config` |
| Arch | `pipewire clang pkgconf` |

On immutable/atomic distros (Silverblue, Kinoite, Fedora IoT) where the base
`/usr` is read-only, build inside a `toolbox`/`distrobox` that has
`pipewire-devel`; the resulting binary links the stable `libpipewire-0.3.so.0`
and runs against your host session.

## Install

```sh
# Easiest — puts `piwiplay` on your PATH (~/.cargo/bin), runnable from anywhere:
make install
#   ...or, on an immutable/atomic OS where the build needs a toolbox:
make install-toolbox            # BOX=<name> to pick the toolbox

# Equivalent raw commands:
cargo install --path crates/tui          # -> ~/.cargo/bin/piwiplay
cargo install --path crates/tui --root ~/.local   # -> ~/.local/bin instead
```

Then just `piwiplay ~/Music` from any directory. `make run ARGS=~/Music` runs
it without installing.

Make sure the target bin dir is on your `PATH`. No system files are written;
config/state/playlists are created lazily on first run under XDG paths:

| Path | Contents |
|---|---|
| `~/.config/piwiplay/config.toml` | configuration |
| `~/.local/state/piwiplay/piwiplay.log` | log file |
| `~/.local/share/piwiplay/playlists/` | saved playlists |

### Homebrew (third-party tap)

A community tap is provided (not an official Homebrew formula):

```sh
brew tap vladekk/piwiplay
brew install piwiplay
```

The tap lives in a separate repository; see `packaging/homebrew/README.md`.

## Usage

```sh
piwiplay                 # start in the current directory
piwiplay ~/Music/DSD     # start with a folder queued
piwiplay track.dsf       # queue and play a file
```

### Keys

| Key | Action | Key | Action |
|---|---|---|---|
| `space` | play / pause | `Tab` | switch Browser ⇄ Playlist |
| `⏎` | open dir / play file / load .m3u | `↑↓` `k`/`j` | move selection |
| `S` | stop | `g` / `G` | top / bottom |
| `n` / `p` | next / previous | `←→` `h`/`l` | seek ∓5s |
| `a` | add selection to playlist | `Shift+←→` | seek ∓30s |
| `d` | remove from playlist | `[` `]` `-` `+` | volume |
| `x` | save playlist | `m` | mute |
| `/` | find in current pane | `r` | repeat cycle |
| `?` | help | `z` | shuffle |
| `q` / `Esc` / `Ctrl-C` | quit | | |

### A note on volume

DSD is a 1-bit stream; attenuating it in software would mean decoding to PCM and
would break bit-perfect playback. In v1 piwiplay therefore does **not** apply
software volume to DSD — the on-screen volume reflects intent but the bits stay
untouched (use your DAC's volume). This is deliberate (SPEC §5.4); a
configurable PCM-volume path arrives in v2.

## Architecture

Two crates with a strict split:

- **`piwiplay-engine`** — headless: DSD decoding, the native PipeWire sink,
  playlist, config, and waveform extraction, all behind a `Command`/`Event`
  API. No UI dependencies.
- **`piwiplay`** (TUI) — a ratatui frontend that sends `Command`s and renders
  `Event`s.

The engine seam is intentionally frontend-agnostic: the same Command/Event API
is what a future WebUI/Electron frontend would drive over a socket.

## Development

```sh
cargo test                              # 43 tests (decoders, layout repack,
                                        # playlist, waveform, config, TUI render)
cargo run -p piwiplay-engine --example play_once -- track.dsf   # headless smoke test
```

## License

MIT.
