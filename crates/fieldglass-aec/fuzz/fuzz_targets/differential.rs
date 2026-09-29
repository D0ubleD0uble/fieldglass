//! libFuzzer differential target: `decode_to_bytes` against `decode`.
//!
//! The two are one kernel behind two sinks, so they must agree on everything:
//! the verdict, and on success every output byte. The reference here lays the
//! samples out itself, independently of the crate's byte sink, so this also
//! holds `decode_to_bytes`' 1, 2, 3 and 4 byte layouts in both orders to the
//! documented rule: each sample truncated to its width, most significant byte
//! first with `Flags::MSB`. On `Err` the contents of the output are
//! unspecified, so only the errors are compared.
//!
//! Input layout: see `common.rs`.

#![no_main]

mod common;

use fieldglass_aec::{decode, decode_to_bytes, AecError, Flags, Sink};
use libfuzzer_sys::fuzz_target;

/// Collects every sample as a `u32`.
#[derive(Default)]
struct Collect(Vec<u32>);

impl Sink for Collect {
    fn samples(&mut self, block: &[u32]) {
        self.0.extend_from_slice(block);
    }

    fn repeat(&mut self, value: u32, count: usize) {
        // `count` is bounded by the requested total, at most 65,535.
        self.0.resize(self.0.len() + count, value);
    }
}

fuzz_target!(|data: &[u8]| {
    let Some((params, count, stream)) = common::split(data) else {
        return;
    };
    let width = params.bytes_per_sample();
    let msb = params.flags().contains(Flags::MSB);

    let mut samples = Collect::default();
    let by_sink = decode(stream, &params, count, &mut samples);

    let mut out = vec![0u8; count * width];
    let by_bytes = decode_to_bytes(stream, &params, &mut out);

    assert_eq!(by_sink, by_bytes, "the two entry points disagree");
    if by_sink.is_ok() {
        assert_eq!(samples.0.len(), count);
        let want: Vec<u8> = samples
            .0
            .iter()
            .flat_map(|&v| {
                let mut bytes = v.to_be_bytes()[4 - width..].to_vec();
                if !msb {
                    bytes.reverse();
                }
                bytes
            })
            .collect();
        assert_eq!(out, want, "byte layout differs from the sink's samples");
    }

    // A buffer that is not a whole number of samples is refused by name.
    if width > 1 {
        let mut odd = vec![0u8; count * width + 1];
        assert_eq!(
            decode_to_bytes(stream, &params, &mut odd),
            Err(AecError::OutputLength {
                len: odd.len(),
                bytes_per_sample: width
            })
        );
    }
});
