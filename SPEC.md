# piwiplay — Specification

A console (TUI) audio player for Linux that plays **DSD** files natively through
**PipeWire**, with a colorful, resizable, unicode-rich interface.

- **Status:** Draft v1 (design specification, pre-implementation)
- **Stack:** Rust (edition 2021) · `ratatui` + `crossterm` (TUI) · `pipewire-rs` (audio)
- **Install target:** local user profile (XDG paths), `cargo install`

---

## 1. Overview & Goals

`piwiplay` is a keyboard-driven terminal audio player. Its first release does one
thing well: **bit-perfect native DSD playback to a PipeWire sink**, wrapped in a
clean, resizable TUI with a file/folder browser, simple playlists, transport
controls (play/pause/seek), volume, and a live waveform view.

### Design principles

1. **Bit-perfect first.** DSD is passed to the sink untouched; no resampling, no
   volume applied in the DSD path (see §5.4). Correctness of the audio path beats
   feature count.
2. **Basic on purpose.** No music library, no tag database, no scrobbling, no
   internet. Browse the filesystem, queue files, play them.
3. **Terminal-native beauty.** Colors and advanced unicode (block elements,
   eighth-blocks, braille) are used wherever they measurably improve readability
   of meters, the seek bar, and the waveform — never as decoration for its own sake.
4. **Adapts to the terminal.** The layout reflows for any reasonable window size
   and refuses to render garbage in unreasonable ones (§8.5).
5. **Extensible core, narrow v1.** Internal interfaces are designed so PCM/DoP and
   more formats can be added later, but v1 ships DSD-only (§14).

---

## 2. Scope

### In scope (v1)

- Native DSD playback (DSD64/128/256/512) to a PipeWire sink.
- Supported containers: **`.dsf`** (Sony) and **`.dff`/`.dsdiff`** (Philips).
- File/folder browser; play a single file or enqueue a whole folder.
- Playlist: build, reorder, save, load, clear; play/next/previous.
- Transport: play, pause, stop, seek (relative + absolute), next/previous track.
- Volume control (see §5.4 for the DSD-specific caveat).
- Live waveform + level meters rendered in unicode.
- Resizable, color, unicode TUI.
- Local install into the user profile; XDG-compliant config/state.

### Out of scope (v1) — see §14

- PCM playback of any kind (FLAC/WAV/MP3/…) and DoP transport.
- Sinks other than PipeWire (no ALSA/Pulse/JACK direct, no network sinks).
- Music library, tag editing, cover art, lyrics, gapless crossfade, EQ/DSP.
- Any network feature.

---

## 3. Target Platform & Dependencies

| Concern | Choice |
|---|---|
| OS | Linux with PipeWire ≥ 0.3.60 (DSD format support landed in this era) |
| Language | Rust, edition 2021, MSRV pinned in `Cargo.toml` |
| TUI | `ratatui` (rendering) + `crossterm` (backend, input, resize events) |
| Audio | `pipewire` + `libspa` crates (the `pipewire-rs` project) |
| DSD parsing | Hand-written parsers for DSF/DFF (small, no suitable crate) |
| Tags (optional) | `id3` for the ID3v2 chunk some `.dsf` files carry (best-effort) |
| Config | `serde` + `toml` |
| Logging | `tracing` + `tracing-appender` to a rotating log file |
| Errors | `thiserror` (library errors) + `anyhow` (top-level) |

**Runtime prerequisites** (documented for the user, not vendored):
- A running PipeWire session (`pipewire`, `pipewire-pulse` optional, `wireplumber`).
- `libpipewire-0.3` present at runtime (dynamically linked).
- A **DAC that accepts native DSD** for actual sound. Without native-DSD-capable
  hardware, v1 has no fallback (DoP is a future item, §14) and will surface a
  clear "sink does not accept DSD format" error (§13).

---

## 4. Architecture

Three long-lived threads communicate over bounded channels. The UI never blocks
on audio, and the real-time audio callback never allocates, locks, or does I/O.

