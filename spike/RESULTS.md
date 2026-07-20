# Spike Results — Native DSD via pipewire-rs (SPEC.md Milestone 0)

**Verdict: SUCCESS. Stay in Rust.** `pipewire-rs` (crates `pipewire` / `libspa`
0.9.2) can express, negotiate, and stream native DSD. No C fallback needed.

## What was proven

1. **Builds:** `pipewire-sys`/`libspa-sys` compile against `libpipewire-0.3` +
   `libspa-0.2` dev headers (bindgen). PipeWire 1.6.x.
2. **Negotiates DSD:** a hand-built `SPA_TYPE_OBJECT_Format` POD with
   `mediaSubtype = dsd` negotiates with the graph and reaches `Streaming`.
3. **Native passthrough works:** on the test rig (Topping E30 II) the sink chose
   `interleave=4, bitorder=msb` — the SPA representation of ALSA `DSD_U32_BE`
   (`/proc/asound/card2/stream0` → `Format: DSD_U32_BE, DSD raw: DOP=0`), i.e.
   bit-perfect native DSD, not DoP and not PCM conversion.
4. **Round-trip:** a synthetic sigma-delta `.dsf` (500 Hz tone) played fully and
   exited cleanly (exit 0).

## Key technical findings (feed these into the real implementation)

### DSD format POD — build it like `pw-cat`
`spa_format_audio_dsd_build` **omits** `interleave` and `bitorder` when they are
`0`/`unknown`. So the client should advertise only:
`mediaType=audio`, `mediaSubtype=dsd`, `rate` (Int), `channels` (Int),
`position` (Id array). The **sink chooses** interleave + bitorder; the client
reads them back in `param_changed` and repacks the source data to match.

```rust
// EnumFormat offered by the client (no interleave/bitorder):
Object { type_: SPA_TYPE_OBJECT_Format, id: SPA_PARAM_EnumFormat, properties: [
    mediaType=audio, mediaSubtype=dsd,
    AUDIO_rate=Int(dsd_rate/8), AUDIO_channels=Int(n),
    AUDIO_position=ValueArray::Id([FL, FR]) ] }
```

### Reading the negotiated layout — use `PodObject`, not `Value` deserialize
`PodDeserializer::deserialize_from::<Value>` returns `InvalidType` on a Format
object. Instead:
```rust
let obj = param.as_object()?;
let interleave = obj.find_prop(Id(SPA_FORMAT_AUDIO_interleave))?.value().get_int()?;
let lsb = obj.find_prop(Id(SPA_FORMAT_AUDIO_bitorder))
             .and_then(|p| p.value().get_id().ok())
             .map(|Id(b)| b != SPA_PARAM_BITORDER_msb).unwrap_or(true);
```

### SPA DSD conventions (from `spa/param/audio/dsd.h`)
- `rate` is in **bytes/sec** = DSD sample rate / 8 (DSD64 → 352800).
- `interleave` = bytes per channel per group; `0` = planar; **negative** = bytes
  reversed within the group.
- `bitorder` = `msb`/`lsb`. **DSF is LSB-first; DFF is MSB-first.** If the sink
  picks the opposite, reverse the bits in each byte (`u8::reverse_bits`).
- DSF stores per-channel 4096-byte planar blocks: `[ch0][ch1][ch0][ch1]…`.

### Stream properties that matter
`media.class = "Stream/Output/Audio"` plus MEDIA_TYPE/ROLE/CATEGORY. Connect with
`AUTOCONNECT | MAP_BUFFERS | RT_PROCESS`.

### Environment / build notes
- Build needs `pipewire-devel` + `clang` (bindgen). On this immutable-OS host the
  headers live only inside a Fedora **toolbox** (`podman exec <box> cargo build`).
- The built binary links the stable `libpipewire-0.3.so.0` soname and **runs on
  the host** against the real daemon + DAC. The PipeWire socket is per-user
  (`/run/user/1000/pipewire-0`); a root container has none — run playback on the
  host user, not as container-root.

## Caveat carried into the spec
Native passthrough requires the DAC's active profile to expose a DSD ALSA format.
Where it does not, PipeWire silently converts DSD→PCM. The player must detect the
negotiated sink format and surface whether playback is truly native (see v2 spec:
"mark native / DoP / transcoded").
