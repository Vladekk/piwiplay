# piwiplay

A console (TUI) audio player for Linux that plays **DSD** files natively through
**PipeWire**, with a colorful, resizable, unicode-rich interface.

- Native DSD (DSD64/128/256/512) via `.dsf` and `.dff`/`.dsdiff`, **including
  DST-compressed DFF** (losslessly decompressed to native DSD, bit-exact)
- Bit-perfect passthrough to a DSD-capable PipeWire sink — no resampling
- **All other formats via ffmpeg** (FLAC, ALAC, WAV, MP3, AAC, Opus, …),
  decoded to PCM — plus a per-track toggle to transcode DSD to PCM so software
  volume applies (press `t`)
- Live **output-mode badge**: `NATIVE` (bit-perfect DSD) / `PCM` (decoded)
- File/folder browser with **multi-select**, mouse support, simple playlists
  (M3U), a saved-playlists pane, transport, seek, volume
- Braille waveform, eighth-block sub-cell seek/volume bars, level meters
- Resizable layout; **terminal-themeable palette** (your terminal's colors, e.g.
  Konsole "Vapor") or custom hex; `NO_COLOR` support
- Installs into your user profile — no root, no system files

See [`SPEC.md`](SPEC.md) for the v1 design and [`SPEC-v2.md`](SPEC-v2.md) for the
planned all-formats/ffmpeg/DoP release. The `spike/` directory documents the
Milestone 0 proof that native DSD works through `pipewire-rs`
([`spike/RESULTS.md`](spike/RESULTS.md)).

## Requirements

**Runtime**
- A running PipeWire session (≥ 0.3.60; developed against 1.6).
- `libpipewire-0.3` shared library.
- A DAC that accepts **native DSD** for bit-perfect DSD playback. If the sink's
  active profile exposes no DSD format, native playback errors — press `t` to
  transcode that track to PCM instead.
- **`ffmpeg`** (and `ffprobe`) on `PATH` for non-DSD formats and for the DSD→PCM
  transcode toggle. DSD-only playback works without it.

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
| `space` | play / pause | `Tab` | cycle Browser → Playlist → Saved |
| `⏎` | open / play / add marked / load .m3u | `Shift+Tab` | cycle panes backward |
| `S` | stop | `↑↓` `k`/`j` | move selection |
| `n` / `p` | next / previous | `Shift+↑↓` | multi-select (mark a range) |
| `t` | toggle native DSD ⇄ transcode (PCM) | `PgUp` / `PgDn` | first / last item |
| `a` | add selection to playlist | `←→` `h`/`l` | seek ∓5s |
| `d` | remove from playlist | `Shift+←→` | seek ∓30s |
| `x` | save playlist (default name) | `[` `]` `-` `+` | volume |
| `X` | save playlist as… (prompt) | `m` | mute |
| `L` | jump to system music library | `r` / `z` | repeat / shuffle |
| `/` | find in current pane | `?` | help |
| `q` / `Esc` / `Ctrl-C` | quit | mouse | click=select, dbl-click=play, wheel=scroll, click seek bar |

### Output modes

The status bar shows how the current track reaches the DAC:

- **`NATIVE`** — bit-perfect 1-bit DSD passthrough. Volume is **fixed** (the
  bar shows `·fix`); use your DAC. Attenuating DSD in software would require
  decoding to PCM and break bit-perfect playback (SPEC §5.4).
- **`PCM`** — decoded to PCM via ffmpeg (any non-DSD file, or a DSD track after
  pressing `t`). Here the on-screen **volume is active**.

Press `t` to switch a DSD track between native and PCM when you want a working
software volume.

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
