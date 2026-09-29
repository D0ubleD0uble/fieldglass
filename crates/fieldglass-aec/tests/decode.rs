//! The decoder's edges, on streams small enough to write by hand: truncation,
//! trailing bytes, a zero count, codes no valid encoder writes, and the
//! smallest block.
//!
//! The libaec corpus (`corpus.rs`) is the oracle for real streams. These
//! streams are built bit by bit from CCSDS 121.0-B-3 so each test states the
//! exact code it feeds the decoder.

mod common;

use common::{AEC_CASES, Bits, FIXTURES, manifest, narrow_u8, narrow_u16, rows, text, uint};
use fieldglass_aec::{AecError, Flags, Params, Sink, decode, decode_to_bytes};

/// Collects samples, and counts how they arrived.
#[derive(Default)]
struct Collect {
    samples: Vec<u32>,
    blocks: usize,
    runs: usize,
}

impl Sink for Collect {
    fn samples(&mut self, block: &[u32]) {
        assert!(!block.is_empty() && block.len() <= 256);
        self.samples.extend_from_slice(block);
        self.blocks += 1;
    }

    fn repeat(&mut self, value: u32, count: usize) {
        assert!(count > 0);
        self.samples.extend(std::iter::repeat_n(value, count));
        self.runs += 1;
    }
}

fn params(bits: u8, block: u16, rsi: u16, flags: Flags) -> Params {
    Params::new(bits, block, rsi, flags).unwrap()
}

fn collect(stream: &[u8], params: &Params, count: usize) -> Result<Collect, AecError> {
    let mut sink = Collect::default();
    decode(stream, params, count, &mut sink).map(|()| sink)
}

/// Two uncompressed 8-bit blocks of 4: 1 2 3 4, 5 6 7 8.
fn two_raw_blocks() -> Vec<u8> {
    let mut b = Bits::default();
    for block in [[1, 2, 3, 4], [5, 6, 7, 8]] {
        b.put(0b111, 3);
        for s in block {
            b.put(s, 8);
        }
    }
    b.done()
}

#[test]
fn a_zero_count_reads_nothing() {
    let p = params(8, 4, 1, Flags::empty());
    assert!(collect(&[], &p, 0).unwrap().samples.is_empty());
    assert_eq!(decode_to_bytes(&[], &p, &mut []), Ok(()));
}

#[test]
fn truncation_reports_the_samples_decoded_before_it() {
    let p = params(8, 4, 1, Flags::empty());
    let stream = two_raw_blocks();
    assert_eq!(
        collect(&stream, &p, 8).unwrap().samples,
        [1, 2, 3, 4, 5, 6, 7, 8]
    );

    // Cut into the second block: the first block reached the sink.
    let mut sink = Collect::default();
    let err = decode(&stream[..6], &p, 8, &mut sink).unwrap_err();
    assert_eq!(
        err,
        AecError::Truncated {
            decoded: 4,
            requested: 8
        }
    );
    assert_eq!(sink.samples, [1, 2, 3, 4]);

    // Asking for more than the stream holds is the same error.
    assert_eq!(
        collect(&stream, &p, 12).err(),
        Some(AecError::Truncated {
            decoded: 8,
            requested: 12
        })
    );
    assert_eq!(
        collect(&[], &p, 1).err(),
        Some(AecError::Truncated {
            decoded: 0,
            requested: 1
        })
    );
}

#[test]
fn bytes_after_the_last_sample_are_never_read() {
    let p = params(8, 4, 1, Flags::empty());
    let mut stream = two_raw_blocks();
    // A zero block whose run passes the end of the interval: an error if read.
    stream.extend([0b0000_0000, 0x00, 0xFF, 0xFF]);
    assert_eq!(
        collect(&stream, &p, 8).unwrap().samples,
        [1, 2, 3, 4, 5, 6, 7, 8]
    );
    // Stopping inside a block needs only that block's samples so far.
    assert_eq!(collect(&stream[..3], &p, 2).unwrap().samples, [1, 2]);
}

