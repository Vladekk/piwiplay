//! Decode any DSD file (DSF/DFF, incl. DST-compressed) via the engine's native
//! decoders and write the raw DSD out as an *uncompressed* DFF. Used to validate
//! the DST decoder: ffmpeg-decoding this output must match ffmpeg-decoding the
//! original DST file bit-for-bit.
//!
//! Usage: dsd_dump <in.dff|in.dsf> <out.dff>

use std::fs::File;
use std::io::{BufWriter, Write};

use piwiplay_engine::decode;

fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(12 + body.len() + 1);
    v.extend_from_slice(id);
    v.extend_from_slice(&(body.len() as u64).to_be_bytes());
    v.extend_from_slice(body);
    if body.len() % 2 == 1 {
        v.push(0);
    }
    v
}

fn main() {
    let inp = std::env::args().nth(1).expect("usage: dsd_dump <in> <out.dff>");
    let out = std::env::args().nth(2).expect("usage: dsd_dump <in> <out.dff>");

    let mut dec = decode::open(std::path::Path::new(&inp)).expect("open");
    let info = dec.info().clone();
    let ch = info.channels as usize;
    eprintln!(
        "decoded source: {ch}ch, rate={} ({:?}), {} bytes/chan",
        info.sample_rate,
        info.bit_order,
        info.total_bytes()
    );

    // Drain all planar DSD.
    let mut planes: Vec<Vec<u8>> = vec![Vec::new(); ch];
    let mut buf = Vec::new();
    loop {
        let n = dec.read_planar(65536, &mut buf).expect("read");
        if n == 0 {
            break;
        }
        for c in 0..ch {
            planes[c].extend_from_slice(&buf[c]);
        }
    }
    let per = planes[0].len();

    // Our DffDecoder always yields MSB-first planar bytes. Byte-interleave for
    // an uncompressed DFF (also MSB-first) so bit values are preserved exactly.
    let mut snd = Vec::with_capacity(per * ch);
    for b in 0..per {
        for c in 0..ch {
            snd.push(planes[c][b]);
        }
    }

    let mut prop = Vec::new();
    prop.extend_from_slice(b"SND ");
    prop.extend_from_slice(&chunk(b"FS  ", &info.sample_rate.to_be_bytes()));
    let mut chnl = Vec::new();
    chnl.extend_from_slice(&(ch as u16).to_be_bytes());
    for _ in 0..ch {
        chnl.extend_from_slice(b"SLFT");
    }
    prop.extend_from_slice(&chunk(b"CHNL", &chnl));
    prop.extend_from_slice(&chunk(b"CMPR", b"DSD \0\x0Cnot compressed"));

    let mut form_body = Vec::new();
    form_body.extend_from_slice(b"DSD ");
    form_body.extend_from_slice(&chunk(b"FVER", &[1, 5, 0, 0]));
    form_body.extend_from_slice(&chunk(b"PROP", &prop));
    form_body.extend_from_slice(&chunk(b"DSD ", &snd));

    let f = File::create(&out).expect("create");
    let mut w = BufWriter::new(f);
    w.write_all(b"FRM8").unwrap();
    w.write_all(&(form_body.len() as u64).to_be_bytes()).unwrap();
    w.write_all(&form_body).unwrap();
    w.flush().unwrap();
    eprintln!("wrote {out}: {} bytes DSD ({per} bytes/chan)", snd.len());
}
