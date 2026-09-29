//! libFuzzer target for szip decompression, `sz::decompress`.
//!
//! HDF5 szip chunks are attacker-controlled, so `sz::decompress` runs on
//! arbitrary bytes under arbitrary valid szip parameters and output lengths,
//! and is held against a reference written here the way libsz does it
//! (`SZ_BufftoBuffDecompress`, `sz_compat.c`): decode the padded stream into
//! a buffer, every scanline whole when scanlines are padded, remove each
//! scanline's pads, then deinterleave byte planes into a second buffer. The
//! crate does none of that, so the two share only the kernel. It must:
//!
//! * never panic, and never overflow (cargo-fuzz builds with overflow checks
//!   on);
//! * refuse a length that is not a whole number of pixels, and only that,
//!   before decoding;
//! * agree with the reference on the verdict, and on success on every byte;
//! * on `Truncated`, report fewer samples than it was asked for.
//!
//! One difference is allowed, and it is the one ADR-0012 records: the crate
//! stops at the last pixel, so where the reference fails in a block at or
//! after the one holding the last pixel (a bad code or the end of the input
//! in samples nobody asked for), the crate returns `Ok`. The block holding
//! the last pixel counts because the crate reads only the part of it that
//! the output needs.
//!
//! Input layout: an 8-byte header, then the stream.
//!
//! | byte | meaning |
//! | --- | --- |
//! | 0 | options mask |
//! | 1 | bits per pixel: `b % 33`, with 0 meaning 64 |
//! | 2 | pixels per block / 2 - 1, modulo 128 |
//! | 3-4 | pixels per scanline - 1, little endian, modulo 4096 |
//! | 5-6 | output length in bytes, little endian, modulo 8192 |
//! | 7 | unused, so seeds keep the header's shape if it grows |
//!
//! Every header maps to parameters `SzParams::new` accepts; the parameter
//! checks themselves are pinned by `tests/sz_corpus.rs`.

#![no_main]

use fieldglass_aec::sz::{self, SzParams};
use fieldglass_aec::{decode, AecError, Flags, Params, Sink};
use libfuzzer_sys::fuzz_target;

const HEADER: usize = 8;

/// Collects every sample, pads included.
#[derive(Default)]
struct Collect(Vec<u32>);

impl Sink for Collect {
    fn samples(&mut self, block: &[u32]) {
        self.0.extend_from_slice(block);
    }

    fn repeat(&mut self, value: u32, count: usize) {
        self.0.resize(self.0.len() + count, value);
    }
}

/// Where the block holding the last output pixel starts: as a stream index,
/// pads included, and as an output index, pads dropped.
#[derive(Debug, Clone, Copy)]
struct LastBlock {
    stream: usize,
    output: usize,
}

/// libsz's decompression, the long way, or the kernel's error; and where the
/// block holding the last output pixel starts.
fn reference(stream: &[u8], p: &SzParams, len: usize) -> (Result<Vec<u8>, AecError>, LastBlock) {
    let bpp = p.bits_per_pixel();
    let planes = bpp == 32 || bpp == 64;
    let bits = if planes { 8 } else { bpp };
    // bits_to_bytes, sz_compat.c:59-67.
    let sample_bytes = match bits {
        0..=8 => 1,
        9..=16 => 2,
        _ => 4,
    };
    let ppb = p.pixels_per_block() as usize;
    let pps = p.pixels_per_scanline() as usize;
    let rsi = pps.div_ceil(ppb);
    let mut flags = Flags::empty();
    if p.options_mask() & sz::MSB_OPTION_MASK != 0 {
        flags.insert(Flags::MSB);
    }
    if p.options_mask() & sz::NN_OPTION_MASK != 0 {
        flags.insert(Flags::PREPROCESS);
    }
    let params = Params::new(bits as u8, ppb as u16, rsi as u16, flags).unwrap();

    // What libsz decodes: every scanline whole when scanlines are padded
    // (sz_compat.c:264-267), and exactly the output otherwise.
    let samples = len / sample_bytes;
    let line = rsi * ppb;
    let count = if pps % ppb != 0 {
        samples.div_ceil(pps) * line
    } else {
        samples
    };
    let start = match samples.checked_sub(1) {
        None => 0,
        Some(last) => (last / pps * line + last % pps) / ppb * ppb,
    };
    let last_block = LastBlock {
        stream: start,
        output: start / line * pps + (start % line).min(pps),
    };
    let mut padded = Collect::default();
    if let Err(e) = decode(stream, &params, count, &mut padded) {
        return (Err(e), last_block);
    }

    // remove_padding, sz_compat.c:119-130.
    let mut pixels = Vec::with_capacity(samples);
    for (i, &v) in padded.0.iter().enumerate() {
        if i % line < pps && pixels.len() < samples {
            pixels.push(v);
        }
    }
    assert_eq!(pixels.len(), samples);
    let result = expand(
        pixels,
        flags,
        sample_bytes,
        planes.then_some((bpp / 8) as usize),
    );
    (Ok(result), last_block)
}