#[test]
fn a_value_of_two_to_the_n_is_an_invalid_code() {
    // 4-bit samples, block 2, no preprocessing: a split block with k = 0
    // (id 0b001) whose first fundamental sequence is 16, one past 4 bits.
    let p = params(4, 2, 1, Flags::empty());
    let stream = Bits::default().put(0b001, 3).fs(16).fs(0).done();
    assert!(matches!(
        collect(&stream, &p, 2),
        Err(AecError::InvalidCode { sample: 0, .. })
    ));
    // 15 fits.
    let stream = Bits::default().put(0b001, 3).fs(15).fs(0).done();
    assert_eq!(collect(&stream, &p, 2).unwrap().samples, [15, 0]);
}

#[test]
fn a_split_high_part_that_leaves_no_room_for_k_bits_is_an_invalid_code() {
    // 4 bits, k = 2 (id 0b011): a high part of 4 makes 4 << 2 = 16.
    let p = params(4, 2, 1, Flags::empty());
    let stream = Bits::default()
        .put(0b011, 3)
        .fs(4)
        .fs(0)
        .put(0, 2)
        .put(0, 2)
        .done();
    assert!(matches!(
        collect(&stream, &p, 2),
        Err(AecError::InvalidCode { .. })
    ));
    let stream = Bits::default()
        .put(0b011, 3)
        .fs(3)
        .fs(0)
        .put(3, 2)
        .put(1, 2)
        .done();
    assert_eq!(collect(&stream, &p, 2).unwrap().samples, [15, 1]);
}

#[test]
fn a_k_wider_than_the_sample_is_held_to_the_sample_width() {
    // 3 bits, k = 5 (id 0b110), which libaec's identifiers allow.
    let p = params(3, 2, 1, Flags::empty());
    let ok = Bits::default()
        .put(0b110, 3)
        .fs(0)
        .fs(0)
        .put(7, 5)
        .put(2, 5)
        .done();
    assert_eq!(collect(&ok, &p, 2).unwrap().samples, [7, 2]);
    let wide = Bits::default()
        .put(0b110, 3)
        .fs(0)
        .fs(0)
        .put(8, 5)
        .put(2, 5)
        .done();
    assert!(matches!(
        collect(&wide, &p, 2),
        Err(AecError::InvalidCode { .. })
    ));
}

#[test]
fn a_second_extension_value_wider_than_the_sample_is_an_invalid_code() {
    // 2 bits, block 2: codeword 3 is the pair (2, 0), codeword 6 is (3, 0),
    // codeword 10 is (4, 0), one past 2 bits.
    let p = params(2, 2, 1, Flags::empty());
    let se = |codeword| Bits::default().put(0b000, 3).put(1, 1).fs(codeword).done();
    assert_eq!(collect(&se(3), &p, 2).unwrap().samples, [2, 0]);
    assert_eq!(collect(&se(6), &p, 2).unwrap().samples, [3, 0]);
    assert!(matches!(
        collect(&se(10), &p, 2),
        Err(AecError::InvalidCode { .. })
    ));
}

#[test]
fn a_second_extension_pair_beside_a_reference_must_start_with_zero() {
    // 8 bits, block 2, preprocessed: the reference 50, then one codeword for
    // the pair (0, δ2). CCSDS 121.0-B-3 §3.4.1 and §5.2.6 put the 0 there.
    let p = params(8, 2, 1, Flags::PREPROCESS);
    let se = |codeword| {
        Bits::default()
            .put(0b000, 3)
            .put(1, 1)
            .put(50, 8)
            .fs(codeword)
            .done()
    };
    // Codeword 2 is (0, 1): δ2 = 1, an error of -1.
    assert_eq!(collect(&se(2), &p, 2).unwrap().samples, [50, 49]);
    // Codeword 1 is (1, 0): libaec would read δ2 = 0; the standard has no
    // such pair here.
    assert!(matches!(
        collect(&se(1), &p, 2),
        Err(AecError::InvalidCode { sample: 0, .. })
    ));
}

