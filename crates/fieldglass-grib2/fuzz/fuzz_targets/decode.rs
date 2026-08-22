//! libFuzzer target for the GRIB2 decode path.
//!
//! `fieldglass-grib2` parses attacker-controllable bytes
//! (IS/IDS/LUS/GDS/PDS/DRS/BMS/DS), the highest-severity bug class for a binary
//! parser. This target drives the full scan-plus-decode pipeline —
//! `Grib2Reader::from_bytes` followed by every public decode entry point for
//! each message it finds — asserting the parser never panics, over-reads, or
//! hangs on arbitrary input. The §5 DRS templates each carry their own
//! length/offset-driven bit unpacking, which is the main thing this exercises.
//!
//! `decode_message_values` covers only the one-scalar-per-grid-point packings.
//! The forms whose output is not a scalar field have their own entry points and
//! their own bit unpacking, so driving only the scalar path would leave them
//! unfuzzed:
//!
//! * `decode_matrix_message` — template 5.1 with `matrixBitmapsPresent = 1`,
//!   the true `NR×NC` per-point matrix delimited by secondary bitmaps. Stock
//!   eccodes divides by zero and crashes on this variant, so there is no
//!   second implementation to compare against; it is exactly the code that
//!   most needs a fuzzer.
//! * `decode_spectral_message` — §5.50 / 5.51 spherical-harmonic coefficients,
//!   where 5.51 reads a sub-truncation of raw IEEE floats followed by a
//!   Laplacian-rescaled simple-packed remainder.
//! * `decode_bifourier_message` — §5.53, four coefficients per wavenumber pair
//!   over a rectangle / ellipse / diamond truncation.
//! * `synthesize_spectral_message` — the inverse spherical-harmonic transform,
//!   whose cost is driven by the truncation the *file* declares.

#![no_main]

use libfuzzer_sys::fuzz_target;

use fieldglass_grib2::Grib2Reader;

/// Latitudes/longitudes for the synthesis probe. Deliberately tiny: the
/// transform's cost is `O(points × coefficients)` and the coefficient count
/// comes from the file, so a large grid here would turn a legitimate
/// high-truncation input into a fuzzer timeout rather than a finding.
const PROBE_LATS: [f64; 3] = [-60.0, 0.0, 60.0];
const PROBE_LONS: [f64; 3] = [0.0, 120.0, 240.0];

fuzz_target!(|data: &[u8]| {
    // A malformed buffer must surface a structured error, never panic.
    if let Ok(reader) = Grib2Reader::from_bytes(data.to_vec()) {
        for i in 0..reader.message_count() {
            // Ignore the results: we only care that decoding cannot panic or
            // over-read. Errors on individual messages are expected and fine —
            // most inputs are the wrong packing for most of these entry points,
            // and a clean rejection is the correct outcome there.
            let _ = reader.decode_message_values(i);
            let _ = reader.decode_matrix_message(i);
            let _ = reader.decode_bifourier_message(i);
            // Only attempt the synthesis when the coefficients themselves
            // decoded, so a failure here is a transform bug rather than a
            // re-run of the decode error above.
            if reader.decode_spectral_message(i).is_ok() {
                let _ = reader.synthesize_spectral_message(i, &PROBE_LATS, &PROBE_LONS);
            }
        }
    }
});