/// The pixels in the coder's byte layout, then deinterleaved when they are
/// byte planes of `w`-byte pixels.
fn expand(pixels: Vec<u32>, flags: Flags, sample_bytes: usize, planes: Option<usize>) -> Vec<u8> {
    // The coder's byte layout, as `decode_to_bytes` writes it.
    let msb = flags.contains(Flags::MSB);
    let bytes: Vec<u8> = pixels
        .iter()
        .flat_map(|&v| {
            let mut b = v.to_be_bytes()[4 - sample_bytes..].to_vec();
            if !msb {
                b.reverse();
            }
            b
        })
        .collect();
    let Some(w) = planes else {
        return bytes;
    };
    // deinterleave_buffer, sz_compat.c:84-93.
    let n = bytes.len() / w;
    let mut out = vec![0u8; bytes.len()];
    for i in 0..n {
        for j in 0..w {
            out[i * w + j] = bytes[j * n + i];
        }
    }
    out
}

fuzz_target!(|data: &[u8]| {
    let Some((head, stream)) = data.split_at_checked(HEADER) else {
        return;
    };
    let bpp = match u32::from(head[1]) % 33 {
        0 => 64,
        b => b,
    };
    let ppb = (u32::from(head[2]) % 128 + 1) * 2;
    let pps = u32::from(u16::from_le_bytes([head[3], head[4]])) % 4096 + 1;
    let len = usize::from(u16::from_le_bytes([head[5], head[6]])) % 8192;
    let params = SzParams::new(u32::from(head[0]), bpp, ppb, pps)
        .expect("the header mapping only produces parameters libsz accepts");

    let mut out = vec![0u8; len];
    let got = sz::decompress(stream, &params, &mut out);

    let pixel = params.bytes_per_pixel();
    if len % pixel != 0 {
        assert_eq!(
            got,
            Err(AecError::OutputLength {
                len,
                bytes_per_sample: pixel
            })
        );
        return;
    }
    let (want, last_block) = reference(stream, &params, len);
    let failed_at = match &want {
        Err(AecError::InvalidCode { sample, .. }) => Some(*sample),
        Err(AecError::Truncated { decoded, .. }) => Some(*decoded),
        _ => None,
    };
    if failed_at.is_some_and(|at| at >= last_block.stream) {
        // The recorded divergence: the reference failed only in samples the
        // output does not need, so the crate returns `Ok`. Or both failed in
        // the block holding the last pixel, which the crate reads only in
        // part, so the two can meet a different fault first (the end of the
        // input against a value too wide); the crate's must be in that block.
        match got {
            Ok(()) => {}
            Err(AecError::InvalidCode { sample: at, .. })
            | Err(AecError::Truncated { decoded: at, .. }) => {
                assert_eq!(at, last_block.output, "{got:?} against {want:?}");
            }
            Err(e) => panic!("{e:?} against {want:?}"),
        }
        return;
    }
    match (&got, &want) {
        (Ok(()), Ok(bytes)) => assert_eq!(&out, bytes, "bytes differ from libsz's way"),
        (Err(AecError::Truncated { decoded, requested }), Err(AecError::Truncated { .. })) => {
            assert!(decoded < requested);
        }
        (Err(AecError::InvalidCode { .. }), Err(AecError::InvalidCode { .. })) => {}
        _ => panic!("verdicts differ: {got:?} against {want:?}"),
    }
});
