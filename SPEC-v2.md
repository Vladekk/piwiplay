# piwiplay — Specification v2

Extends [`SPEC.md`](SPEC.md) (v1: native-DSD-only). v2 makes piwiplay a
**universal** player while preserving v1's bit-perfect DSD path, by adding:

- **All audio formats via ffmpeg** (FLAC, ALAC, WAV/AIFF, MP3, AAC/M4A, Opus,
  Vorbis, WavPack, APE, …) and DSD in any container ffmpeg demuxes.
- **PCM transcoding** — decode/resample anything to PCM for the sink.
- **DoP (DSD over PCM)** — play DSD on DACs that speak DoP but not native DSD.
- **Per-track output-mode marking** — the UI shows whether the *current* track
  is playing **NATIVE / DoP / PCM**, updated live.
- **DFF with DST decompression** (v1 plays raw DFF; DST needs a decoder).
- **Formalized TUI integration tests** (already prototyped in v1).

- **Status:** Partially implemented (see below).
- **Builds on:** the v1 engine/UI split, the `Command`/`Event` API, and the
  `OutputMode { Unknown, Native, Dop, Transcoded }` enum (already present in v1).

### Implementation status

**Done:**
- All-formats decoding via the **ffmpeg CLI** (subprocess → `f32le`), not libav
  linking — robust across ffmpeg versions (`crates/engine/src/pcm.rs`).
- PCM output path in the sink (F32LE) alongside native DSD; software volume.
- Per-track routing + the **`t` transcode toggle** (native DSD ⇄ PCM) with
  active volume on the PCM path; live `NATIVE`/`PCM` badge.