```
        ┌──────────────────────────────────────────────────────────┐
        │                        main thread                        │
        │  TUI event loop (ratatui/crossterm)                       │
        │  - render @ ~30 fps or on event                           │
        │  - handle keys, resize                                    │
        │  - owns AppState (browser, playlist, view)                │
        └───────▲───────────────────────────────┬──────────────────┘
                │ PlayerEvent (status, pos,      │ PlayerCommand
                │ level meters, errors)          │ (load, play, pause,
                │                                 │  seek, volume, stop)
        ┌───────┴───────────────────────────────▼──────────────────┐
        │                    playback controller                    │
        │  - owns the pw_stream + pw_main_loop (its own thread)     │
        │  - decodes container -> raw DSD byte frames               │
        │  - feeds a lock-free ring buffer                          │
        │  - reports position/levels back to UI                     │
        └───────▲───────────────────────────────┬──────────────────┘
                │ pull DSD frames (SPSC ring)     │ format negotiation
        ┌───────┴───────────────────────────────▼──────────────────┐
        │            PipeWire RT data callback (pw-data-loop)       │
        │  - on_process: copy N DSD bytes ring -> pw_buffer          │
        │  - NO alloc / NO lock / NO syscalls beyond the copy       │
        └──────────────────────────────────────────────────────────┘

   Off the hot path:
        ┌──────────────────────────────────────────────────────────┐
        │  waveform worker (rayon task / dedicated thread)          │
        │  - streams the file, decimates 1-bit DSD -> envelope      │
        │  - emits a downsampled peak/RMS array for the UI          │
        └──────────────────────────────────────────────────────────┘
```

### Module layout (crate `piwiplay`)

```
src/
  main.rs            # arg parsing, logging init, wire threads together
  app/
    state.rs         # AppState, focus/view enum, selection
    events.rs        # PlayerCommand, PlayerEvent, InputEvent
    keymap.rs        # key -> action mapping (config-overridable)
  ui/
    mod.rs           # top-level draw(), layout computation, resize policy
    theme.rs         # color palette, NO_COLOR/truecolor detection
    widgets/
      browser.rs     # file/folder tree
      playlist.rs    # queue view
      transport.rs   # seek bar (eighth-block sub-cell fill)
      volume.rs      # volume meter
      levels.rs      # L/R peak/RMS meters
      waveform.rs    # braille/block waveform renderer
      statusbar.rs   # now-playing, format badge, key hints
  audio/
    mod.rs           # PlaybackController, thread lifecycle
    pipewire.rs      # stream setup, DSD format POD, on_process
    ring.rs          # lock-free SPSC byte ring
    dsd_format.rs    # DsdInfo (rate, channels, bit order, block layout)
  decode/
    mod.rs           # Container trait, Reader
    dsf.rs           # .dsf parser
    dff.rs           # .dff/.dsdiff parser
  waveform/
    mod.rs           # envelope extraction (decimating low-pass)
  playlist/
    mod.rs           # Playlist model, m3u-ish load/save
  config/
    mod.rs           # config.toml load, defaults, XDG paths
  fs/
    mod.rs           # directory scanning, extension filter, sorting
```

---

## 5. Audio Engine — PipeWire native DSD

### 5.1 Stream setup

