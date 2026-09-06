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

use fieldglass_grib1::Grib1Reader;

fuzz_target!(|data: &[u8]| {
    // A malformed buffer must surface a structured error, never panic.
    if let Ok(reader) = Grib1Reader::from_bytes(data.to_vec()) {
        for i in 0..reader.message_count() {
            // Ignore the result: we only care that decoding cannot panic or
            // over-read. Errors on individual messages are expected and fine.
            let _ = reader.decode_message_values(i);
            // Total by construction, so the assertion is that it stays total.
            let _ = reader.synthesis_grid(i);
        }
    }
});