- **DST-compressed DFF now plays NATIVELY** (bit-perfect): the `piwiplay-dst`
  crate (a Rust port of ffmpeg's LGPL `dstdec.c`) losslessly decompresses DST
  frames to raw DSD, which flows through the native path. Validated **bit-exact**
  against the FFmpeg FATE conformance sample (decoding both our output and the
  original DST to PCM yields identical bytes). `piwiplay-dst` is LGPL-2.1,
  isolated so `engine`/`tui` stay MIT.

**Remaining (still design-only in this spec):**
- **DoP** packing (`dop_pack`) and a DoP DAC path — the `Dop` badge is defined
  but no code path emits it yet.
- **Honest native detection** (§7) — currently reports `NATIVE` whenever a DSD
  format negotiates; graph/link inspection is not yet implemented.
- DSD→PCM transcode currently relies on ffmpeg's decimation rather than a
  custom multi-stage low-pass.

---

## 1. Goals & non-goals

### Goals
1. Play essentially any consumer audio file, chosen automatically.
2. Never regress v1's bit-perfect native DSD: if a DSD file can go out natively,
   it still does, untouched.
3. Make the output path **visible and honest** — the user always knows if they
   are hearing native DSD, DoP, or decoded PCM.
4. Keep the DSD-only build available via a Cargo feature (no forced ffmpeg dep).

### Non-goals (still)
- Music library / tag DB, cover art, network/streaming, EQ/DSP chains.
- Sinks other than PipeWire.
- Editing tags or writing files.

---

## 2. Decoder backend: ffmpeg

### Dependency
Use **`ffmpeg-next`** (bindings to `libavformat`/`libavcodec`/`libavutil`/
`libswresample`). It covers demuxing, decoding, and resampling in one stack,
matching the user's "use ffmpeg" requirement. Gate it behind a default-on Cargo
feature so a minimal DSD-only build is still possible:

```toml
[features]
default = ["ffmpeg"]
ffmpeg = ["dep:ffmpeg-next"]   # all-formats + transcode + DoP-from-any-DSD
```

Build deps: `libavformat`, `libavcodec`, `libavutil`, `libswresample` dev
packages + `pkg-config`/`clang` (documented per-distro like v1's PipeWire deps).

### Probe
A new `format` module wraps ffmpeg:

```rust
pub struct Probe {
    pub is_dsd: bool,
    pub dsd: Option<DsdInfo>,     // present for .dsf/.dff (incl. DST)
    pub pcm: Option<PcmInfo>,     // sample_rate, channels, sample_format
    pub tags: Tags,               // richer than v1's hand-rolled ID3
    pub duration: Duration,
    pub codec: String,            // "flac", "dsd_lsbf", "dst", "aac", …
}
pub fn probe(path: &Path) -> Result<Probe>;
```

The v1 hand-written `.dsf`/`.dff` parsers are kept for the **native DSD path**
(they give us raw planar bits without ffmpeg touching them). ffmpeg is used for:
everything non-DSD, **and** DST-compressed DFF (ffmpeg's `dst` decoder →
raw DSD, which then re-enters the native/DoP path).

---

## 3. Output-path decision

Per track, the router picks a path from: the source kind, the target sink's
advertised formats, and config. The `OutputMode` reported to the UI mirrors it.

```
                       ┌───────────────────────────────────────────┐
 source is DSD ──────► │ sink advertises native DSD (SPEC.md §5.2)? │
                       └───────────────┬───────────────┬───────────┘
                                    yes│               │no
                                       ▼               ▼
                                  ┌─────────┐   ┌──────────────────────────┐
                                  │ NATIVE  │   │ dop_enabled && DAC≈DoP ?  │
                                  │ (v1)    │   └──────┬────────────┬───────┘
                                  └─────────┘      yes │            │ no
                                                       ▼            ▼
                                                   ┌──────┐   ┌───────────┐
                                                   │ DoP  │   │ DSD→PCM   │
                                                   └──────┘   │ (PCM)     │
                                                              └───────────┘

 source is PCM/lossy ─────────────────────────────────────► decode → PCM
                                                              (PCM)
```

- **Sink capability probing:** enumerate the target node's `EnumFormat` params
  and check for `mediaSubtype = dsd`. Cache per sink; re-probe on device change.
- **DoP selection:** DoP is invisible to PipeWire (it's just PCM to the graph),
  so the DAC — not the graph — detects it. Thus DoP can't be auto-detected
  reliably; it is chosen when `audio.dop = true` in config (or a per-DAC
  allowlist) and native DSD is unavailable. Default `false`.
- **Config knobs** (extend `[audio]`):
  ```toml
  [audio]
  dop = false                 # allow DoP when native DSD is unavailable
  transcode_rate = 0          # 0 = auto (176400 for DSD64/128, else 88200); or fixed Hz
  transcode_bits = 24         # PCM bit depth for the transcode/DoP path
  allow_pcm_volume = true     # v2 default flips to true (software volume in PCM/DoP off)
  ```

---

## 4. The three output paths

### 4.1 Native DSD (unchanged from v1)
Exactly SPEC.md §5. Bit-perfect. Volume is fixed (hardware/DAC). `OutputMode::Native`.

### 4.2 DoP — DSD over PCM
Packs the 1-bit DSD stream into 24-bit PCM so a DoP-aware DAC reconstructs DSD:

- Each DoP PCM sample carries **16 DSD bits** plus an **8-bit marker** in the top
  byte, alternating `0x05` / `0xFA` every sample. Layout (per channel, 24-bit):
  `[marker:8][dsd_hi:8][dsd_lo:8]` (MSB-first DSD in the low 16 bits).
- **PCM rate = DSD sample_rate / 16** (DSD64 → 176 400 Hz; DSD128 → 352 800).
- Sink format: `SPA_AUDIO_FORMAT_S24_32` (or `S24`) at that rate, 2ch.
- The feeder gains a `dop_pack(planar_dsd) -> pcm_s24_frames` step (pure,
  unit-testable) analogous to v1's `repack_planar`. Marker alternation is
  per-PCM-sample and must be continuous across buffers (feeder keeps the phase).
- **Volume must NOT be applied** (it would corrupt the DoP payload) — treated
  like native: fixed. `OutputMode::Dop`.

### 4.3 PCM — decoded / transcoded
- **Non-DSD sources:** ffmpeg decodes to interleaved PCM; `libswresample`
  converts to the sink's negotiated format/rate. Lossless (FLAC/ALAC/WAV) is
  bit-exact PCM; lossy is decoded. `OutputMode::Transcoded` (UI badge "PCM").
- **DSD sources when neither native nor DoP is available:** decimate the 1-bit
  stream to PCM. Pipeline: bits→±1 → multi-stage FIR/CIC low-pass → downsample to
  `transcode_rate` → `transcode_bits` PCM. Reuse the popcount trick only for the
  waveform, not for playback (playback needs a real low-pass to avoid noise).
- **Volume:** applied in the PCM domain when `allow_pcm_volume` (default true in
  v2). This is the one path where the on-screen volume actually attenuates.

The sink learns a second format family: v2 adds `connect_pcm(PcmInfo)` building a
raw-PCM `SPA_TYPE_OBJECT_Format` (via `libspa::AudioInfoRaw`, unlike the
hand-built DSD POD). The stream reconnects with DSD or PCM params per track.

---

## 5. Engine changes

Small, mostly additive — the v1 seam already anticipated this.

- **`decode`/`format`**: add `Source` abstraction:
  ```rust
  enum Source {
      Dsd(Box<dyn crate::decode::Decoder>),   // v1 planar reader
      Pcm(Box<dyn PcmDecoder>),                // ffmpeg-backed, yields f32/i32 frames
  }
  ```
- **Router** (`player`): on load, `probe()` → choose path → tell the sink which
  format to connect and the feeder which transform to run
  (`repack_planar` | `dop_pack` | `pcm_convert`).
- **Sink**: `SinkCmd::Play` carries an `OutputPath` and the appropriate info
  (DSD or PCM). `param_changed` reports the *actual* negotiated subtype, so
  `OutputMode` is derived from what really linked (native only if the negotiated
  format is DSD **and** the linked peer is the DAC, not a converter — see §7).
- **Volume**: engine applies gain in the feeder for the PCM path; no-ops for
  Native/DoP. `Event::Volume { hardware }` becomes `{ effective: bool }`.
- **Events**: `Event::Status.mode` already exists; now it carries real
  Native/DoP/Transcoded and flips mid-session if a device change forces a
  re-route.

No change to the `Command`/`Event` surface the UI depends on (backward
compatible), so a WebUI built against v1 keeps working.

---

## 6. UI changes

- **Mode badge** in the status bar (already rendered from `OutputMode`): color
  it green **NATIVE**, yellow **DoP**, blue **PCM**; add a tooltip line
  ("176.4 kHz/24-bit PCM", "DoP 176.4 kHz", "native DSD256"). The v1 code already
  color-codes these — v2 just feeds real values and codec text.
- **Per-track marking in the playlist**: show a small mode glyph on the currently
  playing row and, once probed, a static format tag per row (e.g. `flac 24/96`,
  `DSD128`). Probing is lazy/background (reuse the waveform worker thread pool).
- **Volume meter**: when volume is effective (PCM path), drop the v1
  "fixed / use DAC" annotation; keep it for Native/DoP.
- Level meters can become **true L/R** now (PCM frames give real per-channel
  peak/RMS), replacing v1's mono-derived approximation.

---

## 7. Honest "native" detection

v1 reports Native whenever the stream subtype is DSD, which is *usually* right
but not guaranteed (a graph could still insert a DSD→PCM converter). v2 tightens
this:

1. After negotiation, walk the graph link from our node to confirm the peer is
   the ALSA device node (not an `audioconvert`/`dsd` filter).
2. Cross-check the device's active `Format` is a DSD ALSA format (e.g.
   `DSD_U32_BE`) via `/proc/asound` or the node params.
3. If a converter is present, downgrade the badge to PCM even though we sent DSD,
   so the user is never misled.

This directly resolves the caveat recorded in `spike/RESULTS.md`.

---

## 8. Testing

### Unit (pure, no hardware)
- `dop_pack`: golden vectors — marker alternation `0x05/0xFA`, 16-bit payload
  placement, rate = DSD/16, phase continuity across buffer boundaries.
- `pcm_convert`: resample known tones; assert output rate/format and that a
  full-scale sine stays within range (no clipping/overflow).
- DSD→PCM low-pass: a modulated tone decimates to a sine of the expected
  frequency (FFT bin check) with noise floor below threshold.
- Router decision matrix: table-driven (source kind × sink caps × config) →
  expected `OutputPath`.
- DST DFF: a small DST fixture decodes (via ffmpeg) to the same PCM/DSD as its
  raw twin.

### TUI integration (already feasible — see v1 `crates/tui/tests/render.rs`)
The v1 work proved ratatui's `TestBackend` renders frames headlessly and lets us
assert on the cell buffer. v2 formalizes and expands:

1. **Golden-frame snapshots**: render representative states (playing native,
   DoP, PCM; empty playlist; tiny/huge terminals) to a buffer and compare against
   committed snapshots (via `insta` or a hand-rolled buffer dump). Catches
   layout/beauty regressions.
2. **Input→command simulation**: drive `App::on_key` with scripted `KeyEvent`s
   against a **fake engine** (a test double implementing the same command sink)
   and assert the emitted `Command` sequence — e.g. `space`→`TogglePlay`,
   `]`→`VolumeStep`, `/abc⏎`→find. Requires extracting a `CommandSink` trait so
   `App` can hold either the real `Engine` or a recording double.
3. **Resize sweeps**: render across a range of sizes (40×10 … 400×100) and assert
   invariants — never panics, body never scrolls horizontally, breakpoints switch
   at the documented sizes.
4. **Waveform/meter rendering**: feed known `WaveColumn` data and assert braille
   density increases with amplitude and the playhead color split lands at the
   right column.

### Engine integration (hardware-gated)
- A `#[ignore]`/`--features hardware` test that plays a synthetic file through
  the real sink and asserts `TrackEnded` with `mode == Native` on a DSD DAC
  (the v1 `play_once` example, promoted to a gated test). Not run in CI.

---

## 9. Milestones

| # | Milestone | Exit criteria |
|---|---|---|
| 1 | ffmpeg probe/decode | `probe()` + PCM decode for FLAC/WAV/MP3/AAC; tags; feature-gated build |
| 2 | PCM sink path | `connect_pcm` + swresample feeder; FLAC plays; software volume works; badge "PCM" |
| 3 | DoP | `dop_pack` + S24 path; DSD plays on a DoP DAC; badge "DoP"; golden tests |
| 4 | DSD→PCM transcode | low-pass decimation; DSD plays on any PCM sink; configurable rate/bits |
| 5 | DST DFF | ffmpeg `dst` → existing DSD paths |
| 6 | Honest native detection | graph/link + device-format check; badge downgrade when converted |
| 7 | UI + tests | per-row format tags, real L/R meters, color badges; snapshot + input-sim TUI tests |

---

## 10. Risks

1. **ffmpeg build/link friction** across distros and the immutable-OS toolbox
   flow; mitigate with the feature flag and clear per-distro docs. Consider
   `symphonia` (pure-Rust) as an optional alt backend for the common lossless
   codecs to allow an ffmpeg-free build.
2. **DoP correctness** (marker phase, endianness, 16-bit packing) is easy to get
   subtly wrong → audible noise; mitigate with golden vectors and a real DoP DAC.
3. **"Native" over-reporting** — §7's detection is the mitigation; until proven,
   prefer under-claiming (show PCM if unsure).
4. **Reconnect churn** switching DSD↔PCM formats per track; keep the v1
   disconnect/reconnect but debounce and pre-probe the next track.
5. **Volume policy confusion** — clearly gate software volume to the PCM path
   only, and keep Native/DoP fixed with a visible annotation.
