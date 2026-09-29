//! libFuzzer target for the AEC decode kernel.
//!
//! GRIB2 template 5.42 payloads and HDF5 szip chunks are attacker-controlled,
//! so the kernel decodes arbitrary bytes under arbitrary valid parameters
//! (see `common.rs` for how the input maps to them) and must:
//!
//! * never panic, and never overflow: cargo-fuzz builds with overflow checks
//!   on, which is where the decoder's wrapping-by-design arithmetic is
//!   audited;
//! * never hand the sink more than `count` samples, and hand it exactly
//!   `count` on `Ok`;
//! * on `Truncated`, report the samples the sink actually received;
//! * with no preprocessing, deliver only n-bit values (a larger code is an
//!   `InvalidCode`, never a wrapped sample).

#![no_main]

mod common;

use fieldglass_aec::{decode, AecError, Flags, Sink};
use libfuzzer_sys::fuzz_target;

/// Counts what arrives, and checks the shape of each delivery.
struct Counting {
    received: usize,
    limit: usize,
    /// Largest value a sample may take, when the stream is not preprocessed.
    max_raw: Option<u32>,
}

impl Counting {
    fn arrived(&mut self, count: usize) {
        self.received = self
            .received
            .checked_add(count)
            .expect("a run length that overflows the running total");
        assert!(
            self.received <= self.limit,
            "{} samples delivered, {} requested",
            self.received,
            self.limit
        );
    }
}

impl Sink for Counting {
    fn samples(&mut self, block: &[u32]) {
        assert!(block.len() <= 256, "a delivery of {} samples", block.len());
        self.arrived(block.len());
        if let Some(max) = self.max_raw {
            assert!(block.iter().all(|&v| v <= max), "a value above {max}");
        }
    }

    fn repeat(&mut self, value: u32, count: usize) {
        self.arrived(count);
        if let Some(max) = self.max_raw {
            assert!(value <= max, "a repeated value above {max}");
        }
    }
}

fuzz_target!(|data: &[u8]| {
    let Some((params, count, stream)) = common::split(data) else {
        return;
    };
    let n = u32::from(params.bits_per_sample());
    let mut sink = Counting {
        received: 0,
        limit: count,
        max_raw: (!params.flags().contains(Flags::PREPROCESS)).then(|| {
            if n == 32 {
                u32::MAX
            } else {
                (1u32 << n) - 1
            }
        }),
    };
    match decode(stream, &params, count, &mut sink) {
        Ok(()) => assert_eq!(sink.received, count, "Ok but short"),
        Err(AecError::Truncated { decoded, requested }) => {
            assert_eq!(requested, count);
            assert_eq!(decoded, sink.received, "Truncated misreports the progress");
            assert!(decoded < count);
        }
        Err(_) => assert!(sink.received <= count),
    }
});