Create a `pw_stream` in its own thread running a `pw_thread_loop` (so the RT data
callback lives on PipeWire's data loop, isolated from the UI). Register it as a
**playback** stream with media role `Music`:

- `media.class = "Stream/Output/Audio"`
- `media.category = "Playback"`, `media.role = "Music"`
- `node.name = "piwiplay"`, plus `media.name` set to the track title.

### 5.2 DSD format negotiation

DSD in PipeWire uses `SPA_MEDIA_TYPE_audio` / `SPA_MEDIA_SUBTYPE_dsd` and the
`spa_audio_info_dsd` structure — **not** `spa_audio_info_raw`. The format POD
advertises:

| Field | Meaning | v1 handling |
|---|---|---|
| `bitorder` | `SPA_PARAM_BITORDER_msb` / `_lsb` | Derived from container (DSF = LSB-first, DFF = MSB-first); converted to whatever the sink accepts, else advertised as-is |
| `channels` | channel count | From file (typically 2; multichannel supported if the file has it) |
| `rate` | DSD sample rate in **bytes/s of the 1-bit stream / 8**, i.e. the DSD frame rate (e.g. DSD64 → `2822400`) | From file |
| `interleave` | per-channel grouping (bytes) | Negotiated; DSF is planar-blocked, DFF is interleaved — the decoder normalizes to the negotiated `interleave` |

The stream advertises the file's DSD parameters and lets the sink/graph confirm.
If negotiation fails (no node accepts DSD), the controller reports a fatal,
user-visible error (§13). Because `pipewire-rs` exposes DSD only at the raw POD
level, `audio/pipewire.rs` builds the `SPA_TYPE_OBJECT_Format` POD by hand via
`libspa` pod builders. **This is the single highest-risk area — prototype it first
(§15, Milestone 0).**

### 5.3 Data flow & buffering

- Decoder produces DSD bytes into a **lock-free SPSC ring buffer** sized for
  ~300–500 ms of audio (DSD is dense: DSD64 stereo ≈ 705 KB/s, DSD256 ≈ 2.8 MB/s,
  so the ring is sized in bytes derived from the negotiated rate).
- `on_process` (RT callback): dequeue an output `pw_buffer`, copy up to its
  requested byte count from the ring, set `chunk` size/stride, requeue. It must
  never block; on ring underrun it emits silence-equivalent (`0x69`/`0x96`
  idle DSD pattern per bit order) and flags an xrun to the controller via an
  atomic counter (not a channel send).
- The controller thread keeps the ring fed and translates byte offset ⇄ time for
  position reporting.

### 5.4 Volume — the DSD caveat (important)

DSD is a 1-bit sigma-delta stream; **you cannot scale its amplitude without
decoding to PCM**, which would break bit-perfect playback. So v1 handles volume as:

1. **Hardware/graph volume (preferred):** request volume changes on the PipeWire
   node; if the route/DAC supports hardware volume, the bits stay perfect.
2. **If hardware volume is unavailable:** the volume control still shows and moves,
   but is annotated in the UI as *"fixed output / use DAC"* and does not silently
   convert to PCM. A config flag `allow_pcm_volume = false` (default) governs this.

The UI always renders a volume meter; the spec is explicit that in the default
bit-perfect mode the app does not apply software attenuation to DSD.

### 5.5 Seeking

Seeking maps a target time to a **byte offset**, aligned to the container's frame
boundary (DSF: channel-block boundary, typically 4096 bytes/channel; DFF: sample
frame). On seek the controller:
1. pauses feeding, 2. flushes the ring, 3. repositions the decoder to the aligned
offset, 4. resumes. Seek granularity equals one aligned frame (sub-millisecond at
DSD rates, so effectively continuous to the user).

---

## 6. File Formats & Decoding

A `Container` trait abstracts the two parsers:

```rust
trait Container {
    fn open(path: &Path) -> Result<Self>;
    fn info(&self) -> DsdInfo;          // rate, channels, bitorder, duration
    fn tags(&self) -> Metadata;         // best-effort: title/artist/album
    fn seek(&mut self, frame: u64) -> Result<()>;
    fn read_frames(&mut self, out: &mut [u8]) -> Result<usize>; // normalized layout
}
```

### 6.1 DSF (`.dsf`)
- Parse `DSD ` header chunk (total size, metadata pointer), `fmt ` chunk
  (version, format id, channel type/num, sampling frequency, bits/sample = 1,
  sample count, block size per channel = 4096), and `data` chunk.
- Samples are stored **LSB-first**, planar in per-channel 4096-byte blocks.
  The reader de-blocks/normalizes to the negotiated interleave + bit order.
- Optional trailing **ID3v2** metadata (via the `metadata` pointer) → title/artist/album.

### 6.2 DFF / DSDIFF (`.dff`)
- Parse the `FRM8`/`DSD ` form, `FVER`, `PROP` (with `FS ` sample rate, `CHNL`
  channels), and `DSD`/`DST` sound-data chunk. **DST (compressed DSD) is out of
  scope for v1** — detect it and report "DST compression unsupported" rather than
  playing noise.
- Samples are **MSB-first**, interleaved.

### 6.3 Duration
`duration = sample_count / dsd_rate` (sample_count is bits-per-channel). Displayed
`mm:ss` (or `h:mm:ss`).

---

## 7. Waveform Generation

DSD carries no PCM samples, so a displayable envelope is **derived**:

1. **Decimate + low-pass.** Convert bits to ±1, apply a simple moving-average /
   CIC-style decimation (factor chosen so DSD64 → ~a few hundred kHz intermediate),
   then reduce to one **peak** and one **RMS** value per output column.
2. **Resolution = terminal width.** The worker produces `~4× terminal width`
   buckets (so horizontal resizes don't force a re-scan) and the UI subsamples.
3. **Computed off the hot path** in the waveform worker as the file loads;
   progress is streamed so the waveform "fills in" left-to-right. Results are
   cached in memory for the current track only (no on-disk cache in v1).

### Rendering (see §8.4 for the glyphs)
- Default: **braille** (`U+2800`–`U+28FF`) — 2×4 dots per cell gives the highest
  vertical resolution and a smooth mirrored envelope around a center line.
- Fallback (fonts/terminals without good braille): **half/eighth block** elements.
- The played portion is colored differently from the un-played portion; the
  playhead column is highlighted.

---

## 8. TUI Design

Rendering via `ratatui`; input, resize, and raw mode via `crossterm`. Redraw on
input, on `Resize`, and on a ~30 fps tick while playing (meters/waveform/position).

### 8.1 Screen layout (normal size)

```
┌ piwiplay ───────────────────────────────────────── DSD256 · 11.29 MHz · 2ch ┐
│ ♪  Artist — Track Title                                          [Playing]   │  status bar
├──────────────────────────────┬───────────────────────────────────────────────┤
│ Browser / Playlist (focused) │  Waveform                                     │
│  ▸ Album A/                   │  ⣀⣤⣶⣿⣿⣷⣤⣀⣀⣤⣾⣿⣿⣶⣤⣀⡀ ⢀⣠⣴⣾⣿⣿⣷⣦⣄  │
│  ▾ Album B/                   │  ⠉⠛⠻⢿⣿⣿⠿⠛⠉⠉⠛⠿⣿⣿⡿⠟⠋ ⠈⠙⠛⠿⣿⣿⠿⠟⠋  │
│    ♫ 01 - Opening.dsf         │      └───────── playhead ──────────┘          │
│    ♫ 02 - Interlude.dff       ├───────────────────────────────────────────────┤
│      03 - Finale.dsf          │  L ▕████████▏▏     -6 dB                       │  level meters
│                               │  R ▕███████▏▏▏     -8 dB                       │
├──────────────────────────────┴───────────────────────────────────────────────┤
│ 01:47 ▕██████████████▊░░░░░░░░░░░░░░░░░░░░░░░░░░░░░▏ 04:32   Vol ▕███████▏ 72% │  transport
├──────────────────────────────────────────────────────────────────────────────┤
│ space play/pause  ← →seek  n/p track  a add  s save  / find  q quit           │  key hints
└──────────────────────────────────────────────────────────────────────────────┘
```

- **Status bar (top):** now-playing text + a **format badge** (`DSD64/128/256/512`,
  rate, channels) + play state.
- **Left pane:** tabbed between **Browser** and **Playlist** (Tab switches focus/view).
- **Right pane:** **waveform** (top) and **L/R level meters** (bottom).
- **Transport (bottom):** elapsed · seek bar · total, and a volume meter.
- **Key-hint bar:** context-sensitive.

### 8.2 Seek bar & volume — sub-cell precision
Fractional fill uses the **eighth-block** ramp so the bar moves smoothly with less
than one cell of change: `▏▎▍▌▋▊▉█` (`U+258F`…`U+2588`). Unfilled uses `░` or a
dim `▏` track. This gives ~8× the effective horizontal resolution of a plain
`#`/`=` bar.

### 8.3 Level meters
Horizontal per-channel peak (fast) + RMS (slow decay) using block fill with a
color gradient (green → yellow → red near clip). A peak-hold marker uses a
brighter cell that decays over ~1.5 s.

### 8.4 Unicode glyph inventory
| Purpose | Glyphs | Notes |
|---|---|---|
| Seek/volume fill | `▏▎▍▌▋▊▉█` (eighth blocks) | sub-cell fractional fill |
| Bar track (empty) | `░` `▏` | dim |
| Waveform (primary) | `⠀`–`⣿` braille (`U+2800`+) | 2×4 dots/cell, mirrored envelope |
| Waveform (fallback) | `▁▂▃▄▅▆▇█` + `▔▏` | half/eighth blocks |
| Tree markers | `▸ ▾ ♫ ♪ ▸` | expand/collapse, track, now-playing |
| Panels | `ratatui` rounded borders | `┌╮╰┘` style set |

### 8.5 Resize policy
- **Reflow:** panes use `ratatui` constraint layout (percentages + minimums); the
  waveform re-buckets to the new width from the cached 4× envelope.
- **Breakpoints:**
  - **≥ 100 cols × ≥ 30 rows:** full two-pane layout above.
  - **60–99 cols or 20–29 rows:** single-column layout — hide the right pane;
    waveform collapses to a one-line braille strip; meters collapse to a compact
    `L ▊ R ▋` form.
  - **< 60 cols or < 20 rows ("too tiny"):** stop rendering widgets; show a
    single centered message: *"Terminal too small — need ≥ 60×20"*. Playback
    continues unaffected.
  - **"Absurdly huge" (e.g. > 400 cols):** clamp content to a max readable width
    (e.g. 200 cols) and center it, rather than stretching meters across the screen.
- Resize is event-driven (`crossterm` `Event::Resize`); no polling.

### 8.6 Color & theme
- Truecolor when `COLORTERM=truecolor|24bit`, else 256-color, else 16-color, else
  monochrome. `NO_COLOR` env var is honored (disables color entirely).
- A default dark theme plus an optional light theme; palette overridable in
  `config.toml` (`[theme]` table with named roles: `accent`, `played`, `unplayed`,
  `meter_ok/warn/clip`, `border`, `text_dim`).

---

## 9. Playlists

- **Model:** an ordered list of absolute file paths + cached `DsdInfo`/tags.
- **Build:** add a file (`a`), add a folder recursively (adds all `.dsf/.dff` in
  sorted order), remove (`d`), reorder (move up/down), clear.
- **Persist:** save/load as extended **M3U-style** text (`#EXTINF` lines with
  duration + title; paths relative when under the same root, else absolute).
  Files live under the state dir (§11). Format chosen for human-readability and
  interop; no proprietary DB.
- **Playback order:** sequential; optional `repeat` (off/one/all) and `shuffle`
  toggles. (Shuffle/repeat are the only "smart" behaviors; still basic.)

---

## 10. Controls (default keymap)

| Key | Action | Key | Action |
|---|---|---|---|
| `space` | Play / pause | `↑`/`↓`, `k`/`j` | Move selection |
| `enter` | Play selected / enter folder | `←`/`→`, `h`/`l` | Seek −5 s / +5 s |
| `n` / `p` | Next / previous track | `Shift+←`/`→` | Seek −30 s / +30 s |
| `s` | Stop | `[` / `]` | Volume − / + |
| `a` | Add selected to playlist | `m` | Mute toggle |
| `d` | Remove from playlist | `r` | Cycle repeat mode |
| `x` | Save playlist | `z` | Toggle shuffle |
| `o` | Load playlist | `Tab` | Switch Browser ⇄ Playlist |
| `/` | Incremental find in current pane | `g`/`G` | Top / bottom |
| `?` | Help overlay | `q` / `Ctrl-C` | Quit |

All bindings are overridable via `[keymap]` in `config.toml` (§11). Actions are
named (e.g. `seek_forward_small`) so remapping is stable across releases.

---

## 11. Configuration & State (XDG)

| Path | Contents |
|---|---|
| `$XDG_CONFIG_HOME/piwiplay/config.toml` (`~/.config/piwiplay/`) | user config |
| `$XDG_STATE_HOME/piwiplay/` (`~/.local/state/piwiplay/`) | last session, log file |
| `$XDG_DATA_HOME/piwiplay/playlists/` (`~/.local/share/piwiplay/`) | saved playlists |
| `$XDG_CACHE_HOME/piwiplay/` (`~/.cache/piwiplay/`) | reserved (no waveform cache in v1) |

### `config.toml` (annotated defaults)

```toml
[audio]
allow_pcm_volume = false      # keep DSD bit-perfect; do not attenuate in software
target_sink      = ""         # "" = default PipeWire sink; or a node name
buffer_ms        = 400        # ring buffer target fill

[ui]
theme            = "dark"     # "dark" | "light"
fps              = 30         # render tick while playing
waveform         = "braille"  # "braille" | "blocks" | "off"
min_cols         = 60
min_rows         = 20
max_content_cols = 200

[theme]
accent   = "#8ec07c"
played   = "#83a598"
unplayed = "#504945"
# ... meter_ok / meter_warn / meter_clip / border / text_dim

[keymap]
# action = "key"  (overrides defaults)
# seek_forward_small = "Right"
```

Missing/invalid config falls back to defaults with a non-fatal warning in the log.

---

## 12. Installation (local user profile)

Native Rust toolchain, no root:

```bash
# from a checkout
cargo install --path .        # -> ~/.cargo/bin/piwiplay
# or from crates.io (once published)
cargo install piwiplay
```

- Binary lands in `~/.cargo/bin` (ensure it's on `PATH`); alternatively a
  `cargo install --root ~/.local` targets `~/.local/bin`.
- No system files are written. Config/state/data dirs (§11) are created lazily on
  first run.
- **Build-time deps:** a Rust toolchain (MSRV per `Cargo.toml`), `pkg-config`, and
  PipeWire dev headers (`libpipewire-0.3` / `libspa` `.pc` files) for the FFI
  build. Documented in the README with per-distro package names.
- **Runtime deps:** `libpipewire-0.3` shared library + a running PipeWire session.
- Optional: `cargo-dist` or a `Makefile` `install` target may be added later for a
  prebuilt-binary path; v1 relies on `cargo install`.

---

## 13. Error Handling & Edge Cases

| Situation | Behavior |
|---|---|
| No PipeWire session / can't connect | Fatal at startup: clear message + exit code 1; log has detail |
| Sink rejects DSD format (negotiation fails) | Non-fatal per-track: banner *"Sink does not accept native DSD (rate X). No DoP fallback in v1."*; skip to next or stop |
| DAC lacks hardware volume | Volume meter annotated *"fixed / use DAC"*; no software attenuation (§5.4) |
| DST-compressed DFF | Skip with message *"DST compression unsupported"* |
| Corrupt/short header | Skip track, log parse error, continue playlist |
| File removed/renamed mid-playlist | Mark row as missing (dim + `!`), skip on play |
| Ring underrun (xrun) | Emit idle DSD pattern, increment xrun counter, show subtle indicator; auto-grow buffer within a cap |
| Terminal too small | Overlay message; playback continues (§8.5) |
| Unsupported extension in folder add | Silently ignored (only `.dsf/.dff/.dsdiff` enqueued) |
| Terminal doesn't support unicode/color | Degrade: `blocks` waveform, ASCII bar chars, no color (`NO_COLOR`/detection) |

Panics are caught at the top level so the terminal is always restored (raw mode
off, alternate screen exited) before the process dies; a crash writes a trace to
the log file and prints its path.

---

## 14. Non-Goals & Future Work

**Explicit non-goals (v1):** music library/DB, tag editing, cover art, gapless
crossfade, EQ/DSP, network/streaming, sinks other than PipeWire.

**Designed-for-later (interfaces left open):**
1. **PCM playback** (FLAC/WAV/ALAC/MP3) — the `Container` trait and format
   negotiation already abstract sample format; add PCM `spa_audio_info_raw` paths.
2. **DoP (DSD over PCM)** fallback for DACs without native DSD.
3. **On-disk waveform cache** keyed by path + mtime.
4. **Gapless playback** across same-format DSD tracks.
5. **MPRIS** D-Bus control (play/pause/next from media keys).

---

## 15. Milestones

| # | Milestone | Exit criteria |
|---|---|---|
| **0** | **DSD spike** | A throwaway binary opens a PipeWire stream, negotiates a hand-built DSD format POD, and plays a raw `.dsf` bit-perfectly to a native-DSD DAC. **De-risks §5.2 before anything else.** |
| 1 | Decoders | `.dsf` + `.dff` parse to normalized frames; unit-tested against reference files; duration/tags correct |
| 2 | Playback core | Controller + ring + seek + position reporting; play/pause/stop/seek/next/prev via a CLI harness (no TUI) |
| 3 | TUI skeleton | Layout, browser, playlist, transport bar, resize breakpoints, theme; wired to the controller |
| 4 | Meters + waveform | Level meters and braille/block waveform worker; sub-cell seek/volume bars |
| 5 | Playlists + config | Save/load M3U, config.toml, keymap overrides, XDG dirs |
| 6 | Polish + install | Error UX, help overlay, `cargo install`, README with distro deps |

---

## 16. Open Technical Risks

1. **`pipewire-rs` DSD ergonomics.** DSD is exposed only at the raw POD level; the
   format-negotiation code is hand-built and version-sensitive. *Mitigation:*
   Milestone 0 spike; pin the `pipewire`/`libspa` crate versions.
2. **Hardware dependence.** Native DSD requires a capable DAC; testing needs real
   hardware (or a loopback/dummy sink that accepts the DSD format for CI-level
   negotiation tests). *Mitigation:* separate "negotiation succeeds" tests from
   "produces sound" tests.
3. **Bit order / interleave correctness.** DSF (LSB, planar-blocked) vs DFF (MSB,
   interleaved) vs what the sink wants — easy to get subtly wrong (plays as noise).
   *Mitigation:* golden-sample tests comparing normalized output byte-for-byte.
4. **Volume expectations.** Users expect a working volume slider; §5.4's
   bit-perfect stance must be surfaced clearly in the UI to avoid "volume doesn't
   work" confusion.
5. **Braille/font availability.** Not all terminal fonts render braille well.
   *Mitigation:* the `blocks` fallback and a config toggle.
