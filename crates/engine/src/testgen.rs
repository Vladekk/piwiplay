//! Test-only synthesizers for valid DSF/DFF byte streams with deterministic
//! payloads, so decoder round-trips can be asserted byte-for-byte.

#![cfg(test)]

/// Deterministic per-channel plane byte: distinct per channel and position.
pub fn plane_byte(ch: usize, i: usize) -> u8 {
    (i as u8).wrapping_mul(3).wrapping_add((ch as u8).wrapping_mul(0x11)).wrapping_add(1)
}

/// Build a DSF file. `per_chan` bytes are packed into `block_size`-sized planar
/// blocks (last block zero-padded). Bytes come from [`plane_byte`].
pub fn dsf_bytes(channels: usize, dsd_rate: u32, per_chan: usize, block_size: usize) -> Vec<u8> {
    let n_blocks = per_chan.div_ceil(block_size).max(1);
    let padded = n_blocks * block_size;

    let mut data = Vec::new();
    for b in 0..n_blocks {
        for c in 0..channels {
            for k in 0..block_size {
                let idx = b * block_size + k;
                data.push(if idx < per_chan { plane_byte(c, idx) } else { 0 });
            }
        }
    }

    let data_chunk_size = 12u64 + data.len() as u64;
    let total = 28u64 + 52 + data_chunk_size;
    let samples_per_chan = (per_chan as u64) * 8;

    let mut out = Vec::new();
    out.extend_from_slice(b"DSD ");
    out.extend_from_slice(&28u64.to_le_bytes());
    out.extend_from_slice(&total.to_le_bytes());
    out.extend_from_slice(&0u64.to_le_bytes()); // no metadata

    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&52u64.to_le_bytes());
    out.extend_from_slice(&1u32.to_le_bytes()); // version
    out.extend_from_slice(&0u32.to_le_bytes()); // format id: DSD raw
    out.extend_from_slice(&(if channels == 1 { 1u32 } else { 2 }).to_le_bytes()); // channel type
    out.extend_from_slice(&(channels as u32).to_le_bytes());
    out.extend_from_slice(&dsd_rate.to_le_bytes());
    out.extend_from_slice(&1u32.to_le_bytes()); // bits
    out.extend_from_slice(&samples_per_chan.to_le_bytes());
    out.extend_from_slice(&(block_size as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // reserved

    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_chunk_size.to_le_bytes());
    out.extend_from_slice(&data);
    out
}

/// Build a DFF file with an uncompressed, byte-interleaved DSD chunk.
pub fn dff_bytes(channels: usize, sample_rate: u32, per_chan: usize) -> Vec<u8> {
    // interleaved sound data
    let mut snd = Vec::with_capacity(per_chan * channels);
    for i in 0..per_chan {
        for c in 0..channels {
            snd.push(plane_byte(c, i));
        }
    }

    fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(id);
        v.extend_from_slice(&(body.len() as u64).to_be_bytes());
        v.extend_from_slice(body);
        if body.len() % 2 == 1 {
            v.push(0); // even padding
        }
        v
    }

    // PROP/SND body: FS, CHNL, CMPR
    let mut prop = Vec::new();
    prop.extend_from_slice(b"SND ");
    prop.extend_from_slice(&chunk(b"FS  ", &sample_rate.to_be_bytes()));
    let mut chnl = Vec::new();
    chnl.extend_from_slice(&(channels as u16).to_be_bytes());
    for _ in 0..channels {
        chnl.extend_from_slice(b"SLFT"); // placeholder channel ids
    }
    prop.extend_from_slice(&chunk(b"CHNL", &chnl));
    prop.extend_from_slice(&chunk(b"CMPR", b"DSD \0\0uncompressed"));

    let fver = chunk(b"FVER", &[1, 5, 0, 0]);
    let prop_chunk = chunk(b"PROP", &prop);
    let dsd_chunk = chunk(b"DSD ", &snd);

    let mut form_body = Vec::new();
    form_body.extend_from_slice(b"DSD "); // form type
    form_body.extend_from_slice(&fver);
    form_body.extend_from_slice(&prop_chunk);
    form_body.extend_from_slice(&dsd_chunk);

    let mut out = Vec::new();
    out.extend_from_slice(b"FRM8");
    out.extend_from_slice(&(form_body.len() as u64).to_be_bytes());
    out.extend_from_slice(&form_body);
    out
}

/// Build a DST-compressed DFF (should be rejected by the decoder).
pub fn dff_dst_bytes(channels: usize, sample_rate: u32) -> Vec<u8> {
    fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(id);
        v.extend_from_slice(&(body.len() as u64).to_be_bytes());
        v.extend_from_slice(body);
        if body.len() % 2 == 1 {
            v.push(0);
        }
        v
    }
    let mut prop = Vec::new();
    prop.extend_from_slice(b"SND ");
    prop.extend_from_slice(&chunk(b"FS  ", &sample_rate.to_be_bytes()));
    let mut chnl = Vec::new();
    chnl.extend_from_slice(&(channels as u16).to_be_bytes());
    prop.extend_from_slice(&chunk(b"CHNL", &chnl));
    prop.extend_from_slice(&chunk(b"CMPR", b"DST \0\0dst"));

    let prop_chunk = chunk(b"PROP", &prop);
    let dst_chunk = chunk(b"DST ", &[0u8; 8]);
    let mut form_body = Vec::new();
    form_body.extend_from_slice(b"DSD ");
    form_body.extend_from_slice(&prop_chunk);
    form_body.extend_from_slice(&dst_chunk);

    let mut out = Vec::new();
    out.extend_from_slice(b"FRM8");
    out.extend_from_slice(&(form_body.len() as u64).to_be_bytes());
    out.extend_from_slice(&form_body);
    out
}