#[test]
fn a_zero_run_past_the_interval_is_an_invalid_code() {
    // RSI of 2 blocks; fs = 2 asks for 3 zero blocks.
    let p = params(8, 4, 2, Flags::empty());
    let stream = Bits::default().put(0b000, 3).put(0, 1).fs(2).done();
    assert!(matches!(
        collect(&stream, &p, 8),
        Err(AecError::InvalidCode { sample: 0, .. })
    ));
    // fs = 1 is two blocks, exactly the interval, in one call.
    let stream = Bits::default().put(0b000, 3).put(0, 1).fs(1).done();
    let sink = collect(&stream, &p, 8).unwrap();
    assert_eq!(sink.samples, [0; 8]);
    assert_eq!((sink.blocks, sink.runs), (0, 1));
}

#[test]
fn block_two_with_preprocessing_codes_one_sample_after_the_reference() {
    // 8 bits, block 2, RSI 2, preprocessed: the first block is a reference
    // sample and one mapped error; the second block two mapped errors.
    let p = params(8, 2, 2, Flags::PREPROCESS);
    let stream = Bits::default()
        // Split k = 0 (id 0b001): reference 100, then error +1 (d = 2).
        .put(0b001, 3)
        .put(100, 8)
        .fs(2)
        // Split k = 0: -1 (d = 1), then +3 (d = 6).
        .put(0b001, 3)
        .fs(1)
        .fs(6)
        // Next interval: an uncompressed block, reference 7 then d = 0.
        .put(0b111, 3)
        .put(7, 8)
        .put(0, 8)
        // A zero block mid-interval: two repeats of 7.
        .put(0b000, 3)
        .put(0, 1)
        .fs(0)
        // Next interval: a zero block opening it carries the reference 9,
        // then one repeat of it.
        .put(0b000, 3)
        .put(0, 1)
        .put(9, 8)
        .fs(0)
        .done();
    let sink = collect(&stream, &p, 10).unwrap();
    assert_eq!(sink.samples, [100, 101, 100, 103, 7, 7, 7, 7, 9, 9]);
    assert_eq!(sink.runs, 2);
}

#[test]
fn signed_samples_are_sign_extended_only_with_preprocessing() {
    // 4-bit signed, block 2: the raw pattern 0b1111 is -1.
    let raw = Bits::default()
        .put(0b111, 3)
        .put(0b1111, 4)
        .put(0b0001, 4)
        .done();
    let unprocessed = params(4, 2, 1, Flags::SIGNED);
    assert_eq!(collect(&raw, &unprocessed, 2).unwrap().samples, [0xF, 1]);
    // With preprocessing the first is the reference -1, the second d = 1,
    // an error of -1.
    let processed = params(4, 2, 1, Flags::SIGNED | Flags::PREPROCESS);
    assert_eq!(
        collect(&raw, &processed, 2).unwrap().samples,
        [u32::MAX, (-2i32).cast_unsigned()]
    );
}

#[test]
fn pad_rsi_skips_to_a_byte_at_every_interval() {
    // 4 bits, block 2, RSI 1: each interval is one uncompressed block of
    // 3 + 8 bits, padded to 16.
    let p = params(4, 2, 1, Flags::PAD_RSI);
    let stream = Bits::default()
        .put(0b111, 3)
        .put(1, 4)
        .put(2, 4)
        .put(0, 5)
        .put(0b111, 3)
        .put(3, 4)
        .put(4, 4)
        .done();
    assert_eq!(collect(&stream, &p, 4).unwrap().samples, [1, 2, 3, 4]);
    // Without the flag the padding is read as the next identifier.
    let unpadded = params(4, 2, 1, Flags::empty());
    assert_ne!(
        collect(&stream, &unpadded, 4).ok().map(|s| s.samples),
        Some(vec![1, 2, 3, 4])
    );
}

