//! libFuzzer target for the GRIB1 decode path.
//!
//! `fieldglass-grib1` parses attacker-controllable bytes (IS/PDS/GDS/BMS/BDS),
//! the highest-severity bug class for a binary parser. This target drives the
//! full scan-plus-decode pipeline — `Grib1Reader::from_bytes` followed by
//! `decode_message_values` for every message it finds — asserting the parser
//! never panics, over-reads, or hangs on arbitrary input. The length- and
//! offset-driven bit reading in the second-order packing decoders is the main
//! thing this exercises.
//!
//! `decode_spectral_message` joins it because it is a second decode path with
//! its own bit unpacking, reached only by a spherical-harmonic BDS and so never
//! touched by `decode_message_values`. Leaving it out is what let an unbounded
//! coefficient allocation live there: `J` is a bare `u16` from the GDS, and at
//! a zero bit width no §7 budget constrains it, so a 112-byte message sized a
//! `Vec` at up to 34 GB (#631). The GRIB2 target has always driven its
//! equivalent, and had a cap; this one did not, and did not.
//!
//! `synthesis_grid` joins it because it is a public entry point that reads §2
//! and answers a grid size derived from attacker-controlled fields (#580). Its
//! partner `synthesize_message_global` is deliberately **not** called: the only
//! GRIB1 synthesis family is spherical-harmonic, so every call would run the
//! full inverse transform on the 720×361 grid, and the transform's cost is
//! `O(points × coefficients)` with the coefficient count coming from the file —
//! a legitimate high-truncation input would surface as a fuzzer timeout rather
//! than a finding. The GRIB2 target states the same rule and can still reach
//! its HEALPix arm, whose cost `Nside` bounds.

#![no_main]

use libfuzzer_sys::fuzz_target;

use fieldglass_grib1::{Grib1Reader, GridDescription};

/// The declared spherical-harmonic truncation of message `i`, if it has one,
/// against a bound chosen for fuzzer throughput rather than for correctness.
///
/// `(J+1)(J+2)` values at eight bytes each is 2.1 MB at `J = 512` and 537 MB at
/// `MAX_TRUNCATION`; the decoder's arithmetic is the same either way.
fn declares_a_large_truncation(reader: &Grib1Reader, i: usize) -> bool {
    const FUZZ_MAX_TRUNCATION: u16 = 512;
    matches!(
        reader.messages.get(i).and_then(|m| m.gds.as_ref()),
        Some(GridDescription::SphericalHarmonic(sh)) if sh.j > FUZZ_MAX_TRUNCATION
    )
}

fuzz_target!(|data: &[u8]| {
    // A malformed buffer must surface a structured error, never panic.
    if let Ok(reader) = Grib1Reader::from_bytes(data.to_vec()) {
        for i in 0..reader.message_count() {
            // Ignore the result: we only care that decoding cannot panic or
            // over-read. Errors on individual messages are expected and fine.
            let _ = reader.decode_message_values(i);
            // The spherical-harmonic decode path, which `decode_message_values`
            // refuses outright. `MAX_TRUNCATION` bounds it, so a declared
            // truncation can no longer turn a short input into an allocation
            // the fuzzer reports as an OOM instead of as a finding — but the
            // bound is 537 MB, which is inside libFuzzer's default RSS limit
            // and still half a second of writes per exec. An input that reaches
            // it would be kept in the corpus and pay that cost forever, so a
            // large declared truncation is skipped here. Nothing in the bit
            // unpacking needs one: the traversal, the IBM float decode and the
            // sub-truncation weave are all exercised at small `J`.
            if !declares_a_large_truncation(&reader, i) {
                let _ = reader.decode_spectral_message(i);
            }
            // Total by construction, so the assertion is that it stays total.
            let _ = reader.synthesis_grid(i);
        }
    }
});
