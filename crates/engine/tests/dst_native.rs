//! Native DST decode validation against FFmpeg's FATE conformance sample.
//!
//! Ignored by default (needs network to fetch the sample). Run with:
//!   cargo test -p piwiplay-engine --test dst_native -- --ignored
//!
//! The full bit-exactness check (our DSD == ffmpeg's DSD) is done by decoding
//! the same file both ways to PCM; see `examples/dsd_dump.rs` and the README.
//! This test asserts the native decode path opens a DST `.dff` and yields the
//! expected amount of DSD without error.

use std::process::Command;

const URL: &str = "http://fate-suite.ffmpeg.org/dst/dst-64fs44-2ch.dff";

#[test]
#[ignore = "needs network to fetch the FATE DST sample"]
fn fate_dst_decodes_natively() {
    let dir = tempfile::tempdir().unwrap();
    let dff = dir.path().join("dst.dff");
    let fetched = Command::new("curl")
        .args(["-sL", URL, "-o"])
        .arg(&dff)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !fetched || !dff.exists() || std::fs::metadata(&dff).map(|m| m.len()).unwrap_or(0) == 0 {
        eprintln!("skipping: could not fetch {URL}");
        return;
    }

    // decode::open must succeed (DST is decoded natively, not rejected).
    let mut dec = piwiplay_engine::decode::open(&dff).expect("open DST dff");
    assert_eq!(dec.info().channels, 2);
    assert_eq!(dec.info().sample_rate, 2_822_400, "DSD64 bit rate");
    // 10 frames × (37632/8) = 47040 bytes/channel.
    assert_eq!(dec.total_bytes(), 47_040);

    let mut out = Vec::new();
    let mut total = 0u64;
    loop {
        let n = dec.read_planar(4096, &mut out).expect("decode DST frame");
        if n == 0 {
            break;
        }
        assert_eq!(out.len(), 2);
        total += n as u64;
    }
    assert_eq!(total, 47_040, "decoded per-channel byte count");
}