#[test]
fn the_restricted_set_uses_short_identifiers() {
    // 2 bits, restricted: a one-bit identifier, 1 for uncompressed.
    let p = params(2, 2, 1, Flags::RESTRICTED);
    let stream = Bits::default().put(1, 1).put(3, 2).put(1, 2).done();
    assert_eq!(collect(&stream, &p, 2).unwrap().samples, [3, 1]);
    // 4 bits, restricted: two bits, 0b10 for k = 1.
    let p = params(4, 2, 1, Flags::RESTRICTED);
    let stream = Bits::default()
        .put(0b10, 2)
        .fs(1)
        .fs(7)
        .put(1, 1)
        .put(0, 1)
        .done();
    assert_eq!(collect(&stream, &p, 2).unwrap().samples, [3, 14]);
}

#[test]
fn an_output_of_part_of_a_sample_is_refused() {
    let p = params(16, 2, 1, Flags::empty());
    assert_eq!(
        decode_to_bytes(&[0xF0, 0, 0, 0, 0], &p, &mut [0; 3]),
        Err(AecError::OutputLength {
            len: 3,
            bytes_per_sample: 2
        })
    );
}

#[test]
fn hostile_bytes_never_panic_and_never_overrun_the_output() {
    // Every parameter shape against short pseudo-random streams: each call
    // either fills the output or errors.
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for bits in 1..=32u8 {
        for &block in &[2u16, 8, 32, 256] {
            for flags in [0u8, 8, 9, 16, 24, 32, 45] {
                let Ok(p) = Params::new(bits, block, 3, Flags::from_bits_truncate(flags)) else {
                    continue;
                };
                for _ in 0..4 {
                    let len = usize::try_from(next() % 64).unwrap();
                    let stream: Vec<u8> = (0..len).map(|_| next().to_le_bytes()[0]).collect();
                    let mut out = vec![0; 1000 * p.bytes_per_sample()];
                    let _ = decode_to_bytes(&stream, &p, &mut out);
                    let _ = collect(&stream, &p, 1000);
                }
                // Long runs of zeros and of ones.
                for byte in [0x00, 0xFF] {
                    let stream = vec![byte; 300];
                    let _ = collect(&stream, &p, 100_000);
                }
            }
        }
    }
}

/// `decode` through a sink and `decode_to_bytes` agree on every corpus case,
/// and zero-block runs arrive as runs.
#[test]
fn the_sink_path_and_the_byte_path_agree_over_the_corpus() {
    let manifest = manifest();
    let mut runs = 0;
    for case in rows(&manifest, "aec_cases", AEC_CASES) {
        let name = text(case, "name");
        if text(case, "kind") == "truncated" {
            continue;
        }
        let p = Params::new(
            narrow_u8(case, "bits_per_sample"),
            narrow_u16(case, "block_size"),
            narrow_u16(case, "rsi"),
            Flags::from_bits_truncate(narrow_u8(case, "flags")),
        )
        .unwrap();
        let samples = usize::try_from(uint(case, "samples")).unwrap();
        let stream = std::fs::read(format!("{FIXTURES}/{}", text(case, "stream"))).unwrap();
        let sink = collect(&stream, &p, samples).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(sink.samples.len(), samples, "{name}");
        runs += sink.runs;

        let width = p.bytes_per_sample();
        let mut bytes = vec![0; samples * width];
        decode_to_bytes(&stream, &p, &mut bytes).unwrap();
        let msb = p.flags().contains(Flags::MSB);
        for (i, (&s, got)) in sink
            .samples
            .iter()
            .zip(bytes.chunks_exact(width))
            .enumerate()
        {
            let want = if msb {
                s.to_be_bytes()[4 - width..].to_vec()
            } else {
                s.to_le_bytes()[..width].to_vec()
            };
            assert_eq!(got, want, "{name}: sample {i}");
        }
    }
    assert!(runs > 100, "the corpus exercises zero-block runs ({runs})");
}
